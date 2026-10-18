//! Helpers shared by the Stage B part 2 review regressions (`review_b2_*.rs`).
#![allow(dead_code)]
use reprise_doc::sections::SectionId;
use reprise_doc::{
    BlockKind, Column, ColumnWidth, Dim, Document, FrameRole, FrameTemplate, LengthExpr, NewBlock,
    NodeId, Style, TableColumns,
};
use reprise_font::{Face, FontStore};
use reprise_geom::Length;
use reprise_layout::incremental::LayoutSession;
use reprise_layout::{Engine, LayoutSnapshot};
use serde_json::json;

pub fn setup() -> (Engine, Document) {
    let mut fonts = FontStore::default();
    fonts.add(
        Face::from_bytes(
            include_bytes!("../../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
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
    (engine, doc)
}

pub fn frame(name: &str, x: i32, y: i32, w: i32, h: i32) -> FrameTemplate {
    FrameTemplate::new(
        name,
        FrameRole::Flow("main".into()),
        (Dim::pt(x), Dim::pt(y)),
        (Dim::pt(w), Dim::pt(h)),
    )
}

pub fn notes_frame(x: i32, y: i32, w: i32, h: i32) -> FrameTemplate {
    FrameTemplate::new(
        "notes",
        FrameRole::Notes,
        (Dim::pt(x), Dim::pt(y)),
        (Dim::pt(w), Dim::pt(h)),
    )
}

pub fn block(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "s", text).unwrap()
}

pub fn story(doc: &Document, text: &str) -> NodeId {
    doc.create_story(&[NewBlock::new(BlockKind::Paragraph, "s", text)])
        .unwrap()
}

pub fn bind(doc: &Document, entries: &[(&str, NodeId)]) {
    let props = entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), json!(v.to_string())))
        .collect();
    doc.write_section_properties(SectionId::First, "0", &props, &[])
        .unwrap();
}

pub fn bind_header(doc: &Document, story: NodeId) {
    bind(doc, &[("region.header.default", story)]);
}

/// The text of the lines of `node` in the running instances of `s`.
pub fn running_text(s: &LayoutSnapshot, node: NodeId) -> Vec<String> {
    s.sections
        .as_ref()
        .unwrap()
        .running
        .iter()
        .flat_map(|r| &r.blocks)
        .filter(|b| b.node == node)
        .map(|b| b.text.clone())
        .collect()
}

/// A running header holding a table of `rows` rows of two cells, each with one
/// paragraph, and a body paragraph so the document has a page.
pub struct Grid {
    pub table: NodeId,
    pub rows: Vec<NodeId>,
    pub cells: Vec<Vec<NodeId>>,
    pub blocks: Vec<Vec<NodeId>>,
}

pub fn two_columns(widths: [ColumnWidth; 2]) -> TableColumns {
    TableColumns {
        columns: widths.into_iter().map(|width| Column { width }).collect(),
    }
}

pub fn grid(doc: &Document, rows: usize) -> Grid {
    let story = doc.create_story(&[]).unwrap();
    let table = doc
        .append_table(two_columns([ColumnWidth::Content, ColumnWidth::Content]))
        .unwrap();
    let mut g = Grid {
        table,
        rows: Vec::new(),
        cells: Vec::new(),
        blocks: Vec::new(),
    };
    for r in 0..rows {
        add_row(doc, &mut g, r);
    }
    doc.move_block(table, Some(story), 0).unwrap();
    bind_header(doc, story);
    block(doc, "body");
    g
}

pub fn add_row(doc: &Document, g: &mut Grid, r: usize) {
    let row = doc.append_table_row(g.table, false).unwrap();
    let mut cells = Vec::new();
    let mut blocks = Vec::new();
    for c in 0..2u32 {
        let cell = doc.append_table_cell(row, c).unwrap();
        let text = format!("r{r}c{c}");
        blocks.push(
            doc.append_cell_block(cell, BlockKind::Paragraph, "s", &text)
                .unwrap(),
        );
        cells.push(cell);
    }
    g.rows.push(row);
    g.cells.push(cells);
    g.blocks.push(blocks);
}

/// Lays out in a session, makes `edit`, commits, and lays out again in the same
/// session, so the second layout can reuse the memo of the first. Returns the
/// layouts before and after (incremental) and a full layout of the edited
/// document.
pub fn edited(
    engine: &Engine,
    doc: &Document,
    edit: impl FnOnce(),
) -> (LayoutSnapshot, LayoutSnapshot, LayoutSnapshot) {
    let mut session = LayoutSession::new(engine);
    let before = session.layout(doc).unwrap();
    assert_eq!(before, engine.layout(doc), "the first layout is the same");
    edit();
    doc.commit();
    let after = session.layout(doc).unwrap();
    let full = engine.layout(doc);
    (before, after, full)
}

/// The edit must show in the layout (or the test proves nothing), and the
/// session must show it too.
pub fn assert_invalidated(
    what: &str,
    (before, after, full): (LayoutSnapshot, LayoutSnapshot, LayoutSnapshot),
) {
    assert!(before != full, "{what}: the edit changes the layout");
    assert!(
        after == full,
        "{what}: the session reused a stale running table"
    );
}
