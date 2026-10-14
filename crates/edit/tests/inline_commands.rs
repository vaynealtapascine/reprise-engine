//! Inline object commands inside transactions (`docs/inline.md`): validated
//! with the rest of the transaction, shown in one undoable step, and undone
//! and redone with the same identity.

use reprise_doc::text::OBJECT;
use reprise_doc::{BlockKind, Document, InlineObject, InlinePatch, NodeId, SchemaRegistry};
use reprise_edit::{Bias, Command, Editor, Reason, Transaction};

const ASSET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn para(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "body", text)
        .unwrap()
}

fn editor(doc: Document) -> Editor {
    Editor::new(doc, SchemaRegistry::builtin())
}

fn text(e: &Editor, node: NodeId) -> String {
    e.document().block(node).unwrap().text.to_string()
}

fn image() -> InlineObject {
    InlineObject::image(ASSET, "a cat").with("width", "pt:20480")
}

fn set(name: &str, value: &str) -> InlinePatch {
    InlinePatch {
        params: [(name.to_string(), Some(value.to_string()))].into(),
        ..InlinePatch::default()
    }
}

#[test]
fn insert_then_update_in_one_transaction_is_one_undoable_step_with_its_identity() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "see  now");
    let mut e = editor(doc);
    let applied = e
        .apply(
            &Transaction::new()
                .with(Command::InsertInline {
                    node: p,
                    at: 4,
                    object: image(),
                })
                .with(Command::UpdateInline {
                    node: p,
                    at: 4,
                    patch: set("width", "em:500"),
                }),
        )
        .unwrap();
    assert_eq!(e.undo_count(), 1);
    assert_eq!(text(&e, p), format!("see {OBJECT} now"));
    let id = e.document().inline_at(p, 4).expect("the object is shown");
    let shown = e.document().inline_object(id).unwrap();
    assert_eq!(shown.param("width"), Some("em:500"));
    assert_eq!(shown.param("alt"), Some("a cat"));

    // The object is three bytes of text: a caret after the insert lands past it.
    assert_eq!(applied.map_position(p, 4, Bias::After), Some((p, 7)));
    assert_eq!(applied.map_position(p, 4, Bias::Before), Some((p, 4)));

    assert!(e.undo().unwrap());
    assert_eq!(text(&e, p), "see  now");
    assert_eq!(e.document().inline_at(p, 4), None);

    assert!(e.redo().unwrap());
    assert_eq!(
        e.document().inline_at(p, 4),
        Some(id),
        "redo shows the same object"
    );
    assert_eq!(
        e.document().inline_object(id).unwrap().param("width"),
        Some("em:500")
    );
}

#[test]
fn an_update_finds_an_object_where_earlier_commands_in_the_batch_left_it() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "see  now");
    let id = doc.insert_inline(p, 4, &image()).unwrap();
    doc.commit_step();
    let mut e = editor(doc);

    // Three bytes before it: the object is now at 7.
    e.apply(
        &Transaction::new()
            .with(Command::InsertText {
                node: p,
                at: 0,
                text: "XYZ".into(),
            })
            .with(Command::UpdateInline {
                node: p,
                at: 7,
                patch: set("alt", "a dog"),
            }),
    )
    .unwrap();
    assert_eq!(e.document().inline_at(p, 7), Some(id));
    assert_eq!(
        e.document().inline_object(id).unwrap().param("alt"),
        Some("a dog")
    );
}

#[test]
fn an_update_at_a_position_the_batch_has_moved_away_from_is_refused() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "see  now");
    doc.insert_inline(p, 4, &image()).unwrap();
    doc.commit_step();
    let mut e = editor(doc);
    let before = text(&e, p);

    let err = e
        .apply(
            &Transaction::new()
                .with(Command::InsertText {
                    node: p,
                    at: 0,
                    text: "XYZ".into(),
                })
                .with(Command::UpdateInline {
                    node: p,
                    at: 4,
                    patch: set("alt", "a dog"),
                }),
        )
        .unwrap_err();
    assert_eq!(err.command, Some(1));
    assert!(matches!(err.reason, Reason::BadOffset { .. }));
    assert_eq!(text(&e, p), before);
    assert_eq!(
        e.undo_count(),
        0,
        "setup made before the editor is not in its history"
    );
}

