use reprise::*;
fn session() -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: "1".into(),
        }))
        .unwrap()
}
fn apply(s: &mut DocumentSession, commands: Vec<Command>) -> Payload<Applied> {
    s.apply(&Payload::new(Transaction { commands })).unwrap()
}
fn finish(s: &mut DocumentSession) {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    for _ in 0..1000 {
        if job.step(s, 1000).unwrap().data.complete {
            return;
        }
    }
    panic!("layout did not finish in bounded steps");
}
#[test]
fn full_marks_editor_clipboard_package_loop() {
    let mut s = session();
    assert_eq!(s.marks(0).unwrap_err().code(), "bindings.layout-required");
    let node = apply(
        &mut s,
        vec![Command::InsertBlock {
            parent: None,
            index: 0,
            block_kind: BlockKind::Paragraph,
            text: "ab".into(),
            style: Style::default(),
        }],
    )
    .data
    .blocks[0]
        .clone();
    apply(
        &mut s,
        vec![
            Command::InsertTab {
                node: node.clone(),
                at: 1,
            },
            Command::InsertLineBreak {
                node: node.clone(),
                at: 3,
            },
            Command::SetTabStops {
                node: node.clone(),
                tabs: TabStops {
                    interval: 36 * 1024,
                    stops: vec![TabStop {
                        position: None,
                        alignment: Alignment::End,
                        leader: Some(".".into()),
                    }],
                },
            },
            Command::SetAlignment {
                node: node.clone(),
                at: Some(4),
                alignment: Alignment::Centre,
            },
        ],
    );
    let anchor = apply(
        &mut s,
        vec![Command::AddAnchor {
            node: node.clone(),
            at: 4,
            edge: LineEdge::Start,
            target: node.clone(),
            target_at: 1,
            target_edge: AnchorEdge::GapEnd,
        }],
    )
    .data
    .relations[0]
        .clone();
    finish(&mut s);
    let before = s.display_json(0).unwrap();
    let marks = s.marks(0).unwrap().data;
    assert_eq!(marks.page, 0);
    for kind in [
        MarkKind::ParagraphEnd,
        MarkKind::LineBreak,
        MarkKind::Gap,
        MarkKind::Alignment,
        MarkKind::Anchor,
    ] {
        assert!(marks.marks.iter().any(|m| m.kind == kind));
    }
    let guide = marks
        .marks
        .iter()
        .find(|m| m.relation.as_ref() == Some(&anchor))
        .unwrap();
    assert_eq!(guide.state, Some(RelationState::Valid));
    assert!(guide.applied);
    assert_eq!(guide.from.x, guide.to.as_ref().unwrap().x);
    assert_eq!(before, s.display_json(0).unwrap());
    assert_eq!(s.marks(1).unwrap_err().code(), "bindings.invalid");
    let bytes = s.save().unwrap().data.bytes;
    assert_ne!(
        u64::from_le_bytes(bytes[28..36].try_into().unwrap()) & (1 << 11),
        0
    );
    let mut reopen = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    finish(&mut reopen);
    assert_eq!(marks.marks, reopen.marks(0).unwrap().data.marks);
    for format in [
        ExportFormat::PlainText,
        ExportFormat::Html,
        ExportFormat::Native,
        ExportFormat::Pdf,
    ] {
        assert!(
            !s.export(&Payload::new(format))
                .unwrap()
                .data
                .content
                .bytes
                .is_empty()
        );
    }
    let caret = |offset| Caret {
        node: node.clone(),
        offset,
        affinity: Affinity::Downstream,
    };
    let selection = Selection {
        anchor: caret(0),
        focus: caret(4),
    };
    let copy = s.copy(&Payload::new(selection)).unwrap().data.bytes;
    let mut pasted = session();
    pasted
        .paste(&Payload::new(Paste { at: None }), &copy)
        .unwrap();
    finish(&mut pasted);
    assert!(
        pasted
            .marks(0)
            .unwrap()
            .data
            .marks
            .iter()
            .any(|m| m.kind == MarkKind::Anchor && m.applied)
    );
    apply(&mut s, vec![Command::RemoveAnchor { id: anchor.clone() }]);
    finish(&mut s);
    assert!(
        !s.marks(0)
            .unwrap()
            .data
            .marks
            .iter()
            .any(|m| m.relation.as_ref() == Some(&anchor))
    );
    s.undo().unwrap();
    finish(&mut s);
    assert!(
        s.marks(0)
            .unwrap()
            .data
            .marks
            .iter()
            .any(|m| m.relation.as_ref() == Some(&anchor))
    );
}
#[test]
fn refused_marks_transaction_is_atomic_and_style_commands_patch() {
    let mut s = session();
    let node = apply(
        &mut s,
        vec![Command::InsertBlock {
            parent: None,
            index: 0,
            block_kind: BlockKind::Paragraph,
            text: "éx".into(),
            style: Style {
                weight: Some(700),
                ..Default::default()
            },
        }],
    )
    .data
    .blocks[0]
        .clone();
    let before = s.state().unwrap();
    let result = s.apply(&Payload::new(Transaction {
        commands: vec![
            Command::InsertTab {
                node: node.clone(),
                at: 0,
            },
            Command::AddAnchor {
                node: node.clone(),
                at: 2,
                edge: LineEdge::Start,
                target: node.clone(),
                target_at: 0,
                target_edge: AnchorEdge::Position,
            },
        ],
    }));
    assert!(result.is_err());
    assert_eq!(before, s.state().unwrap());
    for interval in [i32::MIN, -1, 0] {
        assert!(
            s.apply(&Payload::new(Transaction {
                commands: vec![Command::SetTabStops {
                    node: node.clone(),
                    tabs: TabStops {
                        interval,
                        stops: Vec::new()
                    }
                }]
            }))
            .is_err()
        );
    }
    apply(
        &mut s,
        vec![Command::SetAlignment {
            node,
            at: None,
            alignment: Alignment::End,
        }],
    );
    finish(&mut s);
    let display = s.display_json(0).unwrap().data;
    assert!(
        s.marks(0)
            .unwrap()
            .data
            .marks
            .iter()
            .any(|m| m.alignment == Some(Alignment::End))
    );
    assert!(display.contains("glyphs"));
}
