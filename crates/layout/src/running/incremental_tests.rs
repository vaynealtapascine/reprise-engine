//! S7a: running regions under the incremental machinery (sections §11).
use super::layout_tests::*;
use crate::incremental::{JobError, LayoutSession, Viewport};
use reprise_doc::sections::BreakKind;
use reprise_doc::{LengthExpr, NodeId, Style};
use reprise_geom::Length;
use serde_json::json;

/// Twelve one-paragraph pages with a header, a footer and a background.
fn twelve() -> (
    crate::Engine,
    reprise_doc::Document,
    Vec<NodeId>,
    NodeId,
    NodeId,
) {
    let (engine, doc) = setup();
    let head = story(&doc, &["Running head"]);
    let foot = story(&doc, &["Footer"]);
    let mark = story(&doc, &["DRAFT"]);
    first(
        &doc,
        &[
            ("region.header.default", bind(head)),
            ("region.footer.default", bind(foot)),
            ("region.background.default", bind(mark)),
        ],
    );
    let nodes = pages(&doc, 12);
    doc.commit();
    (engine, doc, nodes, head, foot)
}

fn edit_text(doc: &reprise_doc::Document, node: NodeId, text: &str) {
    doc.block(node).unwrap().text.insert(0, text).unwrap();
    doc.commit();
}

fn story_block(doc: &reprise_doc::Document, story: NodeId) -> NodeId {
    doc.top(reprise_doc::Domain::Story(story))[0]
}

#[test]
fn full_equals_incremental_after_body_header_style_image_and_geometry_edits() {
    let (mut engine, doc, nodes, head, foot) = twelve();
    let hash = engine
        .assets
        .insert(&include_bytes!("../../../../fixtures/images/red-1x1.png")[..])
        .unwrap();
    let image = doc
        .append_image(
            "s",
            &reprise_doc::image::ImageData::new(hash.clone()),
            "alt",
        )
        .unwrap();
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    let check = |session: &mut LayoutSession<'_>, what: &str| {
        doc.commit();
        let full = engine.layout(&doc);
        let incremental = session.layout(&doc).unwrap();
        assert_eq!(full, incremental, "{what}");
        full
    };
    check(&mut session, "initial");
    edit_text(&doc, nodes[8], "body edit ");
    check(&mut session, "body");
    edit_text(&doc, story_block(&doc, head), "HEADER ");
    check(&mut session, "header text, same height");
    edit_text(&doc, story_block(&doc, foot), &"taller footer ".repeat(40));
    let tall = check(&mut session, "footer grows");
    assert!(
        tall.sections.as_ref().unwrap().pages[3].reserve.1 > Length::ZERO,
        "the footer now takes room from the body"
    );
    doc.define_style(
        "s",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(14))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(16))),
            ..Default::default()
        },
    )
    .unwrap();
    check(&mut session, "style");
    let mut data = reprise_doc::image::ImageData::new(hash);
    data.width = Some(LengthExpr::Pt(Length::from_pt(90)));
    doc.set_image(image, &data).unwrap();
    check(&mut session, "image");
    doc.set_page_setup_patch(reprise_doc::page_setup::PageSetupPatch {
        top: Some(Length::from_pt(90)),
        ..Default::default()
    })
    .unwrap();
    check(&mut session, "geometry");
    // A section property of the first section: the header distance.
    first(
        &doc,
        &[
            ("header.distance", json!(8192)),
            ("region.header.default", bind(head)),
            ("region.footer.default", bind(foot)),
        ],
    );
    check(&mut session, "section property");
}

