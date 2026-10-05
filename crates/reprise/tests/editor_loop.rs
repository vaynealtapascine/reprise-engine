use reprise::*;
fn session(peer: &str) -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: peer.into(),
        }))
        .unwrap()
}
fn insert(s: &mut DocumentSession, text: &str) -> String {
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertBlock {
            parent: None,
            index: s.state().unwrap().data.blocks.len() as u32,
            block_kind: BlockKind::Paragraph,
            text: text.into(),
            style: Style {
                families: Some(vec!["Source Serif Pro".into(), "serif".into()]),
                ..Style::default()
            },
        }],
    }))
    .unwrap()
    .data
    .blocks[0]
        .clone()
}
fn layout(s: &mut DocumentSession) -> LayoutProgress {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    for _ in 0..10_000 {
        let progress = job.step(s, 1).unwrap().data;
        if progress.complete {
            return progress;
        }
    }
    panic!("explicit test job bound exceeded")
}
#[test]
fn complete_editor_loop_only_through_the_facade() {
    let mut s = session("1");
    s.declare_font(
        &Payload::new(FontDeclaration {
            family: "Source Serif Pro".into(),
            weight: 400,
            style: FontStyle::Normal,
            stretch: 1000,
            face_index: 0,
        }),
        include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf"),
    )
    .unwrap();
    let node = insert(&mut s, "");
    let text = "office ffi \u{05d0}\u{05d1}\u{05d2} \u{0645}\u{0631}\u{062d}\u{0628}\u{0627}";
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertText {
            node: node.clone(),
            at: 0,
            text: text.into(),
        }],
    }))
    .unwrap();
    let progress = layout(&mut s);
    assert!(progress.viewport_ready);
    assert_eq!(progress.pages, vec![0]);
    let caret = Caret {
        node: node.clone(),
        offset: 0,
        affinity: Affinity::Downstream,
    };
    let moved = s
        .move_cursor(&Payload::new(Move {
            cursor: Cursor {
                caret: caret.clone(),
                goal_x: None,
            },
            movement: Movement::VisualRight,
        }))
        .unwrap();
    assert!(moved.data.caret.offset > 0);
    let selection = Selection {
        anchor: caret,
        focus: Caret {
            node: node.clone(),
            offset: text.len() as u32,
            affinity: Affinity::Upstream,
        },
    };
    assert!(
        !s.selection_rects(&Payload::new(selection.clone()))
            .unwrap()
            .data
            .is_empty()
    );
    let copied_plain = s
        .copy_as(&Payload::new(CopyAs {
            selection: selection.clone(),
            format: CopyFormat::PlainText,
        }))
        .unwrap()
        .data;
    assert_eq!(String::from_utf8(copied_plain.content.bytes).unwrap(), text);
    let copied_html = s
        .copy_as(&Payload::new(CopyAs {
            selection: selection.clone(),
            format: CopyFormat::Html,
        }))
        .unwrap()
        .data;
    assert!(
        String::from_utf8(copied_html.content.bytes)
            .unwrap()
            .contains("office ffi")
    );
    let copied = s.copy(&Payload::new(selection)).unwrap().data.bytes;
    s.paste(&Payload::new(Paste { at: None }), &copied).unwrap();
    assert_eq!(s.state().unwrap().data.blocks.len(), 2);
    assert!(s.undo().unwrap().data);
    assert_eq!(s.state().unwrap().data.blocks.len(), 1);
    layout(&mut s);
    let before = s.display_json(0).unwrap().data;
    let saved = s.save().unwrap().data.bytes;
    let mut reopened = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "2".into(),
            }),
            &saved,
        )
        .unwrap();
    layout(&mut reopened);
    assert_eq!(before, reopened.display_json(0).unwrap().data);
    let plain = s
        .export(&Payload::new(ExportFormat::PlainText))
        .unwrap()
        .data;
    assert_eq!(String::from_utf8(plain.content.bytes).unwrap(), text);
    assert_eq!(plain.losses.len(), 10);
    let pdf = s.export(&Payload::new(ExportFormat::Pdf)).unwrap().data;
    assert!(pdf.content.bytes.starts_with(b"%PDF"));
    assert_eq!(pdf.losses.len(), 10);
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertText {
            node: node.clone(),
            at: 0,
            text: "A".into(),
        }],
    }))
    .unwrap();
    reopened
        .apply(&Payload::new(Transaction {
            commands: vec![Command::InsertText {
                node,
                at: text.len() as u32,
                text: "B".into(),
            }],
        }))
        .unwrap();
    let a = s.export_updates().unwrap();
    let b = reopened.export_updates().unwrap();
    s.import_updates(&b).unwrap();
    reopened.import_updates(&a).unwrap();
    assert_eq!(s.sync_info().data.vector, reopened.sync_info().data.vector);
    assert_eq!(
        s.state().unwrap().data.blocks,
        reopened.state().unwrap().data.blocks
    );
    layout(&mut s);
    layout(&mut reopened);
    assert_eq!(
        s.display_json(0).unwrap(),
        reopened.display_json(0).unwrap()
    );
}
#[test]
fn incremental_relayout_reuses_unaffected_paragraphs() {
    let mut s = session("1");
    let first = insert(&mut s, "a");
    for _ in 0..12 {
        insert(&mut s, "unaffected");
    }
    layout(&mut s);
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertText {
            node: first,
            at: 0,
            text: "b".into(),
        }],
    }))
    .unwrap();
    let p = layout(&mut s);
    assert_eq!(p.counters.shapes, 1);
    assert!(p.counters.reused_compositions >= 12);
}
#[test]
fn generated_types_are_current() {
    assert_eq!(
        typescript::declarations(),
        include_str!("../../reprise-wasm/ts/types.d.ts")
    );
}
#[test]
fn native_display_golden_is_shared_with_wasm_smoke() {
    let mut s = session("1");
    insert(&mut s, "office ffi \u{05d0}\u{05d1}\u{05d2}");
    layout(&mut s);
    let json = s.display_json(0).unwrap().data;
    assert_eq!(json, include_str!("display.json").trim_end());
}

