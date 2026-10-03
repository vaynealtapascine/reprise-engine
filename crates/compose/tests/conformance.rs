//! Composer conformance: every composer, against adversarial geometry and
//! texts, checked against every guarantee on the [`Composer`] trait.
//!
//! The texts are shaped with the bundled font through `reprise-shape`, as
//! the crate's own tests do (depending on `reprise-fixtures` would be a cycle).

use std::ops::Range;

use reprise_compose::{
    AuthoredBreak, Available, Break, BreakKind, BreakReason, ComposeRequest, Composer, Composition,
    GeometryProvider, Greedy, Interval, Limits, LineFragment, LineQuery, Measure, Optimal, Polygon,
    Runaround, Turnover, break_opportunities, is_word_space,
};
use reprise_diag::Severity;
use reprise_font::{Face, FontStore};
use reprise_geom::{FrameSpace, Length, Point, Rect};
use reprise_shape::{
    HarfRust, Item, ParagraphInput, Reshape, ShapedRun, ShapedText, Shaper, StyleRun, itemize,
};

const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");

fn pt(v: i32) -> Length {
    Length::from_pt(v)
}

/// Every composer, and variants that force each of their code paths.
fn composers() -> Vec<Box<dyn Composer>> {
    let starved = Optimal {
        limits: Limits {
            max_active: 1,
            ..Limits::default()
        },
        ..Optimal::default()
    };
    let broke = Optimal {
        limits: Limits {
            max_candidates: 5,
            ..Limits::default()
        },
        ..Optimal::justified()
    };
    let extreme = Optimal {
        line_penalty: i64::MAX,
        fitness_demerits: i64::MIN,
        ragged_stretch: 0,
        stretch: u32::MAX,
        shrink: u32::MAX,
        ..Optimal::default()
    };
    vec![
        Box::new(Greedy),
        Box::new(Optimal::default()),
        Box::new(Optimal::justified()),
        Box::new(starved),
        Box::new(broke),
        Box::new(extreme),
        Box::new(AuthoredBreak::default()),
        Box::new(AuthoredBreak {
            turnover_indent: pt(30),
            turnover: Turnover::FirstFit,
        }),
        Box::new(AuthoredBreak {
            turnover_indent: Length::MAX,
            turnover: Turnover::Optimal(Optimal::justified()),
        }),
    ]
}

struct Shaped {
    text: String,
    fonts: FontStore,
    items: Vec<Item>,
}

impl Shaped {
    fn new(text: &str) -> Shaped {
        let mut fonts = FontStore::default();
        fonts.add(Face::from_bytes(SERIF).expect("the bundled font loads"));
        let styles = [StyleRun {
            range: 0..text.len(),
            families: vec!["Source Serif Pro".into()],
            size: pt(10),
            language: None,
            features: Vec::new(),
        }];
        let items = itemize(
            &ParagraphInput {
                text,
                styles: &styles,
                direction: None,
            },
            &fonts,
        )
        .items;
        Shaped {
            text: text.into(),
            fonts,
            items,
        }
    }

    fn shaper(&self) -> Shaper<'_> {
        Shaper {
            text: &self.text,
            items: &self.items,
            fonts: &self.fonts,
            adapter: &HarfRust,
        }
    }
}

// ---------------------------------------------------------------------------
// Adversarial geometry.

/// The region ends before the first line.
struct EndNow;
impl GeometryProvider for EndNow {
    fn available(&self, _: &LineQuery<'_>) -> Available {
        Available::End
    }
}

/// Always skips, moving down a line height (or 1pt).
struct SkipForever;
impl GeometryProvider for SkipForever {
    fn available(&self, q: &LineQuery<'_>) -> Available {
        Available::Skip {
            next: q.block_offset + q.line_height.max(pt(1)),
        }
    }
}

/// Skips without moving down.
struct SkipStuck;
impl GeometryProvider for SkipStuck {
    fn available(&self, q: &LineQuery<'_>) -> Available {
        Available::Skip {
            next: q.block_offset,
        }
    }
}

/// Skips up the page.
struct SkipUp;
impl GeometryProvider for SkipUp {
    fn available(&self, q: &LineQuery<'_>) -> Available {
        Available::Skip {
            next: q.block_offset - pt(5),
        }
    }
}

/// Creeps down one sub-unit at a time.
struct Creep;
impl GeometryProvider for Creep {
    fn available(&self, q: &LineQuery<'_>) -> Available {
        Available::Skip {
            next: q.block_offset + Length(1),
        }
    }
}