#[test]
fn an_unaffected_prefix_and_suffix_are_reused() {
    let (engine, doc, nodes, head, _) = twelve();
    let mut session = LayoutSession::new(&engine);
    let baseline = session.layout(&doc).unwrap();
    assert_eq!(baseline.pages.len(), 12);
    assert_eq!(
        session.counters().running_compositions,
        3,
        "header, footer, background once each"
    );

    // A body edit recomposes that paragraph only; the other transitions hit.
    edit_text(&doc, nodes[8], "edit ");
    session.layout(&doc).unwrap();
    let c = session.counters();
    assert!(c.compositions <= 2, "{c:?}");
    assert!(c.reused_compositions >= 10, "{c:?}");
    assert!(
        c.reflowed_pages.iter().all(|p| (7..=9).contains(p)),
        "{c:?}"
    );
    assert_eq!(c.running_compositions, 0, "the stories are unchanged");

    // A header edit that keeps its height: every body transition is reused;
    // the story is composed once and shared by all twelve pages.
    edit_text(&doc, story_block(&doc, head), "X");
    let after = session.layout(&doc).unwrap();
    let c = session.counters();
    assert_eq!(c.compositions, 0, "reservations are unchanged: {c:?}");
    assert!(c.reused_compositions >= 12, "{c:?}");
    assert_eq!(c.running_compositions, 1, "{c:?}");
    assert!(c.running_reused >= 11, "{c:?}");
    assert!(
        after
            .sections
            .as_ref()
            .unwrap()
            .running
            .iter()
            .filter(|r| r.slot == "header")
            .all(|r| r.blocks[0].text.starts_with('X')),
        "every instance shows the edit"
    );
    assert_eq!(after, engine.layout(&doc));

    // A header edit that changes its height reflows the body from the first
    // page whose reservation changed: here, all of them.
    edit_text(&doc, story_block(&doc, head), &"much taller ".repeat(60));
    session.layout(&doc).unwrap();
    let c = session.counters();
    assert!(c.compositions >= 12, "{c:?}");
    assert_eq!(c.reflowed_pages.iter().min(), Some(&0));
}

#[test]
fn a_footer_edit_that_keeps_its_height_leaves_the_header_and_body_alone() {
    let (engine, doc, _, _, foot) = twelve();
    let mut session = LayoutSession::new(&engine);
    session.layout(&doc).unwrap();
    edit_text(&doc, story_block(&doc, foot), "Z");
    session.layout(&doc).unwrap();
    let c = session.counters();
    assert_eq!(c.compositions, 0, "{c:?}");
    assert_eq!(c.running_compositions, 1, "only the footer: {c:?}");
}

#[test]
fn budgets_of_any_size_reach_the_same_snapshot() {
    let (engine, doc, _, _, _) = twelve();
    let expected = engine.layout(&doc);
    for chunk in [1usize, 2, 3, 7, 64, 100_000] {
        let mut session = LayoutSession::new(&engine);
        let mut job = session.start(&doc, Viewport::Pages(0..1000));
        let mut steps = 0usize;
        loop {
            let step = job.step(chunk).unwrap();
            steps += 1;
            assert!(steps < 200_000, "chunk {chunk} makes progress");
            if step.complete {
                break;
            }
        }
        assert_eq!(job.complete().unwrap().unwrap(), expected, "chunk {chunk}");
    }
    // Budget 0 is a checked no-op even with regions.
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..1));
    let before = job.partial().unwrap().snapshot().to_json();
    assert_eq!(job.step(0).unwrap().used, 0);
    assert_eq!(before, job.partial().unwrap().snapshot().to_json());
}

#[test]
fn a_partial_view_never_shows_a_page_without_its_regions() {
    let (engine, doc, _, _, _) = twelve();
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..1000));
    for _ in 0..100_000 {
        let step = job.step(1).unwrap();
        let partial = job.partial().unwrap();
        let s = partial.snapshot();
        if let Some(m) = &s.sections {
            for page in 0..s.pages.len() {
                let on = |slot: &str| {
                    m.running
                        .iter()
                        .filter(|r| r.page == page && r.slot == slot)
                        .count()
                };
                // Every page that is shown has all three of its instances, and
                // every instance's frame and lines are in the view.
                assert_eq!(
                    (on("header"), on("footer"), on("background")),
                    (1, 1, 1),
                    "page {page} of {}",
                    s.pages.len()
                );
            }
            for r in &m.running {
                assert!(r.page < s.pages.len());
                assert!(r.frame < s.frames.len());
                assert!(
                    r.blocks
                        .iter()
                        .flat_map(|b| &b.lines)
                        .all(|l| l.frame == r.frame)
                );
            }
        }
        if step.complete {
            return;
        }
    }
    panic!("one-unit layout did not finish");
}

