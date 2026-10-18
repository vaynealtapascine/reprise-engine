//! Refusals preserve authored state, retained history, and both undo stacks.
use reprise_doc::formatting::{MAX_FORMATS_PER_HOST, TextStyle};
use reprise_doc::{BlockKind, Document, NodeId, PersistenceMode};
use reprise_edit::{Affinity, Caret, Command, Editor, Selection, StableSelection, Transaction};
use std::sync::OnceLock;

fn crowded() -> (Editor, NodeId, NodeId) {
    static BASE: OnceLock<(Vec<u8>, NodeId, NodeId)> = OnceLock::new();
    let (bytes, a, b) = BASE.get_or_init(|| {
        let doc = Document::new(1).unwrap();
        let a = doc.append_block(BlockKind::Paragraph, "", "AB").unwrap();
        let b = doc.append_block(BlockKind::Paragraph, "", "CD").unwrap();
        doc.commit();
        // Build supported history on eight concurrent peers, avoiding a
        // quadratic 4095-record capacity scan on every setup insertion.
        let replicas: Vec<_> = (2..10).map(|peer| doc.fork(peer).unwrap()).collect();
        for (i, replica) in replicas.iter().enumerate() {
            let count =
                (MAX_FORMATS_PER_HOST - 1) / 8 + usize::from(i < (MAX_FORMATS_PER_HOST - 1) % 8);
            for _ in 0..count {
                replica
                    .format_text(a, 0..1, &bold(), reprise_doc::text::RangePolicy::EXPANDING)
                    .unwrap();
            }
            replica.commit();
            doc.merge(replica).unwrap();
        }
        doc.commit();
        (doc.export(PersistenceMode::History), a, b)
    });
    (
        Editor::new(
            Document::import(bytes, 1).unwrap(),
            reprise_fixtures::engine().schemas.clone(),
        ),
        *a,
        *b,
    )
}
fn bold() -> TextStyle {
    TextStyle {
        weight: Some(700),
        ..TextStyle::default()
    }
}
fn frozen(e: &Editor) -> (Vec<u8>, reprise_doc::Revision, usize, usize) {
    (
        e.document().export(PersistenceMode::History),
        e.document().revision(),
        e.undo_count(),
        e.redo_count(),
    )
}

#[test]
fn late_join_capacity_refusal_preserves_bytes_history_and_undo() {
    let (mut e, a, b) = crowded();
    let before = frozen(&e);
    let error = e
        .apply(&Transaction::from(vec![
            Command::DeleteText {
                node: a,
                range: 1..2,
            },
            Command::JoinBlocks {
                first: a,
                second: b,
            },
            Command::FormatText {
                node: a,
                range: 0..1,
                style: bold(),
            },
        ]))
        .unwrap_err();
    assert!(
        frozen(&e) == before,
        "refusal changed history bytes, revision or undo"
    );
    assert_eq!(error.command, Some(2));
}

#[test]
fn late_replacement_capacity_refusal_preserves_bytes_history_and_undo() {
    let (mut e, a, b) = crowded();
    let source = Document::new(2).unwrap();
    let x = source.append_block(BlockKind::Paragraph, "", "X").unwrap();
    source
        .format_text(x, 0..1, &bold(), reprise_doc::text::RangePolicy::EXPANDING)
        .unwrap();
    source.commit();
    let f = source.copy_all_fragment("source", e.schemas()).unwrap();
    let staged = e.stage_replacement(&f, "target").unwrap();
    let caret = |node, offset| Caret {
        node,
        offset,
        affinity: Affinity::Downstream,
    };
    let selection = StableSelection::anchor(
        e.document(),
        &Selection {
            anchor: caret(a, 1),
            focus: caret(b, 1),
        },
    )
    .unwrap();
    let before = frozen(&e);
    assert!(e.replace_selection(&selection, &staged).is_err());
    assert!(
        frozen(&e) == before,
        "refusal changed history bytes, revision or undo"
    );
}

#[test]
fn malformed_transactions_preserve_existing_undo_and_redo() {
    let (mut e, a, b) = crowded();
    e.apply_command(Command::InsertText {
        node: a,
        at: 2,
        text: "!".into(),
    })
    .unwrap();
    e.undo().unwrap();
    // Exercise all structural multi-write commands before a malformed final command.
    let prefixes = [
        Command::SplitBlock { node: a, at: 1 },
        Command::JoinBlocks {
            first: a,
            second: b,
        },
        Command::MoveBlock {
            node: b,
            parent: None,
            index: 0,
        },
        Command::DeleteBlock { node: b },
    ];
    for prefix in prefixes {
        let before = frozen(&e);
        assert!(
            e.apply(&Transaction::from(vec![
                prefix,
                Command::InsertText {
                    node: a,
                    at: usize::MAX,
                    text: "bad".into()
                }
            ]))
            .is_err()
        );
        assert!(
            frozen(&e) == before,
            "refusal changed history bytes, revision or undo"
        );
    }
}

