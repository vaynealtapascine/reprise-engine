//! Commands, transactions, validation and per-user undo (29).

use reprise_doc::relation::builtin::FOLLOW;
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, Document, LayoutQuery, LengthExpr, NewBlock, NodeId, RangeState, Relation,
    SchemaRegistry, Style, Target,
};
use reprise_edit::{
    Bias, Command, EditError, Editor, MAX_COMMANDS, MAX_INSERT_BYTES, Reason, Transaction,
};
use reprise_geom::Length;

fn para(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "body", text)
        .unwrap()
}

fn editor(doc: Document) -> Editor {
    Editor::new(doc, SchemaRegistry::builtin())
}

/// Every live block in document order with its depth and text.
fn dump(doc: &Document) -> Vec<(NodeId, usize, String)> {
    doc.document_order()
        .into_iter()
        .map(|n| {
            let mut depth = 0;
            let mut up = doc.parent_of(n).flatten();
            while let Some(p) = up {
                depth += 1;
                up = doc.parent_of(p).flatten();
            }
            (n, depth, doc.block(n).unwrap().text.to_string())
        })
        .collect()
}

fn texts(doc: &Document) -> Vec<String> {
    dump(doc).into_iter().map(|(_, _, t)| t).collect()
}

#[test]
fn formatting_validates_against_prior_text_edits_and_undoes_as_one_step() {
    use reprise_doc::formatting::TextStyle;
    let doc = Document::new(1).unwrap();
    let node = para(&doc, "abc");
    let mut e = editor(doc);
    let style = TextStyle {
        size: Some(Length::from_pt(24)),
        ..TextStyle::default()
    };
    e.apply(
        &Transaction::new()
            .with(Command::InsertText {
                node,
                at: 3,
                text: "def".into(),
            })
            .with(Command::FormatText {
                node,
                range: 2..6,
                style: style.clone(),
            }),
    )
    .unwrap();
    assert_eq!(e.undo_count(), 1);
    assert_eq!(e.document().text_formats(node).unwrap().runs[1].bytes, 2..6);
    assert!(e.undo().unwrap());
    assert_eq!(texts(e.document()), ["abc"]);
    assert_eq!(
        e.document().text_formats(node).unwrap().runs[0].style,
        TextStyle::default()
    );
    assert!(e.redo().unwrap());
    assert_eq!(
        e.document().text_formats(node).unwrap().runs[1].style,
        style
    );
    refused(
        &mut e,
        Transaction::new()
            .with(Command::InsertText {
                node,
                at: 0,
                text: "not written".into(),
            })
            .with(Command::FormatText {
                node,
                range: 0..1,
                style: TextStyle {
                    size: Some(Length::ZERO),
                    ..TextStyle::default()
                },
            }),
    );
}

#[test]
fn many_staging_commits_still_undo_as_one_step_with_a_one_step_limit() {
    let doc = Document::new(1).unwrap();
    let head = para(&doc, &"x".repeat(40));
    let mut e = editor(doc);
    e.set_undo_limit(1);
    let mut tx = Transaction::new();
    for at in (1..40).rev() {
        tx = tx.with(Command::SplitBlock { node: head, at });
    }
    let made = e.apply(&tx).unwrap().blocks;
    assert_eq!(texts(e.document()), vec!["x"; 40]);
    assert_eq!(e.undo_count(), 1);
    assert!(e.undo().unwrap());
    assert_eq!(texts(e.document()), ["x".repeat(40)]);
    assert!(!e.undo().unwrap());
    assert!(e.redo().unwrap());
    assert_eq!(texts(e.document()), vec!["x"; 40]);
    assert!(made.iter().all(|id| e.document().is_live(*id)));
}

#[test]
fn flow_validation_refuses_the_whole_transaction_before_writing() {
    let doc = Document::new(1).unwrap();
    let head = para(&doc, "abcd");
    let tail = doc.split_block(head, 2).unwrap();
    let mut e = editor(doc);
    let first = Command::InsertText {
        node: head,
        at: 0,
        text: "must not appear".into(),
    };
    refused(
        &mut e,
        Transaction::new()
            .with(first.clone())
            .with(Command::InsertText {
                node: tail,
                at: 0,
                text: "\u{fdd0}".into(),
            }),
    );
    refused(
        &mut e,
        Transaction::new().with(first).with(Command::InsertBlock {
            parent: Some(tail),
            index: 0,
            block: NewBlock::new(BlockKind::Paragraph, "", "child"),
        }),
    );
    assert!(!e.can_undo());
}

#[test]
fn splitting_non_flow_text_after_an_edit_is_one_complete_step() {
    let doc = Document::new(1).unwrap();
    let head = para(&doc, "abc");
    let image = doc.append_block(BlockKind::Image, "", "héllo").unwrap();
    let mut e = editor(doc);
    let applied = e
        .apply(
            &Transaction::new()
                .with(Command::InsertText {
                    node: head,
                    at: 0,
                    text: "X".into(),
                })
                .with(Command::SplitBlock { node: image, at: 3 }),
        )
        .unwrap();
    assert_eq!(texts(e.document()), ["Xabc", "hé", "llo"]);
    assert_eq!(e.undo_count(), 1);
    assert!(e.undo().unwrap());
    assert_eq!(texts(e.document()), ["abc", "héllo"]);
    assert!(e.redo().unwrap());
    assert_eq!(texts(e.document()), ["Xabc", "hé", "llo"]);
    assert!(e.document().is_live(applied.blocks[0]));
}

