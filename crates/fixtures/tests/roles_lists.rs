//! Lists through layout (`docs/roles-and-lists.md`): nested and mixed markers,
//! numbering across body paragraphs and restarts, roman numerals past 3,999,
//! marker geometry in right-to-left and vertical text, list indents against
//! paragraph indents, the first-run marker style, wide markers, continuation
//! pages, empty items and incremental layout after list edits. Compact
//! goldens are `roles_lists__*`.

use std::collections::{BTreeMap, BTreeSet};

use reprise_diag::Severity;
use reprise_doc::formatting::TextStyle;
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    Authored, BlockKind, Dim, Document, Expr, LengthExpr, LevelDef, ListDefinition, ListId, NodeId,
    NumberFormat, PageTemplate, Property, Style, WritingMode,
};
use reprise_fixtures::templates::flow_frame;
use reprise_fixtures::{PEER, engine};
use reprise_geom::{FrameSpace, Length, Point, Rect};
use reprise_layout::incremental::LayoutSession;
use reprise_layout::{BlockLayout, Engine, LayoutSnapshot, LineLayout, MarkerLayout};
use serde_json::{Value, json};

const LATIN: &str = "The quick brown fox jumps over the lazy dog while the patient \
                     reader waits for the paragraph to wrap across several lines.";
const HEBREW: &str = "שלום עולם זהו טקסט בעברית שנכתב כדי לבדוק הזחות ומרווחים בשורות \
                      רבות של פסקה אחת ארוכה מאוד.";
const RED: [u8; 4] = [200, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 200, 255];

fn pt(n: i32) -> Length {
    Length::from_pt(n)
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
        size: Some(LengthExpr::Pt(pt(10))),
        ..Style::default()
    }
}

/// A document with one main frame per page, `width` by `depth` points, laid
/// out in `mode`.
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
        &PageTemplate::new("lists", Dim::pt(width + 40), Dim::pt(depth + 40)).with_frame(frame),
    )
    .unwrap();
    doc
}

/// A paragraph that is not in a list.
fn paragraph(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "", text).unwrap()
}

/// A paragraph at `level` of `list`.
fn item(doc: &Document, list: ListId, level: u8, text: &str) -> NodeId {
    let node = paragraph(doc, text);
    doc.set_list_item(node, list, level).unwrap();
    node
}

fn block(s: &LayoutSnapshot, node: NodeId) -> &BlockLayout {
    s.blocks.iter().find(|b| b.node == node).unwrap()
}

/// A block's first line and its marker.
fn first_line(s: &LayoutSnapshot, node: NodeId) -> (&LineLayout, &MarkerLayout) {
    let line = &block(s, node).lines[0];
    (line, line.marker.as_ref().unwrap())
}

/// Each node's first-line marker text, or `""` when it has none.
fn texts(s: &LayoutSnapshot, nodes: &[NodeId]) -> Vec<String> {
    nodes
        .iter()
        .map(|&n| {
            block(s, n)
                .lines
                .first()
                .and_then(|l| l.marker.as_ref())
                .map_or_else(String::new, |m| m.text.clone())
        })
        .collect()
}

fn codes(s: &LayoutSnapshot) -> Vec<&str> {
    s.diagnostics.iter().map(|d| d.code.as_str()).collect()
}

/// The marker's box in its frame: from its start, over the line's block extent.
fn marker_box(line: &LineLayout, m: &MarkerLayout) -> Rect<FrameSpace> {
    Rect::new(
        Point::new(m.x, line.rect.origin.y),
        m.width,
        line.rect.height,
    )
}

fn line_json(s: &LayoutSnapshot, l: &LineLayout) -> Value {
    let marker = l.marker.as_ref().map(|m| {
        let page = s
            .frames
            .get(l.frame)
            .map(|f| f.to_page.bounds(&marker_box(l, m)));
        json!([
            m.text,
            m.x.0,
            m.baseline.0,
            m.width.0,
            page.map(|r| json!([r.origin.x.0, r.origin.y.0, r.width.0, r.height.0])),
        ])
    });
    json!([
        l.frame,
        l.text.start,
        l.text.end,
        l.rect.origin.x.0,
        l.rect.origin.y.0,
        l.rect.width.0,
        marker,
    ])
}