#[test]
fn objects_inserted_out_of_order_in_one_batch_keep_their_positions() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "see  now");
    let mut e = editor(doc);
    e.apply(
        &Transaction::new()
            .with(Command::InsertInline {
                node: p,
                at: 8,
                object: InlineObject::image(ASSET, "second"),
            })
            .with(Command::InsertInline {
                node: p,
                at: 4,
                object: InlineObject::image(ASSET, "first"),
            }),
    )
    .unwrap();
    let objects = e.document().inline_objects(p).unwrap();
    assert_eq!(objects.iter().map(|o| o.at).collect::<Vec<_>>(), [4, 11]);
    assert_eq!(objects[1].object.param("alt"), Some("second"));

    e.apply(&Transaction::new().with(Command::UpdateInline {
        node: p,
        at: 11,
        patch: set("alt", "moved"),
    }))
    .unwrap();
    assert_eq!(
        e.document().inline_objects(p).unwrap()[1]
            .object
            .param("alt"),
        Some("moved")
    );
}

#[test]
fn a_later_invalid_command_refuses_the_whole_batch_before_anything_is_allocated() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "see  now");
    let mut e = editor(doc);
    let err = e
        .apply(
            &Transaction::new()
                .with(Command::InsertInline {
                    node: p,
                    at: 4,
                    object: image(),
                })
                .with(Command::UpdateInline {
                    node: p,
                    at: 99,
                    patch: set("width", "em:500"),
                }),
        )
        .unwrap_err();
    assert_eq!(err.command, Some(1));
    assert!(matches!(err.reason, Reason::BadOffset { .. }));
    assert_eq!(text(&e, p), "see  now");
    assert!(!e.document().has_inline_objects(), "no record was written");
    assert_eq!(e.undo_count(), 0);
}

#[test]
fn invalid_objects_and_patches_are_refused_whole() {
    const TEXT: &str = "see  now";
    type Build = fn(NodeId) -> Vec<Command>;
    // Each case names the command that is invalid.
    let cases: [(Build, usize); 3] = [
        (
            |p| {
                vec![Command::InsertInline {
                    node: p,
                    at: 4,
                    object: InlineObject::image("not-a-hash", "x"),
                }]
            },
            0,
        ),
        (
            |p| {
                vec![Command::InsertInline {
                    node: p,
                    at: 4,
                    object: InlineObject {
                        kind: "field".into(),
                        ..InlineObject::default()
                    },
                }]
            },
            0,
        ),
        (
            |p| {
                vec![
                    Command::InsertInline {
                        node: p,
                        at: 4,
                        object: image(),
                    },
                    Command::UpdateInline {
                        node: p,
                        at: 4,
                        patch: set("width", "pt"),
                    },
                ]
            },
            1,
        ),
    ];
    for (i, (build, index)) in cases.into_iter().enumerate() {
        let doc = Document::new(1).unwrap();
        let p = para(&doc, TEXT);
        let mut e = editor(doc);
        let mut tx = Transaction::new();
        for command in build(p) {
            tx.push(command);
        }
        let err = e.apply(&tx).unwrap_err();
        assert_eq!(err.command, Some(index), "case {i}");
        assert!(matches!(err.reason, Reason::Inline(_)), "case {i}: {err:?}");
        assert_eq!(text(&e, p), TEXT, "case {i}");
        assert!(!e.document().has_inline_objects(), "case {i}");
        assert_eq!(e.undo_count(), 0, "case {i}");
    }
}

