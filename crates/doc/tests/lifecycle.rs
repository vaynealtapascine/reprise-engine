//! Soft deletion by metadata flag (07, 29): identity under delete, undo,
//! redo and concurrency, and the structural operations built on it.

use reprise_doc::relation::OnTargetDeleted;
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, Document, LayoutQuery, NewBlock, NodeId, RangeState, Relation, SchemaRegistry,
    Target,
};

const BIT: u64 = 1 << 62;

fn para(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "body", text)
        .unwrap()
}

fn texts(doc: &Document) -> Vec<String> {
    doc.blocks()
        .into_iter()
        .map(|b| doc.block(b).unwrap().text.to_string())
        .collect()
}

fn exchange(a: &Document, b: &Document) {
    a.merge(b).unwrap();
    b.merge(a).unwrap();
}

#[test]
fn deleting_costs_one_counter_so_later_ids_do_not_move() {
    let doc = Document::new(1).unwrap();
    let a = para(&doc, "a");
    doc.commit();
    let before = doc.revision();
    doc.delete_block(a).unwrap();
    doc.commit();
    let after = doc.revision();
    assert_eq!(
        (after.0.len(), after.0[0].0, after.0[0].1 - before.0[0].1),
        (1, 1, 1),
        "a flag write consumes exactly one operation on the real peer"
    );
    // Subsequent deletions also consume one operation.
    let b = para(&doc, "b");
    doc.commit();
    let before = doc.revision();
    doc.delete_block(b).unwrap();
    doc.commit();
    assert_eq!(doc.revision().0[0].1 - before.0[0].1, 1);
}

#[test]
fn a_deleted_block_is_gone_for_every_question_and_can_be_restored() {
    let doc = Document::new(1).unwrap();
    let (a, b, c) = (para(&doc, "a"), para(&doc, "b"), para(&doc, "c"));
    let r = doc.add_range(b, 0..1, RangePolicy::FIXED).unwrap();
    doc.delete_block(b).unwrap();
    assert_eq!(doc.blocks(), [a, c]);
    assert!(doc.block(b).is_err());
    assert!(!doc.is_live(b));
    assert!(doc.is_soft_deleted(b));
    assert_eq!(doc.document_order(), [a, c]);
    assert_eq!(doc.children(None), [a, c]);
    assert_eq!(doc.parent_of(b), None);
    assert_eq!(doc.kind_of(b), None);
    assert_eq!(doc.resolve_range(r), RangeState::Missing { node: Some(b) });
    assert!(doc.delete_block(b).is_err(), "already deleted");

    doc.restore_block(b).unwrap();
    assert_eq!(doc.blocks(), [a, b, c], "same ID, same place");
    assert_eq!(
        doc.resolve_range(r),
        RangeState::Valid {
            node: b,
            bytes: 0..1
        }
    );
    assert!(doc.restore_block(b).is_err(), "it's alive");
    assert!(!doc.is_soft_deleted(b));
}

#[test]
fn a_flagged_block_takes_its_subtree_and_brings_it_back() {
    let doc = Document::new(1).unwrap();
    let parent = para(&doc, "parent");
    let child = doc
        .insert_block_at(
            Some(parent),
            0,
            &NewBlock::new(BlockKind::Paragraph, "body", "child"),
        )
        .unwrap();
    assert_eq!(doc.children(Some(parent)), [child]);
    assert_eq!(doc.document_order(), [parent, child]);
    doc.delete_block(parent).unwrap();
    assert!(!doc.is_live(child), "its subtree is gone with it");
    assert_eq!(doc.document_order(), []);
    assert_eq!(doc.children(Some(parent)), []);
    doc.restore_block(parent).unwrap();
    assert_eq!(doc.document_order(), [parent, child]);
    assert_eq!(doc.parent_of(child), Some(Some(parent)));
}

#[test]
fn undo_of_a_deletion_is_the_same_node_in_the_same_place() {
    let doc = Document::new(1).unwrap();
    let (a, b, c) = (para(&doc, "a"), para(&doc, "b"), para(&doc, "c"));
    let mut undo = doc.undo_stack();
    doc.delete_block(b).unwrap();
    doc.commit_step();
    assert_eq!(doc.blocks(), [a, c]);
    assert!(undo.undo().unwrap());
    assert_eq!(doc.blocks(), [a, b, c]);
    assert!(undo.redo().unwrap());
    assert_eq!(doc.blocks(), [a, c]);
    assert!(undo.undo().unwrap());
    assert_eq!(doc.blocks(), [a, b, c]);
    assert!(!undo.can_undo());
}

