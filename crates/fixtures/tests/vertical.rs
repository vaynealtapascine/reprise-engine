//! Vertical shaping, display compensation, editing and export share one layout.
use reprise_doc::{BlockKind, Dim, Document, PageTemplate, Style, WritingMode};
use reprise_geom::{Length, TextCombineUpright};
use reprise_layout::{DisplayOptions, Engine, LineRef};

fn fixture(mode: WritingMode) -> (Document, Engine, reprise_doc::NodeId) {
    let mut fonts = reprise_fixtures::fonts();
    fonts.add(
        reprise_font::Face::from_bytes(
            include_bytes!("../../../fixtures/fonts/NotoSansJP-VerticalSubset.otf").as_slice(),
        )
        .unwrap(),
    );
    let engine = Engine::new(fonts);
    let doc = Document::new(1).unwrap();
    let mut frame = reprise_fixtures::templates::flow_frame(
        "column",
        Dim::pt(20),
        Dim::pt(20),
        Dim::pt(120),
        Dim::pt(200),
    );
    frame.writing_mode = mode;
    doc.set_page_template(
        &PageTemplate::new("vertical", Dim::pt(180), Dim::pt(250)).with_frame(frame),
    )
    .unwrap();
    let node = doc
        .append_block(BlockKind::Paragraph, "", "日本語 12 abc 12345 日本語")
        .unwrap();
    doc.set_overrides(
        node,
        &Style {
            families: Some(vec!["Noto Sans JP".into(), "serif".into()]),
            text_combine_upright: Some(TextCombineUpright::Digits(2)),
            ..Style::default()
        },
    )
    .unwrap();
    (doc, engine, node)
}

#[test]
fn upright_cjk_combined_digits_and_sideways_latin_render_in_both_column_directions() {
    for mode in [WritingMode::VerticalRl, WritingMode::VerticalLr] {
        let (doc, engine, node) = fixture(mode);
        let layout = engine.layout(&doc);
        let block = layout.block(node).unwrap();
        let runs: Vec<_> = block.lines.iter().flat_map(|l| &l.runs).collect();
        assert!(runs.iter().any(|r| r.upright && !r.combined));
        assert!(runs.iter().any(|r| !r.upright));
        let combined: Vec<_> = runs.iter().filter(|r| r.combined).collect();
        assert_eq!(combined.len(), 1);
        assert_eq!(&block.text[combined[0].range.clone()], "12");
        assert_eq!(combined[0].width, combined[0].size);
        assert!(combined[0].horizontal_scale.0 > 0);
        for (index, line) in block.lines.iter().enumerate() {
            let at = LineRef { node, line: index };
            let transform = layout.line_to_page(at).unwrap();
            assert!(transform.inverse().is_some());
            for run in &line.runs {
                assert!(run.width >= Length::ZERO);
            }
        }
        let navigator = reprise_edit::Navigator::new(&layout, [node]);
        let range = &combined[0].range;
        assert!(
            navigator
                .caret_positions(node)
                .iter()
                .all(|c| c.offset <= range.start || c.offset >= range.end)
        );
        for fraction in [1, 3] {
            let line_index = block
                .lines
                .iter()
                .position(|l| l.runs.iter().any(|r| r.combined))
                .unwrap();
            let run = block.lines[line_index]
                .runs
                .iter()
                .find(|r| r.combined)
                .unwrap();
            let caret = navigator
                .caret_at_x(
                    LineRef {
                        node,
                        line: line_index,
                    },
                    run.x + run.width.mul_ratio(fraction, 4),
                )
                .unwrap();
            assert_eq!(
                caret.offset,
                if fraction == 1 {
                    range.start
                } else {
                    range.end
                }
            );
        }

        for caret in navigator.caret_positions(node) {
            let rect = navigator.caret_rect(caret).unwrap();
            let hit = navigator.hit(rect.page, rect.point()).unwrap();
            let round = navigator.caret_rect(hit.caret).unwrap();
            assert_eq!(rect.page, round.page);
            assert_eq!(
                rect.rect, round.rect,
                "vertical caret/hit round trip at {caret:?}"
            );
        }
        let list = layout.to_display_list(0, DisplayOptions::default());
        let svg = reprise_display::svg::render(&list, &engine.fonts).unwrap();
        let png = reprise_display::png::render(&list, &engine.fonts, 2.0).unwrap();
        assert!(!svg.is_empty() && !png.is_empty());
        assert_eq!(
            png,
            reprise_display::png::render(&list, &engine.fonts, 2.0).unwrap()
        );
        let reading = layout.pdf_reading_order(&doc);
        assert!(!reading.is_empty());
        reprise_display::pdf::render_ordered(&[list], &engine.fonts, &reading).unwrap();
        if let Some(directory) = std::env::var_os("REPRISE_VERTICAL_PREVIEW") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(format!("{mode:?}.png")), png).unwrap();
            std::fs::write(directory.join(format!("{mode:?}.svg")), svg).unwrap();
        }
    }
}

