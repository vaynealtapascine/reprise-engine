//! Astra's review of Stage B part 2, finding 1: footnotes are allocated inside
//! the body that remains after the running reservation, so they never overlap
//! a header or footer. Each test compares against the regions' physical
//! rectangles on the note's own page.
mod common;
use common::*;
use reprise_doc::relation::builtin;
use reprise_doc::sections::{BreakKind, SectionId};
use reprise_doc::text::RangePolicy;
use reprise_doc::{BlockKind, Dim, Document, NodeId, PageTemplate, Param, Relation, Target};
use reprise_layout::incremental::LayoutSession;
use reprise_layout::{Engine, LayoutSnapshot};
use serde_json::json;

/// Physical page rectangle of a frame.
fn page_rect(s: &LayoutSnapshot, frame: usize) -> reprise_geom::Rect<reprise_geom::PageSpace> {
    let f = &s.frames[frame];
    f.to_page.bounds(&f.rect)
}

/// No line of `node` overlaps a header or footer instance on its page.
fn assert_clear_of_regions(s: &LayoutSnapshot, node: NodeId, what: &str) {
    let block = s.block(node).unwrap();
    assert!(!block.lines.is_empty(), "{what}: the note has lines");
    for l in &block.lines {
        let f = &s.frames[l.frame];
        let nr = f.to_page.bounds(&l.rect);
        for r in &s.sections.as_ref().unwrap().running {
            if r.page != f.page || r.slot == "background" {
                continue;
            }
            let rr = page_rect(s, r.frame);
            let overlap = nr.origin.x < rr.max_x()
                && rr.origin.x < nr.max_x()
                && nr.origin.y < rr.max_y()
                && rr.origin.y < nr.max_y();
            assert!(
                !overlap,
                "{what}: note {nr:?} overlaps the {} region {rr:?}",
                r.slot
            );
        }
    }
}

fn add_note(doc: &Document, engine: &Engine, anchor: NodeId, text: &str) -> NodeId {
    let note = doc.append_block(BlockKind::Annotation, "s", text).unwrap();
    let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::NOTE)
            .owned_by(note)
            .target("anchor", Target::Range(range))
            .param("placement", Param::Text("page".into())),
    )
    .unwrap();
    note
}

#[test]
fn notes_stay_inside_running_reservations() {
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(120))
            .with_frame(frame("main", 0, 0, 200, 120))
            .with_frame(notes_frame(0, 30, 200, 90)),
    )
    .unwrap();
    let head = story(&doc, "head\nhead");
    let foot = story(&doc, "foot");
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[
            ("columns.count".into(), json!(1)),
            ("header.distance".into(), json!(0)),
            ("footer.distance".into(), json!(0)),
            ("region.header.default".into(), json!(head.to_string())),
            ("region.footer.default".into(), json!(foot.to_string())),
        ]
        .into(),
        &[],
    )
    .unwrap();
    let anchor = block(&doc, "body");
    let note = add_note(&doc, &engine, anchor, "note\nnote");
    doc.commit();
    let s = engine.layout(&doc);
    assert_clear_of_regions(&s, note, "footer edge");
    // Reflowed, not clipped: both lines are still placed, above the footer.
    assert_eq!(s.block(note).unwrap().lines.len(), 2);
    let mut session = LayoutSession::new(&engine);
    assert!(session.layout(&doc).unwrap() == s);
}