#[test]
fn uncommitted_raw_edits_are_not_published_by_a_refused_composite_edit() {
    let doc = Document::new(1).unwrap();
    let a = doc.append_block(BlockKind::Paragraph, "", "AB").unwrap();
    let b = doc.append_block(BlockKind::Paragraph, "", "CD").unwrap();
    doc.format_text(a, 0..1, &bold(), reprise_doc::text::RangePolicy::EXPANDING)
        .unwrap();
    doc.commit();
    let mut e = Editor::new(doc, reprise_fixtures::engine().schemas.clone());
    e.document().block(a).unwrap().text.insert(2, "!").unwrap();
    let before = (e.document().revision(), e.undo_count(), e.redo_count());
    let error = e
        .apply(&Transaction::from(vec![
            Command::JoinBlocks {
                first: a,
                second: b,
            },
            Command::FormatText {
                node: a,
                range: 0..1,
                style: bold(),
            },
        ]))
        .unwrap_err();
    assert_eq!(error.reason, reprise_edit::Reason::UncommittedEdit);
    assert_eq!(
        (e.document().revision(), e.undo_count(), e.redo_count()),
        before
    );
    assert_eq!(e.document().block(a).unwrap().text.to_string(), "AB!");
}

#[test]
fn malformed_style_and_relation_batches_do_not_materialise_roots() {
    let doc = Document::new(1).unwrap();
    let a = doc.append_block(BlockKind::Paragraph, "", "A").unwrap();
    doc.commit();
    let mut e = Editor::new(doc, reprise_fixtures::engine().schemas.clone());
    let before = frozen(&e);
    let prefixes = [
        Command::DefineStyle {
            name: "Unused".into(),
            style: reprise_doc::Style::default(),
        },
        Command::RemoveRelation {
            id: reprise_doc::RelationId::parse("1000@9").unwrap(),
        },
    ];
    for prefix in prefixes {
        assert!(
            e.apply(&Transaction::from(vec![
                prefix,
                Command::InsertText {
                    node: a,
                    at: 99,
                    text: "bad".into()
                }
            ]))
            .is_err()
        );
        assert!(frozen(&e) == before);
    }
}

#[test]
fn renaming_a_malformed_imported_style_preserves_prior_content_and_history() {
    let doc = Document::new(1).unwrap();
    let a = doc.append_block(BlockKind::Paragraph, "", "AB").unwrap();
    // The legacy/native store can retain values a local style edit would refuse.
    doc.define_style(
        "Old",
        &reprise_doc::Style {
            weight: Some(1500),
            ..Default::default()
        },
    )
    .unwrap();
    doc.commit();
    let mut e = Editor::new(doc, reprise_fixtures::engine().schemas.clone());
    let before = frozen(&e);
    let error = e
        .apply(&Transaction::from(vec![
            Command::DeleteText {
                node: a,
                range: 1..2,
            },
            Command::RenameStyle {
                from: "Old".into(),
                to: "New".into(),
            },
        ]))
        .unwrap_err();
    assert_eq!(error.command, Some(1));
    assert!(
        frozen(&e) == before,
        "rename refusal changed bytes, revision or undo"
    );
}

#[test]
fn a_batch_can_repair_an_imported_style_before_renaming_it() {
    let doc = Document::new(1).unwrap();
    let a = doc.append_block(BlockKind::Paragraph, "", "AB").unwrap();
    doc.define_style(
        "Old",
        &reprise_doc::Style {
            weight: Some(1500),
            ..Default::default()
        },
    )
    .unwrap();
    doc.commit();
    let old_style = doc.style("Old").unwrap();
    let mut e = Editor::new(doc, reprise_fixtures::engine().schemas.clone());
    e.apply(&Transaction::from(vec![
        Command::DeleteText {
            node: a,
            range: 1..2,
        },
        Command::PatchStyle {
            name: "Old".into(),
            patch: reprise_doc::StylePatch {
                set: reprise_doc::Style {
                    weight: Some(700),
                    ..Default::default()
                },
                ..Default::default()
            },
        },
        Command::RenameStyle {
            from: "Old".into(),
            to: "New".into(),
        },
    ]))
    .unwrap();
    assert_eq!(e.document().block(a).unwrap().text.to_string(), "A");
    assert_eq!(e.document().style("New").unwrap().weight, Some(700));
    assert_eq!(e.undo_count(), 1);
    e.undo().unwrap();
    assert_eq!(e.document().block(a).unwrap().text.to_string(), "AB");
    assert_eq!(e.document().style("Old").unwrap(), old_style);
    assert!(e.document().style("New").is_none());
}