#[test]
fn moving_a_head_after_splitting_in_the_same_transaction_uses_a_copy() {
    let doc = Document::new(1).unwrap();
    let head = para(&doc, "abcd");
    let mut e = editor(doc);
    e.apply(
        &Transaction::new()
            .with(Command::SplitBlock { node: head, at: 2 })
            .with(Command::MoveBlock {
                node: head,
                parent: None,
                index: 1,
            }),
    )
    .unwrap();
    assert_eq!(texts(e.document()), ["cd", "ab"]);
    assert!(e.undo().unwrap());
    assert_eq!(texts(e.document()), ["abcd"]);
    assert!(e.redo().unwrap());
    assert_eq!(texts(e.document()), ["cd", "ab"]);
}

fn refused(editor: &mut Editor, tx: impl Into<Transaction>) -> EditError {
    let before = (dump(editor.document()), editor.document().revision());
    let err = editor.apply(&tx.into()).expect_err("must be refused");
    let after = (dump(editor.document()), editor.document().revision());
    assert_eq!(before, after, "a refused transaction changes nothing");
    err
}

#[test]
fn every_command_does_what_it_says() {
    let mut e = editor(Document::new(1).unwrap());
    let a = para(e.document(), "héllo wörld");
    let b = para(e.document(), "second");
    let ins = |node, at, text: &str| Command::InsertText {
        node,
        at,
        text: text.into(),
    };

    e.apply_command(ins(a, 0, ">> ")).unwrap();
    assert_eq!(texts(e.document()), [">> héllo wörld", "second"]);
    e.apply_command(Command::DeleteText {
        node: a,
        range: 0..3,
    })
    .unwrap();
    assert_eq!(texts(e.document()), ["héllo wörld", "second"]);

    let applied = e
        .apply_command(Command::SplitBlock {
            node: a,
            at: "héllo".len(),
        })
        .unwrap();
    let tail = applied.blocks[0];
    assert_eq!(e.document().blocks(), [a, tail, b]);
    assert_eq!(texts(e.document()), ["héllo", " wörld", "second"]);

    e.apply_command(Command::JoinBlocks {
        first: a,
        second: tail,
    })
    .unwrap();
    assert_eq!(e.document().blocks(), [a, b]);
    assert_eq!(texts(e.document()), ["héllo wörld", "second"]);

    let applied = e
        .apply_command(Command::InsertBlock {
            parent: None,
            index: 1,
            block: NewBlock::new(BlockKind::Paragraph, "body", "middle"),
        })
        .unwrap();
    let m = applied.blocks[0];
    assert_eq!(e.document().blocks(), [a, m, b]);

    e.apply_command(Command::MoveBlock {
        node: b,
        parent: None,
        index: 0,
    })
    .unwrap();
    assert_eq!(e.document().blocks(), [b, a, m]);

    let style = Style {
        size: Some(LengthExpr::Pt(Length::from_pt(20))),
        ..Default::default()
    };
    e.apply_command(Command::SetStyleOverride {
        node: a,
        style: style.clone(),
    })
    .unwrap();
    assert_eq!(e.document().block(a).unwrap().overrides, style);

    e.apply_command(Command::DeleteBlock { node: m }).unwrap();
    assert_eq!(e.document().blocks(), [b, a]);

    // Relations.
    let range = e.document().add_range(a, 0..5, RangePolicy::FIXED).unwrap();
    let note = e
        .document()
        .append_block(BlockKind::Annotation, "note", "n")
        .unwrap();
    let relation = Relation::new(FOLLOW).owned_by(note).target(
        "line",
        Target::Layout(LayoutQuery::LineContaining { range }),
    );
    let applied = e
        .apply_command(Command::AddRelation {
            relation: relation.clone(),
        })
        .unwrap();
    let id = applied.relations[0];
    assert_eq!(e.document().relations(), vec![(id, Ok(relation))]);
    e.apply_command(Command::RemoveRelation { id }).unwrap();
    assert!(e.document().relations().is_empty());
}

#[test]
fn a_transaction_is_one_undo_step() {
    let doc = Document::new(1).unwrap();
    let a = para(&doc, "one");
    let b = para(&doc, "two");
    let mut e = editor(doc);
    let before = dump(e.document());
    let tx = Transaction::new()
        .with(Command::InsertText {
            node: a,
            at: 3,
            text: "!".into(),
        })
        .with(Command::DeleteBlock { node: b })
        .with(Command::InsertBlock {
            parent: None,
            index: 1,
            block: NewBlock::new(BlockKind::Paragraph, "body", "new"),
        });
    let applied = e.apply(&tx).unwrap();
    let new = applied.blocks[0];
    let after = dump(e.document());
    assert_eq!(texts(e.document()), ["one!", "new"]);
    assert!(e.undo().unwrap());
    assert_eq!(dump(e.document()), before, "one undo undoes all of it");
    assert!(!e.can_undo());
    assert!(e.redo().unwrap());
    assert_eq!(dump(e.document()), after);
    assert_eq!(
        e.document().blocks(),
        [a, new],
        "redo brings back the inserted block under the same ID"
    );
    assert!(e.undo().unwrap());
    assert!(e.document().is_live(b), "and the deleted block with its ID");
}

