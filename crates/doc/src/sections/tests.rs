use super::*;
use crate::{BlockKind, PersistenceMode};

fn paragraph(text: &str) -> NewBlock {
    NewBlock::new(BlockKind::Paragraph, "", text)
}

#[test]
fn domain_traversal_and_nested_story_liveness_agree() {
    let doc = Document::new(1).unwrap();
    let body = doc.append_block(BlockKind::Paragraph, "", "body").unwrap();
    let story = doc.create_story(&[paragraph("header")]).unwrap();
    let text = doc.top(Domain::Story(story))[0];
    assert_eq!(doc.top(Domain::Body), [body]);
    assert_eq!(doc.order(Domain::Story(story)), [text]);
    assert_eq!(doc.authored_order(), [body, text]);
    assert_eq!(doc.domain_of(text), Some(Domain::Story(story)));
    assert!(doc.block(text).is_ok());
    let nested = doc.create_story(&[paragraph("nested")]).unwrap();
    let child = doc.top(Domain::Story(nested))[0];
    doc.tree("content")
        .mov(nested.node, Some(story.node))
        .unwrap();
    assert!(!doc.is_live(nested));
    assert!(!doc.is_live(child));
    assert!(doc.block(child).is_err());
    assert_eq!(doc.domain_of(child), None);
    assert_eq!(doc.top(Domain::Story(nested)), []);
    assert!(
        doc.audit()
            .iter()
            .any(|f| f.note.code.as_str() == "story.nested")
    );
    doc.delete_block(story).unwrap();
    assert!(!doc.is_live(text));
    assert!(doc.stories().is_empty());
    assert!(doc.has_sections());
}

#[test]
fn reads_and_empty_patches_create_no_section_roots() {
    let doc = Document::new(1).unwrap();
    let before = doc.try_export(PersistenceMode::History).unwrap();
    assert!(!doc.has_sections());
    assert!(doc.start_records().is_empty());
    assert!(doc.effective_starts().is_empty());
    assert!(doc.geometry_mode_record().is_none());
    assert!(doc.stories().is_empty());
    doc.write_section_properties(SectionId::First, "0", &BTreeMap::new(), &["width".into()])
        .unwrap();
    doc.audit();
    assert!(!doc.has_sections());
    assert_eq!(doc.try_export(PersistenceMode::History).unwrap(), before);
}

#[test]
fn lifetimes_are_operation_ids_and_undo_redo_keeps_them() {
    let doc = Document::new(1).unwrap();
    let block = doc.append_block(BlockKind::Paragraph, "", "a").unwrap();
    doc.commit();
    let mut undo = doc.undo_stack();
    let life = doc.mint_lifetime(block).unwrap();
    let props = BTreeMap::from([("width".into(), Value::from(100))]);
    doc.activate_start(block, &life, BreakKind::Section, &props)
        .unwrap();
    doc.commit_step();
    assert_eq!(doc.effective_starts()[0].own["width"], Value::from(100));
    assert_eq!(
        doc.effective_starts()[0].life.as_deref(),
        Some(life.as_str())
    );
    undo.undo().unwrap();
    assert!(doc.effective_starts().is_empty());
    let audit = doc.audit();
    assert!(
        audit.iter().any(|f| f.note.code == "section.inactive"
            && f.note.severity == reprise_diag::Severity::Info)
    );
    assert!(!audit.iter().any(|f| f.note.code == "section.orphan-start"));
    undo.redo().unwrap();
    assert_eq!(
        doc.effective_starts()[0].life.as_deref(),
        Some(life.as_str())
    );
    undo.undo().unwrap();
    let new = doc.mint_lifetime(block).unwrap();
    assert_ne!(life, new);
    assert!(doc.has_sections());
}

#[test]
fn first_block_deleted_prefix_and_empty_body_keep_records_without_rekeying() {
    let doc = Document::new(1).unwrap();
    let prefix = doc
        .append_block(BlockKind::Paragraph, "", "prefix")
        .unwrap();
    let block = doc.append_block(BlockKind::Paragraph, "", "start").unwrap();
    let life = doc.mint_lifetime(block).unwrap();
    doc.activate_start(block, &life, BreakKind::Section, &BTreeMap::new())
        .unwrap();
    doc.delete_block(prefix).unwrap();
    assert_eq!(doc.effective_starts()[0].id, SectionId::Block(block));
    doc.delete_block(block).unwrap();
    assert!(doc.top(Domain::Body).is_empty());
    assert!(doc.effective_starts().is_empty());
    assert!(
        doc.audit()
            .iter()
            .any(|f| f.note.code.as_str() == "section.orphan-start")
    );
    doc.restore_block(block).unwrap();
    assert_eq!(
        doc.effective_starts()[0].life.as_deref(),
        Some(life.as_str())
    );
}

