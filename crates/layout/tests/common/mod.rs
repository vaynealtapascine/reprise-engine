//! Helpers shared by the Stage B part 2 review regressions (`review_b2_*.rs`).
#![allow(dead_code)]
use reprise_doc::sections::SectionId;
use reprise_doc::{
    BlockKind, Dim, Document, FrameRole, FrameTemplate, LengthExpr, NewBlock, NodeId, Style,
};
use reprise_font::{Face, FontStore};
use reprise_geom::Length;
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
