//! Deletion as a move into the trash (07, 29): identity under delete, undo,
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
        "the trash root is created under another peer ID: no counter of ours is spent on it"
    );
    // A second deletion finds the same trash.
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
    assert!(doc.is_trashed(b));
    assert_eq!(doc.document_order(), [a, c]);
    assert_eq!(doc.children(None), [a, c]);
    assert_eq!(doc.parent_of(b), None);
    assert_eq!(doc.kind_of(b), None);
    assert_eq!(doc.resolve_range(r), RangeState::Missing { node: Some(b) });
    assert!(doc.delete_block(b).is_err(), "already deleted");

    doc.restore_block(b, None, 1).unwrap();
    assert_eq!(doc.blocks(), [a, b, c], "same ID, same place");
    assert_eq!(
        doc.resolve_range(r),
        RangeState::Valid {
            node: b,
            bytes: 0..1
        }
    );
    assert!(doc.restore_block(b, None, 0).is_err(), "it's alive");
    assert!(!doc.is_trashed(b));
}

#[test]
fn a_trashed_block_takes_its_subtree_and_brings_it_back() {
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
    doc.restore_block(parent, None, 0).unwrap();
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
            live == [a, c] || live == [a, c, b],
            "one of the two wins: {live:?}"
        );
        assert_eq!(one.is_trashed(b), two.is_trashed(b));
    }
}

#[test]
fn concurrent_restores_converge_and_keep_the_id() {
    let one = Document::new(1).unwrap();
    let (a, b) = (para(&one, "a"), para(&one, "b"));
    one.delete_block(b).unwrap();
    let two = one.fork(2).unwrap();
    one.restore_block(b, None, 0).unwrap();
    two.restore_block(b, None, 1).unwrap();
    exchange(&one, &two);
    assert_eq!(one.blocks(), two.blocks());
    assert!(one.is_live(b) && one.blocks().contains(&a));
}

#[test]
fn a_peer_whose_derived_trash_id_is_taken_still_deletes() {
    // The ID that would create peer 1's trash is 1 ^ BIT. Another replica
    // already wrote under it, so peer 1 must use its own ID.
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
fn every_replica_has_its_own_trash() {
    let one = Document::new(1).unwrap();
    let (a, b) = (para(&one, "a"), para(&one, "b"));
    let two = one.fork(2).unwrap();
    one.delete_block(a).unwrap();
    two.delete_block(b).unwrap();
    exchange(&one, &two);
    assert_eq!(one.blocks(), []);
    assert_eq!(two.blocks(), []);
    one.restore_block(b, None, 0).unwrap();
    two.restore_block(a, None, 0).unwrap();
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
    assert!(doc.is_trashed(staged));
    assert_eq!(undo.undo_count(), 0, "staging is outside the history");
    doc.restore_block(staged, None, 1).unwrap();
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
fn deleting_a_relation_is_a_trash_move_too() {
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

#[test]
fn moving_blocks_counts_live_siblings_and_refuses_cycles() {
    let doc = Document::new(1).unwrap();
    let (a, b, c) = (para(&doc, "a"), para(&doc, "b"), para(&doc, "c"));
    let gone = para(&doc, "gone");
    doc.delete_block(gone).unwrap();
    doc.move_block(a, None, 2).unwrap();
    assert_eq!(doc.blocks(), [b, c, a]);
    doc.move_block(a, None, 0).unwrap();
    assert_eq!(doc.blocks(), [a, b, c]);
    assert!(doc.move_block(a, None, 3).is_err(), "past the end");
    doc.move_block(c, Some(a), 0).unwrap();
    assert_eq!(doc.blocks(), [a, b]);
    assert_eq!(doc.children(Some(a)), [c]);
    assert!(doc.move_block(a, Some(a), 0).is_err());
    assert!(
        doc.move_block(a, Some(c), 0).is_err(),
        "into its own subtree"
    );
    assert!(doc.move_block(a, Some(gone), 0).is_err(), "into the trash");
    assert!(doc.move_block(gone, None, 0).is_err(), "a deleted block");
    assert_eq!(doc.blocks(), [a, b]);
}

#[test]
fn inserting_checks_its_parent_and_index() {
    let doc = Document::new(1).unwrap();
    let a = para(&doc, "a");
    let new = NewBlock::new(BlockKind::Paragraph, "body", "x");
    assert!(doc.insert_block_at(None, 2, &new).is_err());
    let x = doc.insert_block_at(None, 0, &new).unwrap();
    let y = doc.insert_block_at(None, 2, &new).unwrap();
    assert_eq!(doc.blocks(), [x, a, y]);
    doc.delete_block(a).unwrap();
    assert!(doc.insert_block_at(Some(a), 0, &new).is_err());
    assert_eq!(doc.blocks(), [x, y]);
}

#[test]
fn splitting_and_joining_keep_ids_text_and_succession() {
    let doc = Document::new(1).unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "body", "héllo world")
        .unwrap();
    doc.set_overrides(
        a,
        &reprise_doc::Style {
            family: Some("serif".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let after = para(&doc, "after");
    assert!(doc.split_block(a, 2).is_err(), "inside the é");
    let b = doc.split_block(a, "héllo".len()).unwrap();
    assert_eq!(doc.blocks(), [a, b, after]);
    assert_eq!(texts(&doc), ["héllo", " world", "after"]);
    let moved = doc.block(b).unwrap();
    assert_eq!(
        (moved.kind, moved.style.as_deref()),
        (BlockKind::Paragraph, Some("body"))
    );
    assert_eq!(moved.overrides.family.as_deref(), Some("serif"));

    // Joining `b` back: relations that named it follow the text.
    doc.join_blocks(a, b).unwrap();
    assert_eq!(doc.blocks(), [a, after]);
    assert_eq!(texts(&doc), ["héllo world", "after"]);
    assert_eq!(doc.successors(b), [a]);
    assert!(doc.join_blocks(a, a).is_err());
    assert!(doc.join_blocks(a, b).is_err(), "b is deleted");
    let note = doc
        .append_block(BlockKind::Annotation, "note", "n")
        .unwrap();
    assert!(doc.join_blocks(a, note).is_err(), "different kinds");
    assert!(doc.split_block(a, 999).is_err());
    assert_eq!(doc.blocks(), [a, after, note], "refusals change nothing");
}

#[test]
fn two_peers_edit_and_each_undoes_only_their_own_text() {
    let one = Document::new(1).unwrap();
    let p = para(&one, "abc");
    let two = one.fork(2).unwrap();
    let mut undo = one.undo_stack();
    // I type at the start, you type at the end, I undo.
    one.block(p).unwrap().text.insert(0, "ME ").unwrap();
    one.commit_step();
    two.block(p).unwrap().text.insert(3, " YOU").unwrap();
    exchange(&one, &two);
    assert_eq!(texts(&one), ["ME abc YOU"]);
    assert!(undo.undo().unwrap());
    assert_eq!(texts(&one), ["abc YOU"], "only my text went");
    exchange(&one, &two);
    assert_eq!(texts(&two), ["abc YOU"]);
    assert!(undo.redo().unwrap());
    assert_eq!(texts(&one), ["ME abc YOU"]);
}