/// Room with no intervals, forever.
struct NoIntervals;
impl GeometryProvider for NoIntervals {
    fn available(&self, _: &LineQuery<'_>) -> Available {
        Available::Room(Vec::new())
    }
}

/// Only zero-width and reversed intervals.
struct Degenerate;
impl GeometryProvider for Degenerate {
    fn available(&self, _: &LineQuery<'_>) -> Available {
        Available::Room(vec![
            Interval::new(pt(10), pt(10)),
            Interval::new(pt(50), pt(5)),
            Interval::new(Length::MAX, Length::MIN),
        ])
    }
}

/// Room and skips alternating every 12pt down the block axis.
struct Alternating;
impl GeometryProvider for Alternating {
    fn available(&self, q: &LineQuery<'_>) -> Available {
        if (q.block_offset.0 / pt(12).0).rem_euclid(2) == 0 {
            Available::Room(vec![Interval::new(pt(0), pt(70))])
        } else {
            Available::Skip {
                next: q.block_offset + pt(12),
            }
        }
    }
}

/// Intervals at the edges of the fixed-point range.
struct NearMax;
impl GeometryProvider for NearMax {
    fn available(&self, _: &LineQuery<'_>) -> Available {
        Available::Room(vec![
            Interval::new(Length(i32::MAX - 100), Length::MAX),
            Interval::new(Length::MIN, Length::MAX),
            Interval::new(Length::MAX - pt(40), Length::MAX),
        ])
    }
}

/// Three intervals, the first too narrow for most words.
struct Split;
impl GeometryProvider for Split {
    fn available(&self, _: &LineQuery<'_>) -> Available {
        Available::Room(vec![
            Interval::new(pt(0), pt(4)),
            Interval::new(pt(10), pt(60)),
            Interval::new(pt(70), pt(120)),
        ])
    }
}

/// Narrows by 15pt for every line already composed, then skips once, then
/// ends: everything depends on `previous`.
struct Narrowing;
impl GeometryProvider for Narrowing {
    fn available(&self, q: &LineQuery<'_>) -> Available {
        let n = q.previous.len() as i32;
        if n == 4 && q.line < 6 {
            return Available::Skip {
                next: q.block_offset + pt(3),
            };
        }
        if n >= 9 {
            return Available::End;
        }
        Available::Room(vec![Interval::new(pt(0), pt(150 - 15 * n))])
    }
}

/// Wider for lines that start right after a forced break in the previous
/// fragment's text, and with a second interval once anything is composed.
struct LooksBack;
impl GeometryProvider for LooksBack {
    fn available(&self, q: &LineQuery<'_>) -> Available {
        match q.previous.last() {
            None => Available::Room(vec![Interval::new(pt(0), pt(80))]),
            Some(l) if l.explanation.reason == BreakReason::Forced => {
                Available::Room(vec![Interval::new(pt(0), pt(200))])
            }
            Some(_) => Available::Room(vec![
                Interval::new(pt(0), pt(40)),
                Interval::new(pt(50), pt(90)),
            ]),
        }
    }
}

fn circle() -> Polygon {
    Polygon::circle(Point::new(pt(80), pt(80)), pt(80), 16)
}

fn notched() -> Polygon {
    let p = |x: i32, y: i32| Point::<FrameSpace>::new(pt(x), pt(y));
    Polygon::new([
        p(0, 0),
        p(60, 0),
        p(60, 40),
        p(90, 40),
        p(90, 0),
        p(160, 0),
        p(160, 200),
        p(0, 200),
    ])
}

fn runaround() -> Runaround<Measure> {
    let figure = Polygon::rect(Rect::new(Point::new(pt(50), pt(15)), pt(40), pt(40)));
    Runaround {
        base: Measure(pt(160)),
        exclusions: vec![figure, circle()],
        margin: pt(4),
        min_width: pt(20),
    }
}