#[test]
fn commands_see_the_effect_of_the_commands_before_them() {
    let mut e = editor(Document::new(1).unwrap());
    let a = para(e.document(), "abc");
    // Offsets are valid only after the insertion before them.
    e.apply(
        &Transaction::new()
            .with(Command::InsertText {
                node: a,
                at: 3,
                text: "def".into(),
            })
            .with(Command::DeleteText {
                node: a,
                range: 4..6,
            })
            .with(Command::SplitBlock { node: a, at: 4 })
            .with(Command::InsertBlock {
                parent: None,
                index: 2,
                block: NewBlock::new(BlockKind::Paragraph, "body", "tail"),
            }),
    )
    .unwrap();
    assert_eq!(texts(e.document()), ["abcd", "", "tail"]);
}

#[test]
fn a_half_invalid_transaction_is_refused_whole() {
    let mut e = editor(Document::new(1).unwrap());
    let a = para(e.document(), "héllo");
    let b = para(e.document(), "b");
    let ins = |node, at, text: &str| Command::InsertText {
        node,
        at,
        text: text.into(),
    };
    // The first three commands are fine; the last is inside the é.
    let err = refused(
        &mut e,
        Transaction::new()
            .with(ins(a, 0, "x"))
            .with(Command::DeleteBlock { node: b })
            .with(Command::InsertBlock {
                parent: None,
                index: 0,
                block: NewBlock::new(BlockKind::Paragraph, "body", "new"),
            })
            .with(ins(a, 3, "oops")),
    );
    assert_eq!(err.command, Some(3));
    assert_eq!(err.reason, Reason::BadOffset { node: a, offset: 3 });
    // A command that depends on an earlier one that deleted its target.
    let err = refused(
        &mut e,
        Transaction::new()
            .with(Command::DeleteBlock { node: a })
            .with(ins(a, 0, "late")),
    );
    assert_eq!(err.command, Some(1));
    assert_eq!(err.reason, Reason::NoSuchBlock(a));
    assert!(!e.can_undo(), "nothing was recorded");
    assert!(e.document().is_live(a) && e.document().is_live(b));
}

#[test]
#[allow(clippy::reversed_empty_ranges)] // reversed on purpose
fn every_kind_of_refusal_is_typed() {
    let mut e = editor(Document::new(1).unwrap());
    let a = para(e.document(), "héllo");
    let b = para(e.document(), "b");
    let note = e
        .document()
        .append_block(BlockKind::Annotation, "note", "n")
        .unwrap();
    let child = e
        .document()
        .insert_block_at(
            Some(a),
            0,
            &NewBlock::new(BlockKind::Paragraph, "body", "c"),
        )
        .unwrap();
    let gone = para(e.document(), "gone");
    e.document().delete_block(gone).unwrap();
    let mut reason = |c: Command| refused(&mut e, c).reason;

    assert_eq!(
        reason(Command::InsertText {
            node: gone,
            at: 0,
            text: "x".into()
        }),
        Reason::NoSuchBlock(gone)
    );
    assert_eq!(
        reason(Command::DeleteText {
            node: a,
            range: 3..2
        }),
        Reason::BadRange {
            node: a,
            range: 3..2
        }
    );
    assert!(matches!(
        reason(Command::DeleteText {
            node: a,
            range: 0..99
        }),
        Reason::BadRange { .. }
    ));
    assert!(matches!(
        reason(Command::DeleteText {
            node: a,
            range: 0..2
        }),
        Reason::BadRange { .. }
    ));
    assert!(matches!(
        reason(Command::InsertText {
            node: a,
            at: 99,
            text: "x".into()
        }),
        Reason::BadOffset { .. }
    ));
    assert!(matches!(
        reason(Command::SplitBlock { node: a, at: 2 }),
        Reason::BadOffset { .. }
    ));
    assert_eq!(
        reason(Command::JoinBlocks {
            first: a,
            second: a
        }),
        Reason::SameBlock(a)
    );
    assert_eq!(
        reason(Command::JoinBlocks {
            first: a,
            second: note
        }),
        Reason::KindMismatch {
            first: a,
            second: note
        }
    );
    assert_eq!(
        reason(Command::JoinBlocks {
            first: b,
            second: a
        }),
        Reason::HasChildren(a)
    );
    assert!(matches!(
        reason(Command::InsertBlock {
            parent: None,
            index: 9,
            block: NewBlock::new(BlockKind::Paragraph, "body", "")
        }),
        Reason::BadIndex { index: 9, .. }
    ));
    assert_eq!(
        reason(Command::InsertBlock {
            parent: Some(gone),
            index: 0,
            block: NewBlock::new(BlockKind::Paragraph, "body", "")
        }),
        Reason::NoSuchBlock(gone)
    );
    assert_eq!(
        reason(Command::MoveBlock {
            node: a,
            parent: Some(child),
            index: 0
        }),
        Reason::Cycle(a)
    );
    assert_eq!(
        reason(Command::MoveBlock {
            node: a,
            parent: Some(a),
            index: 0
        }),
        Reason::Cycle(a)
    );
    assert!(matches!(
        reason(Command::MoveBlock {
            node: b,
            parent: None,
            index: 9
        }),
        Reason::BadIndex { .. }
    ));
    assert_eq!(
        reason(Command::DeleteBlock { node: gone }),
        Reason::NoSuchBlock(gone)
    );
    assert_eq!(
        reason(Command::SetStyleOverride {
            node: gone,
            style: Style::default()
        }),
        Reason::NoSuchBlock(gone)
    );
}

