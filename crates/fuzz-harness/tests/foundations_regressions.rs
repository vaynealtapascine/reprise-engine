//! Explicit regressions for what the wave-1 foundations fuzz found
//! (`docs/fuzzing.md`). Each test reads as a short story through the public
//! facade and fails without its fix.
//!
//! A test marked `#[ignore = "known: astra-4 R<n>"]` reproduces a fault that
//! the Astra 4 review reported and the `ws/review-fixes-2` stream owns; it is
//! here as an extra regression for that stream. A test marked
//! `#[ignore = "fuzz-2: <id>, see report"]` is a fault this fuzz found whose
//! fix is not local; the report describes it and suggests a fix.
//!
//! Run them all with `cargo test -p reprise-fuzz-harness --test
//! foundations_regressions -- --ignored`.

use reprise::*;

const DOC: &str = "00112233445566778899aabbccddeeff";

fn session() -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: "1".into(),
        }))
        .unwrap()
}

fn apply(s: &mut DocumentSession, commands: Vec<Command>) -> Result<Applied> {
    s.apply(&Payload::new(Transaction { commands }))
        .map(|p| p.data)
}

fn paragraph(s: &mut DocumentSession, text: &str) -> String {
    let index = s.state().unwrap().data.blocks.len() as u32;
    apply(
        s,
        vec![Command::InsertBlock {
            parent: None,
            index,
            block_kind: BlockKind::Paragraph,
            text: text.into(),
            style: Style::default(),
        }],
    )
    .unwrap()
    .blocks[0]
        .clone()
}

fn link(url: &str) -> AnnotationPayload {
    AnnotationPayload::Link {
        target: LinkDestination::Url { url: url.into() },
    }
}

fn field() -> InlineObject {
    InlineObject {
        kind: "field".into(),
        align: InlineAlign::Baseline,
        params: [("field".to_owned(), "page-number".to_owned())].into(),
    }
}

/// Every operation the document holds, as bytes. (A snapshot would also carry
/// empty root containers that even `state()` declares lazily.)
fn operations(s: &DocumentSession) -> Vec<u8> {
    s.sync_export(&Payload::new(SyncRequest {
        since: Some(Vec::new()),
    }))
    .unwrap()
    .data
    .content
    .bytes
}

fn state(s: &DocumentSession) -> State {
    s.state().unwrap().data
}

fn spans(s: &DocumentSession) -> Vec<Vec<(String, u32, u32)>> {
    state(s)
        .annotations
        .iter()
        .map(|a| {
            a.spans
                .iter()
                .map(|x| (x.node.clone(), x.start, x.end))
                .collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// What holds, pinned.

/// A read declares empty root containers in the snapshot, so a refused
/// command is compared by its operations, not by snapshot bytes. This pins
/// both halves: refusals write no operation, and the snapshot may still grow.
#[test]
fn a_refusal_writes_no_operation() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha beta");
    let before = operations(&s);
    for refused in [
        Command::RemoveAnnotation { id: "0@0".into() },
        Command::UpdateAnnotation {
            id: "0@0".into(),
            annotation: link("https://example.com/"),
        },
        Command::SetRole {
            node: "0@0".into(),
            role: BlockRole::Heading1,
        },
        Command::SetListItem {
            node: node.clone(),
            list: ListRef::Existing { id: "0@0".into() },
            level: 0,
        },
        Command::AddAnnotation {
            node,
            start: 0,
            end: 5,
            annotation: link("javascript:alert(1)"),
        },
    ] {
        assert!(apply(&mut s, vec![refused]).is_err());
        assert_eq!(operations(&s), before);
    }
}

/// A stale `expected_revision` is refused as `bindings.stale` and writes
/// nothing, whatever the commands are.
#[test]
fn a_stale_batch_is_refused_and_writes_nothing() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha beta");
    let old = state(&s).revision;
    apply(
        &mut s,
        vec![Command::InsertText {
            node: node.clone(),
            at: 0,
            text: "x".into(),
        }],
    )
    .unwrap();
    let before = operations(&s);
    let refused = s.batch(&Payload::new(Batch {
        commands: vec![Command::SetRole {
            node,
            role: BlockRole::Heading2,
        }],
        expected_revision: Some(old),
        selection: None,
    }));
    assert_eq!(refused.unwrap_err().code(), "bindings.stale");
    assert_eq!(operations(&s), before);
}

// ---------------------------------------------------------------------------
// Open faults of this fuzz.

/// fuzz-2 `annotation-redo`: a link created in the same undo step as the text
/// it covers is gone after undo and redo. Loro's redo re-creates the
/// characters under new identities, and a `FIXED`-policy range anchored to
/// the old ones collapses and disappears (a formatting range, whose policy
/// expands, survives, which is why only annotations show it).
///
/// Seen as: `redo-restores-state` after a paste that carried links (the
/// clipped duplicates vanish on redo); the same with InsertText plus
/// AddAnnotation in one batch.
#[test]
#[ignore = "fuzz-2: annotation-redo, see report"]
fn a_link_made_with_its_text_survives_undo_and_redo() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha ");
    apply(
        &mut s,
        vec![
            Command::InsertText {
                node: node.clone(),
                at: 3,
                text: "XYZ".into(),
            },
            Command::AddAnnotation {
                node,
                start: 3,
                end: 6,
                annotation: link("https://a.example/"),
            },
        ],
    )
    .unwrap();
    let applied = spans(&s);
    assert_eq!(applied.len(), 1);
    s.undo().unwrap();
    assert!(spans(&s).is_empty());
    s.redo().unwrap();
    assert_eq!(spans(&s), applied, "redo lost the link");
}

