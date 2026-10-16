//! Navigation in running regions (sections §12): placed hits, carets and
//! selections in an instance, and the fallback after a relayout.

mod common;

use common::*;
use reprise_doc::sections::{BreakKind, SectionId, SectionProperties};
use reprise_doc::{BlockKind, Document, NewBlock, NodeId};
use reprise_edit::{Caret, HitMode, Navigator, PlacedHit, RegionInstance, Selection};
use reprise_fixtures::engine;
use reprise_geom::{Length, Point};
use serde_json::json;

fn story(doc: &Document, texts: &[&str]) -> NodeId {
    let blocks: Vec<_> = texts
        .iter()
        .map(|t| NewBlock::new(BlockKind::Paragraph, "body", t))
        .collect();
    doc.create_story(&blocks).unwrap()
}

fn bind(doc: &Document, entries: &[(&str, NodeId)]) {
    let props: SectionProperties = entries
        .iter()
        .map(|(k, s)| ((*k).to_owned(), json!(s.to_string())))
        .collect();
    doc.write_section_properties(SectionId::First, "0", &props, &[])
        .unwrap();
}

/// Three one-line pages (page breaks) with a header and footer.
fn three_pages() -> (Document, NodeId, NodeId, Vec<NodeId>) {
    let (doc, body) = doc_with(&["one", "two", "three"]);
    let header = story(&doc, &["Header text"]);
    let footer = story(&doc, &["Footer text"]);
    for node in &body[1..] {
        let life = doc.mint_lifetime(*node).unwrap();
        let mut own = SectionProperties::new();
        own.insert("break".into(), json!("page"));
        doc.activate_start(*node, &life, BreakKind::Page, &own)
            .unwrap();
    }
    bind(
        &doc,
        &[
            ("region.header.default", header),
            ("region.footer.default", footer),
        ],
    );
    (doc, header, footer, body)
}

fn instance(snapshot: &reprise_layout::LayoutSnapshot, page: usize, slot: &str) -> RegionInstance {
    let r = snapshot
        .sections
        .as_ref()
        .unwrap()
        .running
        .iter()
        .find(|r| r.page == page && r.slot == slot)
        .unwrap();
    RegionInstance {
        page: r.page,
        slot: r.slot.clone(),
    }
}

fn centre(
    snapshot: &reprise_layout::LayoutSnapshot,
    page: usize,
    slot: &str,
) -> Point<reprise_geom::PageSpace> {
    let r = snapshot
        .sections
        .as_ref()
        .unwrap()
        .running
        .iter()
        .find(|r| r.page == page && r.slot == slot)
        .unwrap();
    let f = &snapshot.frames[r.frame];
    let b = f.to_page.bounds(&f.rect);
    Point::new(
        b.origin.x + b.width.mul_ratio(1, 2),
        b.origin.y + b.height.mul_ratio(1, 2),
    )
}

#[test]
fn a_header_wins_over_the_body_in_its_rectangle_and_the_body_elsewhere() {
    let (doc, header, footer, body) = three_pages();
    let snapshot = engine().layout(&doc);
    let nav = Navigator::semantic(&snapshot, &doc);
    for (page, node) in body.iter().enumerate() {
        let hit = nav
            .hit_placed(page, centre(&snapshot, page, "header"), HitMode::Normal)
            .unwrap();
        match hit {
            PlacedHit::Running { position, instance } => {
                assert_eq!(
                    instance,
                    RegionInstance {
                        page,
                        slot: "header".into()
                    }
                );
                assert_eq!(
                    doc.domain_of(position.node),
                    Some(reprise_doc::Domain::Story(header))
                );
            }
            other => panic!("page {page}: {other:?}"),
        }
        let hit = nav
            .hit_placed(page, centre(&snapshot, page, "footer"), HitMode::Normal)
            .unwrap();
        assert!(
            matches!(&hit, PlacedHit::Running { instance, position }
                if instance.slot == "footer"
                    && doc.domain_of(position.node) == Some(reprise_doc::Domain::Story(footer))),
            "{hit:?}"
        );
        // The middle of the body is the body.
        let middle = Point::new(Length::from_pt(306), Length::from_pt(300));
        match nav.hit_placed(page, middle, HitMode::Normal).unwrap() {
            PlacedHit::Body(hit) => assert_eq!(hit.caret.node, *node),
            other => panic!("{other:?}"),
        }
    }
    // `hit` itself is unchanged and body-only: the header's point gives body text.
    let body_hit = nav.hit(0, centre(&snapshot, 0, "header")).unwrap();
    assert_eq!(body_hit.caret.node, body[0]);
}