fn geometries() -> Vec<(&'static str, Box<dyn GeometryProvider>)> {
    vec![
        ("measure", Box::new(Measure(pt(60)))),
        ("wide", Box::new(Measure(pt(400)))),
        ("zero", Box::new(Measure(Length::ZERO))),
        ("negative", Box::new(Measure(pt(-50)))),
        ("max", Box::new(Measure(Length::MAX))),
        ("end-now", Box::new(EndNow)),
        ("skip-forever", Box::new(SkipForever)),
        ("skip-stuck", Box::new(SkipStuck)),
        ("skip-up", Box::new(SkipUp)),
        ("creep", Box::new(Creep)),
        ("no-intervals", Box::new(NoIntervals)),
        ("degenerate", Box::new(Degenerate)),
        ("alternating", Box::new(Alternating)),
        ("near-max", Box::new(NearMax)),
        ("split", Box::new(Split)),
        ("narrowing", Box::new(Narrowing)),
        ("looks-back", Box::new(LooksBack)),
        ("circle", Box::new(circle())),
        ("notched", Box::new(notched())),
        ("runaround", Box::new(runaround())),
    ]
}

// ---------------------------------------------------------------------------
// Texts.

const TEXTS: &[&str] = &[
    "",
    "\n\n\n",
    "\u{2028}",
    "incomprehensibilities",
    "the quick brown fox\njumps over the lazy dog\u{2028}and then sleeps for a while",
    "Zalgo: Z\u{335}\u{322}\u{31b}\u{332}a\u{335}\u{322}l\u{338}g\u{334}o, then \
     e\u{301}e\u{301}e\u{301} and a\u{30a}\u{30a}\u{30a}\u{30a}\u{30a}\u{30a} stacked",
    "office affluent final fluffy waffle shuffle ffi ffl efficient",
    "trailing spaces   \n   and leading ones  ",
];

/// Breaks that a careless caller might pass: duplicates where one is forced,
/// breaks inside characters and past the end, extreme penalties, unsorted.
fn hostile_breaks(text: &str) -> Vec<Break> {
    let mut breaks = break_opportunities(text);
    for b in &mut breaks {
        b.penalty = if b.at % 2 == 0 { i32::MAX } else { i32::MIN };
    }
    let mut extra = Vec::new();
    // Authored breaks inside every ligature-prone pair, allowed and forced.
    for (i, _) in text.match_indices("ff").chain(text.match_indices("fi")) {
        extra.push(Break {
            at: i + 1,
            kind: BreakKind::Allowed,
            penalty: 50,
        });
    }
    if let Some(b) = breaks.first().copied() {
        extra.push(Break {
            at: b.at,
            kind: BreakKind::Forced,
            penalty: 0,
        });
    }
    for at in [1, 2, 3, text.len() + 7] {
        extra.push(Break {
            at,
            kind: BreakKind::Forced,
            penalty: -1,
        });
    }
    // Authored breaks appended after the UAX #14 ones and sorted stably, as
    // a caller would: at a shared offset the allowed break comes first.
    breaks.extend(extra);
    breaks.sort_by_key(|b| b.at);
    // And not quite sorted.
    if breaks.len() > 2 {
        breaks.swap(0, 2);
    }
    breaks
}

/// The breaks a composer may honour: on character boundaries, inside the text.
fn valid(text: &str, breaks: &[Break]) -> Vec<Break> {
    breaks
        .iter()
        .filter(|b| b.at <= text.len() && text.is_char_boundary(b.at))
        .copied()
        .collect()
}

fn is_forced_at(breaks: &[Break], at: usize) -> bool {
    breaks
        .iter()
        .any(|b| b.at == at && b.kind == BreakKind::Forced)
}

/// Marks every glyph unsafe to break, so every edge has to be reshaped.
fn all_unsafe(shaped: &ShapedText) -> ShapedText {
    let mut s = shaped.clone();
    for g in s.runs.iter_mut().flat_map(|r| r.glyphs.iter_mut()) {
        g.unsafe_to_break = true;
    }
    s
}

// ---------------------------------------------------------------------------
// The guarantees.

struct Case<'a> {
    label: String,
    request: ComposeRequest<'a>,
}

