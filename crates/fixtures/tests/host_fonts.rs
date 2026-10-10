//! Host settings and synthetic outlines share every layout/rendering path.
use reprise_doc::{BlockKind, Dim, Document, PageTemplate, Style, TextSlant, WritingMode};
use reprise_font::{Descriptors, FontDeclaration, FontSettings, GenericFamily};
use reprise_geom::{Length, Matrix, PageSpace, Rect, Transform};
use reprise_layout::{
    DisplayOptions,
    incremental::{LayoutSession, Viewport},
};

fn configured() -> reprise_layout::Engine {
    let mut engine = reprise_fixtures::engine();
    for (family, bytes) in [
        (
            "Paper Mono",
            include_bytes!("../../../fixtures/fonts/PaperMono-TestSubset.ttf").as_slice(),
        ),
        (
            "Universal",
            include_bytes!("../../../fixtures/fonts/AtkinsonHyperlegibleNext-Regular.ttf")
                .as_slice(),
        ),
        (
            "CJK",
            include_bytes!("../../../fixtures/fonts/NotoSansJP-VerticalSubset.otf").as_slice(),
        ),
    ] {
        engine
            .fonts
            .register(
                bytes,
                FontDeclaration {
                    family: family.into(),
                    descriptors: Descriptors::default(),
                    face_index: 0,
                },
            )
            .unwrap();
    }
    engine
        .fonts
        .set_settings(FontSettings {
            generics: [(GenericFamily::Monospace, "Paper Mono".into())].into(),
            fallbacks: vec!["Universal".into(), "CJK".into()],
            synthetic_oblique: true,
        })
        .unwrap();
    engine
}

fn bounds(
    items: &[reprise_display::Item],
    fonts: &reprise_font::FontStore,
    at: Matrix,
    out: &mut Vec<Rect<PageSpace>>,
) {
    for item in items {
        match item {
            reprise_display::Item::Glyphs(run) => {
                assert!(run.synthetic_oblique);
                if let Some(b) = run.ink_bounds(fonts) {
                    out.push(Transform::new(at).bounds(&b));
                }
            }
            reprise_display::Item::Group {
                transform, items, ..
            } => bounds(items, fonts, transform.then(&at), out),
            _ => {}
        }
    }
}

#[test]
fn oblique_horizontal_vertical_rtl_rotated_and_incremental_have_ink() {
    let engine = configured();
    let mut inks = Vec::new();
    for mode in [
        WritingMode::HorizontalTb,
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
        WritingMode::SidewaysLr,
    ] {
        let doc = Document::new(1).unwrap();
        let mut frame = reprise_fixtures::templates::flow_frame(
            "main",
            Dim::pt(40),
            Dim::pt(30),
            Dim::pt(180),
            Dim::pt(170),
        );
        frame.writing_mode = mode;
        frame.transform.rotation = reprise_doc::Rotation::Quarter(1);
        frame.transform.origin_x = Dim::pt(90);
        frame.transform.origin_y = Dim::pt(85);
        doc.set_page_template(
            &PageTemplate::new("oblique", Dim::pt(300), Dim::pt(300)).with_frame(frame),
        )
        .unwrap();
        let node = doc
            .append_block(BlockKind::Paragraph, "", "אב aéb 日本語 12 ffi")
            .unwrap();
        doc.set_overrides(
            node,
            &Style {
                family: Some("monospace".into()),
                slant: Some(TextSlant::Oblique),
                text_combine_upright: Some(reprise_geom::TextCombineUpright::Digits(2)),
                ..Default::default()
            },
        )
        .unwrap();
        doc.commit();
        let expected = engine.layout(&doc);
        assert_eq!(expected.block(node).unwrap().base_level, 1);
        assert!(
            expected
                .diagnostics_with("font.synthetic")
                .all(|d| d.severity == reprise_diag::Severity::Info)
        );
        assert!(
            expected
                .blocks
                .iter()
                .flat_map(|b| &b.lines)
                .flat_map(|l| &l.runs)
                .all(|r| r.synthetic_oblique)
        );
        let mut session = LayoutSession::new(&engine);
        let actual = session.layout(&doc).unwrap();
        assert_eq!(actual.to_json(), expected.to_json());
        doc.block(node).unwrap().text.insert(4, "é").unwrap();
        doc.commit();
        assert_eq!(
            session.layout(&doc).unwrap().to_json(),
            engine.layout(&doc).to_json()
        );
        let mut job = session.start(&doc, Viewport::Pages(0..usize::MAX));
        let mut complete = false;
        for _ in 0..10000 {
            if job.step(1).unwrap().complete {
                complete = true;
                break;
            }
        }
        assert!(complete, "explicit job bound");
        let list = expected
            .to_display_list(0, DisplayOptions::default())
            .content_only();
        let mut ink = Vec::new();
        bounds(&list.items, &engine.fonts, Matrix::IDENTITY, &mut ink);
        assert!(!ink.is_empty());
        inks.push((format!("{mode:?}"), ink));
        let svg = reprise_display::svg::render(&list, &engine.fonts).unwrap();
        assert!(svg.contains("-oblique"));
        let png = reprise_display::png::render(&list, &engine.fonts, 1.0).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        let pdf = reprise_display::pdf::render(&[list], &engine.fonts).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
    }
    insta::assert_json_snapshot!("oblique_ink_bounds", inks);
}

#[test]
fn unused_fallback_changes_only_the_configuration_envelope() {
    let mut engine = configured();
    let doc = Document::new(1).unwrap();
    let node = doc
        .append_block(BlockKind::Paragraph, "", "plain ascii text")
        .unwrap();
    doc.set_overrides(
        node,
        &Style {
            family: Some("monospace".into()),
            ..Default::default()
        },
    )
    .unwrap();
    doc.commit();
    let before = engine.layout(&doc);
    engine
        .fonts
        .set_settings(FontSettings {
            fallbacks: vec!["Missing".into()],
            ..engine.fonts.settings().clone()
        })
        .unwrap();
    let after = engine.layout(&doc);
    assert_eq!(before.blocks, after.blocks);
    assert_eq!(before.diagnostics, after.diagnostics);
    assert_eq!(
        before.to_display_lists(DisplayOptions::default()),
        after.to_display_lists(DisplayOptions::default())
    );
    assert_ne!(before.font_settings, after.font_settings);
    assert!(
        after
            .blocks
            .iter()
            .flat_map(|b| &b.lines)
            .flat_map(|l| &l.runs)
            .all(|r| r.size > Length::ZERO)
    );
}