#[test]
fn relations_are_validated_against_their_schema_and_their_targets() {
    let mut e = editor(Document::new(1).unwrap());
    let a = para(e.document(), "some text");
    let note = e
        .document()
        .append_block(BlockKind::Annotation, "note", "n")
        .unwrap();
    let range = e.document().add_range(a, 0..4, RangePolicy::FIXED).unwrap();
    let line = Target::Layout(LayoutQuery::LineContaining { range });
    let gone = para(e.document(), "gone");
    e.document().delete_block(gone).unwrap();
    fn reason_of(e: &mut Editor, r: Relation) -> Reason {
        refused(e, Command::AddRelation { relation: r }).reason
    }
    assert!(matches!(
        reason_of(&mut e, Relation::new(FOLLOW).target("line", line.clone())),
        Reason::Schema(_)
    ));
    assert!(matches!(
        reason_of(
            &mut e,
            Relation::new(FOLLOW)
                .owned_by(note)
                .target("line", Target::Range(range))
        ),
        Reason::Schema(_)
    ));
    // Valid in kind, but the owner is gone by then.
    assert_eq!(
        reason_of(
            &mut e,
            Relation::new(FOLLOW)
                .owned_by(gone)
                .target("line", line.clone())
        ),
        Reason::DeadBlock(gone)
    );
    // The same transaction deletes the owner first.
    let rel = Relation::new(FOLLOW)
        .owned_by(note)
        .target("line", line.clone());
    let err = refused(
        &mut e,
        Transaction::new()
            .with(Command::DeleteBlock { node: note })
            .with(Command::AddRelation { relation: rel }),
    );
    assert_eq!(err.reason, Reason::DeadBlock(note));
    // Removing what isn't there, or twice.
    let rel = Relation::new(FOLLOW).owned_by(note).target("line", line);
    let id = e
        .apply_command(Command::AddRelation { relation: rel })
        .unwrap()
        .relations[0];
    let err = refused(
        &mut e,
        Transaction::new()
            .with(Command::RemoveRelation { id })
            .with(Command::RemoveRelation { id }),
    );
    assert_eq!(err.reason, Reason::NoSuchRelation(id));
    assert_eq!(e.document().relations().len(), 1);
}

#[test]
fn transactions_and_inserts_are_bounded() {
    let mut e = editor(Document::new(1).unwrap());
    let a = para(e.document(), "x");
    let many: Vec<Command> = (0..MAX_COMMANDS + 1)
        .map(|_| Command::InsertText {
            node: a,
            at: 0,
            text: String::new(),
        })
        .collect();
    let err = refused(&mut e, many);
    assert_eq!(err.command, None);
    assert!(matches!(err.reason, Reason::TooManyCommands { .. }));
    let err = refused(
        &mut e,
        Command::InsertText {
            node: a,
            at: 0,
            text: "y".repeat(MAX_INSERT_BYTES + 1),
        },
    );
    assert!(matches!(err.reason, Reason::TextTooLong { .. }));
    // Right at the limit of commands is accepted.
    let ok: Vec<Command> = (0..MAX_COMMANDS)
        .map(|_| Command::InsertText {
            node: a,
            at: 0,
            text: "z".into(),
        })
        .collect();
    e.apply(&Transaction::from(ok)).unwrap();
    assert_eq!(e.document().block(a).unwrap().text.len(), MAX_COMMANDS + 1);
}

#[test]
fn an_empty_transaction_is_not_a_step() {
    let mut e = editor(Document::new(1).unwrap());
    let a = para(e.document(), "x");
    let applied = e.apply(&Transaction::new()).unwrap();
    assert_eq!(applied.blocks, []);
    e.apply_command(Command::InsertText {
        node: a,
        at: 0,
        text: String::new(),
    })
    .unwrap();
    assert!(!e.can_undo(), "nothing changed, so nothing to undo");
}

