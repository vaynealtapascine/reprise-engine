//! Inline object tests (see `docs/inline.md`).

use std::collections::BTreeMap;

use loro::LoroMap;
use reprise_text::{OBJECT, TextError};

use crate::inline::{INLINE, InlineObject, InlinePatch};
use crate::{BlockKind, Document, InlineAlign, NodeId, SchemaRegistry};

const ASSET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn text(doc: &Document, id: NodeId) -> String {
    doc.block(id).unwrap().text.to_string()
}

fn one(s: &str) -> (Document, NodeId) {
    let doc = Document::new(1).unwrap();
    let p = doc.append_block(BlockKind::Paragraph, "body", s).unwrap();
    doc.commit();
    (doc, p)
}

fn sync(from: &Document, to: &Document) {
    from.commit();
    to.commit();
    let packet = from.export_delta(&to.version_vector()).unwrap();
    to.import_packet(&packet).unwrap();
}

fn image() -> InlineObject {
    InlineObject::image(ASSET, "a cat").with("width", "pt:20480")
}

#[test]
fn an_object_is_one_character_of_its_paragraph() {
    let (doc, p) = one("see page  now");
    let field = InlineObject::field("page-number");
    let id = doc.insert_inline(p, 9, &field).unwrap();
    doc.commit_step();
    assert_eq!(text(&doc, p), format!("see page {OBJECT} now"));
    let objects = doc.inline_objects(p).unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!((objects[0].id, objects[0].at), (id, 9));
    assert_eq!(objects[0].object, field);
    assert_eq!(doc.inline_at(p, 9), Some(id));
    assert_eq!(doc.inline_location(id), Some((p, 9)));
    assert_eq!(crate::InlineId::parse(&id.to_string()), Some(id));
    assert!(doc.has_inline_objects());
}

#[test]
fn objects_are_reserved_in_authored_text() {
    let (doc, p) = one("ab");
    assert!(matches!(
        doc.block(p).unwrap().text.insert(1, "\u{FFFC}"),
        Err(TextError::Reserved)
    ));
    assert!(doc.insert_inline(p, 1, &InlineObject::default()).is_err());
    assert!(
        doc.insert_inline(p, 1, &InlineObject::image("nothex", ""))
            .is_err()
    );
}

#[test]
fn undo_and_redo_of_an_insertion_keep_the_object() {
    let (doc, p) = one("ab");
    let mut undo = doc.undo_stack();
    let id = doc.insert_inline(p, 1, &image()).unwrap();
    doc.commit_step();
    assert_eq!(undo.undo_count(), 1, "staging is not a step of its own");
    assert!(undo.undo().unwrap());
    assert_eq!(text(&doc, p), "ab");
    assert!(doc.inline_objects(p).unwrap().is_empty());
    assert!(undo.redo().unwrap());
    assert_eq!(doc.inline_at(p, 1), Some(id), "the same object comes back");
}

#[test]
fn deleting_an_object_retires_it_and_undo_restores_it() {
    let (doc, p) = one("abcd");
    let id = doc.insert_inline(p, 2, &image()).unwrap();
    doc.commit_step();
    let mut undo = doc.undo_stack();
    let len = doc.block(p).unwrap().text.len();
    doc.block(p).unwrap().text.delete(1..len - 1).unwrap();
    doc.commit_step();
    assert_eq!(text(&doc, p), "ad");
    assert!(doc.inline_location(id).is_none());
    // The character stays, hidden: its record is only inactive.
    assert!(doc.text_of_any(p).unwrap().to_string().contains(OBJECT));
    assert!(undo.undo().unwrap());
    assert_eq!(text(&doc, p), format!("ab{OBJECT}cd"));
    assert_eq!(doc.inline_location(id), Some((p, 2)));
    assert_eq!(doc.inline_object(id).unwrap(), image());
}

