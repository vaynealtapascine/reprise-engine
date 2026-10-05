use reprise::*;

fn session(peer: &str) -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: peer.into(),
        }))
        .unwrap()
}
fn finish(s: &mut DocumentSession, options: LayoutOptions) {
    let mut job = s.start_layout(&Payload::new(options)).unwrap();
    for _ in 0..1000 {
        if job.step(s, 8).unwrap().data.complete {
            return;
        }
    }
    panic!("explicit regression job bound exceeded");
}
fn insert(s: &mut DocumentSession, text: &str) -> String {
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertBlock {
            parent: None,
            index: 0,
            block_kind: BlockKind::Paragraph,
            text: text.into(),
            style: Style::default(),
        }],
    }))
    .unwrap()
    .data
    .blocks[0]
        .clone()
}
fn selection(node: &str, len: u32) -> Payload<Selection> {
    Payload::new(Selection {
        anchor: Caret {
            node: node.into(),
            offset: 0,
            affinity: Affinity::Downstream,
        },
        focus: Caret {
            node: node.into(),
            offset: len,
            affinity: Affinity::Upstream,
        },
    })
}

#[test]
fn default_text_works_without_font_registration_and_after_reopen() {
    let mut s = session("1");
    let node = insert(&mut s, "office ffi café אבג مرحبا e\u{301}");
    finish(&mut s, LayoutOptions::default());
    assert!(
        s.diagnostics()
            .data
            .iter()
            .all(|d| d.severity == Severity::Info)
    );
    assert!(s.display_json(0).unwrap().data.contains("office ffi café"));
    s.copy(&selection(&node, 6)).unwrap();
    let before = s.display_json(0).unwrap().data;
    let bytes = s.save().unwrap().data.bytes;
    let mut opened = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    finish(&mut opened, LayoutOptions::default());
    assert_eq!(before, opened.display_json(0).unwrap().data);
    assert!(
        opened
            .diagnostics()
            .data
            .iter()
            .all(|d| d.severity == Severity::Info)
    );
}

#[test]
fn open_supplies_missing_base_but_preserves_existing_authored_base() {
    use reprise_doc::{Document, PersistenceMode};
    use reprise_format::{DocumentId, Package};
    for authored in [false, true] {
        let doc = Document::new(1).unwrap();
        if authored {
            doc.define_style(
                "",
                &reprise_doc::Style {
                    families: Some(vec!["monospace".into()]),
                    ..reprise_doc::default_style()
                },
            )
            .unwrap();
        }
        doc.append_block(reprise_doc::BlockKind::Paragraph, "", "office")
            .unwrap();
        doc.commit();
        let bytes = Package::new(&doc, DocumentId([5; 16]), PersistenceMode::History)
            .unwrap()
            .save()
            .unwrap();
        let mut s = Workspace::new()
            .open(
                &Payload::new(Open {
                    peer_id: "2".into(),
                }),
                &bytes,
            )
            .unwrap();
        finish(&mut s, LayoutOptions::default());
        assert!(
            s.diagnostics()
                .data
                .iter()
                .all(|d| d.severity == Severity::Info)
        );
        let saved = s.save().unwrap().data.bytes;
        let opened = Package::open(
            &saved,
            3,
            Default::default(),
            &reprise_format::MigrationRegistry::builtin(),
        )
        .unwrap();
        let base = opened.editable_document().unwrap().style("").unwrap();
        if authored {
            assert_eq!(base.families, Some(vec!["monospace".into()]));
        } else {
            assert_eq!(base.family.as_deref(), Some("serif"));
            assert_eq!(base.families, None);
        }
    }
}

#[test]
fn valid_unplaced_block_is_layout_required_and_absent_id_is_id_error() {
    let mut s = session("1");
    let node = insert(&mut s, "office");
    // A preceding paragraph fills the one-page budget, leaving this live node unplaced.
    insert(&mut s, &"fills the page ".repeat(1000));
    finish(
        &mut s,
        LayoutOptions {
            max_pages: 1,
            ..Default::default()
        },
    );
    let error = match s.copy(&selection(&node, 6)) {
        Err(error) => error,
        Ok(_) => panic!("unplaced text cannot be copied through a layout selection"),
    };
    assert_eq!(error, Error::NoLayout);
    assert_eq!(error.code(), "bindings.layout-required");
    assert_eq!(error.payload().data.severity, Severity::Error);
    let mut other = session("2");
    let absent = insert(&mut other, "absent");
    assert!(matches!(
        s.copy(&selection(&absent, 6)),
        Err(Error::InvalidId(_))
    ));
}

#[test]
fn image_insertions_are_atomic_and_missing_extreme_images_are_bounded() {
    let mut s = session("1");
    let node = insert(&mut s, "office");
    let before = s.state().unwrap();
    let image = ImageInsert {
        at: None,
        asset: "1".repeat(64),
        alt: "missing".into(),
        width: Some(i32::MAX),
        height: Some(i32::MIN),
        style: Style::default(),
    };
    assert!(
        s.apply(&Payload::new(Transaction {
            commands: vec![
                Command::InsertText {
                    node,
                    at: 0,
                    text: "must not commit".into()
                },
                Command::InsertImage {
                    image: image.clone()
                },
            ]
        }))
        .is_err()
    );
    assert_eq!(s.state().unwrap(), before);
    for asset in ["bad".into(), "A".repeat(64)] {
        assert!(matches!(
            s.insert_image(&Payload::new(ImageInsert {
                asset,
                ..image.clone()
            })),
            Err(Error::InvalidId(_))
        ));
        assert_eq!(s.state().unwrap(), before);
    }
    let id = s.insert_image(&Payload::new(image)).unwrap().data.blocks[0].clone();
    finish(&mut s, LayoutOptions::default());
    assert!(
        s.diagnostics()
            .data
            .iter()
            .any(|d| d.code == "layout.image-missing")
    );
    assert_eq!(
        s.state()
            .unwrap()
            .data
            .blocks
            .iter()
            .find(|b| b.id == id)
            .unwrap()
            .kind,
        BlockKind::Image
    );
    assert!(s.undo().unwrap().data);
    assert_eq!(s.state().unwrap().data.blocks, before.data.blocks);
}