/// The same fault through the clipboard: pasting text that carries links,
/// undoing and redoing the paste drops the pasted links.
#[test]
#[ignore = "fuzz-2: annotation-redo, see report"]
fn pasted_links_survive_undo_and_redo() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha ");
    apply(
        &mut s,
        vec![Command::AddAnnotation {
            node: node.clone(),
            start: 0,
            end: 6,
            annotation: link("https://a.example/"),
        }],
    )
    .unwrap();
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    while !job.step(&mut s, 100_000).unwrap().data.complete {}
    let caret = |offset| Caret {
        node: node.clone(),
        offset,
        affinity: Affinity::Downstream,
    };
    let copied = s
        .copy(&Payload::new(Selection {
            anchor: caret(0),
            focus: caret(4),
        }))
        .unwrap();
    s.paste(
        &Payload::new(Paste { at: Some(caret(0)) }),
        &copied.data.bytes,
    )
    .unwrap();
    let pasted = spans(&s);
    assert_eq!(pasted.len(), 2, "the paste carries a clipped duplicate");
    s.undo().unwrap();
    s.redo().unwrap();
    assert_eq!(spans(&s), pasted, "redo lost the pasted link");
}

/// fuzz-2 `annotation-redo`, the stretched form: a comment grows over the text
/// its own step inserted beside it. A comment's end takes text typed there
/// (`Kind::Comment`), and redo replays the insertion with the comment's anchors
/// already in place; the step made the text first. A link, whose edges stay
/// outside, comes back exact (the next test).
/// Seen as: `redo-restores-state` after a join followed by a comment (soak
/// seed 500763), and after a paste (seed 401231, below).
#[test]
#[ignore = "fuzz-2: annotation-redo, see report"]
fn a_comment_made_beside_its_text_keeps_its_extent_on_redo() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha ");
    apply(
        &mut s,
        vec![
            Command::InsertText {
                node: node.clone(),
                at: 6,
                text: "beta ".into(),
            },
            Command::AddAnnotation {
                node,
                start: 0,
                end: 6,
                annotation: AnnotationPayload::Comment { thread: "t".into() },
            },
        ],
    )
    .unwrap();
    let applied = spans(&s);
    s.undo().unwrap();
    s.redo().unwrap();
    assert_eq!(
        spans(&s),
        applied,
        "redo stretched the comment over the new text"
    );
}

/// The contrast: the same step with a link.
#[test]
fn a_link_made_beside_its_text_keeps_its_extent_on_redo() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha ");
    apply(
        &mut s,
        vec![
            Command::InsertText {
                node: node.clone(),
                at: 6,
                text: "beta ".into(),
            },
            Command::AddAnnotation {
                node,
                start: 0,
                end: 6,
                annotation: link("https://a.example/"),
            },
        ],
    )
    .unwrap();
    let applied = spans(&s);
    s.undo().unwrap();
    s.redo().unwrap();
    assert_eq!(spans(&s), applied);
}

/// `import_copy` keeps live definitions nothing refers to (docs/roles-and-lists.md),
/// but not when no block was left: the paste returned early for a fragment
/// without blocks, before it reached the lists. (Soak seeds 400112, 400958,
/// 401115, 500252, 500599, 500658, 500667, 600161, 600501 and 600621.)
#[test]
fn import_copy_keeps_the_lists_of_a_document_with_no_blocks() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha");
    let level = ListLevel {
        level: 0,
        format: Some("decimal".into()),
        ..ListLevel::default()
    };
    apply(
        &mut s,
        vec![
            Command::CreateList {
                definition: ListDefinition {
                    base: None,
                    levels: vec![level],
                },
            },
            Command::SetListItem {
                node: node.clone(),
                list: ListRef::New { index: 0 },
                level: 0,
            },
        ],
    )
    .unwrap();
    apply(&mut s, vec![Command::DeleteBlock { node }]).unwrap();
    let source = state(&s);
    assert!(source.blocks.is_empty());
    assert_eq!(
        source.lists.len(),
        1,
        "the definition outlives its only item"
    );
    let bytes = s.save().unwrap().data.bytes;
    let copy = Workspace::new()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: "ffeeddccbbaa99887766554433221100".into(),
                peer_id: "9".into(),
            }),
            &bytes,
        )
        .unwrap();
    let definitions = |s: &State| {
        s.lists
            .iter()
            .map(|l| l.definition.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(definitions(&state(&copy)), definitions(&source));
}

/// A list based on another, the paragraph in the derived one, and the base
/// deleted or not. Returns the session and its `import_copy`.
fn based_lists(delete_base: bool) -> (DocumentSession, DocumentSession) {
    let mut s = lettered_workspace()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: "1".into(),
        }))
        .unwrap();
    let node = paragraph(&mut s, "alpha");
    let level = |format: &str| ListLevel {
        level: 0,
        format: Some(format.into()),
        ..ListLevel::default()
    };
    let base = apply(
        &mut s,
        vec![Command::CreateList {
            definition: ListDefinition {
                base: None,
                levels: vec![level("upper-alpha")],
            },
        }],
    )
    .unwrap()
    .lists[0]
        .clone();
    apply(
        &mut s,
        vec![
            Command::CreateList {
                definition: ListDefinition {
                    base: Some(base.clone()),
                    levels: vec![],
                },
            },
            Command::SetListItem {
                node,
                list: ListRef::New { index: 0 },
                level: 0,
            },
        ],
    )
    .unwrap();
    if delete_base {
        apply(
            &mut s,
            vec![Command::DeleteList {
                list: ListRef::Existing { id: base },
            }],
        )
        .unwrap();
    }
    let bytes = s.save().unwrap().data.bytes;
    let copy = lettered_workspace()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: "ffeeddccbbaa99887766554433221100".into(),
                peer_id: "9".into(),
            }),
            &bytes,
        )
        .unwrap();
    (s, copy)
}