#[test]
fn retained_empty_malformed_and_inactive_roots_require_sections() {
    for root in [STARTS, GEOMETRY] {
        let doc = Document::new(1).unwrap();
        let map = doc.doc.get_map(root);
        map.insert("bad", true).unwrap();
        assert!(doc.has_sections());
        map.delete("bad").unwrap();
        assert!(doc.has_sections());
        doc.commit();
        let reopened = Document::import(&doc.export(PersistenceMode::History), 2).unwrap();
        assert!(reopened.has_sections());
    }
    let doc = Document::new(1).unwrap();
    let block = doc.append_block(BlockKind::Paragraph, "", "").unwrap();
    let map = doc.doc.get_map(STARTS);
    map.insert(&format!("{block}/active"), false).unwrap();
    map.insert(&format!("{block}/life"), 123).unwrap();
    assert!(doc.has_sections());
    assert!(doc.effective_starts().is_empty());
    assert!(
        doc.audit()
            .iter()
            .any(|f| f.note.code.as_str() == "section.inactive")
    );
}

#[test]
fn geometry_alone_sets_package_and_sync_requirement() {
    let doc = Document::new(1).unwrap();
    doc.set_geometry_mode("pageless").unwrap();
    assert_eq!(
        doc.geometry_mode_record().unwrap()["mode"],
        Value::from("pageless")
    );
    let packet = doc.export_snapshot_packet().unwrap();
    let (header, _) = crate::sync::read_header(&packet).unwrap();
    assert_eq!(header.features.0 & REQUIRED_SECTIONS, REQUIRED_SECTIONS);
    let other = Document::new(2).unwrap();
    other.import_packet(&packet).unwrap();
    assert_eq!(other.geometry_mode_record(), doc.geometry_mode_record());
}

#[test]
fn ten_thousand_addresses_are_read_together_and_audit_samples_are_bounded() {
    let doc = Document::new(1).unwrap();
    let map = doc.doc.get_map(STARTS);
    for index in 0..10_000 {
        let block = doc.append_block(BlockKind::Paragraph, "", "x").unwrap();
        map.insert(&format!("{block}/active"), index % 2 == 0)
            .unwrap();
        map.insert(&format!("{block}/life"), "1@2").unwrap();
        map.insert(&format!("{block}/1@2/break"), "section")
            .unwrap();
        map.insert(&format!("{block}/1@2/width"), 100).unwrap();
    }
    doc.commit();
    assert_eq!(doc.start_records().len(), 10_000);
    assert_eq!(doc.effective_starts().len(), 5_000);
    let inactive: Vec<_> = doc
        .audit()
        .into_iter()
        .filter(|f| f.note.code == "section.inactive")
        .collect();
    assert_eq!(inactive.len(), 1);
    assert!(inactive[0].note.message.contains("5000 retained starts"));
    assert!(inactive[0].note.message.len() < 256);
}

#[test]
fn malformed_lifetimes_stay_inert_and_physical_story_tombstones_keep_the_bit() {
    for life in ["-1@1", "01@1", "0@01", "0", "0@1/1@1", "not-an-id"] {
        let doc = Document::new(1).unwrap();
        let block = doc.append_block(BlockKind::Paragraph, "", "x").unwrap();
        let map = doc.doc.get_map(STARTS);
        map.insert(&format!("{block}/active"), true).unwrap();
        map.insert(&format!("{block}/life"), life).unwrap();
        map.insert(&format!("{block}/{life}/break"), "section")
            .unwrap();
        assert!(doc.has_sections());
        assert!(doc.effective_starts().is_empty(), "{life}");
        assert!(
            doc.audit()
                .iter()
                .any(|f| f.note.code == "section.orphan-start")
        );
    }
    let doc = Document::new(1).unwrap();
    let story = doc.create_story(&[paragraph("retained")]).unwrap();
    doc.tree("content").delete(story.node).unwrap();
    assert!(!doc.is_live(story));
    assert!(doc.stories().is_empty());
    assert!(doc.has_sections());
    doc.commit();
    assert!(
        Document::import(&doc.export(PersistenceMode::History), 2)
            .unwrap()
            .has_sections()
    );
}

#[test]
fn story_metadata_has_no_meaning_outside_content() {
    let doc = Document::new(1).unwrap();
    let tree = doc.tree("relations");
    let parent = tree.create(None).unwrap();
    let child = tree.create(Some(parent)).unwrap();
    tree.get_meta(child)
        .unwrap()
        .insert("kind", "story")
        .unwrap();
    assert!(doc.live(&tree, child));
    assert!(!doc.has_sections());
}