#[test]
fn concurrent_insert_next_to_an_object_converges() {
    let (a, p) = one("xy");
    a.insert_inline(p, 1, &image()).unwrap();
    a.commit_step();
    let b = a.fork(2).unwrap();
    a.block(p).unwrap().text.insert(1, "L").unwrap();
    b.block(p).unwrap().text.insert(4, "R").unwrap();
    sync(&a, &b);
    sync(&b, &a);
    assert_eq!(text(&a, p), text(&b, p));
    assert_eq!(text(&a, p), format!("xL{OBJECT}Ry"));
}

#[test]
fn deleting_while_another_peer_resizes_converges_on_a_deleted_resized_object() {
    let (a, p) = one("xy");
    let id = a.insert_inline(p, 1, &image()).unwrap();
    a.commit_step();
    let b = a.fork(2).unwrap();
    let mut undo = a.undo_stack();
    a.block(p).unwrap().text.delete(1..4).unwrap();
    a.commit_step();
    let mut patch = InlinePatch::default();
    patch.params.insert("width".into(), Some("pt:4096".into()));
    patch.align = Some(InlineAlign::Middle);
    b.update_inline(id, &patch).unwrap();
    b.commit_step();
    sync(&a, &b);
    sync(&b, &a);
    for doc in [&a, &b] {
        assert_eq!(text(doc, p), "xy");
        assert!(doc.inline_location(id).is_none());
        assert_eq!(
            doc.inline_object(id).unwrap().param("width"),
            Some("pt:4096")
        );
    }
    // Undoing the delete shows the object with the collaborator's size.
    assert!(undo.undo().unwrap());
    assert_eq!(a.inline_location(id), Some((p, 1)));
    let object = a.inline_object(id).unwrap();
    assert_eq!(object.param("width"), Some("pt:4096"));
    assert_eq!(object.align, InlineAlign::Middle);
}

#[test]
fn concurrent_changes_to_different_parameters_both_survive() {
    let (a, p) = one("x");
    let id = a.insert_inline(p, 0, &image()).unwrap();
    a.commit_step();
    let b = a.fork(2).unwrap();
    let patch = |k: &str, v: &str| InlinePatch {
        align: None,
        params: BTreeMap::from([(k.to_string(), Some(v.to_string()))]),
    };
    a.update_inline(id, &patch("width", "em:500")).unwrap();
    b.update_inline(id, &patch("alt", "a dog")).unwrap();
    sync(&a, &b);
    sync(&b, &a);
    for doc in [&a, &b] {
        let o = doc.inline_object(id).unwrap();
        assert_eq!(
            (o.param("width"), o.param("alt")),
            (Some("em:500"), Some("a dog"))
        );
    }
}

#[test]
fn flow_splits_and_joins_move_no_object() {
    let (doc, p) = one("abcd");
    let id = doc.insert_inline(p, 2, &image()).unwrap();
    doc.commit_step();
    let q = doc.split_block(p, 1).unwrap();
    doc.commit_step();
    assert_eq!(doc.inline_location(id), Some((q, 1)));
    doc.join_blocks(p, q).unwrap();
    doc.commit_step();
    assert_eq!(doc.inline_location(id), Some((p, 2)));
}

#[test]
fn copies_give_objects_new_identity() {
    let doc = Document::new(1).unwrap();
    let p = doc
        .append_block(BlockKind::Paragraph, "body", "ab")
        .unwrap();
    let r = doc
        .append_block(BlockKind::Paragraph, "body", "cd")
        .unwrap();
    doc.commit();
    let id = doc.insert_inline(r, 1, &image()).unwrap();
    doc.commit_step();
    // A join across hosts copies the text, with its object.
    let mut undo = doc.undo_stack();
    doc.join_blocks(p, r).unwrap();
    doc.commit_step();
    assert_eq!(text(&doc, p), format!("abc{OBJECT}d"));
    let copied = doc.inline_objects(p).unwrap();
    assert_eq!(copied.len(), 1);
    assert_ne!(copied[0].id, id);
    assert_eq!(copied[0].object, image());
    let copy = copied[0].id;
    assert!(undo.undo().unwrap());
    assert_eq!(text(&doc, p), "ab");
    assert_eq!(doc.inline_location(id), Some((r, 1)));
    assert!(undo.redo().unwrap());
    assert_eq!(doc.inline_at(p, 3), Some(copy), "redo keeps the copy's ID");
}