#[test]
fn relations_work_again_after_undoing_the_deletion_of_their_target() {
    let doc = Document::new(1).unwrap();
    let schemas = SchemaRegistry::builtin();
    let note = doc
        .append_block(BlockKind::Annotation, "note", "n")
        .unwrap();
    let text = para(&doc, "some text");
    let range = doc.add_range(text, 0..4, RangePolicy::FIXED).unwrap();
    let rel = doc
        .add_relation(
            &schemas,
            &Relation::new(reprise_doc::relation::builtin::FOLLOW)
                .owned_by(note)
                .target(
                    "line",
                    Target::Layout(LayoutQuery::LineContaining { range }),
                ),
        )
        .unwrap();
    doc.commit();
    let mut undo = doc.undo_stack();
    doc.delete_block(text).unwrap();
    doc.commit_step();
    assert_eq!(
        doc.resolve_range(range),
        RangeState::Missing { node: Some(text) }
    );
    let mut history = reprise_doc::HistoryCache::default();
    let schema = schemas
        .get(&reprise_doc::relation::builtin::FOLLOW)
        .unwrap();
    let relation = doc.relations().remove(0).1.unwrap();
    let gone = doc.resolve_relation(schema, &relation, &mut history);
    assert!(gone.targets[0].outcome.is_deleted());
    assert_eq!(schema.on_target_deleted, OnTargetDeleted::Rebind);

    undo.undo().unwrap();
    assert_eq!(
        doc.resolve_range(range),
        RangeState::Valid {
            node: text,
            bytes: 0..4
        }
    );
    let back = doc.resolve_relation(schema, &relation, &mut history);
    assert!(!back.targets[0].outcome.is_deleted());
    assert_eq!(doc.relations()[0].0, rel, "the relation is untouched");
}

#[test]
fn undo_after_a_concurrent_edit_inside_the_deleted_block_keeps_that_edit() {
    let one = Document::new(1).unwrap();
    let a = para(&one, "keep");
    let b = para(&one, "hello");
    let two = one.fork(2).unwrap();
    let mut undo = one.undo_stack();
    one.delete_block(b).unwrap();
    one.commit_step();
    // The other peer types into the block while it is deleted here.
    two.block(b).unwrap().text.insert(5, ", world").unwrap();
    exchange(&one, &two);
    assert_eq!(one.blocks(), [a], "a concurrent edit does not resurrect");
    assert_eq!(two.blocks(), [a]);
    assert!(undo.undo().unwrap());
    assert_eq!(one.blocks(), [a, b]);
    assert_eq!(
        one.block(b).unwrap().text.to_string(),
        "hello, world",
        "the same node, with the other peer's edit"
    );
    exchange(&one, &two);
    assert_eq!(texts(&one), texts(&two));
}

#[test]
fn a_concurrent_move_and_delete_converge_on_both_replicas() {
    for first in [true, false] {
        let one = Document::new(1).unwrap();
        let (a, b, c) = (para(&one, "a"), para(&one, "b"), para(&one, "c"));
        let two = one.fork(2).unwrap();
        one.delete_block(b).unwrap();
        two.move_block(b, None, 2).unwrap();
        if first {
            exchange(&one, &two);
        } else {
            exchange(&two, &one);
        }
        assert_eq!(one.blocks(), two.blocks(), "replicas agree");
        let live = one.blocks();
        assert!(
            live == [a, c],
            "a concurrent move cannot clear the deletion flag: {live:?}"
        );
        assert_eq!(one.is_soft_deleted(b), two.is_soft_deleted(b));
    }
}

#[test]
fn concurrent_restores_converge_and_keep_the_id() {
    let one = Document::new(1).unwrap();
    let (a, b) = (para(&one, "a"), para(&one, "b"));
    one.delete_block(b).unwrap();
    let two = one.fork(2).unwrap();
    one.restore_block(b).unwrap();
    two.restore_block(b).unwrap();
    exchange(&one, &two);
    assert_eq!(one.blocks(), two.blocks());
    assert!(one.is_live(b) && one.blocks().contains(&a));
}

#[test]
fn concurrent_delete_and_restore_converge_by_map_lww() {
    for reversed in [false, true] {
        let one = Document::new(1).unwrap();
        let b = para(&one, "b");
        one.delete_block(b).unwrap();
        let two = one.fork(2).unwrap();
        one.restore_block(b).unwrap();
        // Write a fresh deletion concurrent with one's restore.
        two.restore_block(b).unwrap();
        two.delete_block(b).unwrap();
        if reversed {
            exchange(&two, &one);
        } else {
            exchange(&one, &two);
        }
        assert_eq!(one.is_live(b), two.is_live(b));
        assert!(
            !one.is_live(b),
            "the later Lamport write wins, on both peers"
        );
        one.restore_block(b).unwrap();
        exchange(&one, &two);
        assert_eq!(one.blocks(), [b]);
        assert_eq!(two.blocks(), [b]);
    }
}

