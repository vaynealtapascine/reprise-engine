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

#[test]
fn moving_a_flow_paragraph_reports_its_replacement_to_hosts() {
    let mut s = create("18446744073709551614");
    let head = paragraph(&mut s, "abcd");
    let tail = tx(&mut s, vec![Command::SplitBlock { node: head, at: 2 }]).blocks[0].clone();
    let moved = tx(
        &mut s,
        vec![Command::MoveBlock {
            node: tail.clone(),
            parent: None,
            index: 0,
        }],
    );
    assert_eq!(
        moved.effects,
        vec![Effect::Moved {
            node: tail,
            new: moved.blocks[0].clone()
        }]
    );
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
fn remote_split_and_join_carets_follow_character_lineage() {
    let mut a = create("1");
    let p = paragraph(&mut a, "abcdef");
    let mut b = join(&a, "2");
    a.set_selection(&Payload::new(Some(Selection {
        anchor: caret(&p, 5),
        focus: caret(&p, 6),
    })))
    .unwrap();
    let applied = tx(
        &mut b,
        vec![Command::SplitBlock {
            node: p.clone(),
            at: 3,
        }],
    );
    let new = applied.blocks[0].clone();
    let selection = sync(&b, &mut a).selection.unwrap();
    assert_eq!(selection.focus.node, new);
    assert_eq!((selection.anchor.offset, selection.focus.offset), (2, 3));
    type_at(&mut b, &new, 0, ">");
    let selection = sync(&b, &mut a).selection.unwrap();
    assert_eq!(selection.focus.offset, 4);
    tx(
        &mut b,
        vec![Command::JoinBlocks {
            first: p.clone(),
            second: new.clone(),
        }],
    );
    let selection = sync(&b, &mut a).selection.unwrap();
    assert_eq!(selection.focus.node, p);
    assert_eq!((selection.anchor.offset, selection.focus.offset), (6, 7));
    assert_eq!(blocks(&a), blocks(&b));
    b.undo_report().unwrap();
    let selection = sync(&b, &mut a).selection.unwrap();
    assert_eq!(selection.focus.node, new);
    assert_eq!((selection.anchor.offset, selection.focus.offset), (3, 4));
    assert_eq!(blocks(&a), blocks(&b));
    b.redo_report().unwrap();
    let selection = sync(&b, &mut a).selection.unwrap();
    assert_eq!(selection.focus.node, p);
    assert_eq!((selection.anchor.offset, selection.focus.offset), (6, 7));
    assert_eq!(blocks(&a), blocks(&b));
    let saved = a.save().unwrap().data.bytes;
    let reopened = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "3".into(),
            }),
            &saved,
        )
        .unwrap();
    assert_eq!(blocks(&a), blocks(&reopened));
}

#[test]
fn split_undo_redo_keeps_original_and_fresh_carets_on_the_suffix() {
    let mut a = create("1");
    let p = paragraph(&mut a, "abcdef");
    let mut b = join(&a, "2");
    let selection = Selection {
        anchor: caret(&p, 5),
        focus: caret(&p, 6),
    };
    let original = a
        .anchor_selection(&Payload::new(selection.clone()))
        .unwrap();
    a.set_selection(&Payload::new(Some(selection.clone())))
        .unwrap();
    let new = tx(
        &mut b,
        vec![Command::SplitBlock {
            node: p.clone(),
            at: 3,
        }],
    )
    .blocks[0]
        .clone();
    sync(&b, &mut a);
    b.undo_report().unwrap();
    let current = sync(&b, &mut a).selection.unwrap();
    assert_eq!(current, selection);
    assert_eq!(
        a.resolve_selection(&original).unwrap().data.unwrap(),
        selection
    );
    a.set_selection(&Payload::new(Some(selection.clone())))
        .unwrap();
    let fresh = a
        .anchor_selection(&Payload::new(selection.clone()))
        .unwrap();
    type_at(&mut a, &p, 4, "X");
    let inserted = Selection {
        anchor: caret(&p, 6),
        focus: caret(&p, 7),
    };
    assert_eq!(a.local_selection().data.unwrap(), inserted);
    assert_eq!(
        a.resolve_selection(&original).unwrap().data.unwrap(),
        inserted
    );
    assert_eq!(a.resolve_selection(&fresh).unwrap().data.unwrap(), inserted);
    tx(
        &mut a,
        vec![Command::DeleteText {
            node: p.clone(),
            start: 3,
            end: 4,
        }],
    );
    assert_eq!(a.local_selection().data.unwrap(), selection);
    assert_eq!(
        a.resolve_selection(&original).unwrap().data.unwrap(),
        selection
    );
    assert_eq!(
        a.resolve_selection(&fresh).unwrap().data.unwrap(),
        selection
    );
    sync(&a, &mut b);
    b.redo_report().unwrap();
    let current = sync(&b, &mut a).selection.unwrap();
    let expected = Selection {
        anchor: caret(&new, 2),
        focus: caret(&new, 3),
    };
    assert_eq!(current, expected);
    assert_eq!(
        a.resolve_selection(&original).unwrap().data.unwrap(),
        expected
    );
    assert_eq!(a.resolve_selection(&fresh).unwrap().data.unwrap(), expected);
}

