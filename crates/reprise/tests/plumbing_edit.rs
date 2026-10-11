//! Atomic selection replacement and the batch, through the facade.
use reprise::*;

const DOC: &str = "00112233445566778899aabbccddeeff";

fn session(peer: &str) -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: peer.into(),
        }))
        .unwrap()
}
fn finish(s: &mut DocumentSession) {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    while !job.step(s, 1000).unwrap().data.complete {}
}
fn tx(s: &mut DocumentSession, commands: Vec<Command>) -> Payload<Applied> {
    s.apply(&Payload::new(Transaction { commands })).unwrap()
}
fn block(text: &str, index: u32) -> Command {
    Command::InsertBlock {
        parent: None,
        index,
        block_kind: BlockKind::Paragraph,
        text: text.into(),
        style: Style::default(),
    }
}
fn texts(s: &DocumentSession) -> Vec<String> {
    s.state().unwrap().data.blocks.into_iter().map(|b| b.text).collect()
}
fn caret(node: &str, offset: u32) -> Caret {
    Caret {
        node: node.into(),
        offset,
        affinity: Affinity::Downstream,
    }
}
fn selection(node: &str, a: u32, b: u32) -> Selection {
    Selection {
        anchor: caret(node, a),
        focus: caret(node, b),
    }
}
fn sync(from: &DocumentSession, to: &mut DocumentSession) {
    let packet = from
        .sync_export(&Payload::new(SyncRequest {
            since: Some(to.sync_info().data.vector),
        }))
        .unwrap();
    to.sync_import(&packet).unwrap();
}
fn revision(s: &DocumentSession) -> Vec<Clock> {
    s.state().unwrap().data.revision
}

#[test]
fn a_batch_with_a_failing_third_command_applies_nothing() {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("hello", 0)]).data.blocks[0].clone();
    let before = (s.state().unwrap().data, s.sync_info().data);
    let batch = Batch {
        commands: vec![
            Command::InsertText {
                node: node.clone(),
                at: 5,
                text: "!".into(),
            },
            block("second", 1),
            // Past the end of the block, so the third command is refused.
            Command::DeleteText {
                node: node.clone(),
                start: 50,
                end: 60,
            },
        ],
        expected_revision: None,
        selection: None,
    };
    let error = s.batch(&Payload::new(batch)).unwrap_err();
    let payload = error.payload().data;
    assert_eq!(payload.command, Some(2), "the error names the third command");
    assert_eq!(s.state().unwrap().data, before.0);
    assert_eq!(s.sync_info().data, before.1, "not even a tombstone was written");
}

#[test]
fn a_batch_is_one_undo_step_and_maps_the_selection() {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("hello world", 0)]).data.blocks[0].clone();
    let report = s
        .batch(&Payload::new(Batch {
            commands: vec![
                Command::InsertText {
                    node: node.clone(),
                    at: 0,
                    text: ">> ".into(),
                },
                Command::FormatText {
                    node: node.clone(),
                    start: 3,
                    end: 8,
                    style: TextStyle {
                        weight: Some(700),
                        ..TextStyle::default()
                    },
                },
                block("tail", 1),
            ],
            expected_revision: Some(revision(&s)),
            selection: Some(selection(&node, 6, 11)),
        }))
        .unwrap()
        .data;
    assert_eq!(texts(&s), [">> hello world", "tail"]);
    let mapped = report.selection.unwrap();
    assert_eq!((mapped.anchor.offset, mapped.focus.offset), (9, 14));
    assert!(s.undo().unwrap().data);
    assert_eq!(texts(&s), ["hello world"], "one undo undoes the whole batch");
    assert!(s.undo().unwrap().data, "the setup block is the next step");
    assert!(texts(&s).is_empty());
}

#[test]
fn a_batch_checks_its_revision_before_writing() {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("x", 0)]).data.blocks[0].clone();
    let stale = revision(&s);
    tx(
        &mut s,
        vec![Command::InsertText {
            node: node.clone(),
            at: 1,
            text: "y".into(),
        }],
    );
    let state = s.state().unwrap().data;
    let error = s
        .batch(&Payload::new(Batch {
            commands: vec![Command::InsertText {
                node,
                at: 0,
                text: "!".into(),
            }],
            expected_revision: Some(stale),
            selection: None,
        }))
        .unwrap_err();
    assert_eq!(error.code(), "bindings.stale");
    assert_eq!(s.state().unwrap().data, state);
    assert_eq!(
        s.batch(&Payload::new(Batch {
            commands: vec![],
            expected_revision: None,
            selection: None,
        }))
        .unwrap_err()
        .code(),
        "bindings.invalid"
    );
}

