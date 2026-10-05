//! Adversarial tests for header repetition and spans (24, 33, 37).
use super::*;
use reprise_doc::{
    BlockKind, Column, ColumnWidth, Dim, Document, FrameRole, FrameTemplate, LengthExpr, NodeId,
    PageTemplate, Style, TableColumns,
};
use reprise_font::{Face, FontStore};
use reprise_geom::Length;

const ROW: i32 = 20; // one 12pt line plus the 8pt spacing

fn setup(frame_height: i32) -> (Engine, Document) {
    let mut fonts = FontStore::default();
    fonts.add(
        Face::from_bytes(
            include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
        )
        .unwrap(),
    );
    let engine = Engine::new(fonts);
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
        &PageTemplate::new("t", Dim::pt(100), Dim::pt(frame_height)).with_frame(
            FrameTemplate::new(
                "body",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(100), Dim::pt(frame_height)),
            ),
        ),
    )
    .unwrap();
    (engine, doc)
}

fn table(doc: &Document, columns: usize) -> NodeId {
    doc.append_table(TableColumns {
        columns: (0..columns)
            .map(|_| Column {
                width: ColumnWidth::Proportional(1),
            })
            .collect(),
    })
    .unwrap()
}

fn cell(doc: &Document, row: NodeId, column: u32, text: &str) -> (NodeId, NodeId) {
    let cell = doc.append_table_cell(row, column).unwrap();
    let block = doc
        .append_cell_block(cell, BlockKind::Paragraph, "s", text)
        .unwrap();
    (cell, block)
}

fn row(doc: &Document, table: NodeId, header: bool, texts: &[&str]) -> Vec<NodeId> {
    let row = doc.append_table_row(table, header).unwrap();
    texts
        .iter()
        .enumerate()
        .map(|(i, t)| cell(doc, row, i as u32, t).1)
        .collect()
}

fn covers_once(s: &LayoutSnapshot, node: NodeId) {
    let b = s.block(node).expect("block laid out");
    let mut at = 0;
    for l in &b.lines {
        assert_eq!(l.text.start, at);
        at = l.text.end;
    }
    assert_eq!(at, b.text.len());
}

#[test]
fn header_rows_repeat_on_every_continuation_frame_and_stay_out_of_blocks() {
    let (engine, doc) = setup(60);
    let t = table(&doc, 2);
    let header = row(&doc, t, true, &["Name", "Value"]);
    let mut body = Vec::new();
    for i in 0..12 {
        body.extend(row(&doc, t, false, &[&format!("r{i}"), "x"]));
    }
    let s = engine.layout(&doc);
    assert!(s.pages.len() > 2);
    assert_eq!(s.repeated_headers.len(), s.pages.len() - 1);
    assert!(
        s.diagnostics_with("layout.table-header-unrepeated")
            .next()
            .is_none()
    );
    for (i, copy) in s.repeated_headers.iter().enumerate() {
        assert_eq!(copy.blocks.len(), 2);
        assert_eq!(copy.frame, i + 1, "one copy per continuation frame");
        for (b, authored) in copy.blocks.iter().zip(&header) {
            assert_eq!(b.node, *authored, "copies keep the authored identity");
            assert_eq!(b.lines.len(), 1);
            assert_eq!(b.lines[0].frame, copy.frame);
            assert_eq!(b.lines[0].rect.origin.y, Length::ZERO, "at the frame top");
        }
    }
    // The authored header appears once, only on the first frame; body rows
    // start below each copy.
    let authored: Vec<_> = s.blocks.iter().filter(|b| b.node == header[0]).collect();
    assert_eq!(authored.len(), 1);
    assert_eq!(authored[0].lines[0].frame, 0);
    for node in header.iter().chain(&body) {
        covers_once(&s, *node);
    }
    for copy in &s.repeated_headers {
        let top = Length::from_pt(ROW);
        let first_body = s
            .blocks
            .iter()
            .flat_map(|b| &b.lines)
            .filter(|l| l.frame == copy.frame)
            .map(|l| l.rect.origin.y)
            .min()
            .unwrap();
        assert!(first_body >= top, "body starts below the copy");
    }
}

