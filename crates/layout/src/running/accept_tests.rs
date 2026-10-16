//! S6 acceptance: variants, inheritance, reservation, limits and fillers.
use super::layout_tests::*;
use crate::{LayoutSnapshot, PageKind};
use reprise_doc::sections::BreakKind;
use reprise_doc::{BlockKind, Domain, FrameRole, NewBlock};
use reprise_geom::Length;
use serde_json::json;

pub(super) fn run_text(s: &LayoutSnapshot, page: usize, slot: &str) -> Option<String> {
    instances(s, slot)
        .into_iter()
        .find(|r| r.page == page)
        .map(text_of)
}

#[test]
fn first_even_and_default_headers_follow_the_page_and_the_switches() {
    let (engine, doc) = setup();
    let default = story(&doc, &["DEFAULT"]);
    let first_story = story(&doc, &["FIRST"]);
    let even = story(&doc, &["EVEN"]);
    first(
        &doc,
        &[
            ("region.header.default", bind(default)),
            ("region.header.first", bind(first_story)),
            ("region.header.even", bind(even)),
            ("region.first", json!(true)),
            ("region.even", json!(true)),
        ],
    );
    pages(&doc, 4);
    let s = engine.layout(&doc);
    let shown: Vec<_> = (0..4).map(|p| run_text(&s, p, "header").unwrap()).collect();
    // Page 1 is the first owned page; pages 2 and 4 are even (physical,
    // 1-based); page 3 is the default.
    assert_eq!(shown, ["FIRST", "EVEN", "DEFAULT", "EVEN"]);
    let variants: Vec<_> = instances(&s, "header")
        .iter()
        .map(|r| r.variant.clone())
        .collect();
    assert_eq!(variants, ["first", "even", "default", "even"]);

    // Switches off: everything is the default.
    let (engine, doc) = setup();
    let default = story(&doc, &["DEFAULT"]);
    let other = story(&doc, &["OTHER"]);
    first(
        &doc,
        &[
            ("region.header.default", bind(default)),
            ("region.header.first", bind(other)),
            ("region.header.even", bind(other)),
        ],
    );
    pages(&doc, 3);
    let s = engine.layout(&doc);
    assert!((0..3).all(|p| run_text(&s, p, "header").unwrap() == "DEFAULT"));
}

#[test]
fn an_unset_first_does_not_hide_even_and_an_explicit_none_stops_fallback() {
    let (engine, doc) = setup();
    let default = story(&doc, &["DEFAULT"]);
    let even = story(&doc, &["EVEN"]);
    first(
        &doc,
        &[
            ("region.header.default", bind(default)),
            ("region.header.even", bind(even)),
            ("region.footer.default", bind(default)),
            ("region.footer.first", json!("")),
            ("region.first", json!(true)),
            ("region.even", json!(true)),
        ],
    );
    pages(&doc, 3);
    let s = engine.layout(&doc);
    // The header's `first` is unset, so page 1 falls to default and page 2 to even.
    assert_eq!(run_text(&s, 0, "header").unwrap(), "DEFAULT");
    assert_eq!(run_text(&s, 1, "header").unwrap(), "EVEN");
    // The footer's explicit None for `first` selects the empty region on page 1.
    assert_eq!(run_text(&s, 0, "footer"), None);
    assert_eq!(run_text(&s, 1, "footer").unwrap(), "DEFAULT");
}