#[test]
fn deep_unrelated_trees_refuse_staging_without_entering_the_recursive_codec() {
    let doc = Document::new(1).unwrap();
    let a = doc.append_block(BlockKind::Paragraph, "", "AB").unwrap();
    let b = doc.append_block(BlockKind::Paragraph, "", "CD").unwrap();
    doc.format_text(a, 0..1, &bold(), reprise_doc::text::RangePolicy::EXPANDING)
        .unwrap();
    let mut parent = doc.append_block(BlockKind::Paragraph, "", "root").unwrap();
    for _ in 0..65 {
        parent = doc
            .insert_block_at(
                Some(parent),
                0,
                &reprise_doc::NewBlock::new(BlockKind::Paragraph, "", ""),
            )
            .unwrap();
    }
    doc.commit();
    let mut e = Editor::new(doc, reprise_fixtures::engine().schemas.clone());
    let before = frozen(&e);
    let error = e
        .apply_command(Command::JoinBlocks {
            first: a,
            second: b,
        })
        .unwrap_err();
    assert_eq!(
        error.reason,
        reprise_edit::Reason::PreflightDepth { max: 64 }
    );
    assert_eq!(error.note().code.as_str(), "edit.limit");
    assert!(frozen(&e) == before);
}

fn semantic_commands(a: NodeId, b: NodeId, list: reprise_doc::ListId) -> Vec<Command> {
    use reprise_doc::{InlineObject, InlinePatch, ListDefinition};
    use reprise_edit::ListRef;
    vec![
        Command::CreateList {
            definition: ListDefinition::numbered(),
        },
        Command::UpdateList {
            list,
            definition: ListDefinition::bulleted(),
        },
        Command::DeleteList {
            list: ListRef::Existing(list),
        },
        Command::SetListItem {
            node: b,
            list: ListRef::Existing(list),
            level: 1,
        },
        Command::RemoveListItem { node: a },
        Command::SetListLevel { node: a, level: 2 },
        Command::IndentListItem { node: a, by: 1 },
        Command::RestartList {
            node: a,
            value: Some(8),
        },
        Command::ContinueList { node: a },
        Command::Enter { node: a, at: 1 },
        Command::BackspaceAtStart { node: a },
        Command::InsertInline {
            node: a,
            at: 0,
            object: InlineObject::field("page"),
        },
        Command::UpdateInline {
            node: a,
            at: 1,
            patch: InlinePatch::default(),
        },
    ]
}

#[test]
fn list_and_inline_refusal_matrix_preserves_history_and_both_undo_stacks() {
    for cause in 0..2 {
        let doc = Document::new(1).unwrap();
        let a = doc.append_block(BlockKind::Paragraph, "", "AB").unwrap();
        let b = doc.append_block(BlockKind::Paragraph, "", "CD").unwrap();
        let list = doc
            .create_list(&reprise_doc::ListDefinition::numbered())
            .unwrap();
        doc.set_list_item(a, list, 0).unwrap();
        doc.insert_inline(a, 1, &reprise_doc::InlineObject::field("page"))
            .unwrap();
        doc.commit();
        let mut e = Editor::new(doc, reprise_fixtures::engine().schemas.clone());
        e.apply_command(Command::InsertText {
            node: b,
            at: 2,
            text: "!".into(),
        })
        .unwrap();
        e.undo().unwrap();
        for prefix in semantic_commands(a, b, list) {
            let suffix = if cause == 0 {
                Command::InsertText {
                    node: b,
                    at: usize::MAX,
                    text: "bad".into(),
                }
            } else {
                Command::InsertText {
                    node: b,
                    at: 0,
                    text: "x".repeat(reprise_edit::MAX_INSERT_BYTES + 1),
                }
            };
            let before = frozen(&e);
            let tx = Transaction::from(vec![prefix, suffix]);
            assert!(e.apply(&tx).is_err(), "{tx:?}");
            let after = frozen(&e);
            assert!(
                after == before,
                "refusal changed state for {tx:?}: bytes={} revision={} undo={} redo={}, lengths={}/{}",
                before.0 == after.0,
                before.1 == after.1,
                before.2 == after.2,
                before.3 == after.3,
                before.0.len(),
                after.0.len()
            );
        }
    }
}

