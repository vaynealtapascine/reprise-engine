//! Paragraph properties through layout (`docs/paragraph-styles.md`): indents,
//! collapsing spacing, justification and keeps, in horizontal, right-to-left
//! and vertical text, with compact geometry goldens.

use reprise_diag::Severity;
use reprise_doc::marks::Alignment;
use reprise_doc::{
    Authored, BlockKind, Dim, Document, Expr, LengthExpr, NodeId, PageTemplate, Property, Style,
    StylePatch, WritingMode,
};
use reprise_fixtures::templates::flow_frame;
use reprise_fixtures::{PEER, engine};
use reprise_geom::Length;
use reprise_layout::LayoutSnapshot;
use reprise_layout::incremental::LayoutSession;
use serde_json::{Value, json};

const LATIN: &str = "The quick brown fox jumps over the lazy dog while the patient \
                     reader waits for the paragraph to wrap across several lines.";
const HEBREW: &str = "שלום עולם זהו טקסט בעברית שנכתב כדי לבדוק הזחות ומרווחים בשורות \
                      רבות של פסקה אחת ארוכה מאוד.";

#[test]
fn table_cell_uses_resolved_paragraph_indents() {
    let doc = document(WritingMode::HorizontalTb, 220, 400);
    let t = doc
        .append_table(reprise_doc::TableColumns {
            columns: vec![reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Fixed(Length::from_pt(220)),
            }],
        })
        .unwrap();
    let r = doc.append_table_row(t, false).unwrap();
    let c = doc.append_table_cell(r, 0).unwrap();
    let n = doc
        .append_cell_block(c, BlockKind::Paragraph, "", LATIN)
        .unwrap();
    doc.set_overrides(
        n,
        &with(
            Style::default(),
            &[
                (Property::IndentStart, "20pt"),
                (Property::IndentEnd, "10pt"),
                (Property::IndentFirst, "-5pt"),
            ],
        ),
    )
    .unwrap();
    doc.commit();
    let snapshot = engine().layout(&doc);
    let lines = &block(&snapshot, n).lines;
    assert!(lines.len() > 1);
    assert_eq!(lines[0].available.start, Length::from_pt(15));
    assert_eq!(lines[1].available.start, Length::from_pt(20));
    assert_eq!(lines[0].available.end, Length::from_pt(210));
}

fn length(text: &str) -> Authored {
    Authored::Expr(Expr::parse(text).unwrap())
}

fn with(mut style: Style, props: &[(Property, &str)]) -> Style {
    for (p, text) in props {
        style.set(*p, length(text));
    }
    style
}

fn body() -> Style {
    Style {
        size: Some(LengthExpr::Pt(Length::from_pt(10))),
        ..Style::default()
    }
}

/// A document with one main frame per page, `width` by `depth` points.
fn document(mode: WritingMode, width: i32, depth: i32) -> Document {
    let doc = Document::new(PEER).unwrap();
    doc.define_style("", &body()).unwrap();
    let mut frame = flow_frame(
        "main",
        Dim::pt(20),
        Dim::pt(20),
        Dim::pt(width),
        Dim::pt(depth),
    );
    frame.writing_mode = mode;
    doc.set_page_template(
        &PageTemplate::new("paragraphs", Dim::pt(width + 40), Dim::pt(depth + 40))
            .with_frame(frame),
    )
    .unwrap();
    doc
}