#[test]
fn independently_deleted_children_stay_deleted_when_parent_is_restored() {
    let doc = Document::new(1).unwrap();
    let parent = para(&doc, "parent");
    let child = doc
        .insert_block_at(
            Some(parent),
            0,
            &NewBlock::new(BlockKind::Paragraph, "body", "child"),
        )
        .unwrap();
    doc.delete_block(child).unwrap();
    doc.delete_block(parent).unwrap();
    assert!(
        doc.restore_block(child).is_err(),
        "cannot restore under a deleted parent"
    );
    doc.restore_block(parent).unwrap();
    assert!(doc.children(Some(parent)).is_empty());
    doc.restore_block(child).unwrap();
    assert_eq!(doc.children(Some(parent)), [child]);
}

#[test]
fn any_real_peer_id_can_collaborate_with_deletion() {
    // The former synthetic peer ID is an ordinary valid collaborator now.
    let one = Document::new(1).unwrap();
    let a = para(&one, "a");
    let squatter = one.fork(1 ^ BIT).unwrap();
    squatter
        .append_block(BlockKind::Paragraph, "body", "x")
        .unwrap();
    one.merge(&squatter).unwrap();
    let mut undo = one.undo_stack();
    one.delete_block(a).unwrap();
    one.commit_step();
    assert!(!one.is_live(a));
    undo.undo().unwrap();
    assert!(one.is_live(a));
}

#[test]
fn replicas_delete_and_restore_independently() {
    let one = Document::new(1).unwrap();
    let (a, b) = (para(&one, "a"), para(&one, "b"));
    let two = one.fork(2).unwrap();
    one.delete_block(a).unwrap();
    two.delete_block(b).unwrap();
    exchange(&one, &two);
    assert_eq!(one.blocks(), []);
    assert_eq!(two.blocks(), []);
    one.restore_block(b).unwrap();
    two.restore_block(a).unwrap();
    exchange(&one, &two);
    assert_eq!(one.blocks().len(), 2);
    assert_eq!(one.blocks(), two.blocks());
}

#[test]
fn staged_blocks_are_invisible_until_placed_and_keep_their_id_through_undo() {
    let doc = Document::new(1).unwrap();
    let a = para(&doc, "a");
    let mut undo = doc.undo_stack();
    let staged = doc
        .stage_block(&NewBlock::new(BlockKind::Paragraph, "body", "new"))
        .unwrap();
    assert_eq!(doc.blocks(), [a]);
    assert!(doc.is_soft_deleted(staged));
    assert_eq!(undo.undo_count(), 0, "staging is outside the history");
    doc.activate_block_at(staged, None, 1).unwrap();
    doc.commit_step();
    assert_eq!(doc.blocks(), [a, staged]);
    assert_eq!(undo.undo_count(), 1);
    undo.undo().unwrap();
    assert_eq!(doc.blocks(), [a]);
    undo.redo().unwrap();
    assert_eq!(doc.blocks(), [a, staged], "redo brings back the same ID");
    assert_eq!(doc.block(staged).unwrap().text.to_string(), "new");
}

#[test]
fn deleting_a_relation_is_a_flag_write_too() {
    let doc = Document::new(1).unwrap();
    let schemas = SchemaRegistry::builtin();
    let note = doc
        .append_block(BlockKind::Annotation, "note", "n")
        .unwrap();
    let text = para(&doc, "text");
    let range = doc.add_range(text, 0..4, RangePolicy::FIXED).unwrap();
    let relation = Relation::new(reprise_doc::relation::builtin::FOLLOW)
        .owned_by(note)
        .target(
            "line",
            Target::Layout(LayoutQuery::LineContaining { range }),
        );
    let id = doc.add_relation(&schemas, &relation).unwrap();
    doc.commit();
    let mut undo = doc.undo_stack();
    doc.delete_relation(id).unwrap();
    doc.commit_step();
    assert!(doc.relations().is_empty());
    assert!(doc.delete_relation(id).is_err());
    undo.undo().unwrap();
    assert_eq!(doc.relations(), vec![(id, Ok(relation.clone()))]);
    undo.redo().unwrap();
    assert!(doc.relations().is_empty());
    // A staged relation is placed by restoring it, with its ID unchanged.
    let staged = doc.stage_relation(&schemas, &relation).unwrap();
    assert!(doc.relations().is_empty());
    doc.restore_relation(staged).unwrap();
    assert_eq!(doc.relations(), vec![(staged, Ok(relation))]);
}