/// A live base that no paragraph uses still comes across, and the derived list
/// keeps pointing at it.
#[test]
fn import_copy_keeps_a_live_base_nothing_uses() {
    let (s, copy) = based_lists(false);
    let (source, copied) = (state(&s), state(&copy));
    assert_eq!(source.lists.len(), 2);
    assert_eq!(
        reprise_fuzz_harness::foundations::oracles::shape(&source),
        reprise_fuzz_harness::foundations::oracles::shape(&copied)
    );
    let bases = |s: &State| {
        s.lists
            .iter()
            .filter_map(|l| l.definition.as_ref()?.base.clone())
            .count()
    };
    assert_eq!((bases(&source), bases(&copied)), (1, 1));
}

/// Pinned as designed: a base deleted since is a tombstone, and a copy replays
/// live definitions only (docs/roles-and-lists.md), so the derived list comes
/// across with no base. The document draws the same either way; only the
/// `list.definition-missing` warning the source carried is gone. (Soak seed
/// 700049: the oracle used to read this as a changed copy.)
#[test]
fn import_copy_drops_a_deleted_base_and_draws_the_same() {
    let (mut s, mut copy) = based_lists(true);
    let (source, copied) = (state(&s), state(&copy));
    assert_eq!(source.lists.len(), 1);
    assert!(
        source.lists[0].definition.as_ref().unwrap().base.is_some(),
        "the source still names the deleted base"
    );
    assert_eq!(copied.lists[0].definition.as_ref().unwrap().base, None);
    assert_eq!(
        reprise_fuzz_harness::foundations::oracles::shape(&source),
        reprise_fuzz_harness::foundations::oracles::shape(&copied)
    );
    assert_eq!(drawing(&mut s), drawing(&mut copy));
}

/// Pinned as designed: an orphaned comment keeps a point in its paragraph and
/// comes across `import_copy` with it; one whose paragraph was deleted has no
/// span in the source either, nothing in a copy to attach it to, and is not
/// carried (a copy replays live content; the tombstone stays with the source).
/// The oracle compares the first kind and skips the second. (Eleven soak seeds,
/// among them 400026, 400392 and 500457.)
#[test]
fn import_copy_carries_an_orphan_with_a_paragraph_and_not_without_one() {
    for delete_block in [false, true] {
        let mut s = session();
        let node = paragraph(&mut s, "alpha");
        paragraph(&mut s, "beta");
        apply(
            &mut s,
            vec![Command::AddAnnotation {
                node: node.clone(),
                start: 0,
                end: 5,
                annotation: AnnotationPayload::Comment { thread: "t".into() },
            }],
        )
        .unwrap();
        apply(
            &mut s,
            vec![if delete_block {
                Command::DeleteBlock { node: node.clone() }
            } else {
                Command::DeleteText {
                    node: node.clone(),
                    start: 0,
                    end: 5,
                }
            }],
        )
        .unwrap();
        let orphans = |s: &DocumentSession| {
            state(s)
                .annotations
                .iter()
                .map(|a| (a.status, a.spans.len()))
                .collect::<Vec<_>>()
        };
        let source = orphans(&s);
        assert_eq!(source.len(), 1);
        assert_eq!(source[0].1, usize::from(!delete_block), "{delete_block}");
        let bytes = s.save().unwrap().data.bytes;
        let copy = Workspace::new()
            .import_copy(
                &Payload::new(ImportCopy {
                    document_id: "ffeeddccbbaa99887766554433221100".into(),
                    peer_id: "9".into(),
                }),
                &bytes,
            )
            .unwrap();
        let copied = orphans(&copy);
        if delete_block {
            assert!(copied.is_empty(), "nothing to attach it to");
        } else {
            assert_eq!(copied, source);
        }
    }
}

/// fuzz-2 `format-undo`: bold a word, delete it, undo: the word comes back
/// without its bold. Annotations have restore hints for this (docs/
/// annotations.md, "Undo"); formatting ranges have none.
#[test]
#[ignore = "fuzz-2: format-undo, see report"]
fn deleted_bold_text_is_bold_again_after_undo() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha beta");
    apply(
        &mut s,
        vec![Command::FormatText {
            node: node.clone(),
            start: 0,
            end: 5,
            style: TextStyle {
                weight: Some(700),
                ..TextStyle::default()
            },
        }],
    )
    .unwrap();
    let formatted = state(&s).blocks[0].formatting.clone();
    assert!(formatted.is_some());
    apply(
        &mut s,
        vec![Command::DeleteText {
            node,
            start: 0,
            end: 5,
        }],
    )
    .unwrap();
    s.undo().unwrap();
    assert_eq!(state(&s).blocks[0].text, "alpha beta");
    assert_eq!(
        state(&s).blocks[0].formatting,
        formatted,
        "undoing the delete lost the bold"
    );
}