#[test]
fn template_modes_survive_storage_and_incremental_layout() {
    for mode in [
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
        WritingMode::SidewaysLr,
    ] {
        let (doc, engine, _) = fixture(mode);
        let saved = doc
            .try_export(reprise_doc::PersistenceMode::History)
            .unwrap();
        let reopened = Document::import(&saved, 2).unwrap();
        let expected = engine.layout(&doc);
        assert_eq!(
            serde_json::to_value(&expected).unwrap(),
            serde_json::to_value(engine.layout(&reopened)).unwrap()
        );
        let mut session = reprise_layout::incremental::LayoutSession::new(&engine);
        let mut job = session.start(
            &reopened,
            reprise_layout::incremental::Viewport::Pages(0..1),
        );
        for _ in 0..10000 {
            if job.step(1).unwrap().complete {
                break;
            }
        }
        assert_eq!(
            serde_json::to_value(&expected).unwrap(),
            serde_json::to_value(job.partial().unwrap().snapshot()).unwrap()
        );
    }
}

#[test]
fn paragraph_continuation_reshapes_for_the_actual_frame_mode() {
    let (doc, engine, node) = fixture(WritingMode::VerticalRl);
    doc.block(node)
        .unwrap()
        .text
        .insert(0, &"日本語".repeat(40))
        .unwrap();
    let horizontal = reprise_fixtures::templates::flow_frame(
        "horizontal",
        Dim::pt(10),
        Dim::pt(10),
        Dim::pt(100),
        Dim::pt(24),
    );
    let mut vertical = reprise_fixtures::templates::flow_frame(
        "vertical",
        Dim::pt(130),
        Dim::pt(10),
        Dim::pt(120),
        Dim::pt(200),
    );
    vertical.writing_mode = WritingMode::VerticalLr;
    doc.set_page_template(
        &PageTemplate::new("mixed", Dim::pt(280), Dim::pt(250))
            .with_frame(horizontal)
            .with_frame(vertical),
    )
    .unwrap();
    let expected = engine.layout(&doc);
    let lines = &expected.block(node).unwrap().lines;
    assert!(
        lines
            .iter()
            .any(
                |l| expected.frame(l.frame).unwrap().writing_mode == WritingMode::HorizontalTb
                    && l.runs.iter().all(|r| !r.upright)
            )
    );
    assert!(
        lines
            .iter()
            .any(
                |l| expected.frame(l.frame).unwrap().writing_mode == WritingMode::VerticalLr
                    && l.runs.iter().any(|r| r.upright)
            )
    );
    let mut session = reprise_layout::incremental::LayoutSession::new(&engine);
    let mut job = session.start(&doc, reprise_layout::incremental::Viewport::Pages(0..2));
    for _ in 0..10000 {
        if job.step(1).unwrap().complete {
            break;
        }
    }
    assert_eq!(
        serde_json::to_value(&expected).unwrap(),
        serde_json::to_value(job.partial().unwrap().snapshot()).unwrap()
    );
}

#[test]
fn explicit_upright_makes_rtl_characters_strong_ltr() {
    let (doc, engine, node) = fixture(WritingMode::VerticalRl);
    let text = doc.block(node).unwrap().text;
    text.delete(0..text.len()).unwrap();
    text.insert(0, "אבג العربية abc").unwrap();
    doc.set_overrides(
        node,
        &Style {
            text_orientation: Some(reprise_geom::TextOrientation::Upright),
            ..Style::default()
        },
    )
    .unwrap();
    let layout = engine.layout(&doc);
    for line in &layout.block(node).unwrap().lines {
        assert!(line.runs.iter().all(|r| r.level == 0 && r.upright));
    }
}

#[test]
fn table_continuation_reshapes_for_the_actual_frame_mode() {
    let (doc, engine, _) = fixture(WritingMode::VerticalRl);
    let horizontal = reprise_fixtures::templates::flow_frame(
        "horizontal",
        Dim::pt(10),
        Dim::pt(10),
        Dim::pt(100),
        Dim::pt(24),
    );
    let mut vertical = reprise_fixtures::templates::flow_frame(
        "vertical",
        Dim::pt(130),
        Dim::pt(10),
        Dim::pt(120),
        Dim::pt(200),
    );
    vertical.writing_mode = WritingMode::VerticalLr;
    doc.set_page_template(
        &PageTemplate::new("mixed", Dim::pt(280), Dim::pt(250))
            .with_frame(horizontal)
            .with_frame(vertical),
    )
    .unwrap();
    let table = doc
        .append_table(reprise_doc::TableColumns {
            columns: vec![reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Proportional(1),
            }],
        })
        .unwrap();
    let row = doc.append_table_row(table, false).unwrap();
    let cell = doc.append_table_cell(row, 0).unwrap();
    let node = doc
        .append_cell_block(cell, BlockKind::Paragraph, "", &"日本語".repeat(80))
        .unwrap();
    doc.set_overrides(
        node,
        &Style {
            families: Some(vec!["Noto Sans JP".into()]),
            ..Style::default()
        },
    )
    .unwrap();
    let expected = engine.layout(&doc);
    let lines = &expected.block(node).unwrap().lines;
    assert!(
        lines
            .iter()
            .any(|l| expected.frame(l.frame).unwrap().writing_mode == WritingMode::VerticalLr)
    );
    for line in lines {
        let vertical = expected.frame(line.frame).unwrap().writing_mode == WritingMode::VerticalLr;
        assert!(line.runs.iter().all(|r| r.upright == vertical));
    }
    let mut session = reprise_layout::incremental::LayoutSession::new(&engine);
    let mut job = session.start(&doc, reprise_layout::incremental::Viewport::Pages(0..3));
    for _ in 0..10000 {
        if job.step(1).unwrap().complete {
            break;
        }
    }
    assert_eq!(
        serde_json::to_value(&expected).unwrap(),
        serde_json::to_value(job.partial().unwrap().snapshot()).unwrap()
    );
}
