//! Orchestrator review: the facade's byte-accepting entry points get bytes
//! straight from the host or the network. This takes real artefacts (a saved
//! package, a copied fragment, awareness data and a JSON payload), damages
//! each at many points, and feeds them to `open`, `paste`, `awareness`,
//! `declare_font`, `register_asset`, `load_plugin` and `decode`. Every call
//! must return, typed error or success, without a panic, and a failed call
//! must leave the session's state exactly as it was.

use reprise::*;

fn session() -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: "7".into(),
        }))
        .unwrap()
}

fn damage(bytes: &[u8], seed: u64) -> Vec<Vec<u8>> {
    let len = bytes.len().max(1);
    let mut out: Vec<Vec<u8>> = (0..16)
        .map(|i| bytes[..bytes.len() * i / 16].to_vec())
        .collect();
    let mut state = seed | 1;
    for _ in 0..24 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let mut d = bytes.to_vec();
        if !d.is_empty() {
            d[(state as usize) % len] ^= 0x5A;
        }
        out.push(d);
    }
    out
}

fn plugin_spec() -> PluginSpec {
    PluginSpec {
        name: "fuzz".into(),
        plugin_version: "1".into(),
        sha256: "0".repeat(64),
        phase: PluginPhase::Layout,
        imports: vec![],
        grants: vec![],
        functions: vec![],
        fuel: 100_000,
        memory_pages: 16,
        table_elements: 1024,
        buffer_bytes: 65_536,
    }
}

#[test]
fn damaged_bytes_at_every_entry_point_are_typed_and_atomic() {
    let mut source = session();
    let applied = source
        .apply(&Payload::new(Transaction {
            commands: vec![Command::InsertBlock {
                parent: None,
                index: 0,
                block_kind: BlockKind::Paragraph,
                text: "office \u{5d0}\u{5d1} e\u{301}".into(),
                style: Style::default(),
            }],
        }))
        .unwrap();
    let node = applied.data.blocks[0].clone();
    // Selections resolve against a layout.
    let mut job = source
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    while !job.step(&mut source, 64).unwrap().data.complete {}
    let package = source.save().unwrap().data.bytes;
    let caret = |offset| Caret {
        node: node.clone(),
        offset,
        affinity: Affinity::Downstream,
    };
    let fragment = source
        .copy(&Payload::new(Selection {
            anchor: caret(0),
            focus: caret(6),
        }))
        .unwrap()
        .data
        .bytes;
    let json = br#"{"version":1,"data":{"commands":[]}}"#.to_vec();

    let mut target = session();
    let before = target.state().unwrap();
    for (i, bytes) in damage(&package, 11).into_iter().enumerate() {
        let _ = Workspace::new().open(
            &Payload::new(Open {
                peer_id: "9".into(),
            }),
            &bytes,
        );
        let _ = i;
    }
    for bytes in damage(&fragment, 13) {
        if target
            .paste(&Payload::new(Paste { at: None }), &bytes)
            .is_err()
        {
            assert_eq!(target.state().unwrap(), before, "failed paste mutated");
        } else {
            target = session();
        }
    }
    for bytes in damage(&json, 17) {
        let _ = decode::<Transaction>(&bytes);
    }
    for bytes in damage(&package, 19) {
        let _ = target.awareness(&bytes);
        let _ = target.declare_font(
            &Payload::new(FontDeclaration {
                family: "Fuzz".into(),
                weight: 400,
                style: FontStyle::Normal,
                stretch: 100,
                face_index: 0,
            }),
            &bytes,
        );
        let _ = target.register_asset(
            &Payload::new(AssetDeclaration {
                id: "fuzz".into(),
                kind: AssetKind::Image,
            }),
            &bytes,
        );
        let _ = target.load_plugin(&Payload::new(plugin_spec()), &bytes);
    }
}