fn check(composer: &dyn Composer, case: &Case<'_>, c: &Composition) {
    let name = format!("{} / {}", composer.name(), case.label);
    let r = &case.request;
    let text = r.text;
    let len = text.len();
    let mut start = r.start.min(len);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let breaks = valid(text, r.breaks);
    let codes: Vec<&str> = c.notes.iter().map(|n| n.code.as_str()).collect();
    for n in &c.notes {
        assert!(
            matches!(
                n.code.as_str(),
                "compose.overflow" | "compose.geometry-stalled" | "compose.fallback"
            ),
            "{name}: unknown code {}",
            n.code
        );
        if n.code == "compose.fallback" {
            assert_eq!(n.severity, Severity::Info, "{name}");
        }
    }

    // Fragments cover the text contiguously from `start` to `rest` or the end.
    let mut at = start;
    for l in &c.lines {
        assert_eq!(l.text.start, at, "{name}: contiguous");
        assert!(
            l.text.end > l.text.start || len == 0,
            "{name}: a fragment with no text"
        );
        at = l.text.end;
    }
    if let Some(rest) = c.rest {
        assert!(rest <= len && rest >= start, "{name}: rest {rest} in range");
    }
    assert_eq!(at, c.rest.unwrap_or(len), "{name}: reaches rest or the end");
    if len == 0 && c.rest.is_none() {
        assert_eq!(c.lines.len(), 1, "{name}: empty text gets one line");
        assert_eq!(c.lines[0].text, 0..0);
    }
    if codes.contains(&"compose.geometry-stalled") {
        assert!(c.rest.is_some(), "{name}: a stall leaves a rest");
    }

    for (i, l) in c.lines.iter().enumerate() {
        let Range { start: s, end: e } = l.text.clone();
        // Lines break only at breaks (or the end).
        assert!(
            e == len || breaks.iter().any(|b| b.at == e) || len == 0,
            "{name}: fragment {s}..{e} ends off a break"
        );
        // Forced breaks always end a line.
        assert!(
            !breaks
                .iter()
                .any(|b| b.kind == BreakKind::Forced && b.at > s && b.at < e),
            "{name}: fragment {s}..{e} runs through a forced break"
        );
        if is_forced_at(&breaks, e)
            && let Some(next) = c.lines.get(i + 1)
        {
            assert!(
                next.line > l.line,
                "{name}: forced break at {e} ends the line"
            );
        }
        // An overflowing fragment is one unbreakable run, and reported. A
        // justified line is set at its natural width plus its word spacing,
        // which is rounded per space.
        let spaces = text[s..e]
            .trim_end()
            .chars()
            .filter(|&c| is_word_space(c))
            .count() as i32;
        let set = l.width + l.explanation.adjustment.word_spacing.mul_ratio(spaces, 1);
        if set > l.available.width() + Length(spaces) {
            assert!(
                !breaks.iter().any(|b| b.at > s && b.at < e),
                "{name}: overflowing fragment {s}..{e} holds a break"
            );
            assert!(
                c.notes
                    .iter()
                    .any(|n| n.code == "compose.overflow" && n.bytes == Some(s..e)),
                "{name}: overflow at {s}..{e} not reported"
            );
        }
        // Reasons agree with the text.
        match l.explanation.reason {
            BreakReason::Forced => assert!(is_forced_at(&breaks, e), "{name}: forced at {e}"),
            BreakReason::End => assert_eq!(e, len, "{name}: end"),
            BreakReason::Opportunity => assert!(e < len, "{name}: opportunity at the end"),
            BreakReason::Overflow => assert!(
                c.notes.iter().any(|n| n.code == "compose.overflow"
                    && n.bytes.as_ref().is_some_and(|b| b.start == s)),
                "{name}: overflow reason without a note"
            ),
            _ => {}
        }
        // A fragment whose edge is unsafe to break is reshaped.
        let unsafe_at = |at: usize| at > 0 && at < len && !r.shaped.is_safe_to_break(at);
        if unsafe_at(s) || unsafe_at(e) {
            assert!(l.explanation.reshaped, "{name}: {s}..{e} not reshaped");
        }
        for run in &l.runs {
            assert!(
                run.range.start >= s && run.range.end <= e,
                "{name}: run range"
            );
            for g in &run.glyphs {
                assert!(run.range.contains(&(g.cluster as usize)), "{name}: glyph");
            }
        }
        assert_eq!(l.height, r.line_height, "{name}: height");
        if let Some(prev) = i.checked_sub(1).and_then(|p| c.lines.get(p)) {
            assert!(l.line >= prev.line, "{name}: lines in order");
            if l.line == prev.line {
                assert_eq!(
                    l.block_offset, prev.block_offset,
                    "{name}: one offset per line"
                );
            }
        }
        if r.line_height >= Length::ZERO {
            assert!(
                c.block_end >= l.block_offset,
                "{name}: block_end below lines"
            );
        }
        // Scores: only the optimising composers have them, and lines without
        // one are explained by a fallback note.
        if composer.name() == "greedy" {
            assert_eq!(l.explanation.score, None, "{name}: greedy has no score");
        } else if l.explanation.score.is_none() {
            assert!(
                codes.contains(&"compose.fallback")
                    || composer.name() == "authored-break-first-fit",
                "{name}: an unscored line with no fallback note"
            );
        }
        if composer.name() == "greedy" || composer.name().starts_with("optimal") {
            assert_eq!(l.explanation.adjustment.letter_spacing, Length::ZERO);
        }
        if matches!(
            composer.name(),
            "greedy" | "optimal" | "authored-break-first-fit"
        ) {
            assert_eq!(
                l.explanation.adjustment.word_spacing,
                Length::ZERO,
                "{name}: only justified lines adjust"
            );
        }
    }

    // Each fragment sits in an interval geometry gave its line (possibly
    // moved in by a turnover indent), in inline order.
    let mut first_of_line = 0;
    for (i, l) in c.lines.iter().enumerate() {
        if i > 0 && l.line != c.lines[i - 1].line {
            first_of_line = i;
        }
        let answer = r.geometry.available(&LineQuery {
            line: l.line,
            block_offset: l.block_offset,
            line_height: r.line_height,
            previous: &c.lines[..first_of_line],
        });
        let Available::Room(intervals) = answer else {
            panic!("{name}: fragment on a line geometry gave no room: {answer:?}");
        };
        let find = |a: &Interval| {
            intervals.iter().position(|iv| iv == a).or_else(|| {
                intervals
                    .iter()
                    .position(|iv| iv.end == a.end && a.start > iv.start)
            })
        };
        let index = find(&l.available)
            .unwrap_or_else(|| panic!("{name}: {:?} is not one of {intervals:?}", l.available));
        if l.available != intervals[index] {
            assert!(
                composer.name().starts_with("authored-break") && index == 0,
                "{name}: only turnovers are indented"
            );
        }
        if i > first_of_line {
            let prev = find(&c.lines[i - 1].available).unwrap_or(usize::MAX);
            assert!(prev < index, "{name}: intervals in order");
        }
    }
}

