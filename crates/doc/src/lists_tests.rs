//! Roles and lists in the document model (see `docs/roles-and-lists.md`).

use loro::LoroValue;

use crate::lists::{LEVEL, LIST, RESTART};
use crate::roles::{ROLE, codes};
use crate::{
    BlockKind, DocError, Document, LevelDef, ListDefinition, ListId, NodeId, NumberFormat, Role,
};

fn doc_with(texts: &[&str]) -> (Document, Vec<NodeId>) {
    let doc = Document::new(1).unwrap();
    let first = doc
        .append_block(BlockKind::Paragraph, "body", texts[0])
        .unwrap();
    let mut ids = vec![first];
    // One flow: every later paragraph is a break of the first host.
    let mut last = first;
    for t in &texts[1..] {
        let len = doc.block(last).unwrap().text.len();
        let next = doc.split_block(last, len).unwrap();
        doc.block(next).unwrap().text.insert(0, t).unwrap();
        ids.push(next);
        last = next;
    }
    doc.commit();
    (doc, ids)
}

fn markers(doc: &Document, ids: &[NodeId]) -> Vec<String> {
    ids.iter()
        .map(|&id| doc.list_item(id).map_or("-".into(), |i| i.marker))
        .collect()
}

fn note_codes(doc: &Document, id: NodeId) -> Vec<String> {
    doc.list_numbering()
        .notes
        .get(&id)
        .map(|n| n.iter().map(|n| n.code.as_str().to_string()).collect())
        .unwrap_or_default()
}

#[test]
fn numbering_continues_across_body_paragraphs_and_restarts() {
    let (doc, ids) = doc_with(&["a", "b", "body", "c", "d"]);
    let list = doc.create_list(&ListDefinition::numbered()).unwrap();
    for &i in [0, 1, 3, 4].iter() {
        doc.set_list_item(ids[i], list, 0).unwrap();
    }
    doc.commit();
    assert_eq!(markers(&doc, &ids), ["1.", "2.", "-", "3.", "4."]);
    doc.set_list_restart(ids[3], Some(1)).unwrap();
    doc.commit();
    assert_eq!(markers(&doc, &ids), ["1.", "2.", "-", "1.", "2."]);
    doc.set_list_restart(ids[3], None).unwrap();
    doc.commit();
    assert_eq!(markers(&doc, &ids), ["1.", "2.", "-", "3.", "4."]);
    let numbering = doc.list_numbering();
    assert_eq!(numbering.sequence, vec![ids[0], ids[1], ids[3], ids[4]]);
}

#[test]
fn deeper_levels_reset_when_a_shallower_level_advances() {
    let (doc, ids) = doc_with(&["1", "1a", "1b", "2", "2a"]);
    let list = doc.create_list(&ListDefinition::numbered()).unwrap();
    for (i, level) in [0, 1, 1, 0, 1].into_iter().enumerate() {
        doc.set_list_item(ids[i], list, level).unwrap();
    }
    doc.commit();
    assert_eq!(markers(&doc, &ids), ["1.", "a.", "b.", "2.", "a."]);
}