#[test]
fn the_whole_margin_band_belongs_to_the_header() {
    let (doc, _, _, _) = three_pages();
    let snapshot = engine().layout(&doc);
    let nav = Navigator::semantic(&snapshot, &doc);
    // Far from the single line of text, still in the top margin band.
    let corner = Point::new(Length::from_pt(80), Length::from_pt(2));
    assert!(matches!(
        nav.hit_placed(1, corner, HitMode::Normal),
        Some(PlacedHit::Running { .. })
    ));
}

#[test]
fn a_background_answers_only_when_asked_and_only_below_everything() {
    let (doc, body) = doc_with(&["body"]);
    let mark = story(&doc, &["DRAFT"]);
    bind(&doc, &[("region.background.default", mark)]);
    let snapshot = engine().layout(&doc);
    let nav = Navigator::semantic(&snapshot, &doc);
    let p = centre(&snapshot, 0, "background");
    let normal = nav.hit_placed(0, p, HitMode::Normal).unwrap();
    assert!(
        matches!(normal, PlacedHit::Body(ref h) if h.caret.node == body[0]),
        "{normal:?}"
    );
    let asked = nav.hit_placed(0, p, HitMode::Background).unwrap();
    assert!(
        matches!(asked, PlacedHit::Running { ref instance, .. } if instance.slot == "background"),
        "{asked:?}"
    );
}

#[test]
fn carets_and_selections_in_an_instance_are_on_its_page() {
    let (doc, header, _, _) = three_pages();
    let snapshot = engine().layout(&doc);
    let nav = Navigator::semantic(&snapshot, &doc);
    let block = doc.top(reprise_doc::Domain::Story(header))[0];
    for page in 0..3 {
        let at = instance(&snapshot, page, "header");
        let caret = nav.caret_in(&at, Caret::new(block, 3)).unwrap();
        assert_eq!(caret.page, page);
        let r = &snapshot
            .sections
            .as_ref()
            .unwrap()
            .running
            .iter()
            .find(|r| r.page == page && r.slot == "header")
            .unwrap();
        let frame = &snapshot.frames[r.frame];
        let frame_box = frame.to_page.bounds(&frame.rect);
        assert!(caret.rect.origin.y >= frame_box.origin.y);
        assert!(caret.rect.max_y() <= frame_box.max_y());
        let rects = nav.selection_rects_in(
            &at,
            &Selection {
                anchor: Caret::new(block, 0),
                focus: Caret::new(block, 6),
            },
        );
        assert!(!rects.is_empty());
        assert!(rects.iter().all(|r| r.page == page));
    }
    // A body caret is not in a region.
    let nope = RegionInstance {
        page: 0,
        slot: "header".into(),
    };
    let body_block = snapshot.blocks[0].node;
    assert!(nav.caret_in(&nope, Caret::new(body_block, 0)).is_none());
}

#[test]
fn after_a_relayout_the_nearest_page_that_shows_the_story_answers() {
    let (doc, header, _, _) = three_pages();
    let block = doc.top(reprise_doc::Domain::Story(header))[0];
    // Page 1 loses its header: an explicit None for the section starting there
    // would hide the whole chain, so drop the second page instead by laying
    // out a shorter document.
    let snapshot = engine().layout(&doc);
    let nav = Navigator::semantic(&snapshot, &doc);
    let stale = RegionInstance {
        page: 7,
        slot: "header".into(),
    };
    // Page 7 does not exist: the nearest page showing the header is page 2.
    let caret = nav.caret_in(&stale, Caret::new(block, 0)).unwrap();
    assert_eq!(caret.page, 2);
    // Equidistant pages: the lower one answers. Pages 0 and 2 are both one
    // away from a header-less page 1, so hide page 1's header.
    let mut hidden = snapshot.clone();
    hidden
        .sections
        .as_mut()
        .unwrap()
        .running
        .retain(|r| !(r.page == 1 && r.slot == "header"));
    let nav = Navigator::semantic(&hidden, &doc);
    let at = RegionInstance {
        page: 1,
        slot: "header".into(),
    };
    assert_eq!(nav.caret_in(&at, Caret::new(block, 0)).unwrap().page, 0);
    assert!(!nav.shows(&at));
    // No page shows it: no geometry, and no selection rectangles.
    hidden.sections.as_mut().unwrap().running.clear();
    let nav = Navigator::semantic(&hidden, &doc);
    assert!(nav.caret_in(&at, Caret::new(block, 0)).is_none());
    assert!(
        nav.selection_rects_in(&at, &Selection::collapsed(Caret::new(block, 0)))
            .is_empty()
    );
}
