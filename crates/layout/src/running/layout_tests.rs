//! Running regions through the whole layout (sections S6).
use crate::{Engine, LayoutSnapshot, PageKind};
use reprise_doc::sections::{BreakKind, SectionId, SectionProperties};
use reprise_doc::{BlockKind, Document, Domain, FrameRole, LengthExpr, NewBlock, NodeId, Style};
use reprise_font::{Face, FontStore};
use reprise_geom::Length;
use serde_json::{Value, json};

pub(crate) fn setup() -> (Engine, Document) {
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
    doc.set_page_setup_patch(reprise_doc::page_setup::PageSetupPatch {
        width: Some(Length::from_pt(612)),
        height: Some(Length::from_pt(792)),
        top: Some(Length::from_pt(72)),
        right: Some(Length::from_pt(72)),
        bottom: Some(Length::from_pt(72)),
        left: Some(Length::from_pt(72)),
    })
    .unwrap();
    (engine, doc)
}

pub(crate) fn story(doc: &Document, texts: &[&str]) -> NodeId {
    let blocks: Vec<_> = texts
        .iter()
        .map(|t| NewBlock::new(BlockKind::Paragraph, "s", t))
        .collect();
    doc.create_story(&blocks).unwrap()
}

pub(crate) fn props(entries: &[(&str, Value)]) -> SectionProperties {
    entries
        .iter()
        .map(|(k, v)| ((*k).into(), v.clone()))
        .collect()
}

pub(crate) fn first(doc: &Document, entries: &[(&str, Value)]) {
    doc.write_section_properties(SectionId::First, "0", &props(entries), &[])
        .unwrap();
}

pub(crate) fn bind(story: NodeId) -> Value {
    json!(story.to_string())
}

pub(crate) fn boundary(doc: &Document, node: NodeId, kind: BreakKind, entries: &[(&str, Value)]) {
    let mut own = props(entries);
    own.insert("break".into(), json!(kind.token()));
    let life = doc.mint_lifetime(node).unwrap();
    doc.activate_start(node, &life, kind, &own).unwrap();
}

pub(crate) fn body(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "s", text).unwrap()
}

/// `count` one-line paragraphs, each its own page: a page break before each.
pub(crate) fn pages(doc: &Document, count: usize) -> Vec<NodeId> {
    let nodes: Vec<_> = (0..count)
        .map(|i| body(doc, &format!("page {i}")))
        .collect();
    for node in nodes.iter().skip(1) {
        boundary(doc, *node, BreakKind::Page, &[]);
    }
    nodes
}

pub(crate) fn instances<'a>(s: &'a LayoutSnapshot, slot: &str) -> Vec<&'a crate::RunningInstance> {
    s.sections
        .as_ref()
        .map(|m| m.running.iter().filter(|r| r.slot == slot).collect())
        .unwrap_or_default()
}

pub(crate) fn text_of(instance: &crate::RunningInstance) -> String {
    instance
        .blocks
        .iter()
        .flat_map(|b| {
            b.lines
                .iter()
                .map(move |l| b.text[l.text.clone()].to_owned())
        })
        .collect::<Vec<_>>()
        .join("|")
}

#[test]
fn a_header_is_made_for_every_page_and_never_enters_the_body() {
    let (engine, doc) = setup();
    let header = story(&doc, &["Running head"]);
    first(&doc, &[("region.header.default", bind(header))]);
    let nodes = pages(&doc, 3);
    let s = engine.layout(&doc);
    assert_eq!(s.pages.len(), 3);
    let running = instances(&s, "header");
    assert_eq!(running.len(), 3);
    for (page, r) in running.iter().enumerate() {
        assert_eq!(
            (r.page, r.variant.as_str(), r.story),
            (page, "default", header)
        );
        assert!(!r.clipped);
        assert_eq!(text_of(r), "Running head");
        let frame = &s.frames[r.frame];
        assert_eq!(frame.page, page);
        assert_eq!(frame.role, FrameRole::Flow("reprise.running.header".into()));
        // header.distance from the top edge, the page's inline margins wide.
        assert_eq!(
            frame.to_page.apply(reprise_geom::Point::origin()).y,
            Length::from_pt(36)
        );
        assert!(
            r.blocks
                .iter()
                .flat_map(|b| &b.lines)
                .all(|l| l.frame == r.frame)
        );
    }
    // The header block is a story block: never in `blocks`, reading order or the body.
    assert!(s.block(running[0].blocks[0].node).is_none());
    for node in &nodes {
        assert!(s.block(*node).is_some());
    }
    assert_eq!(s.sections.as_ref().unwrap().pages.len(), 3);
    assert!(
        s.sections
            .as_ref()
            .unwrap()
            .pages
            .iter()
            .all(|p| p.kind == PageKind::Content && p.reserve == (Length::ZERO, Length::ZERO)),
        "a header inside the margin reserves nothing"
    );
}

#[test]
fn a_tall_header_takes_room_from_the_body() {
    let (engine, doc) = setup();
    let text = "word ".repeat(4000);
    let header = story(&doc, &[&text]);
    first(&doc, &[("region.header.default", bind(header))]);
    let node = body(&doc, "body text");
    let s = engine.layout(&doc);
    let r = instances(&s, "header")[0];
    let d = s.sections.as_ref().unwrap().pages[0].reserve;
    assert!(d.0 > Length::ZERO && d.1 == Length::ZERO, "{d:?}");
    // The body starts below the header and keeps at least a quarter of its depth.
    let line = &s.block(node).unwrap().lines[0];
    let top = s.frames[line.frame]
        .to_page
        .apply(reprise_geom::Point::origin())
        .y;
    assert_eq!(top, Length::from_pt(72) + d.0);
    let frame = &s.frames[line.frame];
    assert!(frame.rect.height >= Length::from_pt(648).mul_ratio(1, 4));
    // The header's frame ends where the body starts.
    let h = &s.frames[r.frame];
    let bottom = h
        .to_page
        .apply(reprise_geom::Point::new(Length::ZERO, h.rect.height))
        .y;
    assert_eq!(bottom, top, "frame depth is the cap");
    assert!(r.clipped, "the rest of the text did not fit");
    assert!(
        s.diagnostics_with("layout.running-clipped").count() == 1,
        "{:?}",
        s.diagnostics
    );
    let _ = Domain::Body;
}
