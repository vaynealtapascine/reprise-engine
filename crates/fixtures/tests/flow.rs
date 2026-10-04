//! Frames, columns, pagination and fragmentation (24), and the page templates
//! that drive them (05, 34).

use reprise_doc::text::RangePolicy;
use reprise_doc::{BlockKind, Document, FrameRole, Medium, NodeId, SchemaRegistry};
use reprise_fixtures::spike::{define_styles, follow};
use reprise_fixtures::templates::{
    flow_frame, long_text, margin_frame, responsive_columns, two_columns,
};
use reprise_fixtures::{PEER, engine};
use reprise_geom::Length;
use reprise_layout::{LayoutSnapshot, LineRef, TemplateSource};

fn document_with(template: &reprise_doc::PageTemplate) -> Document {
    let doc = Document::new(PEER).unwrap();
    define_styles(&doc).unwrap();
    doc.set_page_template(template).unwrap();
    doc
}

/// The snapshot frame indices a block's lines sit in, in order, each once.
fn frames_of(snapshot: &LayoutSnapshot, node: NodeId) -> Vec<usize> {
    let mut frames: Vec<usize> = snapshot
        .block(node)
        .unwrap()
        .lines
        .iter()
        .map(|l| l.frame)
        .collect();
    frames.dedup();
    frames
}

fn lines_in_frame(snapshot: &LayoutSnapshot, node: NodeId, frame: usize) -> Vec<usize> {
    let block = snapshot.block(node).unwrap();
    (0..block.lines.len())
        .filter(|&i| block.lines[i].frame == frame)
        .collect()
}

#[test]
fn a_paragraph_threads_down_column_one_into_column_two_and_onto_page_two() {
    let doc = document_with(&two_columns());
    let text = long_text(3);
    let p = doc
        .append_block(BlockKind::Paragraph, "body", &text)
        .unwrap();
    doc.commit();
    let snapshot = engine().layout(&doc);

    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(snapshot.template.name, "two-columns");
    assert_eq!(snapshot.template.source, TemplateSource::Document);
    assert!(snapshot.pages.len() >= 2, "{} pages", snapshot.pages.len());
    // Three frames a page: left, right, margin. The paragraph never touches
    // the margin frame, and moves left, right, then the next page's left.
    let frames = frames_of(&snapshot, p);
    assert_eq!(&frames[..3], &[0, 1, 3], "{frames:?}");
    for &f in &frames {
        let frame = snapshot.frame(f).unwrap();
        assert_eq!(frame.role, FrameRole::Flow("main".into()));
    }
    assert_eq!(snapshot.frame(0).unwrap().name, "left");
    assert_eq!(snapshot.frame(1).unwrap().name, "right");
    assert_eq!(snapshot.frame(3).unwrap().page, 1);
    // A full column holds six lines, the last of which ends within the frame.
    for f in [0, 1] {
        assert_eq!(lines_in_frame(&snapshot, p, f).len(), 6);
    }
    let depth = snapshot.frame(0).unwrap().rect.height;
    for line in &snapshot.block(p).unwrap().lines {
        let frame = snapshot.frame(line.frame).unwrap();
        assert!(line.rect.origin.y + line.rect.height <= depth.max(frame.rect.height));
    }
}

#[test]
fn a_block_fragments_mid_paragraph_with_contiguous_lines() {
    let doc = document_with(&two_columns());
    let text = long_text(2);
    let p = doc
        .append_block(BlockKind::Paragraph, "body", &text)
        .unwrap();
    let snapshot = engine().layout(&doc);
    let block = snapshot.block(p).unwrap();

    let mut at = 0;
    for line in &block.lines {
        assert_eq!(line.text.start, at, "lines are contiguous across frames");
        at = line.text.end;
    }
    assert_eq!(at, text.len(), "and cover all of the text");

    // The break between column one and two is mid-paragraph: the last line in
    // frame 0 ends exactly where the first line in frame 1 starts, and
    // continues at the top of its frame.
    let first_col = lines_in_frame(&snapshot, p, 0);
    let second_col = lines_in_frame(&snapshot, p, 1);
    let last = &block.lines[*first_col.last().unwrap()];
    let next = &block.lines[second_col[0]];
    assert_eq!(last.text.end, next.text.start);
    assert!(last.text.end < text.len());
    assert_eq!(
        next.rect.origin.y,
        Length::ZERO,
        "no spacing at a frame's top"
    );
    // The queries see through the fragmentation.
    let at_break = snapshot.line_containing(p, next.text.start).unwrap();
    assert_eq!(snapshot.line(at_break).unwrap().frame, 1);
    assert_eq!(snapshot.page_of(at_break), Some(0));
}

