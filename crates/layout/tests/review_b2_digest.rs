//! Astra's review of Stage B part 2, suggestion 6: after an edit that leaves a
//! running story alone, the story's unchanged digest is compared with the
//! cached one once, not on every page. Composition-hit counts cannot see that
//! cost (every page hits), so the layout counts the deep digest comparisons:
//! an unchanged header of thousands of blocks on dozens of pages must cost one
//! comparison per cached composition, not one per page.
mod common;
use common::*;
use reprise_doc::{BlockKind, Document, NewBlock, NodeId};
use reprise_geom::Length;
use reprise_layout::incremental::LayoutSession;
use reprise_layout::{Engine, LayoutSnapshot};
use serde_json::json;

const HEADER_BLOCKS: usize = 2_000;

/// A 200 pt square page with 20 pt margins, a header story of `HEADER_BLOCKS`
/// short paragraphs bound to `slots`, and a body of `lines` forced lines that
/// fills many pages. Returns the first body paragraph.
fn pages_with_a_big_header(doc: &Document, slots: &[&str], lines: usize) -> NodeId {
    doc.set_page_setup_patch(reprise_doc::page_setup::PageSetupPatch {
        width: Some(Length::from_pt(200)),
        height: Some(Length::from_pt(200)),
        top: Some(Length::from_pt(20)),
        bottom: Some(Length::from_pt(20)),
        left: Some(Length::from_pt(20)),
        right: Some(Length::from_pt(20)),
    })
    .unwrap();
    let blocks: Vec<_> = (0..HEADER_BLOCKS)
        .map(|i| NewBlock::new(BlockKind::Paragraph, "s", &format!("header {i}")))
        .collect();
    let story = doc.create_story(&blocks).unwrap();
    let mut props: reprise_doc::sections::SectionProperties = [
        ("header.distance".to_owned(), json!(0)),
        ("footer.distance".to_owned(), json!(0)),
    ]
    .into();
    for slot in slots {
        props.insert(format!("region.{slot}.default"), json!(story.to_string()));
    }
    doc.write_section_properties(reprise_doc::sections::SectionId::First, "0", &props, &[])
        .unwrap();
    let first = block(doc, "first");
    block(doc, &vec!["line"; lines].join("\n"));
    first
}

/// Lays out in a session, types into the first body paragraph, and lays out
/// again. Returns the second layout and the session.
fn after_a_body_edit<'e>(
    engine: &'e Engine,
    doc: &Document,
    first: NodeId,
) -> (LayoutSnapshot, LayoutSession<'e>) {
    let mut session = LayoutSession::new(engine);
    let before = session.layout(doc).unwrap();
    assert!(before.pages.len() >= 20, "{} pages", before.pages.len());
    doc.block(first).unwrap().text.insert(0, "edited ").unwrap();
    doc.commit();
    let after = session.layout(doc).unwrap();
    (after, session)
}

#[test]
fn an_unchanged_header_is_compared_once_not_once_per_page() {
    let (engine, doc) = setup();
    let first = pages_with_a_big_header(&doc, &["header"], 400);
    doc.commit();
    let (after, session) = after_a_body_edit(&engine, &doc, first);
    let c = session.counters();
    assert!(after.pages.len() >= 20);
    assert!(after == engine.layout(&doc));
    assert_eq!(c.running_compositions, 0, "the header was not recomposed");
    assert!(c.running_reused >= after.pages.len(), "{c:?}");
    assert!(
        c.running_digest_compares <= 1,
        "{} pages cost {} deep digest comparisons",
        after.pages.len(),
        c.running_digest_compares
    );
}

#[test]
fn a_story_in_two_slots_costs_one_comparison_per_cached_composition() {
    // Header and footer share the story: two cached compositions (different
    // slots, so different keys), each canonicalised once. The cheap parts of
    // the key (slot, size) are compared before the digest is.
    let (engine, doc) = setup();
    let first = pages_with_a_big_header(&doc, &["header", "footer"], 400);
    doc.commit();
    let (after, session) = after_a_body_edit(&engine, &doc, first);
    let c = session.counters();
    assert!(after == engine.layout(&doc));
    assert_eq!(c.running_compositions, 0);
    assert!(
        c.running_digest_compares <= 2,
        "{} pages cost {} deep digest comparisons",
        after.pages.len(),
        c.running_digest_compares
    );
}

#[test]
fn a_changed_header_is_compared_once_and_recomposed_once() {
    let (engine, doc) = setup();
    let first = pages_with_a_big_header(&doc, &["header"], 400);
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    session.layout(&doc).unwrap();
    // Edit the header's last block: its digest differs.
    let story = doc.stories()[0];
    let last = *doc.top(reprise_doc::Domain::Story(story)).last().unwrap();
    doc.block(last).unwrap().text.insert(0, "changed ").unwrap();
    doc.commit();
    let _ = first;
    let after = session.layout(&doc).unwrap();
    let c = session.counters();
    assert!(after == engine.layout(&doc));
    assert_eq!(
        c.running_compositions, 1,
        "one composition serves every page"
    );
    assert!(
        c.running_digest_compares <= 1,
        "the stale entry is compared once and dropped: {}",
        c.running_digest_compares
    );
}
