//! Frame transforms, path reflow, and accessible reading order (20, 33).
use reprise_doc::text::RangePolicy;
use reprise_doc::{BlockKind, Dim, Document, PageTemplate, Rotation, SchemaRegistry, WritingMode};
use reprise_fixtures::spike::{define_styles, follow};
use reprise_fixtures::templates::{flow_frame, margin_frame};
use reprise_fixtures::{PEER, engine, hostile};
use reprise_geom::{Length, LineSpace, Point};
use reprise_layout::{DisplayOptions, LineRef};

#[test]
fn transformed_line_carets_round_trip_and_bounds_cover_all_corners() {
    for fixture in [
        hostile::transformed_rtl().unwrap(),
        hostile::vertical_rl().unwrap(),
        hostile::spiral_text().unwrap(),
        hostile::rational_rotation_extreme().unwrap(),
    ] {
        let snapshot = fixture.engine.layout(&fixture.doc);
        for block in &snapshot.blocks {
            for (i, line) in block.lines.iter().enumerate() {
                let at = LineRef {
                    node: block.node,
                    line: i,
                };
                let to = snapshot.line_to_page(at).unwrap();
                let back = to.inverse().unwrap();
                for p in [
                    Point::<LineSpace>::origin(),
                    Point::new(line.width, Length::ZERO),
                    Point::new(line.width.mul_ratio(1, 2), Length::ZERO),
                ] {
                    let round = back.apply(to.apply(p));
                    assert!(
                        (round.x.0 as i64 - p.x.0 as i64).abs() <= 4,
                        "{}",
                        fixture.name
                    );
                    assert!(
                        (round.y.0 as i64 - p.y.0 as i64).abs() <= 4,
                        "{}",
                        fixture.name
                    );
                }
                let (_, bounds) = snapshot.line_bounds(at).unwrap();
                let frame = snapshot.frame(line.frame).unwrap();
                for (x, y) in [
                    (line.rect.origin.x, line.rect.origin.y),
                    (line.rect.max_x(), line.rect.origin.y),
                    (line.rect.origin.x, line.rect.max_y()),
                    (line.rect.max_x(), line.rect.max_y()),
                ] {
                    let p = frame.to_page.apply(Point::new(x, y));
                    assert!(bounds.origin.x <= p.x && p.x <= bounds.max_x());
                    assert!(bounds.origin.y <= p.y && p.y <= bounds.max_y());
                }
            }
        }
    }
}

#[test]
fn exact_turns_both_mirrors_and_rational_rotation_preserve_composition() {
    let doc = Document::new(PEER).unwrap();
    define_styles(&doc).unwrap();
    let node = doc
        .append_block(
            BlockKind::Paragraph,
            "body",
            "The same room keeps the same line breaks after rotation.",
        )
        .unwrap();
    let baseline = engine()
        .layout(&doc)
        .block(node)
        .unwrap()
        .lines
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>();
    for rotation in [
        Rotation::Quarter(1),
        Rotation::Quarter(2),
        Rotation::Quarter(3),
        Rotation::Direction { dx: 3, dy: 4 },
    ] {
        for (mirror_x, mirror_y) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut t = PageTemplate::builtin();
            let f = &mut t.frames[0];
            f.transform.rotation = rotation;
            f.transform.mirror_x = mirror_x;
            f.transform.mirror_y = mirror_y;
            f.transform.origin_x = Dim::pt(110);
            f.transform.origin_y = Dim::pt(40);
            doc.set_page_template(&t).unwrap();
            doc.commit();
            let s = engine().layout(&doc);
            assert_eq!(
                s.block(node)
                    .unwrap()
                    .lines
                    .iter()
                    .map(|l| l.text.clone())
                    .collect::<Vec<_>>(),
                baseline
            );
            assert!(s.frames[0].to_page.inverse().is_some());
        }
    }
}