#[test]
fn positions_follow_the_edits() {
    let mut e = editor(Document::new(1).unwrap());
    let a = para(e.document(), "hello world");
    let applied = e
        .apply(
            &Transaction::new()
                .with(Command::InsertText {
                    node: a,
                    at: 0,
                    text: ">> ".into(),
                })
                .with(Command::DeleteText {
                    node: a,
                    range: 8..14,
                })
                .with(Command::SplitBlock { node: a, at: 5 }),
        )
        .unwrap();
    let tail = applied.blocks[0];
    // ">> hello world" loses its last six bytes, then splits after ">> he".
    assert_eq!(applied.map_position(a, 2, Bias::Before), Some((a, 5)));
    assert_eq!(applied.map_position(a, 2, Bias::After), Some((tail, 0)));
    assert_eq!(applied.map_position(a, 0, Bias::Before), Some((a, 0)));
    assert_eq!(applied.map_position(a, 0, Bias::After), Some((a, 3)));
    assert_eq!(applied.map_position(a, 1, Bias::Before), Some((a, 4)));
    assert_eq!(applied.map_position(a, 6, Bias::Before), Some((tail, 3)));
    assert_eq!(applied.map_position(a, 6, Bias::After), Some((tail, 3)));
    assert_eq!(applied.map_position(a, 11, Bias::Before), Some((tail, 3)));
    let join = e
        .apply_command(Command::JoinBlocks {
            first: a,
            second: tail,
        })
        .unwrap();
    assert_eq!(join.map_position(tail, 2, Bias::After), Some((a, 7)));
    let del = e.apply_command(Command::DeleteBlock { node: a }).unwrap();
    assert_eq!(del.map_position(a, 0, Bias::After), None);
}

// ---------------------------------------------------------------------------
// Per-user undo, with two peers (29).

fn exchange(a: &mut Editor, b: &mut Editor) {
    a.merge(b.document()).unwrap();
    b.merge(a.document()).unwrap();
}

fn ins(node: NodeId, at: usize, text: &str) -> Command {
    Command::InsertText {
        node,
        at,
        text: text.into(),
    }
}

#[test]
fn i_type_you_type_i_undo_only_my_text_goes() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "abc");
    let mut one = editor(doc);
    let mut two = editor(one.document().fork(2).unwrap());
    one.apply_command(ins(p, 0, "ME ")).unwrap();
    two.apply_command(ins(p, 3, " YOU")).unwrap();
    exchange(&mut one, &mut two);
    assert_eq!(texts(one.document()), ["ME abc YOU"]);
    assert!(one.undo().unwrap());
    assert_eq!(texts(one.document()), ["abc YOU"]);
    assert!(!two.can_undo() || two.undo().unwrap());
    exchange(&mut one, &mut two);
    // Your undo took your text; mine was already gone.
    assert_eq!(texts(one.document()), ["abc"]);
    assert_eq!(texts(one.document()), texts(two.document()));
    // Redo brings back only mine.
    assert!(one.redo().unwrap());
    assert_eq!(texts(one.document()), ["ME abc"]);
}

#[test]
fn undoing_a_deleted_block_restores_the_same_node_with_everything_on_it() {
    let doc = Document::new(1).unwrap();
    let a = para(&doc, "keep");
    let target = para(&doc, "target text");
    let note = doc
        .append_block(BlockKind::Annotation, "note", "n")
        .unwrap();
    let range = doc.add_range(target, 0..6, RangePolicy::FIXED).unwrap();
    let schemas = SchemaRegistry::builtin();
    let relation = Relation::new(FOLLOW).owned_by(note).target(
        "line",
        Target::Layout(LayoutQuery::LineContaining { range }),
    );
    doc.add_relation(&schemas, &relation).unwrap();
    let anchor = doc
        .block(target)
        .unwrap()
        .text
        .anchor(7, reprise_doc::text::Affinity::After)
        .unwrap();
    let mut e = editor(doc);

    e.apply_command(Command::DeleteBlock { node: target })
        .unwrap();
    assert!(!e.document().is_live(target));
    assert_eq!(
        e.document().resolve_range(range),
        RangeState::Missing { node: Some(target) }
    );
    assert!(e.undo().unwrap());
    let doc = e.document();
    assert_eq!(doc.blocks(), [a, target, note], "same ID, same place");
    assert_eq!(
        doc.resolve_range(range),
        RangeState::Valid {
            node: target,
            bytes: 0..6
        }
    );
    assert_eq!(doc.relations().len(), 1);
    assert_eq!(
        doc.block(target).unwrap().text.resolve(&anchor).unwrap(),
        reprise_doc::text::Resolved::Live(7)
    );
}

#[test]
fn undo_after_a_concurrent_edit_inside_the_deleted_block() {
    let doc = Document::new(1).unwrap();
    let a = para(&doc, "keep");
    let b = para(&doc, "hello");
    let mut one = editor(doc);
    let mut two = editor(one.document().fork(2).unwrap());
    one.apply_command(Command::DeleteBlock { node: b }).unwrap();
    two.apply_command(ins(b, 5, ", world")).unwrap();
    two.apply_command(Command::SetStyleOverride {
        node: b,
        style: Style {
            family: Some("mono".into()),
            ..Default::default()
        },
    })
    .unwrap();
    exchange(&mut one, &mut two);
    assert_eq!(one.document().blocks(), [a], "no resurrection");
    assert_eq!(two.document().blocks(), [a]);
    assert!(one.undo().unwrap());
    assert_eq!(one.document().blocks(), [a, b]);
    let block = one.document().block(b).unwrap();
    assert_eq!(block.text.to_string(), "hello, world");
    assert_eq!(block.overrides.family.as_deref(), Some("mono"));
    exchange(&mut one, &mut two);
    assert_eq!(dump(one.document()), dump(two.document()));
}