// ---------------------------------------------------------------------------
// Known: reported by Astra 4, owned by ws/review-fixes-2.

/// Astra 4 R1. A refused batch publishes the commands before the refused one:
/// target validation of an annotation sees the starting document, not the
/// one the earlier commands leave.
/// Fuzz seed: `FOUNDATIONS=1 explore 31 1 300` (default generator, before the
/// shape was excluded).
#[test]
fn a_refused_batch_after_a_deletion_leaves_the_document_alone() {
    let mut s = session();
    let source = paragraph(&mut s, "source text");
    let target = paragraph(&mut s, "target");
    let before = (operations(&s), state(&s).blocks);
    let refused = s.batch(&Payload::new(Batch {
        commands: vec![
            Command::DeleteBlock {
                node: target.clone(),
            },
            Command::AddAnnotation {
                node: source,
                start: 0,
                end: 6,
                annotation: AnnotationPayload::Link {
                    target: LinkDestination::Node { node: target },
                },
            },
        ],
        expected_revision: Some(state(&s).revision),
        selection: None,
    }));
    assert!(refused.is_err());
    assert_eq!(state(&s).blocks, before.1, "the deletion was published");
    assert_eq!(operations(&s), before.0);
}

/// Astra 4 R1 through `MoveBlock` (moving a flow paragraph copies it, so the
/// annotation's target is dead after the move) and `UpdateAnnotation`.
/// Fuzz seed 31 of the default scenarios.
#[test]
fn a_refused_batch_after_a_move_leaves_the_document_alone() {
    let mut s = session();
    let first = paragraph(&mut s, "first paragraph");
    let split = apply(
        &mut s,
        vec![Command::SplitBlock {
            node: first.clone(),
            at: 5,
        }],
    )
    .unwrap();
    let Effect::Split { new: second, .. } = split.effects[0].clone() else {
        panic!("expected a split");
    };
    let id = apply(
        &mut s,
        vec![Command::AddAnnotation {
            node: first,
            start: 0,
            end: 5,
            annotation: link("https://example.com/"),
        }],
    )
    .unwrap()
    .annotations[0]
        .clone();
    let before = (operations(&s), state(&s).blocks);
    let refused = apply(
        &mut s,
        vec![
            Command::MoveBlock {
                node: second.clone(),
                parent: None,
                index: 0,
            },
            Command::UpdateAnnotation {
                id,
                annotation: AnnotationPayload::Link {
                    target: LinkDestination::Node { node: second },
                },
            },
        ],
    );
    assert!(refused.is_err(), "the target died with the move");
    assert_eq!(state(&s).blocks, before.1, "the move was published");
    assert_eq!(operations(&s), before.0);
}

/// fuzz-2 `join-atom-undo` (Astra 4 R4 does not cover it; this is still open on
/// main): a link that crosses two paragraphs comes back without its first piece
/// after undoing a copying join that retired an active inline object. The join
/// must name the later paragraph first (a forward join is fine), and the object
/// must be active when it goes.
#[test]
#[ignore = "fuzz-2: join-atom-undo, see report"]
fn undoing_a_join_that_retires_an_atom_keeps_the_whole_link() {
    let mut s = session();
    let first = paragraph(&mut s, "alpha beta gamma delta");
    apply(
        &mut s,
        vec![Command::AddAnnotation {
            node: first.clone(),
            start: 0,
            end: 22,
            annotation: link("https://a.example/"),
        }],
    )
    .unwrap();
    let split = apply(
        &mut s,
        vec![Command::SplitBlock {
            node: first.clone(),
            at: 6,
        }],
    )
    .unwrap();
    let Effect::Split { new: second, .. } = split.effects[0].clone() else {
        panic!("expected a split");
    };
    apply(
        &mut s,
        vec![Command::InsertInline {
            node: first.clone(),
            at: 0,
            object: field(),
        }],
    )
    .unwrap();
    let before = spans(&s);
    apply(
        &mut s,
        vec![Command::JoinBlocks {
            first: second,
            second: first,
        }],
    )
    .unwrap();
    s.undo().unwrap();
    assert_eq!(spans(&s), before, "undo lost a piece of the link");
}

// ---------------------------------------------------------------------------
// The package bits, pinned.

fn required_bits(s: &mut DocumentSession) -> u64 {
    use reprise_format::{Container, Limits};
    let bytes = s.save().unwrap().data.bytes;
    Container::decode(&bytes, Limits::default())
        .unwrap()
        .0
        .header
        .features
        .required
}