#[test]
fn first_even_and_default_variants_each_keep_notes_clear_of_their_own_regions() {
    // Page 1 takes the tall `first` header, page 2 the `even` footer, page 3
    // the default pair: each page has its own reservation.
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(150))
            .with_frame(frame("main", 0, 0, 200, 150))
            .with_frame(notes_frame(0, 20, 200, 130)),
    )
    .unwrap();
    let first_head = story(&doc, "first\nfirst\nfirst");
    let head = story(&doc, "head");
    let even_foot = story(&doc, "even\neven");
    let foot = story(&doc, "foot");
    bind(
        &doc,
        &[
            ("region.header.first", first_head),
            ("region.header.default", head),
            ("region.footer.even", even_foot),
            ("region.footer.default", foot),
        ],
    );
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[
            ("region.first".into(), json!(true)),
            ("region.even".into(), json!(true)),
            ("header.distance".into(), json!(0)),
            ("footer.distance".into(), json!(0)),
        ]
        .into(),
        &[],
    )
    .unwrap();
    let mut notes = Vec::new();
    for i in 0..3 {
        let anchor = block(&doc, &format!("anchor {i}"));
        notes.push(add_note(&doc, &engine, anchor, &format!("note {i}")));
        let pb = block(&doc, &format!("page {i}"));
        if i < 2 {
            let life = doc.mint_lifetime(pb).unwrap();
            doc.activate_start(pb, &life, BreakKind::Page, &Default::default())
                .unwrap();
        }
    }
    doc.commit();
    let s = engine.layout(&doc);
    assert!(s.pages.len() >= 3, "{} pages", s.pages.len());
    for (i, note) in notes.iter().enumerate() {
        assert_clear_of_regions(&s, *note, &format!("page {i}"));
    }
}

#[test]
fn notes_in_a_continuous_band_stay_clear_of_the_pages_regions() {
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(200))
            .with_frame(frame("main", 0, 0, 200, 200))
            .with_frame(notes_frame(0, 100, 200, 100)),
    )
    .unwrap();
    let head = story(&doc, "head");
    let foot = story(&doc, "foot\nfoot");
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[
            ("header.distance".into(), json!(0)),
            ("footer.distance".into(), json!(0)),
            ("region.header.default".into(), json!(head.to_string())),
            ("region.footer.default".into(), json!(foot.to_string())),
        ]
        .into(),
        &[],
    )
    .unwrap();
    let a = block(&doc, "first band");
    let b = block(&doc, "second band");
    let life = doc.mint_lifetime(b).unwrap();
    doc.activate_start(
        b,
        &life,
        BreakKind::Section,
        &[("start".into(), json!("continuous"))].into(),
    )
    .unwrap();
    let n1 = add_note(&doc, &engine, a, "n1");
    let n2 = add_note(&doc, &engine, b, "n2\nn2");
    doc.commit();
    let s = engine.layout(&doc);
    assert_eq!(s.sections.as_ref().unwrap().pages[0].bands.len(), 2);
    assert_clear_of_regions(&s, n1, "first band");
    assert_clear_of_regions(&s, n2, "second band");
}

#[test]
fn a_note_that_continues_keeps_clear_of_the_regions_on_every_page() {
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(100))
            .with_frame(frame("main", 0, 0, 200, 100))
            .with_frame(notes_frame(0, 20, 200, 80)),
    )
    .unwrap();
    let head = story(&doc, "head");
    let foot = story(&doc, "foot");
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[
            ("header.distance".into(), json!(0)),
            ("footer.distance".into(), json!(0)),
            ("region.header.default".into(), json!(head.to_string())),
            ("region.footer.default".into(), json!(foot.to_string())),
        ]
        .into(),
        &[],
    )
    .unwrap();
    let anchor = block(&doc, "body");
    let note = add_note(&doc, &engine, anchor, &vec!["note"; 20].join("\n"));
    doc.commit();
    let s = engine.layout(&doc);
    let pages: std::collections::BTreeSet<_> = s
        .block(note)
        .unwrap()
        .lines
        .iter()
        .map(|l| s.frames[l.frame].page)
        .collect();
    assert!(pages.len() > 1, "the note continues: {pages:?}");
    assert_clear_of_regions(&s, note, "continuation");
    // Every line of the note is placed somewhere: nothing was clipped away.
    assert_eq!(s.block(note).unwrap().lines.len(), 20);
}