#[test]
fn a_copying_split_and_a_move_carry_their_objects() {
    let doc = Document::new(1).unwrap();
    let p = doc
        .append_block(BlockKind::Paragraph, "body", "abcd")
        .unwrap();
    doc.commit();
    doc.insert_inline(p, 3, &image()).unwrap();
    doc.commit_step();
    let q = doc.split_block(p, 2).unwrap();
    doc.commit_step();
    let staged = doc
        .stage_block(&crate::NewBlock::new(
            BlockKind::Paragraph,
            "body",
            &text(&doc, q),
        ))
        .unwrap();
    doc.move_by_copy(q, staged, None, 0).unwrap();
    doc.commit_step();
    assert_eq!(text(&doc, staged), format!("c{OBJECT}d"));
    assert_eq!(doc.inline_objects(staged).unwrap()[0].object, image());
}

#[test]
fn fragments_carry_objects_and_refuse_unmatched_markers() {
    let (doc, p) = one("abcd");
    doc.insert_inline(p, 2, &image()).unwrap();
    doc.commit_step();
    let fragment = doc
        .copy_fragment(
            "src",
            &[crate::fragment::CopyBlock {
                node: p,
                bytes: Some(1..6),
            }],
            &SchemaRegistry::builtin(),
        )
        .unwrap();
    assert_eq!(fragment.blocks[0].text, format!("b{OBJECT}c"));
    assert_eq!(fragment.blocks[0].inline, vec![image()]);
    let mut bad = fragment.clone();
    bad.blocks[0].inline.clear();
    assert!(bad.validate().is_err());
    let mut bad = fragment;
    bad.blocks[0].inline[0].kind = String::new();
    assert!(bad.validate().is_err());
}

#[test]
fn hostile_markers_and_records_are_hidden_and_audited() {
    let (doc, p) = one("ab");
    // A raw peer's marker with no record, and a record with no marker.
    doc.text_of_any(p)
        .unwrap()
        .loro()
        .insert(1, "\u{FFFC}")
        .unwrap();
    let orphan = doc
        .doc
        .get_map(INLINE)
        .insert_container("1@1/99@7", LoroMap::new())
        .unwrap();
    orphan.insert("v", 1).unwrap();
    orphan.insert("active", true).unwrap();
    // A future version of a record for a real marker.
    let id = doc.insert_inline(p, 0, &image()).unwrap();
    doc.commit();
    let Some(loro::ValueOrContainer::Container(loro::Container::Map(rec))) =
        doc.doc.get_map(INLINE).get(&id.to_string())
    else {
        panic!("the record exists");
    };
    rec.insert("v", 2).unwrap();
    doc.commit();
    assert_eq!(text(&doc, p), "ab", "unreadable markers are hidden");
    let audit = doc.inline_audit();
    assert_eq!(audit.orphan_markers.get(&p), Some(&2));
    assert_eq!(audit.orphan_records, vec!["1@1/99@7".to_string()]);
}

#[test]
fn field_targets_follow_their_text() {
    let (doc, p) = one("intro target");
    let raw = doc.field_target(p, 6).unwrap();
    let q = doc.split_block(p, 3).unwrap();
    doc.commit_step();
    assert_eq!(doc.resolve_field_target(&raw), Some((q, 3)));
    assert_eq!(doc.resolve_field_target("garbage"), None);
    assert_eq!(doc.resolve_field_target("1@1 zz"), None);
}

#[test]
fn fifty_thousand_objects_in_one_paragraph() {
    let (doc, p) = one("");
    let objects = vec![InlineObject::field("page-number"); 50_000];
    let marks: String = std::iter::repeat_n(OBJECT, 50_000).collect();
    doc.insert_with_inline(p, 0, &marks, &objects).unwrap();
    doc.commit_step();
    let live = doc.inline_objects(p).unwrap();
    assert_eq!(live.len(), 50_000);
    assert_eq!(live[49_999].at, 3 * 49_999);
    assert!(doc.inline_audit().orphan_records.is_empty());
}