/// A compact golden: each item's block, level, value and marker; per block,
/// each line's frame, bytes, origin, room and marker (text, inline position,
/// baseline, width and page box); then the diagnostic codes.
fn summary(doc: &Document, s: &LayoutSnapshot) -> Value {
    let index = |node: NodeId| doc.blocks().iter().position(|b| *b == node);
    let numbering = doc.list_numbering();
    let items: Vec<Value> = numbering
        .sequence
        .iter()
        .filter_map(|n| {
            numbering
                .items
                .get(n)
                .map(|item| json!([index(*n), item.level, item.value, item.marker]))
        })
        .collect();
    let blocks: Vec<Value> = s
        .blocks
        .iter()
        .map(|b| {
            json!({
                "block": index(b.node),
                "lines": b.lines.iter().map(|l| line_json(s, l)).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "items": items, "blocks": blocks, "diagnostics": codes(s) })
}

/// Nine levels, each with its own marker: decimal, lower alpha, lower roman,
/// upper alpha, upper roman, leading zero, a bullet, a literal dash, and a
/// decimal counter for level nine. Indents step 18pt, hanging 18pt.
fn nine_levels() -> ListDefinition {
    let markers = [
        (NumberFormat::Decimal, "%1."),
        (NumberFormat::LowerAlpha, "%2)"),
        (NumberFormat::LowerRoman, "%3."),
        (NumberFormat::UpperAlpha, "%4."),
        (NumberFormat::UpperRoman, "(%5)"),
        (NumberFormat::DecimalLeadingZero, "%6."),
        (NumberFormat::Bullet, "•"),
        (NumberFormat::None, "–"),
        (NumberFormat::Decimal, "%9."),
    ];
    ListDefinition {
        base: None,
        levels: markers
            .into_iter()
            .enumerate()
            .map(|(n, (format, text))| {
                let level = LevelDef {
                    format: Some(format),
                    text: Some(text.into()),
                    indent: Some(pt(36 + 18 * i32::try_from(n).unwrap())),
                    hanging: Some(pt(18)),
                    ..LevelDef::default()
                };
                (u8::try_from(n).unwrap(), level)
            })
            .collect(),
    }
}

/// The level of each of nineteen items.
const SEQUENCE: [u8; 19] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 8, 7, 6, 5, 4, 3, 2, 1, 0, 1];
/// Each item's marker, worked out by hand: a counter resets when a shallower
/// level advances, and a list continues across an item of another list.
const MARKERS: [&str; 19] = [
    "1.", "a)", "i.", "A.", "(I)", "01.", "•", "–", "1.", "2.", "–", "•", "02.", "(II)", "B.",
    "ii.", "b)", "2.", "a)",
];

#[test]
fn nested_mixed_lists_to_depth_nine() {
    let doc = document(WritingMode::HorizontalTb, 300, 400);
    let list = doc.create_list(&nine_levels()).unwrap();
    let bullets = doc.create_list(&ListDefinition::bulleted()).unwrap();
    let mut items = Vec::new();
    for (i, &level) in SEQUENCE.iter().enumerate() {
        if i == 4 {
            // An item of another list between two items of this one.
            let aside = paragraph(&doc, "Aside.");
            doc.set_list_item(aside, bullets, 0).unwrap();
        }
        items.push(item(&doc, list, level, &format!("Item {}", i + 1)));
    }
    doc.commit();
    let s = engine().layout(&doc);
    assert_eq!(texts(&s, &items), MARKERS);
    for (&node, &level) in items.iter().zip(&SEQUENCE) {
        let (line, m) = first_line(&s, node);
        // Each level's text starts 18pt further in, its marker 18pt before that.
        assert_eq!(line.rect.origin.x, pt(36 + 18 * i32::from(level)));
        assert_eq!(m.x, pt(18 + 18 * i32::from(level)));
    }
    insta::assert_json_snapshot!("nested_mixed_nine_levels", summary(&doc, &s));
}

#[test]
fn numbering_continues_across_body_paragraphs_and_restarts() {
    let doc = document(WritingMode::HorizontalTb, 300, 400);
    let list = doc
        .create_list(&ListDefinition {
            base: None,
            levels: BTreeMap::from([(
                0,
                LevelDef {
                    start: Some(5),
                    ..LevelDef::default()
                },
            )]),
        })
        .unwrap();
    let a = item(&doc, list, 0, "First");
    let b = item(&doc, list, 0, "Second");
    let body = paragraph(&doc, "A body paragraph between two runs of items.");
    let c = item(&doc, list, 0, "Third");
    let d = item(&doc, list, 0, "Restarts at one");
    let e = item(&doc, list, 0, "Fourth");
    let f = item(&doc, list, 0, "Restarts at ten");
    let g = item(&doc, list, 0, "Eleven");
    let h = item(&doc, list, 0, "Restarts at three, then continues");
    doc.set_list_restart(d, Some(1)).unwrap();
    doc.set_list_restart(f, Some(10)).unwrap();
    doc.set_list_restart(h, Some(3)).unwrap();
    doc.commit();
    let s = engine().layout(&doc);
    let listed = [a, b, c, d, e, f, g, h];
    assert_eq!(
        texts(&s, &listed),
        ["5.", "6.", "7.", "1.", "2.", "10.", "11.", "3."]
    );
    assert!(
        block(&s, body).lines[0].marker.is_none(),
        "body has no marker"
    );
    insta::assert_json_snapshot!("numbering_restart_pending", summary(&doc, &s));

    // Continuing the previous list: the restart is cleared, so the item takes
    // the number after its predecessor.
    doc.set_list_restart(h, None).unwrap();
    doc.commit();
    let s = engine().layout(&doc);
    assert_eq!(texts(&s, &[h]), ["12."]);
    insta::assert_json_snapshot!("numbering_across_body_and_restart", summary(&doc, &s));
}

#[test]
fn roman_numerals_past_3999_fall_back_to_decimal_with_a_warning() {
    let doc = document(WritingMode::HorizontalTb, 300, 400);
    let list = doc
        .create_list(&ListDefinition {
            base: None,
            levels: BTreeMap::from([(
                0,
                LevelDef {
                    format: Some(NumberFormat::UpperRoman),
                    start: Some(3999),
                    ..LevelDef::default()
                },
            )]),
        })
        .unwrap();
    let nodes: Vec<NodeId> = ["Last roman", "First past", "Second past"]
        .iter()
        .map(|text| item(&doc, list, 0, text))
        .collect();
    doc.commit();
    let s = engine().layout(&doc);
    assert_eq!(texts(&s, &nodes), ["MMMCMXCIX.", "4000.", "4001."]);
    let warned: Vec<_> = s
        .diagnostics
        .iter()
        .filter(|d| d.code.as_str() == "list.counter-range")
        .collect();
    assert_eq!(warned.len(), 2, "one per item past the range");
    assert!(warned.iter().all(|d| d.severity == Severity::Warning));
    insta::assert_json_snapshot!("roman_past_3999", summary(&doc, &s));
}

/// A list item's first line in `mode`, with `text` (right-to-left when `rtl`):
/// the marker sits in the hanging indent before the text, on the first line's
/// baseline, and the text keeps its indent.
fn check_marker_placement(name: &str, mode: WritingMode, text: &str, rtl: bool) {
    let doc = document(mode, 200, 400);
    let list = doc.create_list(&ListDefinition::numbered()).unwrap();
    let first = item(&doc, list, 0, text);
    let second = item(&doc, list, 0, text);
    doc.commit();
    let s = engine().layout(&doc);
    assert_eq!(texts(&s, &[first, second]), ["1.", "2."], "{name}");
    let lines = &block(&s, first).lines;
    assert!(lines.len() > 1, "{name}: wraps");
    // The logical inline size: a vertical frame's lines run along its height.
    let measure = if mode == WritingMode::HorizontalTb {
        pt(200)
    } else {
        pt(400)
    };
    let (line, m) = first_line(&s, first);
    assert_eq!(m.baseline, line.baseline, "{name}: on the first baseline");
    let room_end = line.rect.origin.x + line.rect.width;
    if rtl {
        assert_eq!(
            m.x + m.width,
            measure - pt(18),
            "{name}: hangs from the right"
        );
        assert!(m.x >= room_end, "{name}: marker is outside the room");
        assert_eq!(measure - room_end, pt(36), "{name}: indent from the right");
    } else {
        assert_eq!(m.x, pt(18), "{name}: hangs from the left");
        assert!(
            m.x + m.width <= line.rect.origin.x,
            "{name}: outside the room"
        );
        assert_eq!(line.rect.origin.x, pt(36), "{name}: indent from the left");
    }
    for later in &lines[1..] {
        assert!(later.marker.is_none(), "{name}: only the first line");
    }
    insta::assert_json_snapshot!(name, summary(&doc, &s));
}

#[test]
fn markers_horizontal_left_to_right() {
    check_marker_placement(
        "markers_horizontal_ltr",
        WritingMode::HorizontalTb,
        LATIN,
        false,
    );
}

#[test]
fn markers_horizontal_right_to_left() {
    check_marker_placement(
        "markers_horizontal_rtl",
        WritingMode::HorizontalTb,
        HEBREW,
        true,
    );
}

#[test]
fn markers_vertical_rl() {
    check_marker_placement(
        "markers_vertical_rl_ltr",
        WritingMode::VerticalRl,
        LATIN,
        false,
    );
}

#[test]
fn markers_vertical_rl_right_to_left() {
    check_marker_placement(
        "markers_vertical_rl_rtl",
        WritingMode::VerticalRl,
        HEBREW,
        true,
    );
}

#[test]
fn markers_vertical_lr() {
    check_marker_placement(
        "markers_vertical_lr_ltr",
        WritingMode::VerticalLr,
        LATIN,
        false,
    );
}

#[test]
fn markers_vertical_lr_right_to_left() {
    check_marker_placement(
        "markers_vertical_lr_rtl",
        WritingMode::VerticalLr,
        HEBREW,
        true,
    );
}

#[test]
fn list_start_and_hanging_replace_paragraph_indent_start_and_first() {
    let doc = document(WritingMode::HorizontalTb, 200, 400);
    let list = doc
        .create_list(&ListDefinition {
            base: None,
            levels: BTreeMap::from([(
                0,
                LevelDef {
                    indent: Some(pt(40)),
                    hanging: Some(pt(25)),
                    ..LevelDef::default()
                },
            )]),
        })
        .unwrap();
    // The paragraph's own indents: start 50pt, first line 30pt back, end 20pt.
    let overrides = with(
        Style::default(),
        &[
            (Property::IndentStart, "50pt"),
            (Property::IndentFirst, "-30pt"),
            (Property::IndentEnd, "20pt"),
        ],
    );
    let listed = item(&doc, list, 0, LATIN);
    doc.set_overrides(listed, &overrides).unwrap();
    let plain = paragraph(&doc, LATIN);
    doc.set_overrides(plain, &overrides).unwrap();
    doc.commit();
    let s = engine().layout(&doc);

    // Without a list, the paragraph's indents apply: 20pt on the first line,
    // 50pt on the rest, and the end indent leaves 130pt.
    let plain_lines = &block(&s, plain).lines;
    assert_eq!(plain_lines[0].rect.origin.x, pt(20));
    assert_eq!(plain_lines[1].rect.origin.x, pt(50));
    assert_eq!(plain_lines[1].rect.width, pt(130));

    // With a list, the definition's indent (40pt) and hanging (25pt) take the
    // place of both; the end indent still applies, leaving 140pt.
    let lines = &block(&s, listed).lines;
    assert!(lines.len() > 1, "wraps");
    let (first, m) = first_line(&s, listed);
    assert_eq!(m.x, pt(15), "40pt less the 25pt hang");
    assert_eq!(first.rect.origin.x, pt(40));
    for line in lines {
        assert_eq!(line.rect.width, pt(140), "indent end still applies");
    }
    for later in &lines[1..] {
        assert_eq!(later.rect.origin.x, pt(40));
    }
    insta::assert_json_snapshot!("list_indents_over_paragraph", summary(&doc, &s));
}

#[test]
fn marker_inherits_the_first_run_style_and_the_definition_patch_overrides_it() {
    let doc = document(WritingMode::HorizontalTb, 200, 400);
    let numbered = doc.create_list(&ListDefinition::numbered()).unwrap();
    let patched = doc
        .create_list(&ListDefinition {
            base: None,
            levels: BTreeMap::from([(
                0,
                LevelDef {
                    marker: Some(TextStyle {
                        color: Some(BLUE),
                        size: Some(pt(6)),
                        ..TextStyle::default()
                    }),
                    ..LevelDef::default()
                },
            )]),
        })
        .unwrap();
    let plain = item(&doc, numbered, 0, "Plain first run");
    let lead = item(&doc, numbered, 0, "Lead first run");
    let patch = item(&doc, patched, 0, "Patched first run");
    doc.commit();
    // A 14pt red lead on each of the two items; the plain one keeps 10pt black.
    let red = TextStyle {
        color: Some(RED),
        size: Some(pt(14)),
        ..TextStyle::default()
    };
    doc.format_text(lead, 0..4, &red, RangePolicy::EXPANDING)
        .unwrap();
    doc.format_text(patch, 0..7, &red, RangePolicy::EXPANDING)
        .unwrap();
    doc.commit();
    let s = engine().layout(&doc);

    assert_eq!(texts(&s, &[plain, lead, patch]), ["1.", "2.", "1."]);
    let colors = |m: &MarkerLayout| {
        let set: BTreeSet<[u8; 4]> = m.runs.iter().map(|r| r.color).collect();
        set.into_iter().collect::<Vec<_>>()
    };
    let (_, plain_m) = first_line(&s, plain);
    let (_, lead_m) = first_line(&s, lead);
    let (_, patch_m) = first_line(&s, patch);
    assert_eq!(colors(lead_m), [RED], "takes the first run's colour");
    assert!(lead_m.width > plain_m.width, "and its 14pt size");
    assert_eq!(colors(patch_m), [BLUE], "the definition's patch wins");
    assert!(patch_m.width < plain_m.width, "and its 6pt size");
    insta::assert_json_snapshot!("marker_first_run_and_patch", summary(&doc, &s));
}

#[test]
fn a_marker_wider_than_the_hanging_indent_pushes_the_first_line() {
    let doc = document(WritingMode::HorizontalTb, 200, 400);
    let list = doc
        .create_list(&ListDefinition {
            base: None,
            levels: BTreeMap::from([(
                0,
                LevelDef {
                    text: Some("Section %1:".into()),
                    indent: Some(pt(36)),
                    hanging: Some(pt(4)),
                    ..LevelDef::default()
                },
            )]),
        })
        .unwrap();
    let node = item(&doc, list, 0, LATIN);
    doc.commit();
    let s = engine().layout(&doc);
    let lines = &block(&s, node).lines;
    let (first, m) = first_line(&s, node);
    assert_eq!(m.text, "Section 1:");
    assert!(m.width > pt(4), "wider than the 4pt hang");
    assert_eq!(m.x, pt(32), "36pt less the 4pt hang");
    // The first line starts after the marker and a quarter em of the marker's
    // size; later lines keep the indent.
    let gap = pt(10).mul_ratio(1, 4);
    assert_eq!(first.rect.origin.x, m.x + m.width + gap);
    assert!(first.rect.origin.x > pt(36));
    for later in &lines[1..] {
        assert_eq!(later.rect.origin.x, pt(36));
    }
    assert!(
        first.rect.width < lines[1].rect.width,
        "the pushed line is narrower"
    );
    insta::assert_json_snapshot!("wide_marker_pushes_first_line", summary(&doc, &s));
}

#[test]
fn only_the_first_line_of_a_continued_item_has_a_marker() {
    let doc = document(WritingMode::HorizontalTb, 200, 60);
    let list = doc.create_list(&ListDefinition::numbered()).unwrap();
    let long = item(&doc, list, 0, &LATIN.repeat(4));
    let after = item(&doc, list, 0, "After.");
    doc.commit();
    let s = engine().layout(&doc);
    let lines = &block(&s, long).lines;
    let frames: BTreeSet<usize> = lines.iter().map(|l| l.frame).collect();
    assert!(frames.len() > 1, "the item continues onto a later page");
    assert!(lines[0].marker.is_some());
    for later in &lines[1..] {
        assert!(later.marker.is_none(), "no marker on a continuation line");
        assert_eq!(later.rect.origin.x, pt(36), "the indent continues");
    }
    assert_eq!(texts(&s, &[long, after]), ["1.", "2."]);
    insta::assert_json_snapshot!("continuation_pages", summary(&doc, &s));
}

#[test]
fn an_empty_item_shows_its_marker() {
    let doc = document(WritingMode::HorizontalTb, 200, 400);
    let list = doc.create_list(&ListDefinition::numbered()).unwrap();
    let empty = item(&doc, list, 0, "");
    let nested = item(&doc, list, 1, "");
    let next = item(&doc, list, 0, "Next");
    doc.commit();
    let s = engine().layout(&doc);
    assert_eq!(texts(&s, &[empty, nested, next]), ["1.", "a.", "2."]);
    let (line, m) = first_line(&s, empty);
    assert_eq!(line.text, 0..0, "no text, only the marker");
    assert_eq!(m.x, pt(18));
    assert_eq!(line.rect.origin.x, pt(36));
    insta::assert_json_snapshot!("empty_item", summary(&doc, &s));
}

/// The session's snapshot matches a full layout, as JSON.
fn same_as_full(step: &str, engine: &Engine, session: &mut LayoutSession<'_>, doc: &Document) {
    let incremental = session.layout(doc).unwrap();
    assert_eq!(
        incremental.to_json(),
        engine.layout(doc).to_json(),
        "{step}"
    );
}

#[test]
fn incremental_layout_matches_full_after_inserting_removing_and_restarting() {
    let doc = document(WritingMode::HorizontalTb, 200, 400);
    let list = doc.create_list(&ListDefinition::numbered()).unwrap();
    paragraph(&doc, "Opening body text.");
    let items: Vec<NodeId> = (0..6)
        .map(|i| item(&doc, list, 0, &format!("Item {i}")))
        .collect();
    doc.commit();
    let engine = engine();
    let mut session = LayoutSession::new(&engine);
    same_as_full("first layout", &engine, &mut session, &doc);

    // Insert an item before the third: the items after it renumber.
    let index = doc.blocks().iter().position(|b| *b == items[2]).unwrap();
    let inserted = item(&doc, list, 0, "Inserted");
    doc.move_block(inserted, None, index).unwrap();
    doc.commit();
    same_as_full("insert", &engine, &mut session, &doc);
    let s = engine.layout(&doc);
    assert_eq!(
        texts(
            &s,
            &[
                items[0], items[1], inserted, items[2], items[3], items[4], items[5]
            ]
        ),
        ["1.", "2.", "3.", "4.", "5.", "6.", "7."]
    );

    // Remove the first item: everything after it moves up one.
    doc.delete_block(items[0]).unwrap();
    doc.commit();
    same_as_full("remove", &engine, &mut session, &doc);
    let s = engine.layout(&doc);
    assert_eq!(
        texts(
            &s,
            &[items[1], inserted, items[2], items[3], items[4], items[5]]
        ),
        ["1.", "2.", "3.", "4.", "5.", "6."]
    );

    // Restart at the fourth: the items after it renumber from ten.
    doc.set_list_restart(items[3], Some(10)).unwrap();
    doc.commit();
    same_as_full("restart", &engine, &mut session, &doc);
    let s = engine.layout(&doc);
    assert_eq!(
        texts(&s, &[items[2], items[3], items[4], items[5]]),
        ["3.", "10.", "11.", "12."]
    );

    // Continue the list again: the numbers return to the plain sequence.
    doc.set_list_restart(items[3], None).unwrap();
    doc.commit();
    same_as_full("continue", &engine, &mut session, &doc);
    let s = engine.layout(&doc);
    assert_eq!(
        texts(&s, &[items[2], items[3], items[4], items[5]]),
        ["3.", "4.", "5.", "6."]
    );
    insta::assert_json_snapshot!("incremental_renumbered", summary(&doc, &s));
}
