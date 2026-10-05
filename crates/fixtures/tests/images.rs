use reprise_doc::image::ImageData;
use reprise_doc::{BlockKind, Dim, Document, FrameRole, LengthExpr, PageTemplate, Style};
use reprise_fixtures::{engine, hostile, templates::flow_frame};
use reprise_geom::Length;
use reprise_layout::{DisplayOptions, incremental::LayoutSession};

const RED: &[u8] = include_bytes!("../../../fixtures/images/red-1x1.png");

#[test]
fn image_box_queries_reading_order_and_region_geometry_agree() {
    for fixture in [
        hostile::image_float().unwrap(),
        hostile::image_note().unwrap(),
        hostile::image_rotated().unwrap(),
        hostile::image_vertical().unwrap(),
    ] {
        let snapshot = fixture.engine.layout(&fixture.doc);
        let image = snapshot.blocks.iter().find(|b| b.image.is_some()).unwrap();
        let placed = image.image.as_ref().unwrap();
        assert_eq!(image.lines.len(), 1);
        assert!(image.lines[0].runs.is_empty(), "alt text is not shaped");
        assert_eq!(image.lines[0].rect, placed.rect);
        assert_eq!(image.lines[0].frame, placed.frame);
        assert_eq!(
            snapshot.frame_of(snapshot.first_line(image.node).unwrap()),
            Some(placed.frame)
        );
        assert!(
            snapshot
                .reading_order(&fixture.doc)
                .iter()
                .any(|s| s.line.node == image.node)
        );
        assert!(snapshot.relations.iter().all(|r| r.applied));
        let mut session = LayoutSession::new(&fixture.engine);
        assert_eq!(session.layout(&fixture.doc).unwrap(), snapshot);
        assert_eq!(session.layout(&fixture.doc).unwrap(), snapshot);
        if fixture.name == "image_float" {
            assert_eq!(
                snapshot.frames[placed.frame].role,
                FrameRole::Flow("main".into())
            );
            let body = snapshot
                .blocks
                .iter()
                .find(|b| b.kind == BlockKind::Paragraph)
                .unwrap();
            assert!(
                body.lines
                    .iter()
                    .filter(|l| l.frame == placed.frame && l.rect.origin.y < placed.rect.max_y())
                    .all(|l| l.rect.max_x() <= placed.rect.origin.x),
                "runaround avoids right image float"
            );
        }
    }
}

#[test]
fn authored_dimensions_and_header_sizes_are_integer_and_fonts_are_optional() {
    let mut engine = engine();
    let hash = engine.assets.insert(RED).unwrap();
    let doc = Document::new(1).unwrap();
    let node = doc
        .append_image("", &ImageData::new(hash), "not shaped")
        .unwrap();
    doc.define_style(
        "",
        &Style {
            family: Some("missing font".into()),
            ..Default::default()
        },
    )
    .unwrap();
    doc.commit();
    let snapshot = engine.layout(&doc);
    assert_eq!(
        snapshot
            .block(node)
            .unwrap()
            .image
            .as_ref()
            .unwrap()
            .rect
            .width,
        Length(768)
    );
    assert!(
        !snapshot
            .diagnostics
            .iter()
            .any(|d| d.code.as_str() == "font.missing")
    );
    let mut session = LayoutSession::new(&engine);
    session.layout(&doc).unwrap();
    for (width, height) in [
        (Some(Length::from_pt(30)), None),
        (None, Some(Length::from_pt(20))),
        (Some(Length::ZERO), None),
        (Some(Length::MIN), Some(Length::MAX)),
        (Some(Length::MAX), Some(Length::MAX)),
    ] {
        let mut image = doc.image(node).unwrap();
        image.width = width.map(LengthExpr::Pt);
        image.height = height.map(LengthExpr::Pt);
        doc.set_image(node, &image).unwrap();
        doc.commit();
        let snapshot = engine.layout(&doc);
        assert_eq!(
            session.layout(&doc).unwrap(),
            snapshot,
            "image metadata invalidates caches"
        );
        let rect = snapshot.block(node).unwrap().image.as_ref().unwrap().rect;
        assert!(rect.width >= Length::ZERO && rect.height >= Length::ZERO);
        if width == Some(Length::from_pt(30)) {
            assert_eq!(rect.height, Length::from_pt(30));
        }
        if height == Some(Length::from_pt(20)) {
            assert_eq!(rect.width, Length::from_pt(20));
        }
        if width == Some(Length::MIN) {
            assert!(
                snapshot
                    .diagnostics_with("layout.image-size")
                    .next()
                    .is_some()
            );
        }
    }
}