/// Per block, per line: frame, bytes, line box origin and width, content width.
fn summary(s: &LayoutSnapshot) -> Value {
    json!(
        s.blocks
            .iter()
            .map(|b| {
                b.lines
                    .iter()
                    .map(|l| {
                        json!([
                            l.frame,
                            l.text.start,
                            l.text.end,
                            l.rect.origin.x.0,
                            l.rect.origin.y.0,
                            l.rect.width.0,
                            l.width.0
                        ])
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    )
}

fn codes(s: &LayoutSnapshot) -> Vec<&str> {
    s.diagnostics.iter().map(|d| d.code.as_str()).collect()
}

fn block(s: &LayoutSnapshot, node: NodeId) -> &reprise_layout::BlockLayout {
    s.blocks.iter().find(|b| b.node == node).unwrap()
}

/// Indents (with a hanging first line), collapsed spacing and justification
/// in one document, laid out in every writing mode and direction.
fn properties(mode: WritingMode, text: &str) -> (Document, [NodeId; 3]) {
    let doc = document(mode, 200, 400);
    let first = doc.append_block(BlockKind::Paragraph, "", text).unwrap();
    doc.set_overrides(
        first,
        &with(
            Style::default(),
            &[
                (Property::IndentStart, "20pt"),
                (Property::IndentEnd, "10pt"),
                (Property::IndentFirst, "-15pt"),
                (Property::SpaceAfter, "12pt"),
            ],
        ),
    )
    .unwrap();
    let second = doc.append_block(BlockKind::Paragraph, "", text).unwrap();
    doc.set_overrides(
        second,
        &Style {
            alignment: Some(Alignment::Justify),
            ..with(Style::default(), &[(Property::SpaceBefore, "6pt")])
        },
    )
    .unwrap();
    let third = doc.append_block(BlockKind::Paragraph, "", text).unwrap();
    doc.set_overrides(
        third,
        &with(
            Style::default(),
            &[
                (Property::IndentStart, "10%"),
                (Property::SpaceBefore, "1lh"),
            ],
        ),
    )
    .unwrap();
    doc.commit();
    (doc, [first, second, third])
}

fn check_properties(name: &str, mode: WritingMode, text: &str, rtl: bool) {
    let (doc, [first, second, third]) = properties(mode, text);
    let s = engine().layout(&doc);
    let pt = Length::from_pt;
    // The logical inline size: a vertical frame's lines run along its height.
    let measure = if mode == WritingMode::HorizontalTb {
        pt(200)
    } else {
        pt(400)
    };
    // Indents: the first line hangs 15pt left of the others; the logical
    // start is on the right of a right-to-left paragraph.
    let lines = &block(&s, first).lines;
    assert!(lines.len() > 1, "{name}: wraps");
    let edge = |l: &reprise_layout::LineLayout| {
        if rtl {
            measure - (l.rect.origin.x + l.rect.width)
        } else {
            l.rect.origin.x
        }
    };
    let far = |l: &reprise_layout::LineLayout| {
        if rtl {
            l.rect.origin.x
        } else {
            measure - (l.rect.origin.x + l.rect.width)
        }
    };
    assert_eq!(edge(&lines[0]), pt(5), "{name}: hanging first line");
    assert_eq!(edge(&lines[1]), pt(20), "{name}: indent-start");
    assert_eq!(far(&lines[1]), pt(10), "{name}: indent-end");
    // Spacing collapses: max(12pt after, 6pt before), plus no engine spacing.
    let gap = block(&s, second).lines[0].rect.origin.y
        - (lines.last().unwrap().rect.origin.y + lines.last().unwrap().rect.height);
    assert_eq!(
        gap,
        pt(12) + engine().flow.paragraph_spacing,
        "{name}: collapse"
    );
    // Justified lines fill the measure; the last line keeps its width.
    let justified = &block(&s, second).lines;
    for l in &justified[..justified.len() - 1] {
        assert_ne!(
            l.explanation.adjustment.word_spacing,
            Length::ZERO,
            "{name}"
        );
        assert!((measure - l.width).0.abs() < 64, "{name}: {:?}", l.width);
    }
    let last = justified.last().unwrap();
    assert_eq!(last.explanation.adjustment.word_spacing, Length::ZERO);
    assert!(last.width < measure);
    // A percentage of the block's inline size.
    assert_eq!(
        edge(&block(&s, third).lines[1]),
        measure.mul_ratio(1, 10),
        "{name}: 10%"
    );
    assert!(
        !s.diagnostics
            .iter()
            .any(|d| d.code.as_str().starts_with("style.")
                || d.code.as_str() == "layout.indent-clamped"),
        "{name}: {:?}",
        codes(&s)
    );
    insta::assert_json_snapshot!(name, summary(&s));
}

#[test]
fn properties_horizontal_ltr() {
    check_properties(
        "properties_horizontal_ltr",
        WritingMode::HorizontalTb,
        LATIN,
        false,
    );
}

#[test]
fn properties_horizontal_rtl() {
    check_properties(
        "properties_horizontal_rtl",
        WritingMode::HorizontalTb,
        HEBREW,
        true,
    );
}

#[test]
fn properties_vertical_ltr() {
    check_properties(
        "properties_vertical_ltr",
        WritingMode::VerticalRl,
        LATIN,
        false,
    );
}

#[test]
fn properties_vertical_rtl() {
    check_properties(
        "properties_vertical_rtl",
        WritingMode::VerticalRl,
        HEBREW,
        true,
    );
}

#[test]
fn space_before_is_truncated_at_the_top_of_a_page() {
    let doc = document(WritingMode::HorizontalTb, 200, 60);
    let filler = doc.append_block(BlockKind::Paragraph, "", LATIN).unwrap();
    let spaced = doc
        .append_block(BlockKind::Paragraph, "", "Spaced.")
        .unwrap();
    doc.set_overrides(
        spaced,
        &with(Style::default(), &[(Property::SpaceBefore, "40pt")]),
    )
    .unwrap();
    doc.commit();
    let s = engine().layout(&doc);
    let line = &block(&s, spaced).lines[0];
    assert_ne!(
        line.frame,
        block(&s, filler).lines[0].frame,
        "moved to a new page"
    );
    assert_eq!(line.rect.origin.y, Length::ZERO, "space before truncated");
    insta::assert_json_snapshot!("space_truncated_at_page_top", summary(&s));
}

#[test]
fn heading_keeps_with_next_across_a_page_break() {
    let build = |depth: i32, keep: bool| {
        let doc = document(WritingMode::HorizontalTb, 200, depth);
        let filler = doc.append_block(BlockKind::Paragraph, "", LATIN).unwrap();
        let heading = doc
            .append_block(BlockKind::Paragraph, "Heading 2", "A heading")
            .unwrap();
        if !keep {
            doc.patch_named_style(
                "Heading 2",
                // Clearing would reveal the built-in's own keep: set false.
                &StylePatch {
                    set: Style {
                        keep_with_next: Some(false),
                        ..Style::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let next = doc.append_block(BlockKind::Paragraph, "", LATIN).unwrap();
        doc.commit();
        let s = engine().layout(&doc);
        let frame = |n| block(&s, n).lines[0].frame;
        let frames = (frame(filler), frame(heading), frame(next));
        (s, frames)
    };
    // The depths at which, without the keep, the heading ends page one.
    let depths: Vec<i32> = (40..200)
        .filter(|&d| {
            let (_, (f, h, n)) = build(d, false);
            f == h && h != n
        })
        .collect();
    assert!(!depths.is_empty(), "some depth strands the heading");
    for &depth in &depths {
        let (kept, (f, h, n)) = build(depth, true);
        assert_ne!(f, h, "at {depth}pt the heading moves to page two");
        assert_eq!(h, n, "and stays with its paragraph");
        assert!(!codes(&kept).contains(&"layout.keep-unsatisfied"));
    }
    let (kept, _) = build(depths[0], true);
    insta::assert_json_snapshot!("heading_kept_with_next", summary(&kept));
}

#[test]
fn widows_and_orphans_move_lines_and_paragraphs() {
    // Seven lines in a frame that takes five after a short paragraph.
    let lines_per_frame = |s: &LayoutSnapshot, node| {
        let mut counts = std::collections::BTreeMap::<usize, usize>::new();
        for l in &block(s, node).lines {
            *counts.entry(l.frame).or_default() += 1;
        }
        counts.into_values().collect::<Vec<_>>()
    };
    let build = |widows: Option<u8>, orphans: Option<u8>, together: bool| {
        let doc = document(WritingMode::HorizontalTb, 120, 72);
        doc.append_block(BlockKind::Paragraph, "", "Opening.")
            .unwrap();
        let long = doc
            .append_block(BlockKind::Paragraph, "", &LATIN[..120])
            .unwrap();
        doc.set_overrides(
            long,
            &Style {
                widows,
                orphans,
                keep_together: together.then_some(true),
                ..Style::default()
            },
        )
        .unwrap();
        doc.commit();
        let s = engine().layout(&doc);
        (lines_per_frame(&s, long), s)
    };
    let (plain, _) = build(None, None, false);
    assert!(plain.len() == 2 && *plain.last().unwrap() < 3, "{plain:?}");
    let (widows, s) = build(Some(3), None, false);
    assert!(*widows.last().unwrap() >= 3, "{widows:?}");
    assert_eq!(plain.iter().sum::<usize>(), widows.iter().sum::<usize>());
    insta::assert_json_snapshot!("widows", summary(&s));
    let (orphans, s) = build(None, Some(32), false);
    assert_eq!(orphans.len(), 1, "moved whole: {orphans:?}");
    insta::assert_json_snapshot!("orphans", summary(&s));
    let (together, _) = build(None, None, true);
    assert_eq!(together.len(), 1, "{together:?}");
}

#[test]
fn incremental_layout_matches_full_after_a_style_edit() {
    let doc = document(WritingMode::HorizontalTb, 200, 120);
    doc.define_named_style("Quote", &Style::default()).unwrap();
    let mut quoted = Vec::new();
    for i in 0..8 {
        let style = if i % 2 == 0 { "Quote" } else { "" };
        let n = doc
            .append_block(BlockKind::Paragraph, style, LATIN)
            .unwrap();
        if i % 2 == 0 {
            quoted.push(n);
        }
    }
    doc.commit();
    let engine = engine();
    let mut session = LayoutSession::new(&engine);
    let before = session.layout(&doc).unwrap();
    for patch in [
        with(Style::default(), &[(Property::IndentStart, "24pt")]),
        Style {
            keep_with_next: Some(true),
            alignment: Some(Alignment::Justify),
            ..with(Style::default(), &[(Property::SpaceBefore, "2lh")])
        },
    ] {
        doc.patch_named_style(
            "Quote",
            &StylePatch {
                set: patch,
                ..Default::default()
            },
        )
        .unwrap();
        doc.commit();
        let cached = session.layout(&doc).unwrap();
        assert_eq!(cached.to_json(), engine.layout(&doc).to_json());
    }
    let after = session.layout(&doc).unwrap();
    for n in quoted {
        assert_ne!(block(&before, n).lines, block(&after, n).lines);
    }
    // Editing the paragraph after a keep-with-next one invalidates the
    // heading's placement too.
    let last = *doc.top(reprise_doc::Domain::Body).last().unwrap();
    doc.set_overrides(
        last,
        &with(Style::default(), &[(Property::SpaceBefore, "90pt")]),
    )
    .unwrap();
    doc.commit();
    assert_eq!(
        session.layout(&doc).unwrap().to_json(),
        engine.layout(&doc).to_json()
    );
}

#[test]
fn hostile_indents_spacing_and_keep_storms_terminate() {
    let doc = document(WritingMode::HorizontalTb, 200, 80);
    let absurd = doc.append_block(BlockKind::Paragraph, "", LATIN).unwrap();
    let mut style = with(
        Style::default(),
        &[
            (Property::IndentStart, "100000pt"),
            (Property::IndentEnd, "1000%"),
            (Property::IndentFirst, "-100000pt"),
            (Property::SpaceBefore, "-50pt"),
            (Property::SpaceAfter, "-1000000pt"),
        ],
    );
    style.keep_together = Some(true);
    doc.set_overrides(absurd, &style).unwrap();
    // Keep-with-next on every paragraph of a long document.
    doc.define_named_style(
        "Chain",
        &Style {
            keep_with_next: Some(true),
            widows: Some(32),
            orphans: Some(32),
            ..Style::default()
        },
    )
    .unwrap();
    for _ in 0..120 {
        doc.append_block(BlockKind::Paragraph, "Chain", LATIN)
            .unwrap();
    }
    doc.commit();
    let engine = engine();
    let s = engine.layout(&doc);
    let codes = codes(&s);
    assert!(codes.contains(&"layout.indent-clamped"), "{codes:?}");
    assert!(codes.contains(&"layout.style-clamped"), "{codes:?}");
    assert!(codes.contains(&"layout.keep-unsatisfied"), "{codes:?}");
    assert!(
        !s.diagnostics.iter().any(|d| d.severity == Severity::Error),
        "everything is placed: {codes:?}"
    );
    let line = &block(&s, absurd).lines[0];
    assert!(line.rect.width >= Length::from_pt(50), "a quarter remains");
    // Every byte of every block is still laid out, in order.
    for b in &s.blocks {
        assert_eq!(b.lines.last().unwrap().text.end, b.text.len());
    }
    // Deterministic, and the incremental path agrees.
    assert_eq!(s.to_json(), engine.layout(&doc).to_json());
    let mut session = LayoutSession::new(&engine);
    assert_eq!(session.layout(&doc).unwrap().to_json(), s.to_json());
}