#[test]
fn a_caret_traverses_three_splits_and_a_deleted_intermediate() {
    let mut a = create("1");
    let p = paragraph(&mut a, "abcdefgh");
    let mut b = join(&a, "2");
    a.set_selection(&Payload::new(Some(Selection {
        anchor: caret(&p, 6),
        focus: caret(&p, 8),
    })))
    .unwrap();
    let q = tx(
        &mut b,
        vec![Command::SplitBlock {
            node: p.clone(),
            at: 2,
        }],
    )
    .blocks[0]
        .clone();
    let r = tx(
        &mut b,
        vec![Command::SplitBlock {
            node: q.clone(),
            at: 2,
        }],
    )
    .blocks[0]
        .clone();
    let s = tx(
        &mut b,
        vec![Command::SplitBlock {
            node: r.clone(),
            at: 1,
        }],
    )
    .blocks[0]
        .clone();
    tx(
        &mut b,
        vec![Command::JoinBlocks {
            first: p,
            second: q,
        }],
    );
    let selection = sync(&b, &mut a).selection.unwrap();
    assert_eq!(
        selection,
        Selection {
            anchor: caret(&s, 1),
            focus: caret(&s, 3)
        }
    );
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
    bad.data.anchor.anchor = "00".into();
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
    // A syntactically valid envelope carrying garbage cursor bytes must not
    // paint a fabricated caret at the first block's start.
    let mut corrupt = presence.clone();
    let mut wire: serde_json::Value = serde_json::from_slice(&corrupt.data.content.bytes).unwrap();
    wire["selection"]["anchor"]["anchor"] = serde_json::json!("00");
    corrupt.data.content.bytes = serde_json::to_vec(&wire).unwrap();
    assert!(
        b.resolve_presence(&corrupt)
            .unwrap()
            .data
            .selection
            .is_none()
    );
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

/// Shuffled delivery, offline divergence, replay, undo and reopen through the
/// same boundary the UI uses. Raise REPRISE_COLLAB_STEPS for longer local runs.
#[test]
fn three_peer_partition_rejoin_soak() {
    let steps = std::env::var("REPRISE_COLLAB_STEPS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(300);
    let mut a = create("1");
    let ids: Vec<_> = (0..3).map(|_| paragraph(&mut a, "seed text")).collect();
    let b = join(&a, "2");
    let c = join(&a, "3");
    let mut peers = [a, b, c];
    let mut rng = 0x7b14_02a9_341e_004du64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng as usize
    };
    let mut packets = Vec::new();
    // A deliberately dependent packet arrives before its predecessor.
    let base = peers[1].sync_info().data.vector;
    type_at(&mut peers[0], &ids[0], 0, "a");
    let predecessor = peers[0]
        .sync_export(&Payload::new(SyncRequest { since: Some(base) }))
        .unwrap();
    let since = peers[0].sync_info().data.vector;
    type_at(&mut peers[0], &ids[0], 0, "b");
    let dependent = peers[0]
        .sync_export(&Payload::new(SyncRequest { since: Some(since) }))
        .unwrap();
    assert_eq!(
        peers[1].sync_import(&dependent).unwrap_err().code(),
        "sync.missing"
    );
    peers[1].sync_import(&predecessor).unwrap();
    peers[1].sync_import(&dependent).unwrap();
    let mut missing = 1;
    let mut replayed = 0;
    for step in 0..steps {
        let i = next() % 3;
        let n = next() % ids.len();
        let text = peers[i].state().unwrap().data.blocks[n].text.clone();
        if step % 31 == 0 {
            peers[i].undo_report().unwrap();
        } else if step % 37 == 0 {
            peers[i].redo_report().unwrap();
        } else if text.len() > 80 && next() % 2 == 0 {
            let start = next() % text.len();
            tx(
                &mut peers[i],
                vec![Command::DeleteText {
                    node: ids[n].clone(),
                    start: start as u32,
                    end: (start + 1) as u32,
                }],
            );
        } else {
            let at = next() % (text.len() + 1);
            type_at(&mut peers[i], &ids[n], at as u32, "x");
        }
        let selected = Selection {
            anchor: caret(&ids[n], 0),
            focus: caret(&ids[n], 0),
        };
        peers[i]
            .set_selection(&Payload::new(Some(selected)))
            .unwrap();
        let j = (i + 1 + next() % 2) % 3;
        packets.push((j, delta(&peers[i], &peers[j])));
        // Partitions last 17 steps; packets leave their queue in random order.
        if step % 17 == 0 {
            while !packets.is_empty() {
                let at = next() % packets.len();
                let (to, packet) = packets.swap_remove(at);
                let before = peers[to].sync_info();
                match peers[to].sync_import(&packet) {
                    Ok(_) => {
                        let replay = peers[to].sync_import(&packet).unwrap().data;
                        assert!(
                            !replay.changed
                                && replay.changes.blocks.is_empty()
                                && !replay.changes.structure
                                && !replay.changes.styles
                                && replay.changes.relations.is_empty()
                                && replay.changes.ranges.is_empty()
                                && !replay.changes.other
                        );
                        replayed += 1;
                    }
                    Err(e) if e.code() == "sync.missing" => {
                        missing += 1;
                        assert_eq!(before, peers[to].sync_info());
                    }
                    Err(e) => panic!("step {step}: {e}"),
                }
            }
        }
        if step % 101 == 0 {
            for from in 0..3 {
                for to in 0..3 {
                    if from != to {
                        let packet = delta(&peers[from], &peers[to]);
                        peers[to].sync_import(&packet).unwrap();
                    }
                }
            }
        }
    }
    // Anti-entropy with current vectors recovers refused packets and partitions.
    for _ in 0..2 {
        for from in 0..3 {
            for to in 0..3 {
                if from != to {
                    let packet = delta(&peers[from], &peers[to]);
                    peers[to].sync_import(&packet).unwrap();
                }
            }
        }
    }
    assert!(missing > 0 && replayed > 0);
    assert_eq!(blocks(&peers[0]), blocks(&peers[1]));
    assert_eq!(blocks(&peers[0]), blocks(&peers[2]));
    assert_eq!(
        peers[0].sync_info().data.vector,
        peers[2].sync_info().data.vector
    );
    for i in 0..3 {
        let local = peers[i].local_selection().data.unwrap();
        let stable = peers[i]
            .anchor_selection(&Payload::new(local.clone()))
            .unwrap();
        for peer in &peers {
            assert_eq!(
                peer.resolve_selection(&stable).unwrap().data,
                Some(local.clone())
            );
        }
    }
    for peer in &mut peers {
        layout(peer);
    }
    assert_eq!(
        peers[0].display_json(0).unwrap(),
        peers[2].display_json(0).unwrap()
    );
    let saved = peers[0].save().unwrap();
    let mut reopened = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "4".into(),
            }),
            &saved.data.bytes,
        )
        .unwrap();
    assert_eq!(blocks(&peers[0]), blocks(&reopened));
    layout(&mut reopened);
    assert_eq!(
        peers[0].display_json(0).unwrap().data,
        reopened.display_json(0).unwrap().data
    );
    let vector = peers[1].sync_info().data.vector;
    type_at(&mut peers[1], &ids[0], 0, "z");
    let keystroke = peers[1]
        .sync_export(&Payload::new(SyncRequest {
            since: Some(vector),
        }))
        .unwrap();
    assert!(keystroke.data.content.bytes.len() < 4096);
}