#[test]
fn undo_keeps_the_other_peers_blocks_and_moves() {
    let doc = Document::new(1).unwrap();
    let (a, b, c) = (para(&doc, "a"), para(&doc, "b"), para(&doc, "c"));
    let mut one = editor(doc);
    let mut two = editor(one.document().fork(2).unwrap());
    one.apply_command(Command::DeleteBlock { node: b }).unwrap();
    let x = two
        .apply_command(Command::InsertBlock {
            parent: None,
            index: 1,
            block: NewBlock::new(BlockKind::Paragraph, "body", "x"),
        })
        .unwrap()
        .blocks[0];
    two.apply_command(Command::MoveBlock {
        node: c,
        parent: None,
        index: 0,
    })
    .unwrap();
    exchange(&mut one, &mut two);
    assert!(one.undo().unwrap());
    exchange(&mut one, &mut two);
    let order = one.document().blocks();
    assert!(order.contains(&b) && order.contains(&x));
    assert_eq!(order.len(), 4);
    assert_eq!(order[0], c, "their move stays");
    assert_eq!(order, two.document().blocks());
    let _ = a;
}

// ---------------------------------------------------------------------------
// Property-style tests.

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

const SNIPPETS: [&str; 7] = ["a", "bc", "é", "e\u{301}", "שלום", "👩‍👩‍👧", " "];

/// A random command, valid or not, over the document's current state.
fn random_command(rng: &mut Rng, doc: &Document) -> Command {
    let order = doc.document_order();
    let pick = |rng: &mut Rng| order.get(rng.below(order.len())).copied();
    let Some(node) = pick(rng) else {
        return Command::InsertBlock {
            parent: None,
            index: 0,
            block: NewBlock::new(BlockKind::Paragraph, "body", "seed"),
        };
    };
    let text = doc
        .block(node)
        .map(|b| b.text.to_string())
        .unwrap_or_default();
    // Mostly valid offsets, sometimes any byte.
    let offset = |rng: &mut Rng| {
        if rng.below(8) == 0 {
            rng.below(text.len() + 3)
        } else {
            let chars: Vec<usize> = text
                .char_indices()
                .map(|(i, _)| i)
                .chain([text.len()])
                .collect();
            chars[rng.below(chars.len())]
        }
    };
    match rng.below(10) {
        0..=2 => Command::InsertText {
            node,
            at: offset(rng),
            text: SNIPPETS[rng.below(SNIPPETS.len())].into(),
        },
        3 | 4 => {
            let (a, b) = (offset(rng), offset(rng));
            Command::DeleteText {
                node,
                range: a.min(b)..a.max(b),
            }
        }
        5 => Command::SplitBlock {
            node,
            at: offset(rng),
        },
        6 => Command::JoinBlocks {
            first: node,
            second: pick(rng).unwrap_or(node),
        },
        7 => Command::InsertBlock {
            parent: if rng.below(3) == 0 { Some(node) } else { None },
            index: rng.below(3),
            block: NewBlock::new(BlockKind::Paragraph, "body", SNIPPETS[rng.below(7)]),
        },
        8 => Command::DeleteBlock { node },
        _ => Command::MoveBlock {
            node,
            parent: if rng.below(2) == 0 { pick(rng) } else { None },
            index: rng.below(4),
        },
    }
}

fn random_transaction(rng: &mut Rng, doc: &Document) -> Transaction {
    // Commands in one transaction are generated against the state before it,
    // so later ones are sometimes invalid; that is the point.
    let n = 1 + rng.below(3);
    (0..n)
        .map(|_| random_command(rng, doc))
        .collect::<Vec<_>>()
        .into()
}

/// The structure is a forest: every live block is reached exactly once from
/// the root, its parent agrees, and nothing refers to the dead.
fn check_tree(doc: &Document) {
    let order = doc.document_order();
    let mut seen = std::collections::BTreeSet::new();
    for n in &order {
        assert!(seen.insert(*n), "{n} appears twice");
        assert!(doc.is_live(*n));
        let parent = doc.parent_of(*n).expect("live");
        assert!(doc.children(parent).contains(n));
        assert!(doc.block(*n).is_ok());
    }
    assert_eq!(doc.blocks(), doc.children(None));
}

#[test]
fn random_transactions_never_corrupt_and_refusals_change_nothing() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut e = editor(Document::new(1).unwrap());
    let (mut ok, mut bad) = (0, 0);
    for _ in 0..600 {
        let tx = random_transaction(&mut rng, e.document());
        let before = (dump(e.document()), e.document().revision());
        match e.apply(&tx) {
            Ok(_) => ok += 1,
            Err(error) => {
                bad += 1;
                assert_eq!(
                    before,
                    (dump(e.document()), e.document().revision()),
                    "{tx:?}: {error:?}"
                );
            }
        }
        check_tree(e.document());
    }
    assert!(
        ok > 100 && bad > 20,
        "a useful mix: {ok} applied, {bad} refused"
    );
}

