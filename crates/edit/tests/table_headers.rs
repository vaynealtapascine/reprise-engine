//! Navigation around repeated table header copies (30, 33).
//!
//! Choice: copies are derived display, not text. They are absent from
//! `LayoutSnapshot::blocks`, so the kernel never builds a caret, line or
//! selection rectangle in one; a point over a copy resolves to the nearest
//! *authored* line, deterministically, and reading order visits each authored
//! header block exactly once.

use reprise_doc::{
    BlockKind, Column, ColumnWidth, Dim, Document, FrameRole, FrameTemplate, LengthExpr, NodeId,
    PageTemplate, Style, TableColumns,
};
use reprise_edit::Navigator;
use reprise_fixtures::engine;
use reprise_geom::{Length, Point};

fn document() -> (Document, Vec<NodeId>) {
    let doc = Document::new(1).unwrap();
    doc.define_style(
        "s",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(10))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(12))),
            ..Default::default()
        },
    )
    .unwrap();
    doc.set_page_template(
        &PageTemplate::new("t", Dim::pt(100), Dim::pt(60)).with_frame(FrameTemplate::new(
            "body",
            FrameRole::Flow("main".into()),
            (Dim::pt(0), Dim::pt(0)),
            (Dim::pt(100), Dim::pt(60)),
        )),
    )
    .unwrap();
    let table = doc
        .append_table(TableColumns {
            columns: vec![
                Column {
                    width: ColumnWidth::Proportional(1)
                };
                2
            ],
        })
        .unwrap();
    let mut header = Vec::new();
    let head = doc.append_table_row(table, true).unwrap();
    for (i, t) in ["Head", "Cols"].iter().enumerate() {
        let c = doc.append_table_cell(head, i as u32).unwrap();
        header.push(
            doc.append_cell_block(c, BlockKind::Paragraph, "s", t)
                .unwrap(),
        );
    }
    for i in 0..10 {
        let r = doc.append_table_row(table, false).unwrap();
        for c in 0..2 {
            let cell = doc.append_table_cell(r, c).unwrap();
            doc.append_cell_block(cell, BlockKind::Paragraph, "s", &format!("r{i}"))
                .unwrap();
        }
    }
    (doc, header)
}

#[test]
fn hits_over_a_repeated_header_resolve_to_authored_lines_only() {
    let (doc, header) = document();
    let snapshot = engine().layout(&doc);
    assert!(!snapshot.repeated_headers.is_empty());
    let nav = Navigator::semantic(&snapshot, &doc);
    for copy in &snapshot.repeated_headers {
        let frame = &snapshot.frames[copy.frame];
        for block in &copy.blocks {
            for line in &block.lines {
                let r = line.rect;
                let centre = Point::new(
                    r.origin.x + Length(r.width.0 / 2),
                    r.origin.y + Length(r.height.0 / 2),
                );
                let page_point = frame.to_page.apply(centre);
                let hit = nav.hit(frame.page, page_point).expect("a hit");
                assert!(!header.contains(&hit.line.node), "never an authored header");
                assert!(!hit.inside, "no authored line is under a copy");
                assert!(snapshot.line(hit.line).is_some());
                // Deterministic.
                assert_eq!(Some(hit), nav.hit(frame.page, page_point));
            }
        }
    }
}

#[test]
fn reading_order_and_navigation_see_each_authored_header_once() {
    let (doc, header) = document();
    let snapshot = engine().layout(&doc);
    let nav = Navigator::semantic(&snapshot, &doc);
    for h in &header {
        assert_eq!(nav.reading_order().iter().filter(|n| *n == h).count(), 1);
        let steps = snapshot
            .reading_order(&doc)
            .into_iter()
            .filter(|s| s.line.node == *h)
            .count();
        assert_eq!(steps, snapshot.block(*h).unwrap().lines.len());
        // The authored header is hittable on its own page.
        let line = snapshot.block(*h).unwrap().lines[0].clone();
        let frame = &snapshot.frames[line.frame];
        let p = frame.to_page.apply(Point::new(
            line.rect.origin.x + Length(1),
            line.rect.origin.y + Length(line.rect.height.0 / 2),
        ));
        assert_eq!(nav.hit(frame.page, p).unwrap().line.node, *h);
    }
}
