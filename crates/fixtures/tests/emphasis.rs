//! Selected faces, paint, decoration geometry and incremental parity.
use reprise_doc::{
    BlockKind, Decoration, Dim, Document, PageTemplate, Style, TextSlant, WritingMode,
    formatting::TextStyle, text::RangePolicy,
};
use reprise_font::{Descriptors, FontDeclaration, FontStyle};
use reprise_geom::{Length, Point};
use reprise_layout::{DisplayOptions, incremental::LayoutSession};

fn styled() -> TextStyle {
    TextStyle {
        weight: Some(700),
        slant: Some(TextSlant::Italic),
        decoration: Decoration {
            underline: Some(true),
            strike: Some(true),
        },
        color: Some([30, 60, 90, 128]),
        ..Default::default()
    }
}
fn fonts(engine: &mut reprise_layout::Engine) -> Vec<reprise_font::FaceId> {
    [
        (
            include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
            400,
            FontStyle::Normal,
        ),
        (
            include_bytes!("../../../fixtures/fonts/SourceSans3-Regular.otf").as_slice(),
            700,
            FontStyle::Normal,
        ),
        (
            include_bytes!("../../../fixtures/fonts/SourceCodePro-Regular.otf").as_slice(),
            400,
            FontStyle::Italic,
        ),
    ]
    .into_iter()
    .map(|(bytes, weight, style)| {
        engine
            .fonts
            .register(
                bytes,
                FontDeclaration {
                    family: "Test Faces".into(),
                    descriptors: Descriptors {
                        weight,
                        style,
                        stretch: 1000,
                    },
                    face_index: 0,
                },
            )
            .unwrap()
    })
    .collect()
}
#[test]
fn faces_follow_css_slant_before_weight_and_new_fields_invalidate_memos() {
    let mut engine = reprise_fixtures::engine();
    let faces = fonts(&mut engine);
    let doc = Document::new(1).unwrap();
    let node = doc
        .append_block(BlockKind::Paragraph, "", "regular bold italic both")
        .unwrap();
    doc.set_overrides(
        node,
        &Style {
            families: Some(vec!["Test Faces".into(), "serif".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    doc.append_block(BlockKind::Paragraph, "", "unaffected")
        .unwrap();
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    session.layout(&doc).unwrap();
    doc.format_text(
        node,
        8..12,
        &TextStyle {
            weight: Some(700),
            ..Default::default()
        },
        RangePolicy::FIXED,
    )
    .unwrap();
    doc.format_text(
        node,
        13..19,
        &TextStyle {
            slant: Some(TextSlant::Italic),
            ..Default::default()
        },
        RangePolicy::FIXED,
    )
    .unwrap();
    doc.format_text(node, 20..24, &styled(), RangePolicy::FIXED)
        .unwrap();
    doc.commit();
    let layout = session.layout(&doc).unwrap();
    assert_eq!(layout, engine.layout(&doc));
    assert_eq!(session.counters().shapes, 1);
    let block = layout.block(node).unwrap();
    let runs: Vec<_> = block.lines.iter().flat_map(|l| &l.runs).collect();
    assert_eq!(
        runs.iter().find(|r| r.range.contains(&0)).unwrap().face,
        faces[0]
    );
    assert_eq!(
        runs.iter().find(|r| r.range.contains(&8)).unwrap().face,
        faces[1]
    );
    assert_eq!(
        runs.iter().find(|r| r.range.contains(&13)).unwrap().face,
        faces[2]
    );
    let both = runs.iter().find(|r| r.range.contains(&20)).unwrap();
    assert_eq!(both.face, faces[2]);
    assert_eq!(both.color, [30, 60, 90, 128]);
    assert!(both.underline.is_some() && both.strike.is_some());
    assert!(
        layout
            .diagnostics_with("font.nearest")
            .any(|d| d.severity == reprise_diag::Severity::Warning)
    );
    for patch in [
        TextStyle {
            color: Some([1, 2, 3, 0]),
            ..Default::default()
        },
        TextStyle {
            decoration: Decoration {
                underline: Some(false),
                strike: None,
            },
            ..Default::default()
        },
        TextStyle {
            weight: Some(400),
            ..Default::default()
        },
        TextStyle {
            slant: Some(TextSlant::Normal),
            ..Default::default()
        },
    ] {
        doc.format_text(node, 20..24, &patch, RangePolicy::FIXED)
            .unwrap();
        doc.commit();
        assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
    }
}
#[test]
fn rtl_and_vertical_decorations_have_run_extent_and_physical_right_underline() {
    for mode in [
        WritingMode::HorizontalTb,
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
        WritingMode::SidewaysLr,
    ] {
        let doc = Document::new(1).unwrap();
        let mut engine = reprise_fixtures::engine();
        engine.fonts.add(
            reprise_font::Face::from_bytes(
                include_bytes!("../../../fixtures/fonts/NotoSansJP-VerticalSubset.otf").as_slice(),
            )
            .unwrap(),
        );
        let mut frame = reprise_fixtures::templates::flow_frame(
            "main",
            Dim::pt(20),
            Dim::pt(20),
            Dim::pt(200),
            Dim::pt(200),
        );
        frame.writing_mode = mode;
        doc.set_page_template(
            &PageTemplate::new("decorated", Dim::pt(250), Dim::pt(250)).with_frame(frame),
        )
        .unwrap();
        let node = doc
            .append_block(BlockKind::Paragraph, "", "日本語 12 e\u{301} אבג")
            .unwrap();
        doc.set_overrides(
            node,
            &Style {
                families: Some(vec!["Noto Sans JP".into(), "serif".into()]),
                text_combine_upright: Some(reprise_geom::TextCombineUpright::Digits(2)),
                ..Default::default()
            },
        )
        .unwrap();
        let len = doc.block(node).unwrap().text.len();
        doc.format_text(
            node,
            0..len,
            &TextStyle {
                weight: None,
                slant: None,
                ..styled()
            },
            RangePolicy::FIXED,
        )
        .unwrap();
        doc.commit();
        let layout = engine.layout(&doc);
        let lists = layout.to_display_lists(DisplayOptions::default());
        let mut paths = Vec::new();
        let mut glyphs = Vec::new();
        fn collect<'a>(
            items: &'a [reprise_display::Item],
            paths: &mut Vec<&'a reprise_display::Path>,
            glyphs: &mut Vec<&'a reprise_display::GlyphRun>,
        ) {
            for item in items {
                match item {
                    reprise_display::Item::Path {
                        path,
                        layer: reprise_display::Layer::Content,
                        fill: Some(color),
                        ..
                    } => {
                        assert_eq!(*color, reprise_display::Color(30, 60, 90, 128));
                        paths.push(path)
                    }
                    reprise_display::Item::Glyphs(g) => glyphs.push(g),
                    reprise_display::Item::Group { items, .. } => collect(items, paths, glyphs),
                    _ => {}
                }
            }
        }
        collect(&lists[0].items, &mut paths, &mut glyphs);
        assert!(!paths.is_empty());
        assert!(
            glyphs
                .iter()
                .all(|g| g.color == reprise_display::Color(30, 60, 90, 128))
        );
        let block = layout.block(node).unwrap();
        let line = &block.lines[0];
        assert_eq!(
            paths.len(),
            line.runs.iter().filter(|r| r.width > Length::ZERO).count() * 2
        );
        if matches!(mode, WritingMode::VerticalRl | WritingMode::VerticalLr) {
            let run = &line.runs[0];
            let metric = run.underline.unwrap();
            let sign = if mode == WritingMode::VerticalRl {
                -1
            } else {
                1
            };
            let baseline = layout.frames[line.frame]
                .to_page
                .apply(Point::new(run.x, line.baseline));
            let underline = layout.frames[line.frame].to_page.apply(Point::new(
                run.x,
                line.baseline + metric.offset.mul_ratio(sign, 1),
            ));
            assert!(underline.x > baseline.x);
            assert!(line.runs.iter().any(|r| r.upright));
            assert!(line.runs.iter().any(|r| r.combined));
        }
        let pdf = reprise_display::pdf::render_ordered(
            &lists,
            &engine.fonts,
            &layout.pdf_reading_order(&doc),
        )
        .unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        reprise_display::svg::render(&lists[0], &engine.fonts).unwrap();
        reprise_display::png::render(&lists[0], &engine.fonts, 1.0).unwrap();
    }
}
#[test]
fn empty_combining_zero_width_transparent_and_extreme_sizes_terminate() {
    for (text, size) in [
        ("", Length::ZERO),
        ("\u{200b}", Length::from_pt(10)),
        ("\u{301}", Length::from_pt(10)),
        ("e\u{301}", Length::ZERO),
        ("x", Length::MIN),
        ("x", Length::MAX),
    ] {
        let engine = reprise_fixtures::engine();
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", text).unwrap();
        doc.set_overrides(
            node,
            &Style {
                size: Some(reprise_doc::LengthExpr::Pt(size)),
                color: Some([30, 60, 90, 0]),
                decoration: styled().decoration,
                ..Default::default()
            },
        )
        .unwrap();
        doc.commit();
        let layout = engine.layout(&doc);
        assert_eq!(layout, engine.layout(&doc));
        let list = layout.to_display_list(0, DisplayOptions::default());
        assert!(!list.items.is_empty());
        if size <= Length::ZERO || text == "\u{200b}" {
            fn count(items: &[reprise_display::Item]) -> usize {
                items
                    .iter()
                    .map(|i| match i {
                        reprise_display::Item::Path {
                            layer: reprise_display::Layer::Content,
                            ..
                        } => 1,
                        reprise_display::Item::Group { items, .. } => count(items),
                        _ => 0,
                    })
                    .sum()
            }
            assert_eq!(count(&list.items), 0);
        }
    }
}

#[test]
fn decorated_repeated_headers_keep_pdf_artifact_and_reading_addresses() {
    let fixture = reprise_fixtures::hostile::table_header_repeats().unwrap();
    let before = fixture.engine.layout(&fixture.doc);
    assert!(!before.repeated_headers.is_empty());
    let nodes: std::collections::BTreeSet<_> = before
        .repeated_headers
        .iter()
        .flat_map(|h| h.blocks.iter().map(|b| b.node))
        .collect();
    for node in nodes {
        let len = fixture.doc.block(node).unwrap().text.len();
        if len > 0 {
            fixture
                .doc
                .format_text(
                    node,
                    0..len,
                    &TextStyle {
                        weight: None,
                        slant: None,
                        ..styled()
                    },
                    RangePolicy::FIXED,
                )
                .unwrap();
        }
    }
    fixture.doc.commit();
    let layout = fixture.engine.layout(&fixture.doc);
    let lists = layout.to_display_lists(DisplayOptions::default());
    fn at<'a>(items: &'a [reprise_display::Item], path: &[usize]) -> &'a reprise_display::Item {
        let item = &items[path[0]];
        if path.len() == 1 {
            item
        } else {
            let reprise_display::Item::Group { items, .. } = item else {
                panic!("invalid group")
            };
            at(items, &path[1..])
        }
    }
    let artifacts = layout.pdf_artifact_runs();
    assert!(!artifacts.is_empty());
    for address in &artifacts {
        let reprise_display::Item::Glyphs(g) = at(&lists[address.page].items, &address.path) else {
            panic!("artifact points at a decoration")
        };
        assert_eq!(g.color, reprise_display::Color(30, 60, 90, 128));
    }
    let order = layout.pdf_reading_order(&fixture.doc);
    for address in &order {
        assert!(matches!(
            at(&lists[address.page].items, &address.path),
            reprise_display::Item::Glyphs(_)
        ));
    }
    use reprise_display::pdf::tags::{Child, Content, Node, Role, Structure};
    let structure = Structure {
        children: vec![Node::new(
            Role::P,
            order
                .into_iter()
                .map(|r| Child::Content(Content::whole(r)))
                .collect(),
        )],
        artifacts,
        ..Default::default()
    };
    reprise_display::pdf::render_tagged(
        &lists,
        &fixture.engine.fonts,
        &fixture.engine.assets,
        &structure,
    )
    .unwrap();
}
