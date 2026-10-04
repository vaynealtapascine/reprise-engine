//! General document edit primitives used by kernel commands.

use reprise_doc::{BlockKind, Document, NewBlock, NodeId};

fn para(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "body", text)
        .unwrap()
}
fn texts(doc: &Document) -> Vec<String> {
    doc.blocks()
        .into_iter()
        .map(|n| doc.block(n).unwrap().text.to_string())
        .collect()
}
fn exchange(a: &Document, b: &Document) {
    a.merge(b).unwrap();
    b.merge(a).unwrap();
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
    assert!(
        doc.move_block(a, Some(gone), 0).is_err(),
        "under a deleted block"
    );
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