/// Native/WASM parity: ts/smoke.cjs runs the same script in WASM and compares
/// the packet bytes with this golden. Re-record with REPRISE_RECORD_SYNC=1.
#[test]
fn delta_packet_bytes_are_pinned() {
    let mut s = create("1");
    let p = paragraph(&mut s, "parity");
    type_at(&mut s, &p, 6, " \u{5d0}\u{5d1}");
    let packet = s
        .sync_export(&Payload::new(SyncRequest {
            since: Some(Vec::new()),
        }))
        .unwrap()
        .data;
    let hex: String = packet
        .content
        .bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sync_delta.hex");
    if std::env::var_os("REPRISE_RECORD_SYNC").is_some() {
        std::fs::write(&path, &hex).unwrap();
    }
    assert_eq!(hex, include_str!("sync_delta.hex").trim());
    // The same packet joins a fresh replica.
    let mut fresh = create("2");
    fresh.sync_import(&Payload::new(packet)).unwrap();
    assert_eq!(blocks(&fresh), blocks(&s));
}

#[test]
fn a_contained_panic_poisons_the_session() {
    let mut s = create("1");
    paragraph(&mut s, "kept");
    let poisoned: Result<()> = s.contain(|_| panic!("simulated engine fault"));
    let error = poisoned.unwrap_err();
    assert_eq!(error.code(), "bindings.poisoned");
    assert!(error.to_string().contains("simulated engine fault"));
    assert!(s.is_poisoned());
    // Every later contained call is refused; the host must reopen.
    let again = s.contain(|s| s.state());
    assert_eq!(again.unwrap_err().code(), "bindings.poisoned");
    // Without a panic, contain is transparent.
    let mut fresh = create("2");
    assert_eq!(fresh.contain(|s| s.state()).unwrap().data.blocks.len(), 0);
    assert_eq!(
        reprise::contain(|| -> Result<()> { panic!("no session") })
            .unwrap_err()
            .code(),
        "bindings.poisoned"
    );
}