#[test]
fn the_next_paragraph_follows_a_fragment_with_spacing() {
    let doc = document_with(&two_columns());
    let short = doc
        .append_block(BlockKind::Paragraph, "body", "A short opening paragraph.")
        .unwrap();
    let long = doc
        .append_block(BlockKind::Paragraph, "body", &long_text(1))
        .unwrap();
    let snapshot = engine().layout(&doc);

    let spacing = engine().flow.paragraph_spacing;
    let short_end = {
        let l = snapshot.block(short).unwrap().lines.last().unwrap();
        l.rect.origin.y + l.rect.height
    };
    let first = &snapshot.block(long).unwrap().lines[0];
    assert_eq!(first.frame, snapshot.block(short).unwrap().lines[0].frame);
    assert_eq!(first.rect.origin.y, short_end + spacing);
    assert!(
        frames_of(&snapshot, long).len() > 1,
        "and then it continues"
    );
}

#[test]
fn a_frame_too_short_for_the_block_is_passed_over() {
    use reprise_doc::{Dim, PageTemplate};
    let template = PageTemplate::new("sliver-first", Dim::pt(300), Dim::pt(200))
        .with_frame(flow_frame(
            "sliver",
            Dim::pt(20),
            Dim::pt(20),
            Dim::pt(100),
            Dim::pt(5),
        ))
        .with_frame(flow_frame(
            "column",
            Dim::pt(140),
            Dim::pt(20),
            Dim::pt(100),
            Dim::pt(100),
        ));
    let doc = document_with(&template);
    let p = doc
        .append_block(
            BlockKind::Paragraph,
            "body",
            "Some text that wraps a few times over.",
        )
        .unwrap();
    let snapshot = engine().layout(&doc);
    assert_eq!(
        frames_of(&snapshot, p),
        [1],
        "skipped the sliver, no overflow"
    );
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
}

#[test]
fn annotations_follow_lines_onto_page_two() {
    let doc = document_with(&two_columns());
    let text = long_text(3);
    let p = doc
        .append_block(BlockKind::Paragraph, "body", &text)
        .unwrap();
    doc.commit();
    let snapshot = engine().layout(&doc);

    // Two target lines at the same depth in the left column of pages 1 and 2.
    let block = snapshot.block(p).unwrap();
    let pick = |frame: usize| {
        let i = lines_in_frame(&snapshot, p, frame)[2];
        block.lines[i].text.start
    };
    let (on_first, on_second) = (pick(0), pick(3));
    assert_eq!(snapshot.page_of(LineRef { node: p, line: 0 }), Some(0));

    let schemas = SchemaRegistry::builtin();
    let mut notes = Vec::new();
    for at in [on_first, on_second] {
        let n = doc
            .append_block(BlockKind::Annotation, "note", "A note.")
            .unwrap();
        let r = doc.add_range(p, at..at + 1, RangePolicy::FIXED).unwrap();
        doc.add_relation(&schemas, &follow(n, r)).unwrap();
        notes.push(n);
    }
    doc.commit();
    let snapshot = engine().layout(&doc);

    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
    for (n, page) in notes.iter().zip([0, 1]) {
        let first = LineRef { node: *n, line: 0 };
        let frame = snapshot.line(first).unwrap().frame;
        assert_eq!(
            snapshot.frame(frame).unwrap().role,
            FrameRole::Margin,
            "placed in a margin frame"
        );
        assert_eq!(snapshot.page_of(first), Some(page), "on its target's page");
        assert_eq!(snapshot.margin_frame_on(page), Some(frame));
    }
    // Both targets are the same distance down their columns, so the notes sit
    // at the same height on their own pages. A margin frame that tracked its
    // fill across pages would have pushed the second one down.
    let tops: Vec<_> = notes
        .iter()
        .map(|&n| {
            snapshot
                .line_bounds(LineRef { node: n, line: 0 })
                .unwrap()
                .1
                .origin
                .y
        })
        .collect();
    assert_eq!(tops[0], tops[1]);
    assert_eq!(snapshot.diagnostics_with("relation.pushed").count(), 0);
}

#[test]
fn notes_on_one_page_still_avoid_each_other() {
    let doc = document_with(&two_columns());
    let p = doc
        .append_block(BlockKind::Paragraph, "body", &long_text(1))
        .unwrap();
    let schemas = SchemaRegistry::builtin();
    for at in [0, 1] {
        let n = doc
            .append_block(BlockKind::Annotation, "note", "A note on the first line.")
            .unwrap();
        let r = doc.add_range(p, at..at + 1, RangePolicy::FIXED).unwrap();
        doc.add_relation(&schemas, &follow(n, r)).unwrap();
    }
    let snapshot = engine().layout(&doc);
    assert_eq!(snapshot.diagnostics_with("relation.pushed").count(), 1);
}