fn run(case: &Case<'_>) {
    for composer in composers() {
        let c = composer.compose(&case.request);
        check(composer.as_ref(), case, &c);
        assert_eq!(
            c,
            composer.compose(&case.request),
            "{} / {}: deterministic",
            composer.name(),
            case.label
        );
    }
}

#[test]
fn every_composer_keeps_every_guarantee() {
    let heights = [pt(12), Length::ZERO, pt(-12), Length::MAX];
    for &text in TEXTS {
        let shaped_text = Shaped::new(text);
        let shaper = shaped_text.shaper();
        let shaped = shaper.shape();
        let unsafe_shaped = all_unsafe(&shaped);
        let plain = break_opportunities(text);
        let hostile = hostile_breaks(text);
        for (gname, geometry) in geometries() {
            for (hi, &line_height) in heights.iter().enumerate() {
                let variants: [(&str, &ShapedText, &[Break]); 3] = [
                    ("plain", &shaped, &plain),
                    ("hostile-breaks", &shaped, &hostile),
                    ("all-unsafe", &unsafe_shaped, &plain),
                ];
                for (vname, shaped, breaks) in variants {
                    // The extra variants only at the ordinary line height.
                    if hi > 0 && vname != "plain" {
                        continue;
                    }
                    run(&Case {
                        label: format!("{text:?} / {gname} / {line_height:?} / {vname}"),
                        request: ComposeRequest {
                            text,
                            shaped,
                            reshape: &shaper,
                            breaks,
                            line_height,
                            geometry: geometry.as_ref(),
                            start: 0,
                            block_start: Length::ZERO,
                        },
                    });
                }
            }
        }
    }
}

#[test]
fn every_composer_resumes_from_any_start() {
    let text = TEXTS[4];
    let shaped_text = Shaped::new(text);
    let shaper = shaped_text.shaper();
    let shaped = shaper.shape();
    let breaks = break_opportunities(text);
    // Every byte, including ones inside characters and past the end.
    for start in 0..=text.len() + 2 {
        for block_start in [Length::ZERO, Length::MAX - pt(1), Length::MIN] {
            for (gname, geometry) in [
                ("measure", &Measure(pt(60)) as &dyn GeometryProvider),
                ("circle", &circle()),
                ("narrowing", &Narrowing),
            ] {
                run(&Case {
                    label: format!("start {start} / {block_start:?} / {gname}"),
                    request: ComposeRequest {
                        text,
                        shaped: &shaped,
                        reshape: &shaper,
                        breaks: &breaks,
                        line_height: pt(12),
                        geometry,
                        start,
                        block_start,
                    },
                });
            }
        }
    }
}