#[test]
fn cancellation_and_staleness_reject_publication_with_regions() {
    let (engine, doc, nodes, head, _) = twelve();
    let mut session = LayoutSession::new(&engine);
    let state = session.start(&doc, Viewport::Pages(0..1000)).suspend();
    edit_text(&doc, story_block(&doc, head), "changed ");
    assert!(matches!(session.resume(&doc, state), Err(JobError::Stale)));
    let mut job = session.start(&doc, Viewport::Pages(0..1000));
    job.step(3).unwrap();
    job.cancel();
    assert!(matches!(job.partial(), Err(JobError::Cancelled)));
    assert!(matches!(job.complete(), Err(JobError::Cancelled)));
    // A story edit between steps is stale for the running job.
    let mut job = session.start(&doc, Viewport::Pages(0..1000));
    job.step(3).unwrap();
    edit_text(&doc, nodes[1], "late ");
    assert!(matches!(job.step(1), Err(JobError::Stale)));
    // After the staleness the session recovers on the next job.
    let fresh = session.layout(&doc).unwrap();
    assert_eq!(fresh, engine.layout(&doc));
    let _ = BreakKind::Page;
}

#[test]
fn a_binding_edit_in_an_earlier_section_changes_a_later_sections_pages() {
    let (engine, doc) = setup();
    let one = story(&doc, &["ONE"]);
    let two = story(&doc, &["TWO"]);
    first(&doc, &[("region.header.default", bind(one))]);
    let nodes: Vec<_> = (0..4).map(|i| body(&doc, &format!("s{i}"))).collect();
    boundary(&doc, nodes[2], BreakKind::Section, &[]);
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    session.layout(&doc).unwrap();
    // The second section inherits: rebinding the first changes both.
    first(&doc, &[("region.header.default", bind(two))]);
    doc.commit();
    let s = session.layout(&doc).unwrap();
    assert_eq!(s, engine.layout(&doc));
    assert!(
        s.sections
            .as_ref()
            .unwrap()
            .running
            .iter()
            .all(|r| r.story == two)
    );
}

/// S7b: a header that shows the page count depends on every page.
#[test]
fn a_page_count_in_a_header_follows_a_later_edit_back_to_page_one() {
    use super::field_tests::{P, fill, on, shown, story_of};
    let (engine, doc) = setup();
    let head = story_of(&doc, &[P::T("of "), P::F("page-count")]);
    first(&doc, &[("region.header.default", bind(head))]);
    let nodes = pages(&doc, 12);
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    let s = session.layout(&doc).unwrap();
    assert!((0..12).all(|p| shown(on(&s, p, "header")) == ["12"]));

    // A thirteenth page: every earlier header changes, none changes height,
    // so no earlier body transition is redone while every earlier header says 13.
    let more = body(&doc, "page 12");
    boundary(&doc, more, BreakKind::Page, &[]);
    doc.commit();
    let s = session.layout(&doc).unwrap();
    assert_eq!(s, engine.layout(&doc));
    assert!((0..13).all(|p| shown(on(&s, p, "header")) == ["13"]));
    let c = session.counters();
    assert!(c.reused_compositions >= 12, "bodies are reused: {c:?}");
    // Pages that show the same count share one composition; the guess of the
    // first pass and the settled one are the only two.
    assert!((1..=3).contains(&c.running_compositions), "{c:?}");
    assert!(c.running_reused >= 20, "{c:?}");
    assert_eq!(c.reflowed_pages.iter().collect::<Vec<_>>(), [&12], "{c:?}");

    // The same count with a story edit that does change the header's height:
    // the body reflows from page 0.
    fill(
        &doc,
        doc.top(reprise_doc::Domain::Story(head))[0],
        &[P::T(&" and so on".repeat(120))],
    );
    doc.commit();
    let s = session.layout(&doc).unwrap();
    assert_eq!(s, engine.layout(&doc));
    let c = session.counters();
    assert_eq!(c.reflowed_pages.iter().min(), Some(&0), "{c:?}");
    let _ = nodes;
}