/// A table with two cells, each holding a paragraph, so the paragraphs are in
/// different flow hosts and moving text between them copies it.
fn two_cells(first: &str, second: &str) -> (Document, NodeId, NodeId) {
    let doc = Document::new(1).unwrap();
    let table = doc
        .append_table(crate::TableColumns {
            columns: (0..2)
                .map(|_| crate::Column {
                    width: crate::ColumnWidth::Proportional(1),
                })
                .collect(),
        })
        .unwrap();
    let row = doc.append_table_row(table, false).unwrap();
    let left = doc.append_table_cell(row, 0).unwrap();
    let right = doc.append_table_cell(row, 1).unwrap();
    let a = doc
        .append_cell_block(left, BlockKind::Paragraph, "body", first)
        .unwrap();
    let b = doc
        .append_cell_block(right, BlockKind::Paragraph, "body", second)
        .unwrap();
    doc.commit();
    (doc, a, b)
}

#[test]
fn a_page_of_follows_its_target_through_a_copying_join() {
    // Joining paragraphs of different hosts copies the second's text onto the
    // first. A page-of in it, and a target in it, move with their text.
    let (doc, a, b) = two_cells("head", "see target");
    let to_target = doc.field_target(b, 4).unwrap();
    doc.insert_inline(
        b,
        3,
        &InlineObject::field("page-of").with("target", to_target.clone()),
    )
    .unwrap();
    doc.commit_step();
    let before = text(&doc, b);
    doc.join_blocks(a, b).unwrap();
    doc.commit_step();
    assert_eq!(text(&doc, a), format!("head{before}"));
    // The object came along, and its target still names the same character.
    let moved = doc.inline_objects(a).unwrap();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].at, "head".len() + 3);
    let (node, at) = doc.resolve_field_target(&to_target).unwrap();
    assert_eq!(node, a);
    assert_eq!(&text(&doc, a)[at..at + 6], "target");
}

#[test]
fn a_page_of_follows_its_target_through_a_copying_split() {
    let (doc, _, b) = two_cells("head", "see target and more");
    let to_target = doc.field_target(b, 4).unwrap();
    doc.insert_inline(
        b,
        3,
        &InlineObject::field("page-of").with("target", to_target.clone()),
    )
    .unwrap();
    doc.commit_step();
    // Split before the object: the tail, object and target, moves to a new block.
    let tail = doc.split_block(b, 2).unwrap();
    doc.commit_step();
    let (node, at) = doc.resolve_field_target(&to_target).unwrap();
    assert_eq!(node, tail);
    assert_eq!(&text(&doc, tail)[at..at + 6], "target");
    assert_eq!(doc.inline_objects(tail).unwrap().len(), 1);
    assert!(doc.inline_objects(b).unwrap().is_empty());
}

#[test]
fn a_page_of_aimed_at_the_character_after_a_partial_copy_keeps_its_position() {
    // "Hello World": the target is the space at byte 5. Copying "Hello" ends
    // there, so the space is not in the copy and the target is left alone;
    // copying through the space takes it.
    let (doc, p) = one("Hello World");
    let target = doc.field_target(p, 5).unwrap();
    doc.insert_inline(
        p,
        0,
        &InlineObject::field("page-of").with("target", target.clone()),
    )
    .unwrap();
    doc.commit_step();
    let copy = |bytes: std::ops::Range<usize>| {
        doc.copy_fragment(
            "src",
            &[crate::fragment::CopyBlock {
                node: p,
                bytes: Some(bytes),
            }],
            &SchemaRegistry::builtin(),
        )
        .unwrap()
        .blocks
        .remove(0)
        .inline
        .remove(0)
    };
    // The object is the first three bytes; the space is at 5 + 3 now.
    assert_eq!(copy(0..8).param("target"), Some(target.as_str()));
    let through = copy(0..9);
    assert_eq!(
        through.param("target"),
        Some(format!("copy:{p} 8").as_str())
    );
}