#[test]
fn a_header_taller_than_the_frame_is_not_repeated_and_everything_is_placed() {
    let (engine, doc) = setup(30);
    let t = table(&doc, 1);
    let header = row(&doc, t, true, &["a\nb\nc\nd\ne\nf"]);
    let body = row(&doc, t, false, &["body"]);
    let s = engine.layout(&doc);
    assert!(s.repeated_headers.is_empty());
    assert_eq!(
        s.diagnostics_with("layout.table-header-unrepeated").count(),
        1
    );
    covers_once(&s, header[0]);
    covers_once(&s, body[0]);
}

#[test]
fn a_header_that_leaves_the_body_no_room_is_dropped_not_looped() {
    // Frame fits the header (one line) but then not a body line (two lines).
    let (engine, doc) = setup(20);
    let t = table(&doc, 1);
    let header = row(&doc, t, true, &["h"]);
    let body = row(&doc, t, false, &["b1\nb2\nb3"]);
    let s = engine.layout(&doc);
    covers_once(&s, header[0]);
    covers_once(&s, body[0]);
    assert!(s.pages.len() < 20, "terminates well within the page limit");
}

#[test]
fn only_the_leading_run_of_header_rows_repeats() {
    let (engine, doc) = setup(60);
    let t = table(&doc, 1);
    row(&doc, t, true, &["H1"]);
    for i in 0..4 {
        row(&doc, t, false, &[&format!("b{i}")]);
    }
    let mid = row(&doc, t, true, &["mid"]);
    for i in 0..6 {
        row(&doc, t, false, &[&format!("c{i}")]);
    }
    let s = engine.layout(&doc);
    assert!(!s.repeated_headers.is_empty());
    for copy in &s.repeated_headers {
        assert_eq!(copy.blocks.len(), 1, "the detached header is not copied");
        assert_ne!(copy.blocks[0].node, mid[0]);
    }
}

#[test]
fn column_spans_sum_the_solved_widths() {
    let (engine, doc) = setup(300);
    let t = table(&doc, 2);
    let r = doc.append_table_row(t, false).unwrap();
    let words = "aa bb cc dd ee ff gg hh ii jj kk ll mm nn oo pp qq rr ss tt";
    let (wide, wide_block) = cell(&doc, r, 0, words);
    doc.set_table_cell_span(wide, 2, 1).unwrap();
    let r2 = doc.append_table_row(t, false).unwrap();
    let (_, narrow) = cell(&doc, r2, 0, words);
    let s = engine.layout(&doc);
    let spanned = s.block(wide_block).unwrap();
    let plain = s.block(narrow).unwrap();
    assert!(spanned.lines.len() < plain.lines.len(), "twice the measure");
    let widest = spanned.lines.iter().map(|l| l.width).max().unwrap();
    assert!(widest > Length::from_pt(50) && widest <= Length::from_pt(100));
}

#[test]
fn a_row_span_that_fits_a_fresh_frame_is_kept_together() {
    let (engine, doc) = setup(80); // four rows per frame
    let t = table(&doc, 2);
    row(&doc, t, false, &["a", "b"]);
    row(&doc, t, false, &["c", "d"]);
    row(&doc, t, false, &["e", "f"]);
    // Two rows joined by a rowspan need 40pt; only 20pt is left here.
    let r = doc.append_table_row(t, false).unwrap();
    let (tall, tall_block) = cell(&doc, r, 0, "span");
    doc.set_table_cell_span(tall, 1, 2).unwrap();
    let (_, x) = cell(&doc, r, 1, "x");
    let r2 = doc.append_table_row(t, false).unwrap();
    let (_, y) = cell(&doc, r2, 1, "y");
    let s = engine.layout(&doc);
    assert!(
        s.diagnostics_with("layout.table-rowspan-split")
            .next()
            .is_none()
    );
    let page = |n: NodeId| s.block(n).unwrap().lines[0].frame;
    assert_eq!(page(tall_block), 1, "moved to the second frame as a unit");
    assert_eq!(page(x), 1);
    assert_eq!(page(y), 1);
    let ys = |n: NodeId| s.block(n).unwrap().lines[0].rect.origin.y;
    assert_eq!(ys(tall_block), ys(x));
    assert!(ys(y) > ys(x));
}

