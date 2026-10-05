use reprise::*;
fn session() -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: "18446744073709551614".into(),
        }))
        .unwrap()
}
fn finish(s: &mut DocumentSession) {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    for _ in 0..100 {
        if job.step(s, 8).unwrap().data.complete {
            return;
        }
    }
    panic!("explicit test bound exceeded")
}
#[test]
fn malformed_and_future_payloads_are_typed() {
    for bytes in [b"".as_slice(),b"{",br#"{"version":1,"data":{"commands":[{"kind":"insert-text","node":"bad","at":-1,"text":"x"}]}}"#,br#"{"version":1,"data":{"commands":[],"unexpected":true}}"#] {assert!(matches!(decode::<Transaction>(bytes),Err(Error::Invalid(_))));}
    assert!(matches!(
        decode::<Transaction>(br#"{"version":99,"data":{"unknown_future_command":true}}"#),
        Err(Error::Version(99))
    ));
    let deep = format!("{}0{}", "[".repeat(65), "]".repeat(65));
    assert!(matches!(
        decode::<Transaction>(deep.as_bytes()),
        Err(Error::Limit(_))
    ));
    assert!(matches!(
        decode::<Transaction>(&vec![b' '; MAX_PAYLOAD_BYTES + 1]),
        Err(Error::Limit(_))
    ));
}
#[test]
fn invalid_ids_utf8_transactions_and_oversized_bytes_do_not_mutate() {
    let mut s = session();
    let before = s.state().unwrap();
    for node in [
        "",
        "garbage",
        "-1@0",
        "9999999999999999999999999999999999999999999@1",
    ] {
        let result = s.apply(&Payload::new(Transaction {
            commands: vec![Command::InsertText {
                node: node.into(),
                at: u32::MAX,
                text: "x".into(),
            }],
        }));
        assert!(result.is_err());
        assert_eq!(s.state().unwrap(), before);
    }
    assert!(matches!(
        s.register_asset(
            &Payload::new(AssetDeclaration {
                id: "large".into(),
                kind: AssetKind::Other
            }),
            &vec![0; MAX_ASSET_BYTES + 1]
        ),
        Err(Error::Limit(_))
    ));
    assert!(matches!(
        s.awareness(&vec![0; MAX_AWARENESS_BYTES + 1]),
        Err(Error::Limit(_))
    ));
    assert!(s.paste(&Payload::new(Paste { at: None }), b"bad").is_err());
    assert_eq!(s.state().unwrap(), before);
    let applied = s
        .apply(&Payload::new(Transaction {
            commands: vec![Command::InsertBlock {
                parent: None,
                index: 0,
                block_kind: BlockKind::Paragraph,
                text: "\u{e9}".into(),
                style: Style::default(),
            }],
        }))
        .unwrap();
    let node = applied.data.blocks[0].clone();
    let state = s.state().unwrap();
    assert!(
        s.apply(&Payload::new(Transaction {
            commands: vec![
                Command::InsertText {
                    node: node.clone(),
                    at: 0,
                    text: "valid".into()
                },
                Command::DeleteText {
                    node,
                    start: 1,
                    end: u32::MAX
                }
            ]
        }))
        .is_err()
    );
    assert_eq!(s.state().unwrap(), state);
}
#[test]
fn cancelled_stale_foreign_and_reconfigured_jobs_never_publish() {
    let mut s = session();
    let mut j = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    j.cancel();
    assert_eq!(j.step(&mut s, 1).unwrap_err(), Error::Cancelled);
    let mut j = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    let mut other = session();
    assert_eq!(j.step(&mut other, 1).unwrap_err(), Error::Stale);
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertBlock {
            parent: None,
            index: 0,
            block_kind: BlockKind::Paragraph,
            text: "edited".into(),
            style: Style::default(),
        }],
    }))
    .unwrap();
    assert_eq!(j.step(&mut s, 1).unwrap_err(), Error::Stale);
    let mut j = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    j.step(&mut s, 100).unwrap();
    let _new = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    assert_eq!(j.display_page(&s, 0).unwrap_err(), Error::Stale);
}
#[test]
fn extreme_zero_negative_and_unbalanced_inputs_terminate() {
    let mut s = session();
    s.import_text(&Payload::new(TextImport {
        text: "<div>office <b>\u{05d0}\u{05d1}\u{05d2}</div>".into(),
        html: true,
        at: None,
    }))
    .unwrap();
    for (width, height) in [(i32::MIN, i32::MAX), (0, 0), (-1, -1), (i32::MAX, i32::MAX)] {
        let mut j = s
            .start_layout(&Payload::new(LayoutOptions {
                width,
                height,
                max_pages: 2,
                ..LayoutOptions::default()
            }))
            .unwrap();
        for i in 0..100 {
            if j.step(&mut s, 1).unwrap().data.complete {
                break;
            }
            assert!(i < 99);
        }
    }
    finish(&mut s);
    assert!(
        s.hit_test(&Payload::new(HitTest {
            page: u32::MAX,
            x: i32::MIN,
            y: i32::MAX
        }))
        .is_err()
    );
    assert!(matches!(s.png(0, u32::MAX), Err(Error::Limit(_))));
    let bad = Caret {
        node: "0@999".into(),
        offset: u32::MAX,
        affinity: Affinity::Downstream,
    };
    assert!(s.caret_rect(&Payload::new(bad)).is_err());
}
#[test]
fn tampered_sync_packet_and_foreign_awareness_are_rejected_atomically() {
    let mut s = session();
    let before = s.state().unwrap();
    let mut packet = s.export_updates().unwrap();
    packet.data.from_peer = "2".into();
    packet.data.vector.push(Clock {
        peer: "3".into(),
        counter: 1,
    });
    assert!(s.import_updates(&packet).is_err());
    assert_eq!(s.state().unwrap(), before);
    let mut awareness = s.awareness(&[0, 255]).unwrap();
    awareness.data.document_id = "foreign".into();
    assert!(s.validate_awareness(&awareness).is_err());
}