#[test]
fn follow_places_notes_in_rotated_margin_logical_space() {
    let doc = Document::new(PEER).unwrap();
    define_styles(&doc).unwrap();
    let mut main = flow_frame(
        "main",
        Dim::pt(150),
        Dim::pt(20),
        Dim::pt(140),
        Dim::pt(100),
    );
    main.transform.rotation = Rotation::Quarter(1);
    let mut margin = margin_frame(
        "margin",
        Dim::pt(120),
        Dim::pt(20),
        Dim::pt(100),
        Dim::pt(100),
    );
    margin.transform.rotation = Rotation::Quarter(1);
    doc.set_page_template(
        &PageTemplate::new("rotated-follow", Dim::pt(220), Dim::pt(220))
            .with_frame(main)
            .with_frame(margin),
    )
    .unwrap();
    let p = doc
        .append_block(
            BlockKind::Paragraph,
            "body",
            "This line turns towards a note. Then the second line follows it.",
        )
        .unwrap();
    let n = doc
        .append_block(BlockKind::Annotation, "note", "A note turns too.")
        .unwrap();
    let range = doc.add_range(p, 0..4, RangePolicy::FIXED).unwrap();
    doc.add_relation(&SchemaRegistry::builtin(), &follow(n, range))
        .unwrap();
    doc.commit();
    let s = engine().layout(&doc);
    assert!(s.relations[0].applied, "{:?}", s.diagnostics);
    let source = &s.block(p).unwrap().lines[0];
    let note = &s.block(n).unwrap().lines[0];
    let p = s.frames[source.frame].to_page.apply(source.rect.origin);
    let expected = s.frames[note.frame].to_page.inverse().unwrap().apply(p).y;
    assert_eq!(note.rect.origin.y, expected);
}

#[test]
fn spiral_reflow_reassigns_bytes_to_known_frames_without_hidden_transforms() {
    let f = hostile::spiral_text().unwrap();
    let before = f.engine.layout(&f.doc);
    let node = before.blocks[0].node;
    assert!(
        before.blocks[0].lines.len() > 96,
        "covers all three turns and continues on another page"
    );
    assert!(
        before.blocks[0]
            .lines
            .iter()
            .all(|l| l.rect.origin.y == Length::ZERO)
    );
    let old = before.blocks[0].lines[50].text.clone();
    f.doc
        .block(node)
        .unwrap()
        .text
        .insert(0, "a door a door a door ")
        .unwrap();
    f.doc.commit();
    let after = f.engine.layout(&f.doc);
    assert_ne!(after.blocks[0].lines[50].text, old);
    assert_eq!(before.frames[0].to_page, after.frames[0].to_page);
    assert_eq!(
        after.reading_order(&f.doc).len(),
        after.blocks[0].lines.len()
    );
    let end = after.blocks[0].lines.last().unwrap().text.end;
    assert_eq!(end, after.blocks[0].text.len());
}

#[test]
fn reading_order_uses_document_order_for_annotations_and_completes_partial_orders() {
    let doc = Document::new(PEER).unwrap();
    define_styles(&doc).unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "body", "A.")
        .unwrap();
    let note = doc
        .append_block(BlockKind::Annotation, "note", "Note.")
        .unwrap();
    let b = doc
        .append_block(BlockKind::Paragraph, "body", "B.")
        .unwrap();
    let range = doc.add_range(a, 0..1, RangePolicy::FIXED).unwrap();
    doc.add_relation(&SchemaRegistry::builtin(), &follow(note, range))
        .unwrap();
    doc.commit();
    let s = engine().layout(&doc);
    assert_eq!(
        s.blocks.iter().map(|b| b.node).collect::<Vec<_>>(),
        [a, b, note]
    );
    assert_eq!(
        s.reading_order(&doc)
            .iter()
            .map(|s| s.line.node)
            .collect::<Vec<_>>(),
        [a, note, b]
    );
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &reprise_doc::reading::before(b, a),
    )
    .unwrap();
    doc.commit();
    let s = engine().layout(&doc);
    let order = s.reading_order(&doc);
    assert_eq!(
        order.iter().map(|s| s.line.node).collect::<Vec<_>>(),
        [note, b, a]
    );
    assert!(
        s.diagnostics_with("layout.reading-partial")
            .next()
            .is_some()
    );
    doc.block(b).unwrap().text.insert(0, "changed ").unwrap();
    doc.commit();
    assert!(
        s.reading_order_report(&doc)
            .diagnostics
            .iter()
            .any(|d| d.code == "layout.reading-revision")
    );
}

#[test]
fn cycles_missing_endpoints_and_downstream_nodes_have_total_deterministic_order() {
    let f = hostile::reading_cycle().unwrap();
    let mut s = f.engine.layout(&f.doc);
    let expected = s.reading_order(&f.doc);
    assert_eq!(expected.len(), 3);
    assert_eq!(expected, s.reading_order(&f.doc));
    s.relations[0].targets[0].resolved = None;
    assert!(
        s.reading_order_report(&f.doc)
            .diagnostics
            .iter()
            .any(|d| d.code == "layout.reading-missing")
    );
    let doc = Document::new(PEER).unwrap();
    let downstream = doc
        .append_block(BlockKind::Paragraph, "", "Downstream.")
        .unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "", "Cycle A.")
        .unwrap();
    let b = doc
        .append_block(BlockKind::Paragraph, "", "Cycle B.")
        .unwrap();
    for (a, b) in [(a, b), (b, a), (a, downstream)] {
        doc.add_relation(
            &SchemaRegistry::builtin(),
            &reprise_doc::reading::before(a, b),
        )
        .unwrap();
    }
    doc.commit();
    let s = engine().layout(&doc);
    let order = s.reading_order(&doc);
    assert_eq!(
        order[0].line.node, a,
        "break actual cycle, not earlier-ranked downstream block"
    );
}

