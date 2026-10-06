//! Orchestrator review of table headers and spans (24, 26, 33, 37, 38).
//!
//! 1. Seeded random tables are built with every kind of hostile span: zero,
//!    huge, past the edge, overlapping, header to body, out-of-range columns.
//!    They have random header rows and are laid out in frames from tiny to
//!    roomy. Layout must not panic, must be deterministic, must equal the
//!    incremental session, and every placed block's lines must cover its text
//!    exactly once.
//! 2. Two peers concurrently change spans and header flags of the same table.
//!    After merging either way round, the replicas agree on the resolved grid
//!    and lay out identically.

use reprise_doc::{
    BlockKind, Column, ColumnWidth, Dim, Document, FrameRole, FrameTemplate, LengthExpr, NodeId,
    PageTemplate, Style, TableColumns,
};
use reprise_fixtures::{OTHER_PEER, PEER, engine};
use reprise_geom::Length;
use reprise_layout::incremental::LayoutSession;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn span(&mut self) -> u32 {
        match self.below(10) {
            0 => 0,
            1 => u32::MAX,
            2 => 1000,
            n => (n as u32) % 4 + 1,
        }
    }
}

fn base(height: i32, width: i32) -> Document {
    let doc = Document::new(PEER).unwrap();
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
        &PageTemplate::new("t", Dim::pt(width), Dim::pt(height)).with_frame(FrameTemplate::new(
            "body",
            FrameRole::Flow("main".into()),
            (Dim::pt(0), Dim::pt(0)),
            (Dim::pt(width), Dim::pt(height)),
        )),
    )
    .unwrap();
    doc
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

const WORDS: &[&str] = &["a", "tableau", "spanning", "header", "row", "ink", ""];

fn text(rng: &mut Rng) -> String {
    (0..rng.below(14))
        .map(|_| WORDS[rng.below(WORDS.len() as u64) as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

fn check_layout(doc: &Document, label: &str) -> reprise_layout::LayoutSnapshot {
    doc.commit();
    let engine = engine();
    let a = engine.layout(doc);
    let b = engine.layout(doc);
    assert_eq!(a, b, "{label}: deterministic");
    let mut session = LayoutSession::new(&engine);
    assert_eq!(session.layout(doc).unwrap(), a, "{label}: incremental");
    for block in &a.blocks {
        let mut at = 0;
        for line in &block.lines {
            assert_eq!(line.text.start, at, "{label}: lines are contiguous");
            at = line.text.end;
        }
        // A block cut by the page limit may stop early, but it never overlaps
        // or runs past its text.
        assert!(
            at <= block.text.len(),
            "{label}: lines stay inside the text"
        );
    }
    for copy in &a.repeated_headers {
        for block in &copy.blocks {
            assert!(
                a.block(block.node).is_some(),
                "{label}: a header copy repeats an authored, placed block"
            );
        }
    }
    let json = serde_json::to_string(&a).unwrap();
    assert_eq!(json, serde_json::to_string(&b).unwrap(), "{label}: json");
    a
}

#[test]
fn random_hostile_span_tables_lay_out_deterministically_and_incrementally() {
    let mut rng = Rng(0x7ab1_e5ee_d000_0001);
    let (mut repeats, mut issues, mut splits) = (0, 0, 0);
    for case in 0..48 {
        let height = [14, 25, 40, 90, 300][rng.below(5) as usize];
        let width = [30, 100, 400][rng.below(3) as usize];
        let doc = base(height, width);
        if rng.below(3) == 0 {
            doc.append_block(BlockKind::Paragraph, "s", &text(&mut rng))
                .unwrap();
        }
        let columns = 1 + rng.below(6) as usize;
        let t = table(&doc, columns);
        let rows = rng.below(24) as usize;
        for r in 0..rows {
            let header = r < 3 && rng.below(2) == 0 || rng.below(12) == 0;
            let row = doc.append_table_row(t, header).unwrap();
            for _ in 0..rng.below(columns as u64 + 3) {
                let column = if rng.below(8) == 0 {
                    1000
                } else {
                    rng.below(columns as u64) as u32
                };
                let (cs, rs) = (rng.span(), rng.span());
                let cell = doc.append_table_cell_spanned(row, column, cs, rs).unwrap();
                doc.append_cell_block(cell, BlockKind::Paragraph, "s", &text(&mut rng))
                    .unwrap();
            }
        }
        if rng.below(2) == 0 {
            doc.append_block(BlockKind::Paragraph, "s", &text(&mut rng))
                .unwrap();
        }
        doc.commit();
        let grid = doc.table_structure(t).unwrap().unwrap();
        // No grid position is claimed twice.
        let mut seen = std::collections::BTreeSet::new();
        for c in &grid.cells {
            assert!(c.colspan >= 1 && c.rowspan >= 1);
            for r in c.row..c.row + c.rowspan {
                for k in c.column..c.column + c.colspan {
                    assert!(k < grid.columns && r < grid.rows.len());
                    assert!(seen.insert((r, k)), "case {case}: ({r},{k}) claimed twice");
                }
            }
        }
        issues += grid.issues.len();
        let snapshot = check_layout(&doc, &format!("case {case}"));
        repeats += snapshot.repeated_headers.len();
        splits += snapshot
            .diagnostics
            .iter()
            .filter(|d| d.code.as_str() == "layout.table-rowspan-split")
            .count();
    }
    // The cases must actually exercise what they claim to.
    assert!(
        repeats > 0 && issues > 0 && splits > 0,
        "{repeats} {issues} {splits}"
    );
}

#[test]
fn concurrent_span_and_header_edits_converge() {
    let mut rng = Rng(0xc0c0_5ba5_0000_0007);
    for round in 0..12 {
        let one = base(60, 200);
        let t = table(&one, 4);
        let mut cells = Vec::new();
        let mut rows = Vec::new();
        for r in 0..8 {
            let row = one.append_table_row(t, r == 0).unwrap();
            rows.push(row);
            for c in 0..4 {
                let cell = one.append_table_cell(row, c).unwrap();
                one.append_cell_block(cell, BlockKind::Paragraph, "s", &text(&mut rng))
                    .unwrap();
                cells.push(cell);
            }
        }
        one.commit();
        let two = one.fork(OTHER_PEER).unwrap();
        for peer in [&one, &two] {
            for _ in 0..6 {
                let cell = cells[rng.below(cells.len() as u64) as usize];
                peer.set_table_cell_span(cell, rng.span(), rng.span())
                    .unwrap();
            }
            let row = rows[rng.below(rows.len() as u64) as usize];
            peer.set_table_row_header(row, rng.below(2) == 0).unwrap();
            peer.commit();
        }
        let left = one.fork(PEER).unwrap();
        left.merge(&two).unwrap();
        let right = two.fork(OTHER_PEER).unwrap();
        right.merge(&one).unwrap();
        left.commit();
        right.commit();
        assert_eq!(
            left.table_structure(t).unwrap(),
            right.table_structure(t).unwrap(),
            "round {round}: grids converge"
        );
        let engine = engine();
        let (l, r) = (engine.layout(&left), engine.layout(&right));
        assert_eq!(l.blocks, r.blocks, "round {round}: blocks converge");
        assert_eq!(l.repeated_headers, r.repeated_headers, "round {round}");
        assert_eq!(l.diagnostics, r.diagnostics, "round {round}");
        let _ = check_layout(&left, &format!("round {round} merged"));
    }
}
