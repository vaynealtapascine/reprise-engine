//! Collaboration through the facade only: delta sync, refusals, the anchored
//! local selection across sync and undo, presence, and layout memo reuse.
use std::collections::BTreeMap;

use reprise::*;

const DOC: &str = "00112233445566778899aabbccddeeff";

fn create(peer: &str) -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: peer.into(),
        }))
        .unwrap()
}

fn tx(s: &mut DocumentSession, commands: Vec<Command>) -> Applied {
    s.apply(&Payload::new(Transaction { commands }))
        .unwrap()
        .data
}

fn paragraph(s: &mut DocumentSession, text: &str) -> String {
    let index = s.state().unwrap().data.blocks.len() as u32;
    tx(
        s,
        vec![Command::InsertBlock {
            parent: None,
            index,
            block_kind: BlockKind::Paragraph,
            text: text.into(),
            style: Style::default(),
        }],
    )
    .blocks[0]
        .clone()
}

fn type_at(s: &mut DocumentSession, node: &str, at: u32, text: &str) {
    tx(
        s,
        vec![Command::InsertText {
            node: node.into(),
            at,
            text: text.into(),
        }],
    );
}

fn delta(from: &DocumentSession, to: &DocumentSession) -> Payload<SyncPacket> {
    from.sync_export(&Payload::new(SyncRequest {
        since: Some(to.sync_info().data.vector),
    }))
    .unwrap()
}

fn sync(from: &DocumentSession, to: &mut DocumentSession) -> SyncReport {
    to.sync_import(&delta(from, to)).unwrap().data
}

fn blocks(s: &DocumentSession) -> Vec<(String, String)> {
    s.state()
        .unwrap()
        .data
        .blocks
        .into_iter()
        .map(|b| (b.id, b.text))
        .collect()
}

fn layout(s: &mut DocumentSession) -> LayoutProgress {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    for _ in 0..100_000 {
        let progress = job.step(s, 64).unwrap().data;
        if progress.complete {
            return progress;
        }
    }
    panic!("explicit test job bound exceeded")
}

/// A second replica joining through a snapshot packet.
fn join(a: &DocumentSession, peer: &str) -> DocumentSession {
    let mut b = create(peer);
    let snapshot = a
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    assert_eq!(snapshot.data.kind, SyncKind::Snapshot);
    b.sync_import(&snapshot).unwrap();
    b
}

fn caret(node: &str, offset: u32) -> Caret {
    Caret {
        node: node.into(),
        offset,
        affinity: Affinity::Downstream,
    }
}

#[test]
fn deltas_converge_and_report_changes() {
    let mut a = create("1");
    let p = paragraph(&mut a, "hello");
    let mut b = join(&a, "2");
    assert_eq!(blocks(&a), blocks(&b));
    type_at(&mut b, &p, 5, " world");
    let packet = delta(&b, &a);
    assert_eq!(packet.data.kind, SyncKind::Delta);
    assert_eq!(packet.data.format, 2);
    assert_eq!(packet.data.features, vec!["delta-json".to_owned()]);
    assert!(packet.data.content.bytes.len() < 1024);
    let report = a.sync_import(&packet).unwrap().data;
    assert!(report.changed);
    assert_eq!(report.changes.blocks, vec![p.clone()]);
    assert_eq!(blocks(&a), blocks(&b));
    // A replay changes nothing.
    let again = a.sync_import(&packet).unwrap().data;
    assert!(!again.changed && again.changes == Changes::default());
}

#[test]
fn refused_packets_have_typed_codes_and_change_nothing() {
    let mut a = create("1");
    let p = paragraph(&mut a, "hello");
    let mut b = join(&a, "2");
    let v0 = a.sync_info().data.vector;
    type_at(&mut b, &p, 0, "1");
    let first = delta(&b, &a);
    type_at(&mut b, &p, 0, "2");
    let second = b
        .sync_export(&Payload::new(SyncRequest {
            since: Some(first.data.vector.clone()),
        }))
        .unwrap();
    let before = blocks(&a);
    let missing = a.sync_import(&second).unwrap_err();
    assert_eq!(missing.code(), "sync.missing");
    assert_eq!(a.sync_info().data.vector, v0);
    assert_eq!(blocks(&a), before);
    // Recover: ask for everything since our vector.
    sync(&b, &mut a);
    assert_eq!(blocks(&a), blocks(&b));

    let mut newer = first.clone();
    newer.data.content.bytes[4] = 9;
    assert_eq!(a.sync_import(&newer).unwrap_err().code(), "sync.format");
    let mut lying = first.clone();
    lying.data.format = 3;
    assert_eq!(
        a.sync_import(&lying).unwrap_err().code(),
        "bindings.invalid"
    );
    let mut feature = first.clone();
    feature.data.features = vec!["telepathy".into()];
    assert_eq!(
        a.sync_import(&feature).unwrap_err().code(),
        "bindings.invalid"
    );
    let mut scope = first.clone();
    scope.data.document_id = "ffffffffffffffffffffffffffffffff".into();
    assert_eq!(a.sync_import(&scope).unwrap_err().code(), "bindings.id");
    let mut own = first.clone();
    own.data.from_peer = "1".into();
    assert_eq!(a.sync_import(&own).unwrap_err().code(), "bindings.invalid");
    let mut truncated = first.clone();
    truncated.data.content.bytes.truncate(30);
    assert!(
        a.sync_import(&truncated)
            .unwrap_err()
            .code()
            .starts_with("sync.")
    );
    let mut unknown_bit = first;
    unknown_bit.data.content.bytes[13] = 0x80;
    assert_eq!(
        a.sync_import(&unknown_bit).unwrap_err().code(),
        "sync.feature"
    );
    assert_eq!(blocks(&a), blocks(&b));
}