#[test]
fn vertical_lr_uses_sideways_latin_and_physical_sizes() {
    let doc = Document::new(PEER).unwrap();
    let mut f = flow_frame("lr", Dim::pt(0), Dim::pt(0), Dim::pt(80), Dim::pt(220));
    f.writing_mode = WritingMode::VerticalLr;
    doc.set_page_template(&PageTemplate::new("lr", Dim::pt(100), Dim::pt(240)).with_frame(f))
        .unwrap();
    doc.append_block(BlockKind::Paragraph, "", "Sideways Latin.")
        .unwrap();
    doc.commit();
    let s = engine().layout(&doc);
    assert_eq!(
        (s.frames[0].rect.width, s.frames[0].rect.height),
        (Length::from_pt(220), Length::from_pt(80))
    );
}

/// Renders the actual SVG fixtures and matching PNG previews for visual QA.
#[test]
#[ignore = "writes visual inspection artifacts to the OS temp directory"]
fn export_geometry_previews() {
    let dir = std::env::temp_dir().join("reprise-ws5-geometry");
    std::fs::create_dir_all(&dir).unwrap();
    for f in [
        hostile::transformed_rtl().unwrap(),
        hostile::vertical_rl().unwrap(),
        hostile::spiral_text().unwrap(),
        hostile::reading_cycle().unwrap(),
        hostile::degenerate_transform().unwrap(),
        hostile::rational_rotation_extreme().unwrap(),
    ] {
        let s = f.engine.layout(&f.doc);
        let list = s.to_display_list(0, DisplayOptions::default());
        let svg = reprise_display::svg::render(&list, &f.engine.fonts).unwrap();
        std::fs::write(dir.join(format!("{}.svg", f.name)), svg).unwrap();
        let png = reprise_display::png::render(&list, &f.engine.fonts, 3.0).unwrap();
        std::fs::write(dir.join(format!("{}.png", f.name)), png).unwrap();
    }
    println!("{}", dir.display());
}

#[test]
fn excessive_reading_constraints_report_and_preserve_every_step() {
    use reprise_layout::{RelationLayout, RelationStatus, Resolution, TargetLayout};
    let doc = Document::new(PEER).unwrap();
    let nodes: Vec<_> = (0..100)
        .map(|_| doc.append_block(BlockKind::Paragraph, "", "room").unwrap())
        .collect();
    let id = doc
        .add_relation(
            &SchemaRegistry::builtin(),
            &reprise_doc::reading::before(nodes[0], nodes[1]),
        )
        .unwrap();
    doc.commit();
    let mut s = engine().layout(&doc);
    s.relations.clear();
    for (i, &a) in nodes.iter().enumerate() {
        for &b in nodes.iter().skip(i + 1) {
            s.relations.push(RelationLayout {
                id,
                schema: reprise_doc::reading::READING_ORDER,
                owner: None,
                status: RelationStatus::Valid,
                applied: true,
                targets: vec![
                    TargetLayout {
                        role: "before".into(),
                        status: RelationStatus::Valid,
                        resolved: Some(Resolution::Node(a)),
                    },
                    TargetLayout {
                        role: "after".into(),
                        status: RelationStatus::Valid,
                        resolved: Some(Resolution::Node(b)),
                    },
                ],
            });
        }
    }
    let order = s.reading_order_report(&doc);
    assert!(
        order
            .diagnostics
            .iter()
            .any(|d| d.code == "layout.reading-limit")
    );
    assert_eq!(order.steps.len(), nodes.len());
    assert_eq!(
        order.steps.iter().map(|s| s.line.node).collect::<Vec<_>>(),
        nodes
    );
}

#[test]
fn empty_document_has_an_empty_total_reading_order() {
    let doc = Document::new(PEER).unwrap();
    let s = engine().layout(&doc);
    assert!(s.reading_order(&doc).is_empty());
    assert!(s.pdf_reading_order(&doc).is_empty());
}