#[test]
fn three_sections_inherit_and_an_explicit_none_hides_the_chain() {
    let (engine, doc) = setup();
    let head = story(&doc, &["HEAD ONE"]);
    let other = story(&doc, &["HEAD TWO"]);
    first(&doc, &[("region.header.default", bind(head))]);
    let nodes: Vec<_> = (0..5).map(|i| body(&doc, &format!("s{i}"))).collect();
    // B inherits, C says none, D inherits that none, E binds another story.
    boundary(&doc, nodes[1], BreakKind::Section, &[]);
    boundary(
        &doc,
        nodes[2],
        BreakKind::Section,
        &[("region.header.default", json!(""))],
    );
    boundary(&doc, nodes[3], BreakKind::Section, &[]);
    boundary(
        &doc,
        nodes[4],
        BreakKind::Section,
        &[("region.header.default", bind(other))],
    );
    let s = engine.layout(&doc);
    assert_eq!(s.pages.len(), 5);
    let shown: Vec<_> = (0..5).map(|p| run_text(&s, p, "header")).collect();
    assert_eq!(
        shown,
        [
            Some("HEAD ONE".to_owned()),
            Some("HEAD ONE".to_owned()),
            None,
            None,
            Some("HEAD TWO".to_owned())
        ]
    );
    let stories: Vec<_> = instances(&s, "header").iter().map(|r| r.story).collect();
    assert_eq!(stories, [head, head, other]);
}

#[test]
fn a_binding_to_something_that_is_not_a_live_story_renders_empty_and_is_reported() {
    let (engine, doc) = setup();
    let ghost = story(&doc, &["gone"]);
    let live = story(&doc, &["LIVE"]);
    doc.delete_block(ghost).unwrap();
    first(&doc, &[("region.header.default", bind(live))]);
    let nodes: Vec<_> = (0..2).map(|i| body(&doc, &format!("s{i}"))).collect();
    // The second section names a deleted story: it renders empty and never
    // falls through to the earlier one.
    boundary(
        &doc,
        nodes[1],
        BreakKind::Section,
        &[("region.header.default", bind(ghost))],
    );
    let s = engine.layout(&doc);
    assert_eq!(run_text(&s, 0, "header").unwrap(), "LIVE");
    assert_eq!(run_text(&s, 1, "header"), None);
    let missing: Vec<_> = s.diagnostics_with("layout.running-missing").collect();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].severity, reprise_diag::Severity::Warning);
    assert_eq!(missing[0].subject, crate::Subject::Node(nodes[1]));
}

#[test]
fn header_and_footer_share_the_page_and_both_may_be_tall() {
    let (engine, doc) = setup();
    let tall = "word ".repeat(4000);
    let head = story(&doc, &[&tall]);
    let foot = story(&doc, &[&tall]);
    first(
        &doc,
        &[
            ("region.header.default", bind(head)),
            ("region.footer.default", bind(foot)),
        ],
    );
    let node = body(&doc, "body");
    let s = engine.layout(&doc);
    let page = &s.sections.as_ref().unwrap().pages[0];
    // 792 high, 72 + 72 margins: D = 648, keep = 162. The header may take
    // down to 522 pt: 43 whole 12 pt lines, 516 pt, so it needs 36 + 516 = 552
    // (top = 552). The footer may then take 792 - 552 - 162 - 36 = 42 pt: three
    // lines, 36 + 36 = 72, which the margin already covers. The body keeps 168
    // pt, never less than its quarter.
    let line = &s.block(node).unwrap().lines[0];
    let frame = &s.frames[line.frame];
    assert_eq!(frame.rect.height, Length::from_pt(168));
    let (top, bottom) = page.reserve;
    assert_eq!(
        Length::from_pt(792) - Length::from_pt(72) - top - Length::from_pt(72) - bottom,
        Length::from_pt(168)
    );
    assert!(frame.rect.height >= Length::from_pt(162));
    // Both are clipped and reported; the footer keeps its fitting prefix.
    let h = instances(&s, "header")[0];
    let f = instances(&s, "footer")[0];
    assert!(h.clipped && f.clipped);
    let footer_frame = &s.frames[f.frame];
    let bottom_of_frame = footer_frame
        .to_page
        .apply(reprise_geom::Point::new(
            Length::ZERO,
            footer_frame.rect.height,
        ))
        .y;
    assert_eq!(
        bottom_of_frame,
        Length::from_pt(792 - 36),
        "it ends footer.distance from the edge"
    );
    let last = f.blocks[0].lines.last().unwrap();
    assert!(last.rect.origin.y + last.rect.height <= footer_frame.rect.height);
    assert_eq!(s.diagnostics_with("layout.running-clipped").count(), 2);
}

