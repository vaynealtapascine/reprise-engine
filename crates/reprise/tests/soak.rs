//! Long-session soak: three peers make random edits (text, splits, block
//! inserts and deletes, undo) while the network partitions and heals and
//! delivers packets in shuffled order. Refused packets trigger the documented
//! resync. At the end every replica converges, one-keystroke deltas stay
//! small, a stable selection resolves identically everywhere, and every saved
//! package reopens to the same state.
//!
//! `REPRISE_SOAK_EDITS` sets the number of edits. The default, 600, keeps it
//! near 15 s in debug; 2,000 passed in about 45 s in release.
// Peers are addressed by index throughout: `known[from][to]` and the network.
#![allow(clippy::needless_range_loop)]
use reprise::*;

const DOC: &str = "00112233445566778899aabbccddeeff";

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

struct Peer {
    session: DocumentSession,
    /// Cached blocks: (id, text). Refreshed after every applied change.
    blocks: Vec<(String, String)>,
}

impl Peer {
    fn refresh(&mut self) {
        self.blocks = self
            .session
            .state()
            .unwrap()
            .data
            .blocks
            .into_iter()
            .map(|b| (b.id, b.text))
            .collect();
    }
}

fn boundary(text: &str, rng: &mut Rng) -> u32 {
    let b: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect();
    b[rng.below(b.len())] as u32
}

fn edit(peer: &mut Peer, rng: &mut Rng) {
    if peer.blocks.is_empty() {
        peer.session
            .apply(&Payload::new(Transaction {
                commands: vec![Command::InsertBlock {
                    parent: None,
                    index: 0,
                    block_kind: BlockKind::Paragraph,
                    text: "again".into(),
                    style: Style::default(),
                }],
            }))
            .unwrap();
        peer.refresh();
        return;
    }
    let (node, text) = peer.blocks[rng.below(peer.blocks.len())].clone();
    let command = match rng.below(20) {
        0 if peer.blocks.len() > 2 => Command::DeleteBlock { node },
        1 => Command::SplitBlock {
            node,
            at: boundary(&text, rng),
        },
        2 => Command::InsertBlock {
            parent: None,
            index: rng.below(peer.blocks.len() + 1) as u32,
            block_kind: BlockKind::Paragraph,
            text: "new block".into(),
            style: Style::default(),
        },
        3 => {
            let _ = peer.session.undo();
            peer.refresh();
            return;
        }
        4..=7 if !text.is_empty() => {
            let (a, b) = (boundary(&text, rng), boundary(&text, rng));
            if a == b {
                return;
            }
            Command::DeleteText {
                node,
                start: a.min(b),
                end: a.max(b),
            }
        }
        _ => Command::InsertText {
            node,
            at: boundary(&text, rng),
            text: [
                "a",
                "b ",
                "\u{5d0}",
                "e\u{301}",
                "\u{1f469}\u{200d}\u{1f467}",
            ][rng.below(5)]
            .into(),
        },
    };
    // Nested blocks can make a top-level index invalid; a refused edit is fine.
    let _ = peer.session.apply(&Payload::new(Transaction {
        commands: vec![command],
    }));
    peer.refresh();
}

struct Packet {
    to: usize,
    from: usize,
    payload: Payload<SyncPacket>,
}

fn export(from: &Peer, since: Vec<Clock>) -> Payload<SyncPacket> {
    from.session
        .sync_export(&Payload::new(SyncRequest { since: Some(since) }))
        .unwrap()
}