#[test]
fn undoing_everything_and_redoing_everything_restores_each_state() {
    for seed in 1..=6u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut e = editor(Document::new(1).unwrap());
        let mut states = vec![dump(e.document())];
        for _ in 0..80 {
            let tx = random_transaction(&mut rng, e.document());
            let steps = e.undo_count();
            let before = (dump(e.document()), e.document().revision());
            // A transaction that changes nothing is not a step.
            match e.apply(&tx) {
                Ok(_) if e.undo_count() > steps => states.push(dump(e.document())),
                Ok(_) => {}
                Err(error) => assert_eq!(
                    before,
                    (dump(e.document()), e.document().revision()),
                    "seed {seed}: {tx:?}: {error:?}"
                ),
            }
        }
        let steps = states.len() - 1;
        // Undo back to the start, checking every state on the way.
        for i in (0..steps).rev() {
            assert!(e.undo().unwrap(), "seed {seed}: step {i}");
            assert_eq!(dump(e.document()), states[i], "seed {seed}: undo to {i}");
            check_tree(e.document());
        }
        assert!(!e.can_undo(), "seed {seed}");
        // And redo forward again: same IDs, same texts.
        for (i, state) in states.iter().enumerate().skip(1) {
            assert!(e.redo().unwrap(), "seed {seed}: redo {i}");
            assert_eq!(dump(e.document()), *state, "seed {seed}: redo to {i}");
        }
        assert!(!e.can_redo());
        // Undo after redo restores the state before.
        for i in (0..steps).rev() {
            e.undo().unwrap();
            assert_eq!(
                dump(e.document()),
                states[i],
                "seed {seed}: second undo {i}"
            );
        }
    }
}

#[test]
fn two_peers_editing_randomly_converge_and_stay_a_forest() {
    for seed in 1..=5u64 {
        let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
        let doc = Document::new(1).unwrap();
        for t in ["one", "two", "three"] {
            para(&doc, t);
        }
        let mut one = editor(doc);
        let mut two = editor(one.document().fork(2).unwrap());
        for round in 0..25 {
            for e in [&mut one, &mut two] {
                for _ in 0..1 + rng.below(3) {
                    let tx = random_transaction(&mut rng, e.document());
                    let _ = e.apply(&tx);
                }
                if rng.below(5) == 0 {
                    let _ = e.undo();
                }
            }
            if round % 3 == 2 {
                exchange(&mut one, &mut two);
                assert_eq!(
                    dump(one.document()),
                    dump(two.document()),
                    "seed {seed} round {round}: replicas converge"
                );
                check_tree(one.document());
            }
        }
        exchange(&mut one, &mut two);
        assert_eq!(dump(one.document()), dump(two.document()), "seed {seed}");
        check_tree(one.document());
    }
}

#[test]
fn relation_ranges_see_a_target_deleted_earlier_in_the_same_transaction() {
    let doc = Document::new(1).unwrap();
    let node = para(&doc, "target");
    let note = doc
        .append_block(BlockKind::Annotation, "note", "n")
        .unwrap();
    let range = doc.add_range(node, 0..6, RangePolicy::FIXED).unwrap();
    let mut editor = editor(doc);
    let tx = Transaction::new()
        .with(Command::DeleteBlock { node })
        .with(Command::AddRelation {
            relation: Relation::new(FOLLOW).owned_by(note).target(
                "line",
                Target::Layout(LayoutQuery::LineContaining { range }),
            ),
        });
    let error = refused(&mut editor, tx);
    assert_eq!(error.command, Some(1));
    assert_eq!(error.reason, Reason::DeadBlock(node));
    assert_eq!(error.note().code.as_str(), "edit.invalid-command");
    assert_eq!(error.note().severity, reprise_diag::Severity::Error);
}

#[test]
fn ancestry_and_aggregate_payload_limits_report_instead_of_truncating() {
    use reprise_edit::{MAX_ANCESTORS, MAX_TRANSACTION_BYTES};
    let doc = Document::new(1).unwrap();
    let root = para(&doc, "root");
    let mut deepest = root;
    for _ in 0..MAX_ANCESTORS {
        deepest = doc
            .insert_block_at(
                Some(deepest),
                0,
                &NewBlock::new(BlockKind::Paragraph, "body", ""),
            )
            .unwrap();
    }
    let mut editor = editor(doc);
    let before = editor.document().revision();
    let error = editor
        .apply_command(Command::InsertText {
            node: deepest,
            at: 0,
            text: "x".into(),
        })
        .unwrap_err();
    assert_eq!(before, editor.document().revision());
    assert_eq!(editor.document().block(deepest).unwrap().text.len(), 0);
    assert_eq!(
        error.reason,
        Reason::TreeDepthLimit {
            node: deepest,
            max: MAX_ANCESTORS
        }
    );
    assert_eq!(error.note().code.as_str(), "edit.limit");
    let half = "x".repeat(MAX_TRANSACTION_BYTES / 2 + 1);
    let error = editor
        .apply(
            &Transaction::new()
                .with(ins(root, 0, &half))
                .with(ins(root, 0, &half)),
        )
        .unwrap_err();
    assert_eq!(before, editor.document().revision());
    assert_eq!(
        editor.document().block(root).unwrap().text.to_string(),
        "root"
    );
    assert!(matches!(error.reason, Reason::TransactionTooLarge { .. }));
    assert_eq!(error.note().code.as_str(), "edit.limit");
}