#[test]
fn offsets_and_nodes_are_checked_before_anything_is_written() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "é");
    let q = para(&doc, "gone");
    let mut e = editor(doc);

    // Inside the two-byte "é".
    let err = e
        .apply(&Transaction::new().with(Command::InsertInline {
            node: p,
            at: 1,
            object: image(),
        }))
        .unwrap_err();
    assert!(matches!(err.reason, Reason::BadOffset { .. }));

    // A block deleted earlier in the batch is no longer there.
    let err = e
        .apply(
            &Transaction::new()
                .with(Command::DeleteBlock { node: q })
                .with(Command::InsertInline {
                    node: q,
                    at: 0,
                    object: image(),
                }),
        )
        .unwrap_err();
    assert_eq!(err.command, Some(1));
    assert!(matches!(err.reason, Reason::NoSuchBlock(_)));
    assert!(!e.document().has_inline_objects());
    assert_eq!(e.undo_count(), 0);
}

#[test]
fn an_object_character_in_inserted_text_refuses_the_whole_batch() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "ab");
    let mut e = editor(doc);
    let err = e
        .apply(
            &Transaction::new()
                .with(Command::InsertText {
                    node: p,
                    at: 0,
                    text: "x".into(),
                })
                .with(Command::InsertText {
                    node: p,
                    at: 0,
                    text: OBJECT.to_string(),
                }),
        )
        .unwrap_err();
    assert_eq!(err.command, Some(1));
    assert!(matches!(err.reason, Reason::Inline(_)));
    assert_eq!(text(&e, p), "ab", "the first insert was not written either");
    assert_eq!(e.undo_count(), 0);
}

#[test]
fn deleting_an_object_then_undoing_brings_it_back_with_its_identity() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "see  now");
    let id = doc.insert_inline(p, 4, &image()).unwrap();
    doc.commit_step();
    let mut e = editor(doc);

    e.apply(&Transaction::new().with(Command::DeleteText {
        node: p,
        range: 4..7,
    }))
    .unwrap();
    assert_eq!(e.document().inline_at(p, 4), None);

    // Updating what was deleted is refused, and the deletion stands.
    let err = e
        .apply(&Transaction::new().with(Command::UpdateInline {
            node: p,
            at: 4,
            patch: set("alt", "gone"),
        }))
        .unwrap_err();
    assert!(matches!(err.reason, Reason::BadOffset { .. }));

    assert!(e.undo().unwrap());
    assert_eq!(e.document().inline_at(p, 4), Some(id));
    assert_eq!(text(&e, p), format!("see {OBJECT} now"));
}

#[test]
fn splitting_a_paragraph_takes_its_objects_out_of_reach() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "see  now");
    doc.insert_inline(p, 4, &image()).unwrap();
    doc.commit_step();
    let mut e = editor(doc);
    let err = e
        .apply(
            &Transaction::new()
                .with(Command::SplitBlock { node: p, at: 2 })
                .with(Command::UpdateInline {
                    node: p,
                    at: 4,
                    patch: set("alt", "moved"),
                }),
        )
        .unwrap_err();
    assert_eq!(err.command, Some(1));
    assert!(matches!(err.reason, Reason::BadOffset { .. }));
    assert_eq!(
        e.undo_count(),
        0,
        "setup made before the editor is not in its history"
    );
}

#[test]
fn joining_paragraphs_carries_their_objects_along() {
    let doc = Document::new(1).unwrap();
    let first = para(&doc, "ab");
    let second = para(&doc, "c");
    let id = doc.insert_inline(second, 1, &image()).unwrap();
    doc.commit_step();
    let mut e = editor(doc);

    // Joined, the object sits at 2 + 1 in the first paragraph.
    e.apply(
        &Transaction::new()
            .with(Command::JoinBlocks { first, second })
            .with(Command::UpdateInline {
                node: first,
                at: 3,
                patch: set("alt", "joined"),
            }),
    )
    .unwrap();
    // Text joined across paragraphs is copied, so the object is new in the
    // first paragraph, with the same parameters and the update applied.
    let joined = e
        .document()
        .inline_at(first, 3)
        .expect("the object moved over");
    assert_ne!(joined, id, "a cross-host join copies the object");
    assert_eq!(
        e.document().inline_object(joined).unwrap().param("alt"),
        Some("joined")
    );
    assert_eq!(
        e.document().inline_object(joined).unwrap().param("width"),
        Some("pt:20480")
    );
}