#[test]
fn one_story_in_two_slots_is_two_instances_on_each_page() {
    let (engine, doc) = setup();
    let both = story(&doc, &["BOTH"]);
    first(
        &doc,
        &[
            ("region.header.default", bind(both)),
            ("region.footer.default", bind(both)),
        ],
    );
    pages(&doc, 3);
    let s = engine.layout(&doc);
    assert_eq!(instances(&s, "header").len(), 3);
    assert_eq!(instances(&s, "footer").len(), 3);
    for page in 0..3 {
        let h = instances(&s, "header")[page];
        let f = instances(&s, "footer")[page];
        assert_ne!(h.frame, f.frame);
        assert_eq!(h.story, f.story);
    }
}

#[test]
fn a_background_paints_below_the_body_and_reserves_nothing() {
    let (engine, doc) = setup();
    let mark = story(&doc, &["WATERMARK"]);
    first(&doc, &[("region.background.default", bind(mark))]);
    let node = body(&doc, "body text");
    let s = engine.layout(&doc);
    let r = instances(&s, "background")[0];
    let frame = &s.frames[r.frame];
    assert_eq!(
        (frame.rect.width, frame.rect.height),
        (Length::from_pt(612), Length::from_pt(792))
    );
    assert_eq!(
        s.sections.as_ref().unwrap().pages[0].reserve,
        (Length::ZERO, Length::ZERO)
    );
    let list = s.to_display_list(0, crate::DisplayOptions::default());
    // The first drawn group is the background's, clipped to its frame; the
    // body's is after it.
    let reprise_display::Item::Group { clip, items, .. } = &list.items[0] else {
        panic!("{:?}", list.items[0]);
    };
    assert!(clip.is_some());
    assert!(
        items
            .iter()
            .any(|i| matches!(i, reprise_display::Item::Glyphs(_)))
    );
    assert!(s.block(node).is_some());
}

#[test]
fn headers_paint_above_the_body_clipped_to_their_frames() {
    let (engine, doc) = setup();
    let head = story(&doc, &["HEAD"]);
    first(&doc, &[("region.header.default", bind(head))]);
    body(&doc, "body");
    let s = engine.layout(&doc);
    let list = s.to_display_list(0, crate::DisplayOptions::default());
    let last = list.items.last().unwrap();
    let reprise_display::Item::Group { clip, .. } = last else {
        panic!("{last:?}");
    };
    assert!(clip.is_some(), "the header is the last group, clipped");
    let reprise_display::Item::Group { clip, .. } = &list.items[0] else {
        panic!()
    };
    assert!(clip.is_none(), "the body's group comes first");
}

#[test]
fn story_lists_number_themselves_inside_a_header() {
    let (engine, doc) = setup();
    let head = story(&doc, &["alpha", "beta"]);
    let list = doc
        .create_list(&reprise_doc::ListDefinition::numbered())
        .unwrap();
    for b in doc.top(Domain::Story(head)) {
        doc.set_list_item(b, list, 0).unwrap();
    }
    first(&doc, &[("region.header.default", bind(head))]);
    body(&doc, "body");
    let s = engine.layout(&doc);
    let r = instances(&s, "header")[0];
    let markers: Vec<_> = r
        .blocks
        .iter()
        .flat_map(|b| b.lines.iter())
        .filter_map(|l| l.marker.as_ref().map(|m| m.text.clone()))
        .collect();
    assert_eq!(markers, ["1.", "2."]);
}