#[test]
fn every_command_round_trips_undo_redo_with_all_authored_fields() {
    let doc = Document::new(1).unwrap();
    let a = para(&doc, "alpha");
    let b = para(&doc, "beta");
    let child = doc
        .insert_block_at(
            Some(a),
            0,
            &NewBlock::new(BlockKind::Paragraph, "body", "child"),
        )
        .unwrap();
    let note = doc
        .append_block(BlockKind::Annotation, "note", "note")
        .unwrap();
    let range = doc.add_range(a, 0..2, RangePolicy::FIXED).unwrap();
    let relation = Relation::new(FOLLOW).owned_by(note).target(
        "line",
        Target::Layout(LayoutQuery::LineContaining { range }),
    );
    let id = doc
        .add_relation(&SchemaRegistry::builtin(), &relation)
        .unwrap();
    let mut editor = editor(doc);
    // Compare authored fields. Resolving persistent character anchors is
    // derived: text undo can reinsert characters with new CRDT identities,
    // so deleted anchors retain the frozen Text/RangePolicy rebound semantics.
    let state = |doc: &Document| {
        (
            doc.document_order()
                .into_iter()
                .map(|n| {
                    let block = doc.block(n).unwrap();
                    (
                        n,
                        doc.parent_of(n),
                        block.text.to_string(),
                        block.style,
                        block.overrides,
                        doc.successors(n),
                    )
                })
                .collect::<Vec<_>>(),
            doc.relations(),
        )
    };
    let commands = [
        ins(a, 0, "X"),
        Command::DeleteText {
            node: a,
            range: 0..1,
        },
        Command::SplitBlock { node: a, at: 2 },
        Command::JoinBlocks {
            first: a,
            second: b,
        },
        Command::InsertBlock {
            parent: Some(a),
            index: 1,
            block: NewBlock::new(BlockKind::Paragraph, "body", "inserted"),
        },
        Command::DeleteBlock { node: a },
        Command::MoveBlock {
            node: b,
            parent: Some(a),
            index: 0,
        },
        Command::SetStyleOverride {
            node: a,
            style: Style {
                size: Some(LengthExpr::Pt(Length::MIN)),
                line_height: Some(LengthExpr::Pt(Length::MAX)),
                ..Default::default()
            },
        },
        Command::AddRelation { relation },
        Command::RemoveRelation { id },
    ];
    for command in commands {
        let before = state(editor.document());
        let label = format!("{command:?}");
        editor.apply_command(command).unwrap();
        let after = state(editor.document());
        assert_eq!(editor.undo_count(), 1, "{label}");
        assert!(editor.undo().unwrap());
        assert_eq!(state(editor.document()), before, "{label}: undo");
        assert!(editor.redo().unwrap());
        assert_eq!(
            state(editor.document()),
            after,
            "{label}: redo keeps IDs and fields"
        );
        assert!(editor.undo().unwrap());
        assert_eq!(state(editor.document()), before, "{label}: undo after redo");
        assert!(!editor.can_undo());
        assert!(editor.document().is_live(child));
    }
}

#[test]
fn split_join_preserve_authored_range_policy_through_undo_redo() {
    use reprise_doc::text::{Affinity, Empty};
    let doc = Document::new(1).unwrap();
    let node = para(&doc, "\u{e9}\u{5d0}\u{5d1}");
    let policy = RangePolicy {
        start: Affinity::Before,
        end: Affinity::After,
        empty: Empty::Keep,
    };
    let range = doc.add_range(node, 0..6, policy).unwrap();
    let mut editor = editor(doc);
    editor
        .apply_command(Command::SplitBlock { node, at: 2 })
        .unwrap();
    assert_eq!(editor.document().range_policy(range).unwrap(), Some(policy));
    assert!(editor.undo().unwrap());
    assert_eq!(editor.document().range_policy(range).unwrap(), Some(policy));
    assert!(editor.redo().unwrap());
    let second = editor.document().blocks()[1];
    let tail_range = editor
        .document()
        .add_range(second, 0..4, RangePolicy::FIXED)
        .unwrap();
    // Range creation is its own authored step, so undoing the following join
    // must leave it in place rather than also undoing its creation.
    editor.document().commit_step();
    editor
        .apply_command(Command::JoinBlocks {
            first: node,
            second,
        })
        .unwrap();
    assert_eq!(editor.document().range_policy(range).unwrap(), Some(policy));
    assert_eq!(
        editor.document().range_policy(tail_range).unwrap(),
        Some(RangePolicy::FIXED)
    );
    assert!(editor.undo().unwrap());
    assert_eq!(
        editor.document().range_policy(tail_range).unwrap(),
        Some(RangePolicy::FIXED)
    );
    assert!(editor.redo().unwrap());
    assert_eq!(editor.document().range_policy(range).unwrap(), Some(policy));
}
