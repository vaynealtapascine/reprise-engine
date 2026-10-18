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
#[ignore = "known: astra-4 R1"]
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
#[ignore = "known: astra-4 R1"]
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

/// Astra 4 R4 as the fuzz reaches it: an annotation over a paragraph that
/// holds an active inline object comes back without one of its pieces after
/// undoing a copying join that retired the object (the join must name the
/// later paragraph first; a forward join is fine).
#[test]
#[ignore = "known: astra-4 R4"]
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
