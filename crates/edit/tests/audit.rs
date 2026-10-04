//! Takeover blockers: these regressions intentionally fail on the inherited
//! implementation. Soft deletion must keep all three assertions passing.

use reprise_doc::{BlockKind, Document, SchemaRegistry};
use reprise_edit::{Command, Editor, Transaction};

#[test]
fn first_delete_after_text_is_one_step() {
    let doc = Document::new(1).unwrap();
    let a = doc.append_block(BlockKind::Paragraph, "body", "a").unwrap();
    let b = doc.append_block(BlockKind::Paragraph, "body", "b").unwrap();
    let mut editor = Editor::new(doc, SchemaRegistry::builtin());
    editor
        .apply(
            &Transaction::new()
                .with(Command::InsertText {
                    node: a,
                    at: 1,
                    text: "!".into(),
                })
                .with(Command::DeleteBlock { node: b }),
        )
        .unwrap();
    assert_eq!(editor.undo_count(), 1);
    editor.undo().unwrap();
    assert_eq!(editor.document().block(a).unwrap().text.to_string(), "a");
    assert!(editor.document().is_live(b));
}

#[test]
fn deletion_identity_does_not_collide_with_a_concurrent_real_peer() {
    let a = Document::new(1).unwrap();
    let block = a.append_block(BlockKind::Paragraph, "body", "a").unwrap();
    let b = a.fork(1 ^ (1 << 62)).unwrap();
    // This formerly synthetic ID remains a valid real collaborator.
    // Neither peer has seen the other's edits. Deletion must use only peer 1.
    a.delete_block(block).unwrap();
    a.commit();
    let other = b
        .append_block(BlockKind::Paragraph, "body", "other")
        .unwrap();
    assert_eq!(b.block(other).unwrap().text.to_string(), "other");
    b.commit();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    assert!(!a.is_live(block));
    assert!(
        a.is_live(other),
        "the collaborator's block must survive merge"
    );
    assert_eq!(a.block(other).unwrap().text.to_string(), "other");
    assert_eq!(a.blocks(), b.blocks());
}

#[test]
fn first_deletion_keeps_previous_transactions_undoable() {
    let doc = Document::new(1).unwrap();
    let a = doc.append_block(BlockKind::Paragraph, "body", "a").unwrap();
    let b = doc.append_block(BlockKind::Paragraph, "body", "b").unwrap();
    let mut editor = Editor::new(doc, SchemaRegistry::builtin());
    editor
        .apply_command(Command::InsertText {
            node: a,
            at: 1,
            text: "!".into(),
        })
        .unwrap();
    assert_eq!(editor.undo_count(), 1);
    editor
        .apply_command(Command::DeleteBlock { node: b })
        .unwrap();
    // The former trash helper switched peers and cleared this history.
    // A metadata flag uses the real peer and must preserve previous steps.
    assert_eq!(editor.undo_count(), 2);
    editor.undo().unwrap();
    assert!(editor.document().is_live(b));
    editor.undo().unwrap();
    assert_eq!(editor.document().block(a).unwrap().text.to_string(), "a");
}
