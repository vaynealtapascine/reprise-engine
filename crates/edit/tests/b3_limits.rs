//! B3 fixes (the Astra B3 review), finding 4: the aggregate limits count the
//! whole envelope, whatever the destination does with it, beyond the review's
//! own probes in `review_b3.rs`.
use reprise_doc::fragment::Fragment;
use reprise_doc::{BlockKind, Document, Domain, NewBlock, NodeId, PersistenceMode, SchemaRegistry};
use reprise_edit::{Command, Editor, Reason, Transaction};
use serde_json::{Value, json};

const BIG: usize = 4097;

fn base() -> Fragment {
    let d = Document::new(1).unwrap();
    d.append_block(BlockKind::Paragraph, "", "body").unwrap();
    d.commit();
    d.copy_all_fragment("source", &SchemaRegistry::builtin())
        .unwrap()
}

fn story_blocks(f: &Fragment, n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| {
            let mut b = serde_json::to_value(&f.blocks[0]).unwrap();
            b["id"] = json!(format!("{}@99", i + 1));
            b["text"] = json!("story");
            b
        })
        .collect()
}

fn with_envelope(mut f: Fragment, envelope: Value) -> Fragment {
    f.sections = Some(envelope.to_string());
    f
}

/// A destination whose undo and redo stacks are both populated, and one
/// paragraph in the body and one in a story.
fn destination() -> (Editor, NodeId, NodeId) {
    let d = Document::new(2).unwrap();
    let body = d
        .append_block(BlockKind::Paragraph, "", "existing")
        .unwrap();
    let story = d
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "", "held")])
        .unwrap();
    let held = d.top(Domain::Story(story))[0];
    d.commit();
    let mut e = Editor::new(d, SchemaRegistry::builtin());
    for text in ["a", "b"] {
        e.apply(&Transaction::from(vec![Command::InsertText {
            node: body,
            at: 0,
            text: text.into(),
        }]))
        .unwrap();
    }
    assert!(e.undo().unwrap());
    assert!(e.undo_count() > 0 && e.redo_count() > 0);
    (e, body, held)
}

type History = (Vec<u8>, reprise_doc::Revision, usize, usize);

fn history(e: &Editor) -> History {
    (
        e.document().export(PersistenceMode::History),
        e.document().revision(),
        e.undo_count(),
        e.redo_count(),
    )
}

/// An over-limit envelope is refused as a copy limit, and the history bytes,
/// revision and both undo stacks are as they were.
fn refused_unchanged(e: &mut Editor, f: &Fragment, at: Option<(NodeId, usize)>, what: &str) {
    let before = history(e);
    let err = e.paste(f, at, "dest").unwrap_err();
    assert!(
        matches!(err.reason, Reason::CopyLimit(_)),
        "{what}: {:?}",
        err.reason
    );
    assert!(history(e) == before, "{what}: the paste changed something");
}

#[test]
fn an_unused_over_limit_story_is_refused_in_every_destination() {
    // The story is bound by nothing, so a paste into a nonempty body or a
    // story would skip or discard it; it still counts.
    let f = base();
    let over = with_envelope(
        f.clone(),
        json!({"v": 1, "mode": "complete", "stories": [
            {"label": "s0", "source": "1@8", "blocks": story_blocks(&f, BIG)}
        ]}),
    );
    let (mut e, body, held) = destination();
    refused_unchanged(&mut e, &over, None, "append to a nonempty body");
    refused_unchanged(&mut e, &over, Some((body, 0)), "into a body paragraph");
    refused_unchanged(&mut e, &over, Some((held, 0)), "into a story");
}

#[test]
fn an_over_limit_body_with_an_envelope_is_a_copy_limit_in_every_destination() {
    let mut f = base();
    let b = f.blocks[0].clone();
    f.blocks = (0..BIG)
        .map(|i| {
            let mut c = b.clone();
            c.id = NodeId::parse(&format!("{}@99", i + 1)).unwrap();
            c
        })
        .collect();
    let with = with_envelope(f.clone(), json!({"v": 1, "mode": "complete"}));
    let (mut e, body, held) = destination();
    refused_unchanged(&mut e, &with, None, "append");
    refused_unchanged(&mut e, &with, Some((body, 0)), "into the body");
    refused_unchanged(&mut e, &with, Some((held, 0)), "into a story");
    // Without an envelope it stays the fragment's own limit.
    let (mut e, _, _) = destination();
    let before = history(&e);
    let err = e.paste(&f, None, "dest").unwrap_err();
    assert!(
        matches!(err.reason, Reason::Fragment(_)),
        "{:?}",
        err.reason
    );
    assert!(history(&e) == before);
}

#[test]
fn a_story_the_paste_would_reuse_in_place_still_counts() {
    // The destination is the document the copy came from, and the story is
    // live: the paste would refer to it and copy nothing. The envelope that
    // carries it is over the limit all the same.
    let d = Document::new(1).unwrap();
    let body = d.append_block(BlockKind::Paragraph, "", "body").unwrap();
    d.create_story(&[NewBlock::new(BlockKind::Paragraph, "", "orphan")])
        .unwrap();
    d.commit();
    let schemas = SchemaRegistry::builtin();
    let mut f = d.copy_document_fragment("src", &schemas).unwrap();
    let mut envelope: Value = serde_json::from_str(f.sections.as_deref().unwrap()).unwrap();
    assert_eq!(envelope["stories"].as_array().unwrap().len(), 1);
    envelope["stories"][0]["blocks"] = Value::Array(story_blocks(&f, BIG));
    f.sections = Some(envelope.to_string());
    let mut e = Editor::new(d, schemas);
    for text in ["a", "b"] {
        e.apply(&Transaction::from(vec![Command::InsertText {
            node: body,
            at: 0,
            text: text.into(),
        }]))
        .unwrap();
    }
    assert!(e.undo().unwrap());
    refused_unchanged(&mut e, &f, None, "same document, story reused in place");
    refused_unchanged(&mut e, &f, Some((body, 0)), "same document, at a caret");
}
