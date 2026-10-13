use crate::{BlockKind, Document, PersistenceMode, Style, StylePatch, StyleStatus};
use reprise_geom::Length;

#[test]
fn rename_and_concurrent_properties_survive_both_delivery_orders() {
    for reverse in [false, true] {
        let a = Document::new(1).unwrap();
        a.define_named_style(
            "Quote",
            &Style {
                weight: Some(400),
                ..Default::default()
            },
        )
        .unwrap();
        let node = a
            .append_block(BlockKind::Paragraph, "Quote", "quote")
            .unwrap();
        a.commit();
        let b = a.fork(2).unwrap();
        a.rename_named_style("Quote", "Quotation").unwrap();
        b.patch_named_style(
            "Quote",
            &StylePatch {
                set: Style {
                    color: Some([0, 0, 255, 255]),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        a.commit();
        b.commit();
        if reverse {
            b.merge(&a).unwrap();
            a.merge(&b).unwrap();
        } else {
            a.merge(&b).unwrap();
            b.merge(&a).unwrap();
        }
        for doc in [&a, &b] {
            assert_eq!(
                doc.computed_style(node).unwrap().color,
                Some([0, 0, 255, 255])
            );
            assert_eq!(
                doc.style_record("Quotation").unwrap().style.weight,
                Some(400)
            );
        }
    }
}

#[test]
fn first_property_patches_remain_owned_through_rename() {
    let a = Document::new(1).unwrap();
    a.patch_named_style(
        "New",
        &StylePatch {
            set: Style {
                weight: Some(400),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    a.rename_named_style("New", "Renamed").unwrap();
    b.patch_named_style(
        "New",
        &StylePatch {
            set: Style {
                color: Some([1, 2, 3, 255]),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    a.commit();
    b.commit();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    assert_eq!(a.style("Renamed").unwrap().color, Some([1, 2, 3, 255]));
    assert_eq!(a.style("Renamed"), b.style("Renamed"));
}

#[test]
fn reuse_of_a_renamed_name_does_not_rebind_old_blocks_or_parents() {
    let doc = Document::new(1).unwrap();
    doc.define_named_style(
        "A",
        &Style {
            size: Some(crate::LengthExpr::Pt(Length::from_pt(12))),
            ..Default::default()
        },
    )
    .unwrap();
    doc.define_named_style(
        "Child",
        &Style {
            parent: Some("A".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let node = doc
        .append_block(BlockKind::Paragraph, "Child", "old")
        .unwrap();
    doc.rename_named_style("A", "B").unwrap();
    doc.define_named_style(
        "A",
        &Style {
            size: Some(crate::LengthExpr::Pt(Length::from_pt(30))),
            ..Default::default()
        },
    )
    .unwrap();
    let new = doc.append_block(BlockKind::Paragraph, "A", "new").unwrap();
    assert_eq!(doc.computed_style(node).unwrap().size, Length::from_pt(12));
    assert_eq!(doc.computed_style(new).unwrap().size, Length::from_pt(30));
}

#[test]
fn unmarked_legacy_heading_resolution_is_read_pure_and_uses_fallback() {
    let doc = Document::new(1).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "", "old").unwrap();
    doc.meta_of(node)
        .unwrap()
        .insert("style", "Heading 1")
        .unwrap();
    doc.commit();
    let bytes = doc.try_export(PersistenceMode::History).unwrap();
    let old = Document::import(&bytes, 2).unwrap();
    assert_eq!(old.computed_style(node).unwrap().size, Length::from_pt(10));
    old.style_names();
    assert!(!old.has_root("styles2"));
    assert_eq!(bytes, old.try_export(PersistenceMode::History).unwrap());
}

#[test]
fn builtin_opt_in_root_survives_history_shallow_and_undo() {
    let doc = Document::new(1).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "", "x").unwrap();
    doc.commit();
    let mut undo = doc.undo_stack();
    doc.set_style_name(node, "Heading 1").unwrap();
    doc.commit();
    assert!(doc.has_root("styles2"));
    assert_eq!(doc.computed_style(node).unwrap().size, Length::from_pt(20));
    assert!(undo.undo().unwrap());
    for mode in [PersistenceMode::History, PersistenceMode::Shallow] {
        let imported = Document::import(&doc.try_export(mode).unwrap(), 2).unwrap();
        assert!(imported.has_root("styles2"));
    }
}

#[test]
fn rename_and_concurrent_delete_preserve_the_tombstone() {
    let a = Document::new(1).unwrap();
    a.define_named_style("A", &Style::default()).unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    a.rename_named_style("A", "B").unwrap();
    b.delete_named_style("A").unwrap();
    a.commit();
    b.commit();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    assert_eq!(a.style_record("B").unwrap().status, StyleStatus::Deleted);
    assert_eq!(a.style_record("B"), b.style_record("B"));
}

#[test]
fn competing_renames_and_colliding_labels_keep_both_owners_selectable() {
    let a = Document::new(1).unwrap();
    a.define_named_style(
        "A",
        &Style {
            weight: Some(400),
            ..Default::default()
        },
    )
    .unwrap();
    a.define_named_style(
        "B",
        &Style {
            weight: Some(700),
            ..Default::default()
        },
    )
    .unwrap();
    let old = a.style_identity("A").unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    a.rename_named_style("A", "Same").unwrap();
    b.rename_named_style("B", "Same").unwrap();
    a.commit();
    b.commit();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    assert_eq!(a.style_names(), b.style_names());
    let labels: Vec<_> = a
        .style_names()
        .0
        .into_iter()
        .filter(|n| n.starts_with("Same [style "))
        .collect();
    assert_eq!(labels.len(), 2);
    let mut weights: Vec<_> = labels
        .iter()
        .map(|n| a.style_record(n).unwrap().style.weight.unwrap())
        .collect();
    weights.sort();
    assert_eq!(weights, vec![400, 700]);
    assert!(
        labels
            .iter()
            .any(|n| a.style_identity(n).as_ref() == Some(&old))
    );
}

#[test]
fn rename_undo_redo_keeps_owner_and_concurrent_patch() {
    let a = Document::new(1).unwrap();
    a.define_named_style("A", &Style::default()).unwrap();
    a.commit();
    let owner = a.style_identity("A").unwrap();
    let b = a.fork(2).unwrap();
    let mut undo = a.undo_stack();
    a.rename_named_style("A", "B").unwrap();
    a.commit();
    b.patch_named_style(
        "A",
        &StylePatch {
            set: Style {
                weight: Some(700),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    b.commit();
    a.merge(&b).unwrap();
    assert!(undo.undo().unwrap());
    assert_eq!(a.style_identity("A"), Some(owner.clone()));
    assert_eq!(a.style("A").unwrap().weight, Some(700));
    assert!(undo.redo().unwrap());
    assert_eq!(a.style_identity("B"), Some(owner));
    assert_eq!(a.style("B").unwrap().weight, Some(700));
}

#[test]
fn renamed_builtin_clear_reveals_pinned_builtin_base() {
    let doc = Document::new(1).unwrap();
    let node = doc
        .append_block(BlockKind::Paragraph, "Heading 1", "heading")
        .unwrap();
    doc.rename_named_style("Heading 1", "Title").unwrap();
    doc.patch_named_style(
        "Title",
        &StylePatch {
            set: Style {
                size: Some(crate::LengthExpr::Pt(Length::from_pt(40))),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    doc.patch_named_style(
        "Title",
        &StylePatch {
            clear: ["size".into()].into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(doc.computed_style(node).unwrap().size, Length::from_pt(20));
    let copy = Document::new(2).unwrap();
    copy.restore_authored_style_graph(&doc.authored_style_graph().unwrap())
        .unwrap();
    let copied = copy
        .append_block(
            BlockKind::Paragraph,
            &doc.style_reference(node).unwrap(),
            "heading",
        )
        .unwrap();
    assert_eq!(
        copy.computed_style(copied).unwrap().size,
        Length::from_pt(20)
    );
    assert_eq!(copy.style_identity("Title"), doc.style_identity("Title"));
    copy.commit();
    assert!(copy.version_vector().iter().all(|(peer, _)| *peer == 2));
}

#[test]
fn old_marked_builtin_context_survives_adoption_and_graph_copy() {
    let old = Document::new(1).unwrap();
    let node = old.append_block(BlockKind::Paragraph, "", "old").unwrap();
    old.meta_of(node)
        .unwrap()
        .insert("style", "Heading 1")
        .unwrap();
    old.doc
        .get_map("styles1")
        .insert("weight/Unused", "400")
        .unwrap();
    old.commit();
    let doc = Document::import(&old.try_export(PersistenceMode::History).unwrap(), 2).unwrap();
    assert_eq!(doc.computed_style(node).unwrap().size, Length::from_pt(20));
    doc.define_named_style("New", &Style::default()).unwrap();
    assert_eq!(doc.computed_style(node).unwrap().size, Length::from_pt(20));
    let copy = Document::new(3).unwrap();
    copy.restore_authored_style_graph(&doc.authored_style_graph().unwrap())
        .unwrap();
    let copied = copy
        .append_block(
            BlockKind::Paragraph,
            &doc.style_reference(node).unwrap(),
            "old",
        )
        .unwrap();
    assert_eq!(
        copy.computed_style(copied).unwrap().size,
        Length::from_pt(20)
    );
}

#[test]
fn malformed_identity_root_refuses_before_authored_style_writes() {
    let doc = Document::new(1).unwrap();
    doc.doc.get_list("styles2").push("wrong kind").unwrap();
    doc.commit();
    let bytes = doc.try_export(PersistenceMode::History).unwrap();
    assert!(doc.define_named_style("A", &Style::default()).is_err());
    assert_eq!(bytes, doc.try_export(PersistenceMode::History).unwrap());
}

#[test]
fn competing_names_for_one_owner_converge_and_keep_both_aliases() {
    for reverse in [false, true] {
        let a = Document::new(1).unwrap();
        a.define_named_style(
            "A",
            &Style {
                weight: Some(500),
                ..Default::default()
            },
        )
        .unwrap();
        a.commit();
        let b = a.fork(2).unwrap();
        a.rename_named_style("A", "Left").unwrap();
        b.rename_named_style("A", "Right").unwrap();
        a.commit();
        b.commit();
        if reverse {
            b.merge(&a).unwrap();
            a.merge(&b).unwrap();
        } else {
            a.merge(&b).unwrap();
            b.merge(&a).unwrap();
        }
        assert_eq!(a.style_names(), b.style_names());
        assert_eq!(a.style_identity("Left"), a.style_identity("Right"));
        assert_eq!(a.style("Left").unwrap().weight, Some(500));
        assert_eq!(a.style("Left"), b.style("Right"));
    }
}

#[test]
fn builtin_rename_preflights_all_metadata_keys_before_writing() {
    let doc = Document::new(1).unwrap();
    let map = doc.doc.get_map("styles2");
    for i in 0..65533 {
        map.insert(&format!("future/{i}"), "retained").unwrap();
    }
    doc.commit();
    let before = doc.try_export(PersistenceMode::History).unwrap();
    assert!(doc.rename_named_style("Heading 1", "Title").is_err());
    assert_eq!(before, doc.try_export(PersistenceMode::History).unwrap());
}
