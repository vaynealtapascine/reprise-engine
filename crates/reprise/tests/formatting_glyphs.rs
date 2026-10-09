//! Character formatting and glyph outlines through the facade only.
use reprise::*;

fn session() -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: "1".into(),
        }))
        .unwrap()
}

fn paragraph(s: &mut DocumentSession, text: &str) -> String {
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

fn format(
    s: &mut DocumentSession,
    node: &str,
    start: u32,
    end: u32,
    style: TextStyle,
) -> Result<Payload<Applied>> {
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::FormatText {
            node: node.into(),
            start,
            end,
            style,
        }],
    }))
}

#[test]
fn format_text_is_reported_in_state_and_undoes() {
    let mut s = session();
    let node = paragraph(&mut s, "plain large plain");
    let block = |s: &DocumentSession| s.state().unwrap().data.blocks[0].clone();
    // Unformatted blocks omit the field, keeping existing JSON unchanged.
    assert_eq!(block(&s).formatting, None);
    assert!(
        !serde_json::to_string(&block(&s))
            .unwrap()
            .contains("formatting")
    );

    let large = TextStyle {
        size: Some(20 * 1024),
        features: Some(vec![TextFeature {
            tag: "smcp".into(),
            value: 1,
        }]),
        ..TextStyle::default()
    };
    format(&mut s, &node, 6, 11, large.clone()).unwrap();
    let runs = block(&s).formatting.unwrap();
    assert_eq!(
        runs.iter().map(|r| (r.start, r.end)).collect::<Vec<_>>(),
        [(0, 6), (6, 11), (11, 17)]
    );
    assert_eq!(runs[0].style, TextStyle::default());
    assert_eq!(runs[1].style, large);

    assert!(s.undo().unwrap().data);
    assert_eq!(block(&s).formatting, None);
    assert!(s.redo().unwrap().data);
    assert_eq!(block(&s).formatting.unwrap()[1].style, large);
}

#[test]
fn invalid_formatting_is_refused_without_writing() {
    let mut s = session();
    let node = paragraph(&mut s, "e\u{301}x");
    let before = s.state().unwrap().data.revision;
    for (start, end, style) in [
        (
            0,
            0,
            TextStyle {
                size: Some(1024),
                ..TextStyle::default()
            },
        ),
        (
            0,
            99,
            TextStyle {
                size: Some(1024),
                ..TextStyle::default()
            },
        ),
        // Inside the combining sequence's UTF-8 bytes.
        (
            2,
            3,
            TextStyle {
                size: Some(1024),
                ..TextStyle::default()
            },
        ),
        (
            0,
            1,
            TextStyle {
                size: Some(0),
                ..TextStyle::default()
            },
        ),
        (
            0,
            1,
            TextStyle {
                size: Some(-5),
                ..TextStyle::default()
            },
        ),
        (
            0,
            1,
            TextStyle {
                families: Some(vec![]),
                ..TextStyle::default()
            },
        ),
        (
            0,
            1,
            TextStyle {
                features: Some(vec![TextFeature {
                    tag: "toolong".into(),
                    value: 1,
                }]),
                ..TextStyle::default()
            },
        ),
    ] {
        assert!(
            format(&mut s, &node, start, end, style.clone()).is_err(),
            "{start}..{end} {style:?}"
        );
        assert_eq!(s.state().unwrap().data.revision, before);
    }
}

#[test]
fn glyph_outlines_draw_every_glyph_the_layout_uses() {
    let mut s = session();
    paragraph(&mut s, "Hallway ffi \u{05d0}\u{05d1}");
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    let mut done = false;
    for _ in 0..10_000 {
        if job.step(&mut s, 1).unwrap().data.complete {
            done = true;
            break;
        }
    }
    assert!(done);
    let page = s.display_page(0).unwrap().data;
    let mut runs = Vec::new();
    fn collect<'a>(items: &'a [DisplayItem], out: &mut Vec<&'a GlyphRun>) {
        for item in items {
            match item {
                DisplayItem::Glyphs(run) => out.push(run),
                DisplayItem::Group { items, .. } => collect(items, out),
                _ => {}
            }
        }
    }
    collect(&page.display.items, &mut runs);
    assert!(!runs.is_empty());
    for run in runs {
        let ids: Vec<u32> = run.glyphs.iter().map(|g| g.id).collect();
        let outlines = s
            .glyph_outlines(&Payload::new(GlyphRequest {
                face: run.face.clone(),
                glyphs: ids.clone(),
            }))
            .unwrap()
            .data;
        assert!(outlines.units_per_em > 0);
        assert_eq!(
            outlines.glyphs.iter().map(|g| g.id).collect::<Vec<_>>(),
            ids
        );
        // Spaces have empty outlines; letters don't.
        assert!(outlines.glyphs.iter().any(|g| g.path.starts_with('M')));
        for glyph in &outlines.glyphs {
            assert!(
                glyph.path.chars().all(|c| "MLQCZ0123456789.- ".contains(c)),
                "{}",
                glyph.path
            );
        }
    }

    // Bounded, and unknown faces are a typed error rather than a panic.
    let face = Face {
        family: "Nope".into(),
        hash: "00".repeat(32),
    };
    let error = s
        .glyph_outlines(&Payload::new(GlyphRequest {
            face: face.clone(),
            glyphs: vec![1],
        }))
        .unwrap_err();
    assert_eq!(error.payload().data.code, "bindings.missing-font");
    assert!(
        s.glyph_outlines(&Payload::new(GlyphRequest {
            face,
            glyphs: vec![0; 4097]
        }))
        .is_err()
    );
}
