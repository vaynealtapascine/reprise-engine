use reprise_doc::page_setup::PageSetupPatch;
use reprise_doc::{BlockKind, Document};
use reprise_geom::Length;
use reprise_layout::incremental::LayoutSession;

#[test]
fn long_text_reflows_without_loss_and_incremental_matches_full_after_each_page_change() {
    let doc = Document::new(1).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    let text = reprise_fixtures::templates::long_text(16);
    let node = doc
        .append_block(BlockKind::Paragraph, "body", &text)
        .unwrap();
    doc.commit();
    let engine = reprise_fixtures::engine();
    let mut incremental = LayoutSession::new(&engine);
    let original = incremental.layout(&doc).unwrap();
    let revision = doc.revision();
    let info = engine.page_setup(&doc);
    assert_eq!(
        doc.revision(),
        revision,
        "state query must not author anything"
    );
    assert!(!doc.has_page_setup());
    assert_eq!(info.dimensions.width, original.pages[0].width);
    for (width, height) in [(612, 792), (792, 612), (300, 240)] {
        doc.set_page_setup_patch(PageSetupPatch {
            width: Some(Length::from_pt(width)),
            height: Some(Length::from_pt(height)),
            top: Some(Length::from_pt(36)),
            right: Some(Length::from_pt(36)),
            bottom: Some(Length::from_pt(36)),
            left: Some(Length::from_pt(36)),
        })
        .unwrap();
        doc.commit();
        let cached = incremental.layout(&doc).unwrap();
        let full = engine.layout(&doc);
        assert_eq!(cached.to_json(), full.to_json());
        assert_eq!(doc.block(node).unwrap().text.to_string(), text);
        let block = full.blocks.iter().find(|b| b.node == node).unwrap();
        assert_eq!(block.text, text);
        assert_eq!(block.lines.last().unwrap().text.end, text.len());
        assert!(
            !full
                .diagnostics
                .iter()
                .any(|d| d.severity == reprise_diag::Severity::Error)
        );
        let page = engine.page_setup(&doc);
        assert_eq!(page.dimensions.width, full.pages[0].width);
        assert_eq!(page.dimensions.height, full.pages[0].height);
        if width == 612 {
            assert!(full.pages.len() < original.pages.len());
        }
    }
}

#[test]
fn tiny_pages_overflow_or_stop_at_the_explicit_page_limit() {
    let doc = Document::new(1).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    doc.append_block(BlockKind::Paragraph, "body", &"a ".repeat(64))
        .unwrap();
    doc.set_page_setup_patch(PageSetupPatch {
        width: Some(Length(1)),
        height: Some(Length(1)),
        top: Some(Length::ZERO),
        right: Some(Length::ZERO),
        bottom: Some(Length::ZERO),
        left: Some(Length::ZERO),
    })
    .unwrap();
    doc.commit();
    let mut engine = reprise_fixtures::engine();
    engine.flow.max_pages = 2;
    let snapshot = engine.layout(&doc);
    assert!(snapshot.pages.len() <= 2);

    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|d| d.code.as_str() == "layout.frame-overflow")
    );
    // A frame shorter than every possible line deliberately overflows in
    // place. A small frame that fits a line must instead paginate and obey
    // the explicit page limit, leaving the remaining text diagnosed.
    doc.set_page_setup_patch(PageSetupPatch {
        width: Some(Length::from_pt(40)),
        height: Some(Length::from_pt(24)),
        ..Default::default()
    })
    .unwrap();
    doc.commit();
    let paginated = engine.layout(&doc);
    assert!(paginated.pages.len() <= 2);
    assert!(
        paginated
            .diagnostics
            .iter()
            .any(|d| d.code.as_str() == "layout.page-limit")
    );
}

#[test]
fn page_state_reports_physical_margins_before_vertical_axes_and_path_expansion() {
    let doc = Document::new(1).unwrap();
    let mut template = reprise_doc::PageTemplate::builtin();
    template.name = "vertical".into();
    template.frames[0].writing_mode = reprise_doc::WritingMode::VerticalRl;
    template.frames[0].transform.mirror_x = true;
    doc.set_page_template(&template).unwrap();
    doc.set_page_setup_patch(PageSetupPatch {
        width: Some(Length::from_pt(612)),
        height: Some(Length::from_pt(792)),
        top: Some(Length::from_pt(36)),
        right: Some(Length::from_pt(48)),
        bottom: Some(Length::from_pt(54)),
        left: Some(Length::from_pt(72)),
    })
    .unwrap();
    doc.commit();
    let engine = reprise_fixtures::engine();
    let info = engine.page_setup(&doc);
    assert_eq!(info.template, "vertical");
    assert_eq!(info.dimensions.right, Length::from_pt(48));
    assert_eq!(info.dimensions.bottom, Length::from_pt(54));
    assert!(info.patched);
    assert_eq!(engine.layout(&doc).pages[0].width, info.dimensions.width);
    template.frames[0].writing_mode = reprise_doc::WritingMode::HorizontalTb;
    template.frames[0].path = Some(reprise_doc::Spiral {
        radius: reprise_doc::Dim::pt(20),
        growth: reprise_doc::Dim::pt(10),
        start_millidegrees: 0,
        sweep_millidegrees: 360_000,
        segments: 32,
    });
    doc.set_page_template(&template).unwrap();
    doc.commit();
    let spiral = engine.page_setup(&doc);
    assert_eq!(spiral.dimensions, info.dimensions);
}