#[test]
fn new_staging_commands_refuse_pending_raw_edits_before_committing_them() {
    for inline in [false, true] {
        let doc = Document::new(1).unwrap();
        let a = doc.append_block(BlockKind::Paragraph, "", "AB").unwrap();
        doc.commit();
        let mut e = Editor::new(doc, reprise_fixtures::engine().schemas.clone());
        e.document().block(a).unwrap().text.insert(2, "!").unwrap();
        let before = (e.document().revision(), e.undo_count(), e.redo_count());
        let command = if inline {
            Command::InsertInline {
                node: a,
                at: 0,
                object: reprise_doc::InlineObject::field("page"),
            }
        } else {
            Command::CreateList {
                definition: reprise_doc::ListDefinition::numbered(),
            }
        };
        let error = e
            .apply(&Transaction::from(vec![
                Command::DeleteText {
                    node: a,
                    range: 1..2,
                },
                command,
            ]))
            .unwrap_err();
        assert_eq!(error.reason, reprise_edit::Reason::UncommittedEdit);
        assert_eq!(
            before,
            (e.document().revision(), e.undo_count(), e.redo_count())
        );
        assert_eq!(e.document().block(a).unwrap().text.to_string(), "AB!");
    }
}
#[test]
fn annotation_target_deletions_are_atomic_in_additions_and_updates() {
    use reprise_doc::annotation::{AnnotationData, CommentAnchor, LinkTarget};
    for update in [false, true] {
        for target_range in [false, true] {
            for ancestor in [false, true] {
                let doc = Document::new(1).unwrap();
                let source = doc
                    .append_block(BlockKind::Paragraph, "", "source")
                    .unwrap();
                let parent = doc
                    .append_block(BlockKind::Paragraph, "", "parent")
                    .unwrap();
                let target = doc
                    .insert_block_at(
                        Some(parent),
                        0,
                        &reprise_doc::NewBlock {
                            kind: BlockKind::Paragraph,
                            style: String::new(),
                            text: "target".into(),
                            overrides: Default::default(),
                        },
                    )
                    .unwrap();
                let range = doc
                    .add_annotation(
                        target,
                        0..6,
                        &AnnotationData::Comment(CommentAnchor {
                            thread: "thread".into(),
                        }),
                    )
                    .unwrap();
                let link = doc
                    .add_annotation(
                        source,
                        0..6,
                        &AnnotationData::Link(LinkTarget::Url {
                            url: "https://example.com".into(),
                        }),
                    )
                    .unwrap();
                doc.commit();
                let mut e = Editor::new(doc, reprise_doc::SchemaRegistry::builtin());
                for text in ["!", "?"] {
                    e.apply_command(Command::InsertText {
                        node: source,
                        at: 6,
                        text: text.into(),
                    })
                    .unwrap();
                }
                e.undo().unwrap();
                assert!(e.undo_count() > 0 && e.redo_count() > 0);
                let before = frozen(&e);
                let data = AnnotationData::Link(if target_range {
                    LinkTarget::Range { range }
                } else {
                    LinkTarget::Node { node: target }
                });
                let delete = if target_range && !ancestor {
                    Command::RemoveAnnotation { id: range }
                } else {
                    Command::DeleteBlock {
                        node: if ancestor { parent } else { target },
                    }
                };
                let write = if update {
                    Command::UpdateAnnotation { id: link, data }
                } else {
                    Command::AddAnnotation {
                        node: source,
                        range: 0..6,
                        data,
                    }
                };
                let error = e
                    .apply(&Transaction::from(vec![delete, write]))
                    .unwrap_err();
                assert_eq!(error.command, Some(1));
                assert!(
                    frozen(&e) == before,
                    "refusal changed history, revision or undo/redo"
                );
                assert_eq!(
                    e.document().block(source).unwrap().text.to_string(),
                    "source!"
                );
                assert!(e.document().is_live(target));
                assert!(e.redo().unwrap());
                assert_eq!(
                    e.document().block(source).unwrap().text.to_string(),
                    "source?!"
                );
            }
        }
    }
}

#[test]
fn missing_annotation_validation_preserves_absent_roots_and_history() {
    use reprise_doc::annotation::{AnnotationData, LinkTarget};
    for update in [false, true] {
        let doc = Document::new(1).unwrap();
        doc.append_block(BlockKind::Paragraph, "", "text").unwrap();
        doc.commit();
        let mut e = Editor::new(doc, reprise_doc::SchemaRegistry::builtin());
        let before = frozen(&e);
        let id = reprise_doc::RangeId::parse("3@9").unwrap();
        let command = if update {
            Command::UpdateAnnotation {
                id,
                data: AnnotationData::Link(LinkTarget::Url {
                    url: "https://example.com".into(),
                }),
            }
        } else {
            Command::RemoveAnnotation { id }
        };
        assert!(e.apply_command(command).is_err());
        assert!(
            frozen(&e) == before,
            "missing-ID validation changed history, revision or undo/redo"
        );
    }
}