/// What each wave-1 stored form does to the required bits: 14 for roles and
/// lists (set while any paragraph metadata *holds* `role1`, `list1`,
/// `level1` or `restart1`, or the `lists` tree has a node: docs/
/// roles-and-lists.md), 15 for inline objects and 16 for annotations (any
/// record, live or not), 18 for style identities. A document that never used
/// a feature carries none of them, and a duplicate (`import_copy`) is judged
/// by its live content because its history starts fresh.
#[test]
fn package_bits_follow_the_stored_forms() {
    use reprise_format::features::{
        REQUIRED_ANNOTATIONS, REQUIRED_BLOCK_SEMANTICS, REQUIRED_INLINE_OBJECTS,
        REQUIRED_PARAGRAPH_STYLES, REQUIRED_STYLE_IDENTITIES,
    };
    let wave1 = REQUIRED_BLOCK_SEMANTICS
        | REQUIRED_INLINE_OBJECTS
        | REQUIRED_ANNOTATIONS
        | REQUIRED_PARAGRAPH_STYLES
        | REQUIRED_STYLE_IDENTITIES;
    let mut s = session();
    assert_eq!(required_bits(&mut s) & wave1, 0, "a new document");
    let node = paragraph(&mut s, "alpha beta gamma");
    assert_eq!(required_bits(&mut s) & wave1, 0, "plain text");

    apply(
        &mut s,
        vec![Command::SetRole {
            node: node.clone(),
            role: BlockRole::Heading1,
        }],
    )
    .unwrap();
    assert_eq!(required_bits(&mut s) & wave1, REQUIRED_BLOCK_SEMANTICS);
    s.undo().unwrap();
    assert_eq!(
        required_bits(&mut s) & wave1,
        0,
        "no paragraph holds a role any more"
    );

    apply(
        &mut s,
        vec![Command::InsertInline {
            node: node.clone(),
            at: 0,
            object: field(),
        }],
    )
    .unwrap();
    assert_eq!(required_bits(&mut s) & wave1, REQUIRED_INLINE_OBJECTS);
    apply(
        &mut s,
        vec![Command::AddAnnotation {
            node: node.clone(),
            start: 3,
            end: 8,
            annotation: AnnotationPayload::Comment { thread: "t".into() },
        }],
    )
    .unwrap();
    assert_eq!(
        required_bits(&mut s) & wave1,
        REQUIRED_INLINE_OBJECTS | REQUIRED_ANNOTATIONS
    );
    apply(
        &mut s,
        vec![Command::DefineStyle {
            name: "S0".into(),
            parent: None,
            style: Style::default(),
        }],
    )
    .unwrap();
    let all = REQUIRED_INLINE_OBJECTS
        | REQUIRED_ANNOTATIONS
        | REQUIRED_PARAGRAPH_STYLES
        | REQUIRED_STYLE_IDENTITIES;
    assert_eq!(required_bits(&mut s) & wave1, all);

    // A duplicate declares what its live content needs.
    let copy_of = |s: &mut DocumentSession, id: &str| {
        let bytes = s.save().unwrap().data.bytes;
        let mut copy = Workspace::new()
            .import_copy(
                &Payload::new(ImportCopy {
                    document_id: id.into(),
                    peer_id: "9".into(),
                }),
                &bytes,
            )
            .unwrap();
        required_bits(&mut copy) & wave1
    };
    assert_eq!(copy_of(&mut s, "ffeeddccbbaa99887766554433221100"), all);
    // A history that used a feature and undid it has nothing to copy.
    let mut t = session();
    let n = paragraph(&mut t, "alpha beta");
    apply(
        &mut t,
        vec![Command::SetRole {
            node: n,
            role: BlockRole::Heading2,
        }],
    )
    .unwrap();
    t.undo().unwrap();
    assert_eq!(copy_of(&mut t, "ffeeddccbbaa99887766554433221101"), 0);
}

/// `import_copy` keeps every readable annotation, but a link to a block that
/// no longer exists has nothing to point at in the copy: it is dropped and the
/// copy says so (`clipboard.annotation-dropped`). A link to a live block is
/// remapped to the copy's block. (Found as a false alarm of the fuzz's copy
/// oracle, which now allows exactly this.)
#[test]
fn import_copy_drops_a_link_to_a_dead_block_and_says_so() {
    let ws = Workspace::new();
    let mut s = session();
    let source = paragraph(&mut s, "source text here");
    let target = paragraph(&mut s, "target");
    apply(
        &mut s,
        vec![Command::AddAnnotation {
            node: source,
            start: 0,
            end: 6,
            annotation: AnnotationPayload::Link {
                target: LinkDestination::Node {
                    node: target.clone(),
                },
            },
        }],
    )
    .unwrap();
    let copy_of = |s: &mut DocumentSession, id: &str| {
        let bytes = s.save().unwrap().data.bytes;
        ws.import_copy(
            &Payload::new(ImportCopy {
                document_id: id.into(),
                peer_id: "9".into(),
            }),
            &bytes,
        )
        .unwrap()
    };
    let live = copy_of(&mut s, "ffeeddccbbaa99887766554433221100");
    assert_eq!(state(&live).annotations.len(), 1);
    assert!(live.diagnostics().data.is_empty());
    apply(&mut s, vec![Command::DeleteBlock { node: target }]).unwrap();
    let dead = copy_of(&mut s, "ffeeddccbbaa99887766554433221101");
    assert!(state(&dead).annotations.is_empty());
    let codes: Vec<String> = dead
        .diagnostics()
        .data
        .into_iter()
        .map(|d| d.code)
        .collect();
    assert_eq!(codes, ["clipboard.annotation-dropped"]);
}