#[test]
fn stories_with_notes_are_reported_unsupported() {
    let (engine, doc) = setup();
    let blocks = [
        NewBlock::new(BlockKind::Paragraph, "s", "plain"),
        NewBlock::new(BlockKind::Annotation, "s", "a note in a story"),
    ];
    let head = doc.create_story(&blocks).unwrap();
    first(&doc, &[("region.header.default", bind(head))]);
    body(&doc, "body");
    let s = engine.layout(&doc);
    let r = instances(&s, "header")[0];
    assert_eq!(text_of(r), "plain");
    assert!(r.clipped);
    let d: Vec<_> = s.diagnostics_with("layout.story-unsupported").collect();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].severity, reprise_diag::Severity::Error);
}

#[test]
fn an_empty_story_reserves_nothing_and_an_empty_paragraph_has_its_strut() {
    let (engine, doc) = setup();
    let none = doc.create_story(&[]).unwrap();
    let strut = story(&doc, &[""]);
    first(
        &doc,
        &[
            ("region.header.default", bind(none)),
            ("region.footer.default", bind(strut)),
        ],
    );
    body(&doc, "body");
    let s = engine.layout(&doc);
    let h = instances(&s, "header")[0];
    assert!(h.blocks.is_empty() && !h.clipped);
    assert_eq!(s.frames[h.frame].rect.height, Length::ZERO);
    let f = instances(&s, "footer")[0];
    assert_eq!(f.blocks.len(), 1);
    assert_eq!(
        s.frames[f.frame].rect.height,
        Length::from_pt(12),
        "the strut's line height"
    );
}

#[test]
fn a_filler_page_shows_regions_only_when_its_requester_says_so() {
    for running in [false, true] {
        let (engine, doc) = setup();
        let head = story(&doc, &["HEAD"]);
        first(&doc, &[("region.header.default", bind(head))]);
        body(&doc, "a");
        let b = body(&doc, "b");
        let mut keys = vec![("start", json!("odd-page"))];
        if running {
            keys.push(("filler.regions", json!("running")));
        }
        boundary(&doc, b, BreakKind::Section, &keys);
        let s = engine.layout(&doc);
        let kinds: Vec<_> = s
            .sections
            .as_ref()
            .unwrap()
            .pages
            .iter()
            .map(|p| p.kind)
            .collect();
        assert_eq!(
            kinds,
            [PageKind::Content, PageKind::Filler, PageKind::Content]
        );
        assert_eq!(run_text(&s, 1, "header").is_some(), running);
        // The filler has no body frames either way.
        assert!(
            s.frames.iter().filter(|f| f.page == 1).all(
                |f| matches!(&f.role, FrameRole::Flow(n) if n.starts_with("reprise.running."))
            )
        );
    }
}

#[test]
fn a_huge_story_reads_a_bounded_prefix() {
    let (engine, doc) = setup();
    let texts: Vec<String> = (0..100_000).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let head = story(&doc, &refs);
    first(&doc, &[("region.header.default", bind(head))]);
    pages(&doc, 3);
    doc.commit();
    let started = std::time::Instant::now();
    let s = engine.layout(&doc);
    assert!(started.elapsed().as_secs() < 120, "{:?}", started.elapsed());
    for r in instances(&s, "header") {
        assert!(r.clipped);
        assert!(r.blocks.len() <= 4096);
        assert!(text_of(r).starts_with("line 0|line 1"));
    }
    assert_eq!(s.diagnostics_with("layout.running-clipped").count(), 1);
}

#[test]
fn a_one_mebibyte_paragraph_is_read_whole_and_composed_to_the_frame() {
    let (engine, doc) = setup();
    let big = "word ".repeat((1 << 20) / 5);
    let head = story(&doc, &[&big]);
    first(&doc, &[("region.header.default", bind(head))]);
    pages(&doc, 3);
    doc.commit();
    let started = std::time::Instant::now();
    let s = engine.layout(&doc);
    assert!(started.elapsed().as_secs() < 120, "{:?}", started.elapsed());
    let rs = instances(&s, "header");
    assert_eq!(rs.len(), 3);
    assert!(rs.iter().all(|r| r.clipped && !r.blocks.is_empty()));
}