#[test]
fn nine_levels_with_a_legal_template() {
    let texts: Vec<String> = (0..9).map(|n| format!("level {n}")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let (doc, ids) = doc_with(&refs);
    let mut def = ListDefinition::default();
    for n in 0..9u8 {
        let text: String = (1..=n + 1).map(|k| format!("%{k}.")).collect();
        def.levels.insert(
            n,
            LevelDef {
                format: Some(NumberFormat::Decimal),
                text: Some(text),
                ..LevelDef::default()
            },
        );
    }
    let list = doc.create_list(&def).unwrap();
    for (n, &id) in ids.iter().enumerate() {
        doc.set_list_item(id, list, n as u8).unwrap();
    }
    doc.commit();
    assert_eq!(doc.list_item(ids[8]).unwrap().marker, "1.1.1.1.1.1.1.1.1.");
    assert_eq!(doc.list_item(ids[8]).unwrap().level, 8);
    assert!(matches!(
        doc.set_list_item(ids[0], list, 9),
        Err(DocError::List(_))
    ));
}

#[test]
fn split_copies_role_list_and_level_but_not_restart_and_join_keeps_the_first() {
    let (doc, ids) = doc_with(&["heading text"]);
    let p = ids[0];
    let list = doc.create_list(&ListDefinition::bulleted()).unwrap();
    doc.set_role(p, Role::Heading(2)).unwrap();
    doc.set_list_item(p, list, 3).unwrap();
    doc.set_list_restart(p, Some(7)).unwrap();
    doc.commit();
    let q = doc.split_block(p, 7).unwrap();
    doc.commit();
    assert_eq!(doc.role(q).unwrap(), Role::Heading(2));
    let item = doc.list_item(q).unwrap();
    assert_eq!((item.list, item.level, item.restart), (list, 3, false));
    assert!(doc.list_item(p).unwrap().restart);
    // Join: the first paragraph's role wins.
    doc.set_role(q, Role::Quote).unwrap();
    doc.commit();
    doc.join_blocks(p, q).unwrap();
    doc.commit();
    assert_eq!(doc.role(p).unwrap(), Role::Heading(2));
    assert_eq!(doc.block(p).unwrap().text.to_string(), "heading text");
}

#[test]
fn annotation_splits_carry_semantics() {
    let doc = Document::new(1).unwrap();
    let note = doc
        .append_block(BlockKind::Annotation, "note", "note text")
        .unwrap();
    doc.commit();
    doc.set_role(note, Role::Code).unwrap();
    doc.commit();
    let q = doc.split_block(note, 4).unwrap();
    doc.commit();
    assert_eq!(doc.role(q).unwrap(), Role::Code);
}

#[test]
fn undo_restores_roles_lists_and_definition_ids() {
    let (doc, ids) = doc_with(&["one", "two"]);
    let mut undo = doc.undo_stack();
    let list = doc.stage_list(&ListDefinition::numbered()).unwrap();
    doc.activate_list(list).unwrap();
    doc.set_list_item(ids[0], list, 0).unwrap();
    doc.set_list_item(ids[1], list, 0).unwrap();
    doc.set_role(ids[0], Role::Title).unwrap();
    doc.commit_step();
    assert_eq!(markers(&doc, &ids), ["1.", "2."]);
    assert!(undo.undo().unwrap());
    assert_eq!(markers(&doc, &ids), ["-", "-"]);
    assert_eq!(doc.role(ids[0]).unwrap(), Role::Body);
    assert!(!doc.is_list_live(list));
    assert!(undo.redo().unwrap());
    assert_eq!(markers(&doc, &ids), ["1.", "2."]);
    assert!(doc.is_list_live(list), "the same definition ID comes back");
    assert_eq!(doc.role(ids[0]).unwrap(), Role::Title);

    doc.delete_list(list).unwrap();
    doc.commit_step();
    assert_eq!(markers(&doc, &ids), ["-", "-"]);
    assert_eq!(note_codes(&doc, ids[0]), [codes::LIST_DEFINITION_MISSING.as_str()]);
    assert!(undo.undo().unwrap());
    assert_eq!(markers(&doc, &ids), ["1.", "2."]);
}

#[test]
fn concurrent_indents_of_different_items_converge() {
    let (a, ids) = doc_with(&["x", "y", "z"]);
    let list = a.create_list(&ListDefinition::numbered()).unwrap();
    for &id in &ids {
        a.set_list_item(id, list, 0).unwrap();
    }
    a.commit();
    let b = a.fork(2).unwrap();
    a.set_list_level(ids[1], 1).unwrap();
    b.set_list_level(ids[2], 1).unwrap();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    assert_eq!(markers(&a, &ids), ["1.", "a.", "b."]);
    assert_eq!(markers(&a, &ids), markers(&b, &ids));
}

#[test]
fn concurrent_conversions_of_the_same_paragraphs_converge_into_one_list() {
    let (a, ids) = doc_with(&["x", "y", "z"]);
    let b = a.fork(2).unwrap();
    for doc in [&a, &b] {
        let list = doc.create_list(&ListDefinition::numbered()).unwrap();
        for &id in &ids {
            doc.set_list_item(id, list, 0).unwrap();
        }
        doc.commit();
    }
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    assert_eq!(markers(&a, &ids), markers(&b, &ids));
    let lists: Vec<ListId> = ids.iter().map(|&i| a.list_item(i).unwrap().list).collect();
    assert!(lists.iter().all(|&l| l == lists[0]), "{lists:?}");
    assert_eq!(markers(&a, &ids), ["1.", "2.", "3."]);
}

#[test]
fn deleting_a_definition_while_another_peer_adds_an_item() {
    let (a, ids) = doc_with(&["x", "y"]);
    let list = a.create_list(&ListDefinition::bulleted()).unwrap();
    a.set_list_item(ids[0], list, 0).unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    a.delete_list(list).unwrap();
    b.set_list_item(ids[1], list, 0).unwrap();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    for doc in [&a, &b] {
        assert_eq!(markers(doc, &ids), ["-", "-"]);
        assert_eq!(
            note_codes(doc, ids[1]),
            [codes::LIST_DEFINITION_MISSING.as_str()]
        );
        assert_eq!(doc.list_membership(ids[1]).unwrap().unwrap().list, Some(list));
    }
}

#[test]
fn concurrent_edits_to_different_levels_of_a_definition_both_win() {
    let (a, ids) = doc_with(&["x", "y"]);
    let list = a.create_list(&ListDefinition::numbered()).unwrap();
    a.set_list_item(ids[0], list, 0).unwrap();
    a.set_list_item(ids[1], list, 1).unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    let mut da = a.list_definition(list).unwrap();
    da.levels.get_mut(&0).unwrap().format = Some(NumberFormat::UpperRoman);
    a.update_list(list, &da).unwrap();
    let mut db = b.list_definition(list).unwrap();
    db.levels.get_mut(&1).unwrap().text = Some("(%2)".into());
    b.update_list(list, &db).unwrap();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    assert_eq!(markers(&a, &ids), ["I.", "(a)"]);
    assert_eq!(markers(&b, &ids), ["I.", "(a)"]);
}

#[test]
fn hostile_membership_and_definitions_are_reported_never_panic() {
    let (doc, ids) = doc_with(&["a", "b", "c", "d", "e", "f"]);
    let list = doc.create_list(&ListDefinition::numbered()).unwrap();
    // Out-of-range level, written raw.
    doc.set_list_item(ids[0], list, 0).unwrap();
    doc.meta_of(ids[0]).unwrap().insert(LEVEL, 99i64).unwrap();
    // Unreadable list ID and a dangling one.
    doc.meta_of(ids[1]).unwrap().insert(LIST, "not an id").unwrap();
    doc.meta_of(ids[2]).unwrap().insert(LIST, "999@77").unwrap();
    // Enormous restart values, and roman past 3,999.
    let roman = doc
        .create_list(&ListDefinition {
            base: None,
            levels: [(
                0,
                LevelDef {
                    format: Some(NumberFormat::UpperRoman),
                    start: Some(3999),
                    ..LevelDef::default()
                },
            )]
            .into(),
        })
        .unwrap();
    doc.set_list_item(ids[3], roman, 0).unwrap();
    doc.set_list_item(ids[4], roman, 0).unwrap();
    doc.set_list_item(ids[5], list, 0).unwrap();
    doc.set_list_restart(ids[5], Some(i64::MAX)).unwrap();
    // Unknown role and an unknown format.
    doc.meta_of(ids[1]).unwrap().insert(ROLE, "sidebar").unwrap();
    doc.commit();
    let n = doc.list_numbering();
    assert_eq!(n.items[&ids[0]].level, 8);
    assert_eq!(note_codes(&doc, ids[0]), [codes::LIST_LEVEL_CLAMPED.as_str()]);
    assert_eq!(note_codes(&doc, ids[1]), [codes::LIST_DEFINITION_MISSING.as_str()]);
    assert_eq!(note_codes(&doc, ids[2]), [codes::LIST_DEFINITION_MISSING.as_str()]);
    assert_eq!(n.items[&ids[3]].marker, "MMMCMXCIX.");
    assert_eq!(n.items[&ids[4]].marker, "4000.");
    assert_eq!(note_codes(&doc, ids[4]), [codes::LIST_COUNTER_RANGE.as_str()]);
    assert_eq!(n.items[&ids[5]].marker, format!("{}.", i64::MAX));
    let stored = doc.stored_role(ids[1]).unwrap();
    assert_eq!(stored.role, Role::Body);
    assert_eq!(stored.note().unwrap().code, codes::ROLE_UNKNOWN);
    // A later item after i64::MAX saturates rather than overflowing.
    let len = doc.block(ids[5]).unwrap().text.len();
    let g = doc.split_block(ids[5], len).unwrap();
    doc.commit();
    assert_eq!(doc.list_item(g).unwrap().value, i64::MAX);
    // Restart written as the wrong type is ignored.
    doc.meta_of(g).unwrap().insert(RESTART, "x").unwrap();
    doc.meta_of(g).unwrap().insert(LEVEL, LoroValue::Null).unwrap();
    doc.commit();
    assert_eq!(doc.list_item(g).unwrap().level, 0);
}

#[test]
fn cyclic_and_missing_bases_stop_inheritance() {
    let (doc, ids) = doc_with(&["a", "b"]);
    let x = doc.create_list(&ListDefinition::default()).unwrap();
    let y = doc
        .create_list(&ListDefinition {
            base: Some(x),
            levels: [(
                0,
                LevelDef {
                    format: Some(NumberFormat::UpperAlpha),
                    ..LevelDef::default()
                },
            )]
            .into(),
        })
        .unwrap();
    doc.update_list(
        x,
        &ListDefinition {
            base: Some(y),
            levels: Default::default(),
        },
    )
    .unwrap();
    doc.set_list_item(ids[0], x, 0).unwrap();
    doc.set_list_item(ids[1], y, 0).unwrap();
    doc.commit();
    assert_eq!(markers(&doc, &ids), ["A.", "A."]);
    assert_eq!(note_codes(&doc, ids[0]), [codes::LIST_DEFINITION_CYCLE.as_str()]);
    doc.delete_list(y).unwrap();
    doc.commit();
    assert_eq!(markers(&doc, &ids), ["1.", "-"]);
    assert_eq!(note_codes(&doc, ids[0]), [codes::LIST_DEFINITION_MISSING.as_str()]);
}

#[test]
fn a_hundred_thousand_items_number_in_one_pass() {
    let doc = Document::new(1).unwrap();
    let list = doc.create_list(&ListDefinition::numbered()).unwrap();
    let mut last = None;
    for i in 0..100_000 {
        let id = doc.append_block(BlockKind::Paragraph, "", "item").unwrap();
        doc.set_list_item(id, list, (i % 2) as u8).unwrap();
        last = Some(id);
    }
    doc.commit();
    let n = doc.list_numbering();
    assert_eq!(n.sequence.len(), 100_000);
    // Level 1 items restart under each level-0 item.
    assert_eq!(n.items[&last.unwrap()].marker, "a.");
    assert_eq!(n.items[&n.sequence[99_998]].marker, "50000.");
    assert!(std::sync::Arc::ptr_eq(&n, &doc.list_numbering()), "cached per revision");
}

#[test]
fn outline_lists_headings_in_document_order() {
    let (doc, ids) = doc_with(&["Title", "Intro", "Body", "Details"]);
    doc.set_role(ids[0], Role::Title).unwrap();
    doc.set_role(ids[1], Role::Heading(1)).unwrap();
    doc.set_role(ids[3], Role::Heading(2)).unwrap();
    doc.commit();
    let outline = doc.outline();
    assert_eq!(
        outline
            .iter()
            .map(|e| (e.node, e.level, e.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(ids[1], 1, "Intro"), (ids[3], 2, "Details")]
    );
    assert!(doc.set_role(ids[2], Role::Heading(7)).is_err());
}

#[test]
fn feature_detection_tracks_roles_lists_and_history() {
    let (doc, ids) = doc_with(&["a", "b"]);
    assert!(!doc.has_block_semantics());
    assert!(doc.list_numbering().items.is_empty());
    // A dangling membership without any lists tree is still reported.
    doc.meta_of(ids[0]).unwrap().insert(LIST, "3@9").unwrap();
    doc.commit();
    assert_eq!(note_codes(&doc, ids[0]), [codes::LIST_DEFINITION_MISSING.as_str()]);
    doc.meta_of(ids[0]).unwrap().insert(LIST, "").unwrap();
    doc.set_role(ids[1], Role::Body).unwrap();
    doc.commit();
    assert!(doc.has_block_semantics(), "a break record holds role1");
    let (doc, ids) = doc_with(&["a"]);
    let list = doc.create_list(&ListDefinition::bulleted()).unwrap();
    doc.delete_list(list).unwrap();
    doc.commit();
    assert!(doc.has_block_semantics(), "the lists tree is retained");
    let _ = ids;
}

#[test]
fn definitions_validate_on_write() {
    let doc = Document::new(1).unwrap();
    let bad = |levels: Vec<(u8, LevelDef)>| ListDefinition {
        base: None,
        levels: levels.into_iter().collect(),
    };
    for def in [
        bad(vec![(9, LevelDef::default())]),
        bad(vec![(
            0,
            LevelDef {
                text: Some("x".repeat(65)),
                ..LevelDef::default()
            },
        )]),
        bad(vec![(
            0,
            LevelDef {
                format: Some(NumberFormat::Unknown("klingon".into())),
                ..LevelDef::default()
            },
        )]),
        bad(vec![(
            0,
            LevelDef {
                indent: Some(reprise_geom::Length(-1)),
                ..LevelDef::default()
            },
        )]),
    ] {
        assert!(doc.create_list(&def).is_err());
    }
    assert!(doc.lists().is_empty());
}