/// fuzz-2 `link-resurrected`: a link whose text was deleted is gone, and stays
/// gone until that delete is undone. Undoing a *later* delete of the same text,
/// typed again, brings it back: the restore hint recorded by the first delete
/// is still on the annotation, and the re-typed characters match its digest and
/// were created after it.
#[test]
#[ignore = "fuzz-2: link-resurrected, see report"]
fn a_link_removed_with_its_text_is_not_brought_back_by_a_later_undo() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha beta gamma delta");
    apply(
        &mut s,
        vec![
            Command::AddAnnotation {
                node: node.clone(),
                start: 0,
                end: 5,
                annotation: link("https://a.example/"),
            },
            Command::DeleteText {
                node: node.clone(),
                start: 0,
                end: 5,
            },
        ],
    )
    .unwrap();
    assert!(spans(&s).is_empty(), "deleting its text removed the link");
    apply(
        &mut s,
        vec![Command::InsertText {
            node: node.clone(),
            at: 0,
            text: "alpha".into(),
        }],
    )
    .unwrap();
    apply(
        &mut s,
        vec![Command::DeleteText {
            node,
            start: 0,
            end: 5,
        }],
    )
    .unwrap();
    s.undo().unwrap();
    assert_eq!(state(&s).blocks[0].text, "alpha beta gamma delta");
    assert!(
        spans(&s).is_empty(),
        "undo brought back a link it never took"
    );
}

/// fuzz-2 `tombstoned-endpoint` (fixed in `annotation_restore.rs`): a comment whose start text was deleted comes
/// back orphaned at the end of the paragraph after undoing the move-by-copy of
/// the flow paragraph that holds it. Without the earlier delete (or with a
/// delete that doesn't reach the start of the range) undo restores it.
#[test]
fn undoing_a_move_keeps_a_comment_whose_start_was_deleted() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha beta gamma delta");
    apply(
        &mut s,
        vec![Command::AddAnnotation {
            node: node.clone(),
            start: 0,
            end: 13,
            annotation: AnnotationPayload::Comment { thread: "t".into() },
        }],
    )
    .unwrap();
    apply(
        &mut s,
        vec![Command::DeleteText {
            node: node.clone(),
            start: 0,
            end: 6,
        }],
    )
    .unwrap();
    let split = apply(&mut s, vec![Command::SplitBlock { node, at: 0 }]).unwrap();
    let Effect::Split { new: second, .. } = split.effects[0].clone() else {
        panic!("expected a split");
    };
    let before = state(&s).annotations;
    apply(
        &mut s,
        vec![Command::MoveBlock {
            node: second,
            parent: None,
            index: 0,
        }],
    )
    .unwrap();
    assert_eq!(state(&s).annotations.len(), 1);
    s.undo().unwrap();
    let after = state(&s).annotations;
    assert_eq!(after.len(), before.len());
    assert_eq!(
        after[0].status,
        AnnotationStatus::Active,
        "undo orphaned the comment: {after:?}"
    );
    assert_eq!(
        after[0]
            .spans
            .iter()
            .map(|x| (x.start, x.end))
            .collect::<Vec<_>>(),
        before[0]
            .spans
            .iter()
            .map(|x| (x.start, x.end))
            .collect::<Vec<_>>()
    );
}

/// fuzz-2 `tombstoned-endpoint` (fixed), without the move: delete the start of a
/// comment's range, then delete all that is left of its text, then undo. The
/// comment comes back orphaned at the end of the paragraph instead of over its
/// text. The restore hint of the second delete needs the range's start
/// anchor, which the first delete had already left a tombstone. (Seed 5002 of
/// the default scenarios, minimised.)
#[test]
fn undoing_a_delete_of_all_the_text_restores_a_comment_whose_start_was_deleted() {
    let mut s = session();
    let node = paragraph(&mut s, "alpha beta gamma delta");
    apply(
        &mut s,
        vec![Command::AddAnnotation {
            node: node.clone(),
            start: 0,
            end: 13,
            annotation: AnnotationPayload::Comment { thread: "t".into() },
        }],
    )
    .unwrap();
    apply(
        &mut s,
        vec![Command::DeleteText {
            node: node.clone(),
            start: 0,
            end: 6,
        }],
    )
    .unwrap();
    let before = state(&s).annotations;
    assert_eq!(before[0].status, AnnotationStatus::Active);
    let len = state(&s).blocks[0].text.len() as u32;
    apply(
        &mut s,
        vec![Command::DeleteText {
            node,
            start: 0,
            end: len,
        }],
    )
    .unwrap();
    s.undo().unwrap();
    let after = state(&s).annotations;
    assert_eq!(after, before, "undo changed the comment");
}

/// fuzz-2 `tombstoned-endpoint` (fixed), the shifting form: a link ends and a comment
/// starts at the same offset, left there by an earlier delete. Deleting a span
/// that covers that offset and undoing it moves both ends to the end of the
/// restored text. (Seed 5002 of the default scenarios.)
#[test]
fn undoing_a_delete_across_an_earlier_delete_point_keeps_both_annotations_in_place() {
    let mut s = session();
    let node = paragraph(&mut s, "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ");
    apply(
        &mut s,
        vec![
            Command::AddAnnotation {
                node: node.clone(),
                start: 0,
                end: 19,
                annotation: link("https://a.example/"),
            },
            Command::AddAnnotation {
                node: node.clone(),
                start: 15,
                end: 39,
                annotation: AnnotationPayload::Comment { thread: "t".into() },
            },
            Command::DeleteText {
                node: node.clone(),
                start: 15,
                end: 19,
            },
        ],
    )
    .unwrap();
    let before = spans(&s);
    assert_eq!(
        before.iter().map(|p| (p[0].1, p[0].2)).collect::<Vec<_>>(),
        [(0, 15), (15, 35)]
    );
    apply(
        &mut s,
        vec![Command::DeleteText {
            node,
            start: 15,
            end: 31,
        }],
    )
    .unwrap();
    s.undo().unwrap();
    assert_eq!(spans(&s), before, "undo moved the annotations' ends");
}