#[test]
fn tall_images_advance_then_overflow_and_page_limits_leave_following_content_diagnosed() {
    let mut engine = engine();
    let hash = engine.assets.insert(RED).unwrap();
    let doc = Document::new(1).unwrap();
    doc.define_page_template(
        &PageTemplate::new("tiny", Dim::pt(100), Dim::pt(100)).with_frame(flow_frame(
            "body",
            Dim::pt(0),
            Dim::pt(0),
            Dim::pt(100),
            Dim::pt(40),
        )),
    )
    .unwrap();
    doc.use_page_template("tiny").unwrap();
    doc.append_block(BlockKind::Paragraph, "", "Before")
        .unwrap();
    let mut image = ImageData::new(hash);
    image.width = Some(LengthExpr::Pt(Length::from_pt(20)));
    image.height = Some(LengthExpr::Pt(Length::from_pt(60)));
    let node = doc.append_image("", &image, "Tall image").unwrap();
    doc.append_block(BlockKind::Paragraph, "", "After").unwrap();
    doc.commit();
    let snapshot = engine.layout(&doc);
    let placed = snapshot.block(node).unwrap().image.as_ref().unwrap();
    assert_eq!(
        snapshot.frames[placed.frame].page, 1,
        "advance past occupied frame before overflowing"
    );
    assert_eq!(placed.rect.origin.y, Length::ZERO);
    assert!(
        snapshot
            .diagnostics_with("layout.frame-overflow")
            .next()
            .is_some()
    );
    engine.flow.max_pages = 1;
    let snapshot = engine.layout(&doc);
    assert!(snapshot.block(node).is_none());
    assert!(
        snapshot
            .diagnostics_with("layout.page-limit")
            .next()
            .is_some()
    );
    assert!(
        snapshot
            .diagnostics_with("layout.text-unplaced")
            .next()
            .is_some()
    );
    assert!(
        !snapshot
            .to_display_lists(DisplayOptions::default())
            .is_empty()
    );
}

#[test]
fn unsupported_image_record_is_retained_and_a_later_block_still_flows() {
    let fixture = hostile::image_unreadable().unwrap();
    let doc = &fixture.doc;
    let node = doc.blocks()[0];
    let raw = doc.image_record(node).unwrap();
    let other = doc
        .append_block(BlockKind::Paragraph, "body", "After")
        .unwrap();
    doc.commit();
    let snapshot = fixture.engine.layout(doc);
    assert!(snapshot.block(other).is_some());
    assert!(
        snapshot
            .block(node)
            .unwrap()
            .image
            .as_ref()
            .unwrap()
            .placeholder
    );
    assert_eq!(doc.image_record(node).unwrap(), raw);
    let fork = doc.fork(2).unwrap();
    assert_eq!(fork.image_record(node).unwrap(), raw);
}

#[test]
fn image_cells_use_image_height_and_do_not_loop_on_oversized_content() {
    let mut engine = engine();
    let hash = engine.assets.insert(RED).unwrap();
    let doc = Document::new(1).unwrap();
    doc.define_page_template(
        &PageTemplate::new("tiny", Dim::pt(100), Dim::pt(100)).with_frame(flow_frame(
            "body",
            Dim::pt(0),
            Dim::pt(0),
            Dim::pt(100),
            Dim::pt(40),
        )),
    )
    .unwrap();
    doc.use_page_template("tiny").unwrap();
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
        .append_cell_block(cell, BlockKind::Image, "", "Tall cell image")
        .unwrap();
    let mut data = ImageData::new(hash);
    data.width = Some(LengthExpr::Pt(Length::from_pt(20)));
    data.height = Some(LengthExpr::Pt(Length::from_pt(60)));
    doc.set_image(node, &data).unwrap();
    doc.commit();
    let layout = engine.layout(&doc);
    let image = layout.block(node).unwrap().image.as_ref().unwrap();
    assert_eq!(image.rect.height, Length::from_pt(60));
    assert!(
        layout
            .diagnostics_with("layout.frame-overflow")
            .next()
            .is_some()
    );
    assert!(layout.pages.len() <= 2);
}