#[test]
fn replacing_text_is_one_undo_step() {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("hello world", 0)]).data.blocks[0].clone();
    let stable = s
        .anchor_selection(&Payload::new(selection(&node, 6, 11)))
        .unwrap()
        .data;
    let done = s
        .replace_text(&Payload::new(ReplaceText {
            selection: stable,
            text: "there".into(),
            html: false,
        }))
        .unwrap()
        .data;
    assert_eq!(texts(&s), ["hello there"]);
    assert_eq!(done.selection.unwrap().focus.offset, 11);
    assert!(s.undo().unwrap().data);
    assert_eq!(texts(&s), ["hello world"]);
    // The next undo is the setup block, not a leftover half of the replacement.
    assert!(s.undo().unwrap().data);
    assert!(texts(&s).is_empty());
}

#[test]
fn a_remote_edit_before_the_replacement_is_part_of_it() {
    let mut a = session("1");
    let node = tx(&mut a, vec![block("abcdefgh", 0)]).data.blocks[0].clone();
    let saved = a.save().unwrap().data.bytes;
    let mut b = Workspace::new()
        .open(&Payload::new(Open { peer_id: "2".into() }), &saved)
        .unwrap();
    let stable = a
        .anchor_selection(&Payload::new(selection(&node, 2, 6)))
        .unwrap()
        .data;
    tx(
        &mut b,
        vec![Command::InsertText {
            node: node.clone(),
            at: 4,
            text: "<remote>".into(),
        }],
    );
    sync(&b, &mut a);
    a.replace_text(&Payload::new(ReplaceText {
        selection: stable,
        text: "-".into(),
        html: false,
    }))
    .unwrap();
    assert_eq!(texts(&a), ["ab-gh"]);
    assert!(a.undo().unwrap().data);
    assert_eq!(texts(&a), ["abcd<remote>efgh"]);
}

#[test]
fn native_fragment_replacement_and_refusals_leave_the_selection() {
    let mut source = session("3");
    let n = tx(&mut source, vec![block("PASTED", 0)]).data.blocks[0].clone();
    finish(&mut source);
    let fragment = source
        .copy(&Payload::new(selection(&n, 0, 6)))
        .unwrap()
        .data
        .bytes;
    let mut s = session("1");
    let node = tx(&mut s, vec![block("hello world", 0)]).data.blocks[0].clone();
    let stable = s
        .anchor_selection(&Payload::new(selection(&node, 0, 5)))
        .unwrap()
        .data;
    // Garbage clipboard bytes erase nothing.
    let before = s.state().unwrap().data;
    let error = s
        .replace_selection(
            &Payload::new(ReplaceSelection {
                selection: stable.clone(),
            }),
            b"not a fragment",
        )
        .unwrap_err();
    assert!(error.code().starts_with("clipboard."), "{}", error.code());
    assert_eq!(s.state().unwrap().data, before);
    // A selection that is not an anchor is refused too.
    let mut bad = stable.clone();
    bad.anchor.anchor = "zz".into();
    assert_eq!(
        s.replace_selection(&Payload::new(ReplaceSelection { selection: bad }), &fragment)
            .unwrap_err()
            .code(),
        "bindings.invalid"
    );
    assert_eq!(s.state().unwrap().data, before);
    // The real fragment replaces in one step.
    s.replace_selection(&Payload::new(ReplaceSelection { selection: stable }), &fragment)
        .unwrap();
    assert_eq!(texts(&s), ["PASTED world"]);
    assert!(s.undo().unwrap().data);
    assert_eq!(texts(&s), ["hello world"]);
}

#[test]
fn replacing_keeps_the_hosts_anchored_selection_at_the_caret() {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("hello world", 0)]).data.blocks[0].clone();
    s.set_selection(&Payload::new(Some(selection(&node, 6, 11))))
        .unwrap();
    let stable = s
        .anchor_selection(&Payload::new(selection(&node, 6, 11)))
        .unwrap()
        .data;
    s.replace_text(&Payload::new(ReplaceText {
        selection: stable,
        text: "ok".into(),
        html: false,
    }))
    .unwrap();
    let local = s.local_selection().data.unwrap();
    assert_eq!((local.anchor.offset, local.focus.offset), (8, 8));
}