/// Astra 4 R4 (fixed on main by review fixes 2): a deleted inline object leaves
/// a hidden marker in the shared text, so a later visible delete across it is
/// several spans. Undoing it must put a link and a comment over "bc" back
/// where they were, not on "cd".
#[test]
fn undoing_a_delete_across_a_hidden_marker_restores_a_link_and_a_comment() {
    let mut s = session();
    let node = paragraph(&mut s, "abcdef");
    apply(
        &mut s,
        vec![Command::InsertInline {
            node: node.clone(),
            at: 2,
            object: field(),
        }],
    )
    .unwrap();
    apply(
        &mut s,
        vec![Command::DeleteText {
            node: node.clone(),
            start: 2,
            end: 5,
        }],
    )
    .unwrap();
    assert_eq!(state(&s).blocks[0].text, "abcdef", "the object is gone");
    apply(
        &mut s,
        vec![
            Command::AddAnnotation {
                node: node.clone(),
                start: 1,
                end: 3,
                annotation: link("https://a.example/"),
            },
            Command::AddAnnotation {
                node: node.clone(),
                start: 1,
                end: 3,
                annotation: AnnotationPayload::Comment { thread: "t".into() },
            },
        ],
    )
    .unwrap();
    let before = state(&s).annotations;
    apply(
        &mut s,
        vec![Command::DeleteText {
            node,
            start: 0,
            end: 4,
        }],
    )
    .unwrap();
    s.undo().unwrap();
    assert_eq!(state(&s).blocks[0].text, "abcdef");
    assert_eq!(
        state(&s).annotations,
        before,
        "undo rebound the annotations"
    );
}

/// fuzz-2 `insert-in-deleted-span`: one transaction types into annotated text
/// and then deletes a span that contains what it typed. Undoing the step
/// brings the text back but not the link, and the comment's start moves. The
/// restore hint describes the deleted span, but the same undo deletes the
/// typed characters again, so what is left no longer matches it. Deleting the
/// same span in a step of its own is restored exactly.
/// (Seed 200278 of the default scenarios, minimised.)
#[test]
#[ignore = "fuzz-2: insert-in-deleted-span, see report"]
fn undoing_a_step_that_types_then_deletes_over_it_restores_the_annotations() {
    let mut s = session();
    let node = paragraph(&mut s, "abcdefghijklmnopqrstuvwxyz0123456789");
    apply(
        &mut s,
        vec![
            Command::AddAnnotation {
                node: node.clone(),
                start: 0,
                end: 10,
                annotation: link("https://a.example/"),
            },
            Command::AddAnnotation {
                node: node.clone(),
                start: 8,
                end: 20,
                annotation: AnnotationPayload::Comment { thread: "t".into() },
            },
        ],
    )
    .unwrap();
    let before = state(&s).annotations;
    apply(
        &mut s,
        vec![
            Command::InsertText {
                node: node.clone(),
                at: 12,
                text: "XY".into(),
            },
            Command::DeleteText {
                node,
                start: 0,
                end: 14,
            },
        ],
    )
    .unwrap();
    s.undo().unwrap();
    assert_eq!(
        state(&s).blocks[0].text,
        "abcdefghijklmnopqrstuvwxyz0123456789"
    );
    assert_eq!(
        state(&s).annotations,
        before,
        "undo lost or moved annotations"
    );
}

/// fuzz-2 `copy-extends-annotation` (not minimised past a scenario): after a
/// flow split and a move-by-copy of the paragraph that holds a comment and a
/// link, `import_copy` gives both a span over an unrelated paragraph that the
/// source's `State` does not list. The copy must cover no more than its source.
/// The bytes are a scenario of `reprise_fuzz_harness::foundations` as the
/// generators stood at the commit that added this test (seed 100632 of the
/// soak, shrunk); a later change to a generator changes what they mean.
#[test]
#[ignore = "fuzz-2: copy-extends-annotation, see report"]
fn an_import_copy_covers_no_more_than_its_source() {
    let bytes: Vec<u8> = "00b2002f0000bb000000007d00cbc20000f5740000d14c000000008bd668c800ce0000000000000000000000fa00000000000000000000006d3000bb8d0000fe96c9e20000150000d000000098000000ad44cb00006e00f0d8140000006e000000005673004517d32d6200c200dd00000065000000da00695900dd0000f2000000000000000000006e00d6fee3"
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    let report = reprise_fuzz_harness::foundations::run_foundations(&bytes).unwrap();
    assert_eq!(
        report
            .counts
            .get("known:fuzz2-copy-extends-annotation")
            .copied()
            .unwrap_or(0),
        0,
        "an import_copy gave an annotation more extent than its source has"
    );
}

// ---------------------------------------------------------------------------
// Layout