#[test]
fn a_row_span_taller_than_a_frame_splits_with_a_warning_and_loses_nothing() {
    let (engine, doc) = setup(60);
    let t = table(&doc, 2);
    let r = doc.append_table_row(t, false).unwrap();
    let long = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10";
    let (tall, tall_block) = cell(&doc, r, 0, long);
    doc.set_table_cell_span(tall, 1, 2).unwrap();
    let (_, x) = cell(&doc, r, 1, "x");
    let r2 = doc.append_table_row(t, false).unwrap();
    let (_, y) = cell(&doc, r2, 1, "y");
    let s = engine.layout(&doc);
    assert_eq!(s.diagnostics_with("layout.table-rowspan-split").count(), 1);
    assert!(s.pages.len() > 1);
    for n in [tall_block, x, y] {
        covers_once(&s, n);
    }
    // The spanning cell never overlaps the rows it spans: y is below x.
    let at = |n: NodeId| {
        let l = &s.block(n).unwrap().lines[0];
        (l.frame, l.rect.origin.y)
    };
    assert!(at(y) > at(x));
}

#[test]
fn spans_covering_the_whole_table_and_malformed_spans_never_panic() {
    let (engine, doc) = setup(60);
    let t = table(&doc, 3);
    let r = doc.append_table_row(t, true).unwrap();
    let (c, b) = cell(&doc, r, 0, "title");
    doc.set_table_cell_span(c, u32::MAX, u32::MAX).unwrap();
    let r2 = doc.append_table_row(t, false).unwrap();
    let (z, zb) = cell(&doc, r2, 0, "zero");
    doc.set_table_cell_span(z, 0, 0).unwrap();
    let (o, ob) = cell(&doc, r2, 1, "over");
    doc.set_table_cell_span(o, 5, 1).unwrap();
    let (late, lb) = cell(&doc, r2, 2, "late");
    doc.set_table_cell_span(late, 1, 1).unwrap(); // collides with `over`
    let s = engine.layout(&doc);
    assert!(s.diagnostics_with("layout.table-span").count() >= 3);
    assert!(s.diagnostics_with("layout.table-invalid").count() >= 1);
    for n in [b, zb, ob] {
        covers_once(&s, n);
    }
    assert!(s.block(lb).is_none(), "the colliding cell is dropped");
}

#[test]
fn a_thousand_columns_omit_the_table_and_the_rest_still_flows() {
    let (engine, doc) = setup(60);
    let t = table(&doc, 1000);
    row(&doc, t, false, &["x"]);
    let after = doc
        .append_block(BlockKind::Paragraph, "s", "still here")
        .unwrap();
    let s = engine.layout(&doc);
    assert_eq!(s.diagnostics_with("layout.table-invalid").count(), 1);
    covers_once(&s, after);
}

#[test]
fn layout_is_deterministic_and_repeats_are_derived_only() {
    let (engine, doc) = setup(60);
    let t = table(&doc, 2);
    row(&doc, t, true, &["a", "b"]);
    for i in 0..10 {
        row(&doc, t, false, &[&format!("{i}"), "z"]);
    }
    let before = doc.blocks().len();
    let one = engine.layout(&doc);
    let two = engine.layout(&doc);
    assert_eq!(one, two);
    assert_eq!(
        doc.blocks().len(),
        before,
        "layout writes nothing to the document"
    );
}