#[test]
fn local_selection_survives_sync_undo_and_redo() {
    let mut a = create("1");
    let p = paragraph(&mut a, "hello world");
    let mut b = join(&a, "2");
    a.set_selection(&Payload::new(Some(Selection {
        anchor: caret(&p, 6),
        focus: caret(&p, 11),
    })))
    .unwrap();
    type_at(&mut b, &p, 0, ">> ");
    let report = sync(&b, &mut a);
    let selection = report.selection.unwrap();
    assert_eq!((selection.anchor.offset, selection.focus.offset), (9, 14));
    assert!(!report.selection_moved);
    // A local edit before the selection, then undo and redo.
    type_at(&mut a, &p, 0, "abc");
    assert_eq!(a.local_selection().data.unwrap().anchor.offset, 12);
    let undone = a.undo_report().unwrap().data;
    assert!(undone.changed && undone.changes.blocks.contains(&p));
    assert_eq!(undone.selection.unwrap().anchor.offset, 9);
    let redone = a.redo_report().unwrap().data;
    assert_eq!(redone.selection.unwrap().anchor.offset, 12);
    // The remote peer deletes the block: the selection falls back and moves.
    let q = paragraph(&mut b, "after");
    sync(&b, &mut a);
    sync(&a, &mut b);
    tx(&mut b, vec![Command::DeleteBlock { node: p.clone() }]);
    let report = sync(&b, &mut a);
    assert!(report.selection_moved);
    let fallen = report.selection.unwrap();
    assert_eq!(fallen.anchor.node, q);
    assert_eq!(fallen.anchor.offset, 0);
    // Explicit anchors round-trip, and malformed ones are refused.
    let stable = a
        .anchor_selection(&Payload::new(Selection {
            anchor: caret(&q, 1),
            focus: caret(&q, 3),
        }))
        .unwrap();
    let back = a.resolve_selection(&stable).unwrap().data.unwrap();
    assert_eq!((back.anchor.offset, back.focus.offset), (1, 3));
    let mut bad = stable.clone();
    bad.data.anchor.anchor = "zz".into();
    assert_eq!(
        a.resolve_selection(&bad).unwrap_err().code(),
        "bindings.invalid"
    );
}

#[test]
fn presence_resolves_to_geometry_and_garbage_degrades() {
    let mut a = create("1");
    let p = paragraph(&mut a, "presence text");
    let mut b = join(&a, "2");
    let meta = BTreeMap::from([("name".to_owned(), "Ada".to_owned())]);
    let presence = a
        .presence(&Payload::new(Presence {
            selection: Some(Selection {
                anchor: caret(&p, 0),
                focus: caret(&p, 8),
            }),
            meta: meta.clone(),
        }))
        .unwrap();
    // Without a layout: selection but no geometry.
    let view = b.resolve_presence(&presence).unwrap().data;
    assert_eq!(view.meta, meta);
    assert!(view.selection.is_some() && view.caret.is_none());
    layout(&mut b);
    let view = b.resolve_presence(&presence).unwrap().data;
    assert!(view.caret.is_some());
    assert!(!view.rects.is_empty());
    // Garbage, foreign and oversized presence give empty views.
    for bytes in [
        b"not json".to_vec(),
        b"{\"presence\":2}".to_vec(),
        vec![0xff; 100],
    ] {
        let garbage = Payload::new(Awareness {
            document_id: DOC.into(),
            peer_id: "1".into(),
            content: Bytes { bytes },
        });
        let view = b.resolve_presence(&garbage).unwrap().data;
        assert!(view.selection.is_none() && view.meta.is_empty());
    }
    let mut foreign = presence.clone();
    foreign.data.document_id = "ffffffffffffffffffffffffffffffff".into();
    assert!(
        b.resolve_presence(&foreign)
            .unwrap()
            .data
            .selection
            .is_none()
    );
    let mut wrong_version = presence.clone();
    wrong_version.version = 7;
    assert_eq!(
        b.resolve_presence(&wrong_version).unwrap_err().code(),
        "bindings.version"
    );
    let too_much = (0..17).map(|i| (i.to_string(), String::new())).collect();
    let refused = a.presence(&Payload::new(Presence {
        selection: None,
        meta: too_much,
    }));
    assert_eq!(refused.unwrap_err().code(), "bindings.limit");
    // Stale presence: the block was deleted since.
    tx(&mut a, vec![Command::DeleteBlock { node: p }]);
    sync(&a, &mut b);
    let view = b.resolve_presence(&presence).unwrap().data;
    assert!(view.selection.is_none() && view.caret.is_none());
}

#[test]
fn one_remote_keystroke_relays_out_a_constant_number_of_paragraphs() {
    let mut a = create("1");
    let ids: Vec<String> = (0..60)
        .map(|i| {
            paragraph(
                &mut a,
                &format!("paragraph {i} with enough words to wrap a line or two"),
            )
        })
        .collect();
    let mut b = join(&a, "2");
    let cold = layout(&mut b).counters;
    assert!(cold.compositions >= 60, "{cold:?}");
    type_at(&mut a, &ids[30], 3, "x");
    let report = sync(&a, &mut b);
    assert_eq!(report.changes.blocks, vec![ids[30].clone()]);
    let warm = layout(&mut b).counters;
    assert!(
        warm.shapes <= 2,
        "one remote keystroke reshaped {} paragraphs",
        warm.shapes
    );
    assert!(warm.compositions <= 2, "{warm:?}");
    assert!(warm.reused_compositions >= 58, "{warm:?}");
}