/// A workspace that can draw text: one face, standing for the serif generic.
fn lettered_workspace() -> Workspace {
    let ws = Workspace::new();
    ws.declare_font(
        &Payload::new(FontDeclaration {
            family: "Source Serif Pro".into(),
            weight: 400,
            style: FontStyle::Normal,
            stretch: 1000,
            face_index: 0,
        }),
        reprise_fixtures::SERIF,
    )
    .unwrap();
    ws.set_font_settings(&Payload::new(FontSettings {
        generics: std::collections::BTreeMap::from([(
            GenericFamily::Serif,
            "Source Serif Pro".to_owned(),
        )]),
        ..FontSettings::default()
    }))
    .unwrap();
    ws
}

/// Every page of a finished layout, as the display list draws it.
fn drawing(s: &mut DocumentSession) -> Vec<String> {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    let pages = loop {
        let progress = job.step(s, 100_000).unwrap().data;
        if progress.complete {
            break progress.pages;
        }
    };
    pages
        .into_iter()
        .map(|page| s.display_json(page).unwrap().data)
        .collect()
}

/// The same document opened from its package and laid out from scratch.
fn fresh_drawing(s: &mut DocumentSession) -> Vec<String> {
    let bytes = s.save().unwrap().data.bytes;
    let mut reopened = lettered_workspace()
        .open(
            &Payload::new(Open {
                peer_id: "8".into(),
            }),
            &bytes,
        )
        .unwrap();
    drawing(&mut reopened)
}

fn page_of(target: String) -> InlineObject {
    InlineObject {
        kind: "field".into(),
        align: InlineAlign::Baseline,
        params: [
            ("field".to_owned(), "page-of".to_owned()),
            ("target".to_owned(), target),
        ]
        .into(),
    }
}

fn set_target(node: &str, at: u32, target: Option<String>) -> Command {
    Command::UpdateInline {
        node: node.into(),
        at,
        patch: InlinePatch {
            align: None,
            params: [("target".to_owned(), target)].into(),
        },
    }
}

fn field_target_of(s: &DocumentSession, node: &str) -> String {
    s.field_target(&Payload::new(FieldTarget {
        node: node.into(),
        at: 0,
    }))
    .unwrap()
    .data
}

/// The region pass is cached on the blocks it flowed, and the blocks hold
/// only the text a field was composed with. Removing the target left that
/// text as it was, so the session kept the page it had found before.
/// (Soak seed 500685, `incremental-equals-reference`.)
#[test]
fn a_page_of_that_loses_its_target_is_drawn_as_a_fresh_layout_draws_it() {
    let mut s = lettered_workspace()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: "1".into(),
        }))
        .unwrap();
    let one = paragraph(&mut s, "Intro");
    let two = paragraph(&mut s, "see  for");
    let target = field_target_of(&s, &one);
    apply(
        &mut s,
        vec![Command::InsertInline {
            node: two.clone(),
            at: 4,
            object: page_of(target),
        }],
    )
    .unwrap();
    let resolved = drawing(&mut s);
    apply(&mut s, vec![set_target(&two, 4, None)]).unwrap();
    let live = drawing(&mut s);
    assert_ne!(live, resolved, "the field no longer finds its page");
    assert_eq!(live, fresh_drawing(&mut s));
}

/// The same cache, retargeted to a block on another page.
#[test]
fn a_page_of_that_is_retargeted_is_drawn_as_a_fresh_layout_draws_it() {
    let mut s = lettered_workspace()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: "1".into(),
        }))
        .unwrap();
    let ids: Vec<String> = (0..=120)
        .map(|i| paragraph(&mut s, &format!("paragraph {i}")))
        .collect();
    let (first, last) = (field_target_of(&s, &ids[0]), field_target_of(&s, &ids[120]));
    apply(
        &mut s,
        vec![Command::InsertInline {
            node: ids[60].clone(),
            at: 0,
            object: page_of(first),
        }],
    )
    .unwrap();
    let before = drawing(&mut s);
    assert!(before.len() > 1, "the document runs to a second page");
    apply(&mut s, vec![set_target(&ids[60], 0, Some(last))]).unwrap();
    let live = drawing(&mut s);
    assert_ne!(live, before, "the field names another page now");
    assert_eq!(live, fresh_drawing(&mut s));
}

/// The same fault through a paste: the link a paste brings has its last piece
/// grow on redo (seed 401231, shrunk). The bytes are a scenario of
/// `reprise_fuzz_harness::foundations` as the generators stood at the commit
/// that added this test; a later change to a generator changes what they mean.
#[test]
#[ignore = "fuzz-2: annotation-redo, see report"]
fn a_pasted_link_keeps_its_extent_on_redo() {
    let bytes: Vec<u8> = "001c0000e982b2df00ed8b5f00000000000000007f001b00100000000000cf000000ddc7a83c000000000000b500008ca5000000008d000022000000d64a00000000000000000000000000a400be0000000000bb00a5000000000000000000a7000000f500cc00000000000000000000ef92000000e7ce0000000000bdbbb3000000fb00008eb6"
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    let report = reprise_fuzz_harness::foundations::run_foundations(&bytes).unwrap();
    assert_eq!(
        report
            .counts
            .get("known:fuzz2-annotation-redo-stretched")
            .copied()
            .unwrap_or(0),
        0,
        "redo gave an annotation its step created more extent than it had"
    );
}