#[test]
fn image_editor_loop_only_through_the_facade() {
    let mut s = session("11");
    let bytes = include_bytes!("../../../fixtures/images/red-1x1.png");
    let hash = s
        .register_asset(
            &Payload::new(AssetDeclaration {
                id: "red".into(),
                kind: AssetKind::Image,
            }),
            bytes,
        )
        .unwrap()
        .data;
    let alt = "A red café image";
    let image = ImageInsert {
        at: None,
        asset: hash.clone(),
        alt: alt.into(),
        width: Some(20 * 1024),
        height: Some(20 * 1024),
        style: Style::default(),
    };
    let node = s
        .apply(&Payload::new(Transaction {
            commands: vec![Command::InsertImage {
                image: image.clone(),
            }],
        }))
        .unwrap()
        .data
        .blocks[0]
        .clone();
    layout(&mut s);
    let before = s.display_json(0).unwrap().data;
    assert!(before.contains("\"type\":\"image\""));
    assert!(before.contains(&hash));
    assert!(before.contains(alt));
    assert_eq!(before, include_str!("images.json").trim_end());
    assert!(
        s.diagnostics()
            .data
            .iter()
            .all(|d| d.severity == Severity::Info)
    );
    let svg = s.svg(0).unwrap().data;
    assert!(svg.contains("data:image/png;base64,"));
    let png = s.png(0, 1000).unwrap().data.bytes;
    assert!(png.starts_with(b"\x89PNG"));
    let pdf = s.export(&Payload::new(ExportFormat::Pdf)).unwrap().data;
    let parsed = lopdf::Document::load_mem(&pdf.content.bytes).unwrap();
    assert!(
        parsed
            .objects
            .values()
            .any(|object| object.as_stream().is_ok_and(|stream| stream
                .dict
                .get(b"Subtype")
                .is_ok_and(|value| value.as_name().ok() == Some(b"Image"))))
    );
    assert_eq!(
        pdf.losses
            .iter()
            .find(|l| l.code == "export.assets")
            .unwrap()
            .disposition,
        Disposition::Preserved
    );
    let plain = s
        .export(&Payload::new(ExportFormat::PlainText))
        .unwrap()
        .data;
    assert_eq!(String::from_utf8(plain.content.bytes).unwrap(), alt);
    let caret = |offset, affinity| Caret {
        node: node.clone(),
        offset,
        affinity,
    };
    let clip = s
        .copy(&Payload::new(Selection {
            anchor: caret(0, Affinity::Downstream),
            focus: caret(alt.len() as u32, Affinity::Upstream),
        }))
        .unwrap()
        .data
        .bytes;
    let mut target = session("12");
    target
        .paste(&Payload::new(Paste { at: None }), &clip)
        .unwrap();
    layout(&mut target);
    assert_eq!(target.svg(0).unwrap().data, svg);
    assert_eq!(target.png(0, 1000).unwrap().data.bytes, png);
    assert!(target.undo().unwrap().data);
    assert!(target.state().unwrap().data.blocks.is_empty());
    assert!(target.redo().unwrap().data);
    layout(&mut target);
    assert_eq!(target.display_json(0).unwrap().data, before);
    let saved = target.save().unwrap().data.bytes;
    let mut opened = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "13".into(),
            }),
            &saved,
        )
        .unwrap();
    layout(&mut opened);
    assert_eq!(opened.display_json(0).unwrap().data, before);
    assert_eq!(opened.svg(0).unwrap().data, svg);
    assert_eq!(opened.png(0, 1000).unwrap().data.bytes, png);
    let resources = opened.resources().unwrap().data;
    let resource = resources.iter().find(|r| r.hash == hash).unwrap();
    assert_eq!(
        opened
            .resource_bytes(&Payload::new(resource.id.clone()))
            .unwrap()
            .data
            .bytes,
        bytes
    );
    let native = opened
        .export(&Payload::new(ExportFormat::Native))
        .unwrap()
        .data;
    assert_eq!(
        native
            .losses
            .iter()
            .find(|l| l.code == "export.assets")
            .unwrap()
            .disposition,
        Disposition::Preserved
    );
    let mut imported = session("14");
    imported
        .paste(&Payload::new(Paste { at: None }), &native.content.bytes)
        .unwrap();
    layout(&mut imported);
    assert_eq!(imported.svg(0).unwrap().data, svg);
    // Pixel output differs from the same authored image without its resource.
    let mut missing = session("15");
    missing.insert_image(&Payload::new(image)).unwrap();
    layout(&mut missing);
    assert_ne!(missing.png(0, 1000).unwrap().data.bytes, png);
}