/// A reshaper that returns nothing at all: composers must still cover the
/// text and never panic.
struct NoGlyphs;
impl Reshape for NoGlyphs {
    fn reshape(&self, _: Range<usize>) -> Vec<ShapedRun> {
        Vec::new()
    }
}

#[test]
fn every_composer_survives_a_reshaper_that_drops_glyphs() {
    let text = TEXTS[6];
    let shaped_text = Shaped::new(text);
    let shaped = all_unsafe(&shaped_text.shaper().shape());
    let breaks = break_opportunities(text);
    run(&Case {
        label: "no glyphs".into(),
        request: ComposeRequest {
            text,
            shaped: &shaped,
            reshape: &NoGlyphs,
            breaks: &breaks,
            line_height: pt(12),
            geometry: &Measure(pt(60)),
            start: 0,
            block_start: Length::ZERO,
        },
    });
}

#[test]
fn huge_paragraphs_stay_bounded() {
    let words = [
        "a",
        "the",
        "quick",
        "brown",
        "fox",
        "jumps",
        "over",
        "lazy",
        "dog",
        "and",
        "incomprehensibly",
        "on",
    ];
    let text: String = (0..3000)
        .map(|i| words[(i * 7 + i / 3) % words.len()])
        .collect::<Vec<_>>()
        .join(" ");
    let shaped_text = Shaped::new(&text);
    let shaper = shaped_text.shaper();
    let shaped = shaper.shape();
    let breaks = break_opportunities(&text);
    for (gname, geometry) in [
        ("measure", &Measure(pt(200)) as &dyn GeometryProvider),
        // One line could hold everything: the search would be quadratic,
        // so its limit sends it to first-fit.
        ("max", &Measure(Length::MAX)),
        ("runaround", &runaround()),
    ] {
        run(&Case {
            label: format!("huge / {gname}"),
            request: ComposeRequest {
                text: &text,
                shaped: &shaped,
                reshape: &shaper,
                breaks: &breaks,
                line_height: pt(12),
                geometry,
                start: 0,
                block_start: Length::ZERO,
            },
        });
    }
}

/// Fragments of an authored-break composition, grouped into authored lines
/// by the rule its docs give: a fragment ends one when its reason is
/// `Forced` or `End`.
fn authored_lines<'a>(text: &'a str, lines: &[LineFragment]) -> Vec<Vec<&'a str>> {
    let mut out = vec![Vec::new()];
    for l in lines {
        out.last_mut()
            .expect("never empty")
            .push(text[l.text.clone()].trim_end());
        if matches!(l.explanation.reason, BreakReason::Forced | BreakReason::End) {
            out.push(Vec::new());
        }
    }
    out.pop();
    out
}

#[test]
fn authored_lines_are_recoverable_from_fragments_for_every_turnover() {
    let text = "Overlongwordthatcannotfitanywhere\nshort line\n\
                a much longer authored line that has to turn over at least once\nend";
    let shaped_text = Shaped::new(text);
    let shaper = shaped_text.shaper();
    let shaped = shaper.shape();
    let breaks = break_opportunities(text);
    for turnover in [Turnover::Optimal(Optimal::default()), Turnover::FirstFit] {
        let composer = AuthoredBreak {
            turnover_indent: pt(12),
            turnover,
        };
        let c = composer.compose(&ComposeRequest {
            text,
            shaped: &shaped,
            reshape: &shaper,
            breaks: &breaks,
            line_height: pt(12),
            geometry: &Measure(pt(90)),
            start: 0,
            block_start: Length::ZERO,
        });
        let authored = authored_lines(text, &c.lines);
        let joined: Vec<String> = authored.iter().map(|l| l.join(" ")).collect();
        assert_eq!(
            joined,
            text.lines().collect::<Vec<_>>(),
            "{}",
            composer.name()
        );
        assert!(authored[2].len() > 1, "the long line turns over");
        for l in &c.lines {
            let turnover = l.text.start > 0 && !text[..l.text.start].ends_with('\n');
            let indent = if turnover { pt(12) } else { Length::ZERO };
            assert_eq!(l.available.start, indent, "{:?}", &text[l.text.clone()]);
        }
    }
}