#[test]
fn a_template_sized_by_the_medium_reflows_when_the_medium_changes() {
    let doc = document_with(&responsive_columns());
    let p = doc
        .append_block(BlockKind::Paragraph, "body", &long_text(2))
        .unwrap();
    doc.commit();

    let mut narrow = engine();
    narrow.medium = Medium::new(Length::from_pt(400), Length::from_pt(260));
    let mut wide = engine();
    wide.medium = Medium::new(Length::from_pt(800), Length::from_pt(400));
    let (a, b) = (narrow.layout(&doc), wide.layout(&doc));

    // The medium is a layout input and the snapshot records it (38).
    assert_eq!(a.medium, narrow.medium);
    assert_eq!(b.medium, wide.medium);
    assert_eq!(
        (a.pages[0].width, a.pages[0].height),
        (Length::from_pt(400), Length::from_pt(260))
    );
    assert_eq!(
        (b.pages[0].width, b.pages[0].height),
        (Length::from_pt(800), Length::from_pt(400))
    );
    // 30% of the page: 120 pt against 240 pt wide.
    assert_eq!(a.frame(0).unwrap().rect.width, Length::from_pt(120));
    assert_eq!(b.frame(0).unwrap().rect.width, Length::from_pt(240));
    // The text reflows: a different number of lines, spread over different pages.
    let lines = |s: &LayoutSnapshot| s.block(p).unwrap().lines.len();
    assert!(lines(&a) > lines(&b), "{} vs {}", lines(&a), lines(&b));
    assert!(
        a.pages.len() > b.pages.len(),
        "{} vs {}",
        a.pages.len(),
        b.pages.len()
    );
    // And each is repeatable.
    assert_eq!(a.to_json(), narrow.layout(&doc).to_json());
    assert_ne!(a.to_json(), b.to_json());
    for s in [&a, &b] {
        assert!(s.diagnostics.is_empty(), "{:?}", s.diagnostics);
    }
}

#[test]
fn an_empty_document_gets_one_page_even_with_a_zero_page_limit() {
    let doc = document_with(&two_columns());
    let mut e = engine();
    e.flow.max_pages = 0;
    let snapshot = e.layout(&doc);
    assert_eq!(snapshot.pages.len(), 1);
    assert_eq!(snapshot.frames.len(), 3);
    assert!(snapshot.diagnostics.is_empty());

    // With a document that needs more, the limit is reported, not looped on.
    doc.append_block(BlockKind::Paragraph, "body", &long_text(3))
        .unwrap();
    let snapshot = e.layout(&doc);
    assert_eq!(snapshot.pages.len(), 1);
    assert_eq!(snapshot.diagnostics_with("layout.page-limit").count(), 1);
    assert!(
        snapshot
            .diagnostics_with("layout.text-unplaced")
            .next()
            .is_some()
    );
}

#[test]
fn the_default_template_is_the_spikes_geometry() {
    let doc = Document::new(PEER).unwrap();
    let snapshot = engine().layout(&doc);
    assert_eq!(snapshot.template.name, "default");
    assert_eq!(snapshot.template.source, TemplateSource::Builtin);
    let page = snapshot.pages[0];
    assert_eq!(
        (page.width, page.height),
        (Length::from_pt(420), Length::from_pt(300))
    );
    let main = snapshot.frame(0).unwrap();
    let margin = snapshot.frame(1).unwrap();
    assert_eq!(
        main.to_page.apply(reprise_geom::Point::origin()).x,
        Length::from_pt(36)
    );
    assert_eq!(main.rect.width, Length::from_pt(220));
    assert_eq!(
        margin.to_page.apply(reprise_geom::Point::origin()).x,
        Length::from_pt(274)
    );
    assert_eq!(margin.rect.width, Length::from_pt(110));
    assert_eq!(main.rect.height, Length::from_pt(228));
}

#[test]
fn a_replica_with_a_template_lays_out_the_same() {
    let doc = document_with(&two_columns());
    doc.append_block(BlockKind::Paragraph, "body", &long_text(2))
        .unwrap();
    doc.commit();
    let other = doc.fork(2).unwrap();
    assert_eq!(
        engine().layout(&doc).to_json(),
        engine().layout(&other).to_json()
    );
}

#[test]
fn a_margin_frame_per_page_is_its_own_frame() {
    // Two margin frames on a page: relations use the first one declared.
    let template = two_columns().with_frame(margin_frame(
        "second-margin",
        reprise_doc::Dim::pt(304),
        reprise_doc::Dim::pt(120),
        reprise_doc::Dim::pt(100),
        reprise_doc::Dim::pt(80),
    ));
    let doc = document_with(&template);
    let p = doc
        .append_block(BlockKind::Paragraph, "body", "Short text with a target.")
        .unwrap();
    let n = doc
        .append_block(BlockKind::Annotation, "note", "Note.")
        .unwrap();
    let r = doc.add_range(p, 0..5, RangePolicy::FIXED).unwrap();
    doc.add_relation(&SchemaRegistry::builtin(), &follow(n, r))
        .unwrap();
    let snapshot = engine().layout(&doc);
    let frame = snapshot.line(LineRef { node: n, line: 0 }).unwrap().frame;
    assert_eq!(snapshot.frame(frame).unwrap().name, "margin");
}