#[test]
fn three_peers_with_partitions_and_shuffled_delivery_converge() {
    let edits: usize = std::env::var("REPRISE_SOAK_EDITS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let mut rng = Rng(2026);
    let workspace = Workspace::new();
    let create = |peer: &str| {
        workspace
            .create(&Payload::new(Create {
                document_id: DOC.into(),
                peer_id: peer.into(),
            }))
            .unwrap()
    };
    let mut first = Peer {
        session: create("1"),
        blocks: Vec::new(),
    };
    for i in 0..5 {
        first
            .session
            .apply(&Payload::new(Transaction {
                commands: vec![Command::InsertBlock {
                    parent: None,
                    index: i,
                    block_kind: BlockKind::Paragraph,
                    text: format!("paragraph {i} of the soak"),
                    style: Style::default(),
                }],
            }))
            .unwrap();
    }
    first.refresh();
    let snapshot = first
        .session
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    let mut peers = vec![first];
    for id in ["2", "3"] {
        let mut p = Peer {
            session: create(id),
            blocks: Vec::new(),
        };
        p.session.sync_import(&snapshot).unwrap();
        p.refresh();
        peers.push(p);
    }
    // A selection anchored at the start, resolved everywhere at the end.
    let target = peers[0].blocks[2].0.clone();
    let stable = peers[0]
        .session
        .anchor_selection(&Payload::new(Selection {
            anchor: Caret {
                node: target.clone(),
                offset: 3,
                affinity: Affinity::Downstream,
            },
            focus: Caret {
                node: target,
                offset: 9,
                affinity: Affinity::Upstream,
            },
        }))
        .unwrap();

    // What each peer believes each other peer has.
    let mut known: Vec<Vec<Vec<Clock>>> =
        vec![vec![peers[0].session.sync_info().data.vector; 3]; 3];
    let mut network: Vec<Packet> = Vec::new();
    let mut partitioned = false;
    let mut largest_keystroke = 0;
    let mut resyncs = 0;
    for round in 0..edits {
        if round % 250 == 0 {
            partitioned = !partitioned;
        }
        let connected = |a: usize, b: usize| !(partitioned && (a == 2 || b == 2));
        let who = rng.below(3);
        edit(&mut peers[who], &mut rng);
        for to in 0..3 {
            if to == who || !connected(who, to) {
                continue;
            }
            let payload = export(&peers[who], known[who][to].clone());
            known[who][to] = payload.data.vector.clone();
            network.push(Packet {
                to,
                from: who,
                payload,
            });
        }
        // Deliver a few packets in random order; drop those across the partition.
        for _ in 0..rng.below(4) {
            if network.is_empty() {
                break;
            }
            let Packet { to, from, payload } = network.swap_remove(rng.below(network.len()));
            if !connected(from, to) {
                // Lost: the sender's belief is now wrong, so forget it.
                known[from][to] = Vec::new();
                continue;
            }
            match peers[to].session.sync_import(&payload) {
                Ok(_) => {}
                Err(e) if e.code() == "sync.missing" => {
                    resyncs += 1;
                    let since = peers[to].session.sync_info().data.vector;
                    let again = export(&peers[from], since);
                    network.push(Packet {
                        to,
                        from,
                        payload: again,
                    });
                }
                Err(e) => panic!("round {round}: {e}"),
            }
            peers[to].refresh();
        }
        // While everyone is caught up, a keystroke's delta is small.
        if round % 97 == 0 && !partitioned {
            let since = peers[1].session.sync_info().data.vector;
            let catch_up = export(&peers[0], since);
            peers[1].session.sync_import(&catch_up).unwrap();
            peers[1].refresh();
            let node = peers[0].blocks.first().map(|b| b.0.clone());
            if let Some(node) = node {
                peers[0]
                    .session
                    .apply(&Payload::new(Transaction {
                        commands: vec![Command::InsertText {
                            node,
                            at: 0,
                            text: "k".into(),
                        }],
                    }))
                    .unwrap();
                peers[0].refresh();
                let one = export(&peers[0], peers[1].session.sync_info().data.vector);
                largest_keystroke = largest_keystroke.max(one.data.content.bytes.len());
                peers[1].session.sync_import(&one).unwrap();
                peers[1].refresh();
            }
        }
    }
    assert!(resyncs > 0, "shuffled delivery should have needed a resync");
    assert!(
        largest_keystroke > 0 && largest_keystroke < 1024,
        "{largest_keystroke} bytes"
    );

    // Heal: everyone pulls from everyone until vectors agree.
    for _ in 0..3 {
        for to in 0..3 {
            for from in 0..3 {
                if to != from {
                    let since = peers[to].session.sync_info().data.vector;
                    let packet = export(&peers[from], since);
                    peers[to].session.sync_import(&packet).unwrap();
                }
            }
        }
    }
    for p in &mut peers {
        p.refresh();
    }
    let vector = peers[0].session.sync_info().data.vector;
    for p in &peers {
        assert_eq!(p.session.sync_info().data.vector, vector);
        assert_eq!(p.blocks, peers[0].blocks, "replicas converge");
    }
    let resolved: Vec<_> = peers
        .iter()
        .map(|p| p.session.resolve_selection(&stable).unwrap().data)
        .collect();
    assert!(resolved.windows(2).all(|w| w[0] == w[1]), "{resolved:?}");
    for (i, p) in peers.iter_mut().enumerate() {
        let saved = p.session.save().unwrap().data.bytes;
        let reopened = workspace
            .open(
                &Payload::new(Open {
                    peer_id: format!("{}", 10 + i),
                }),
                &saved,
            )
            .unwrap();
        let blocks: Vec<_> = reopened
            .state()
            .unwrap()
            .data
            .blocks
            .into_iter()
            .map(|b| (b.id, b.text))
            .collect();
        assert_eq!(blocks, p.blocks, "peer {i} reopens equal");
        assert_eq!(
            reopened.resolve_selection(&stable).unwrap().data,
            resolved[0]
        );
    }
}
