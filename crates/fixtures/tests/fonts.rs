use reprise_doc::{BlockKind, Document, Style};
use reprise_fixtures::fonts::{collection, declaration};
use reprise_font::{FontStore, GenericFamily};
#[test]
fn frontend_errors_are_stable_and_collection_faces_are_addressable() {
    let mut fonts = FontStore::default();
    assert_eq!(
        fonts
            .register(b"bad".as_slice(), declaration("bad", 0))
            .unwrap_err()
            .note()
            .code
            .as_str(),
        "font.unreadable"
    );
    let bytes = collection();
    for index in [0, 1] {
        let id = fonts
            .register(bytes.clone(), declaration("collection", index))
            .unwrap();
        assert!(fonts.get(&id).unwrap().covers('a'));
    }
    assert!(fonts.register(bytes, declaration("bad-index", 2)).is_err());
}
#[test]
fn chain_storage_inheritance_legacy_override_and_unknown_versions() {
    let doc = Document::new(1).unwrap();
    doc.define_style("body", &Style::default()).unwrap();
    doc.define_style(
        "chain",
        &Style {
            families: Some(vec!["absent".into(), "monospace".into()]),
            ..Style::default()
        },
    )
    .unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "chain", "").unwrap();
    doc.block(node).unwrap().text.insert(0, "office").unwrap();
    doc.commit();
    assert_eq!(
        doc.computed_style(node).unwrap().families,
        ["absent", "monospace"]
    );
    let replica = doc.fork(2).unwrap();
    assert_eq!(doc.style("chain"), replica.style("chain"));
    let engine = reprise_fixtures::engine();
    assert_eq!(
        engine.layout(&doc).to_json(),
        engine.layout(&replica).to_json()
    );
    assert_eq!(
        engine.layout(&doc).blocks[0].lines[0].runs[0].face,
        *engine.fonts.generic(GenericFamily::Monospace).id()
    );
    doc.set_overrides(
        node,
        &Style {
            family: Some("Source Serif Pro".into()),
            ..Style::default()
        },
    )
    .unwrap();
    assert!(doc.computed_style(node).unwrap().families.is_empty());
    let raw = "families999:[\"future\"]";
    doc.define_style(
        "unknown",
        &Style {
            unparsed_families: Some(raw.into()),
            ..Style::default()
        },
    )
    .unwrap();
    assert_eq!(
        doc.style("unknown").unwrap().unparsed_families.as_deref(),
        Some(raw)
    );
    let other = doc
        .append_block(BlockKind::Paragraph, "unknown", "")
        .unwrap();
    assert!(
        doc.computed_style(other)
            .unwrap()
            .notes
            .iter()
            .any(|n| n.code.as_str() == "style.unparsed")
    );
    let mut session = reprise_layout::incremental::LayoutSession::new(&engine);
    assert_eq!(
        session.layout(&doc).unwrap().to_json(),
        engine.layout(&doc).to_json()
    );
    doc.set_overrides(
        node,
        &Style {
            families: Some(vec!["serif".into()]),
            ..Style::default()
        },
    )
    .unwrap();
    doc.commit();
    assert_eq!(
        session.layout(&doc).unwrap().to_json(),
        engine.layout(&doc).to_json()
    );
}
