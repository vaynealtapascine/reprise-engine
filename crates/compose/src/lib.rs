//! Line composition (decision 23): composers break shaped text into lines,
//! and geometry providers say how much room each line gets.
//!
//! Everything here is in a frame's logical space: the inline axis runs along a
//! line and the block axis from one line to the next. Writing modes, rotation
//! and mirroring are applied later by the frame's transform (20).

use std::ops::Range;

use icu_segmenter::{GraphemeClusterSegmenter, LineSegmenter};
use icu_segmenter::options::LineBreakOptions;
use reprise_diag::Note;
use reprise_geom::Length;
use reprise_shape::{Reshape, ShapedRun, ShapedText};
use serde::{Deserialize, Serialize};

mod authored;
mod greedy;
mod optimal;
mod para;
mod polygon;
#[cfg(test)]
mod testing;
mod walk;

pub use authored::{AuthoredBreak, Turnover};
pub use greedy::{Greedy, MAX_CONSECUTIVE_SKIPS};
pub use optimal::{Limits, Mode, Optimal};
pub use para::is_word_space;
pub use polygon::{Polygon, Runaround};

/// Diagnostic codes reported by composers.
pub mod codes {
    use reprise_diag::Code;

    /// An unbreakable run is wider than the room it was given.
    pub const OVERFLOW: Code = Code::new("compose.overflow");
    /// The geometry provider made no progress down the block axis, so
    /// composition stopped. The rest of the text is reported as unplaced.
    pub const GEOMETRY_STALLED: Code = Code::new("compose.geometry-stalled");
    /// An optimising composer set some or all lines first-fit instead: its
    /// search hit a limit, or geometry answered differently once earlier
    /// lines were known. Every composer guarantee still holds, and the lines
    /// it set first-fit have no score. Info: the output is still what the
    /// author asked for; only the engine's choice among valid breaks changed.
    pub const FALLBACK: Code = Code::new("compose.fallback");
}

/// A span of the inline axis a line may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    pub start: Length,
    pub end: Length,
}

impl Interval {
    pub fn new(start: Length, end: Length) -> Interval {
        Interval { start, end }
    }

    /// Never negative: a reversed interval has no room.
    pub fn width(&self) -> Length {
        (self.end - self.start).max(Length::ZERO)
    }
}

/// What a geometry provider is asked for one line.
pub struct LineQuery<'a> {
    /// The line's index in this composition, counting from 0.
    pub line: u32,
    /// Where the line's top would sit on the block axis.
    pub block_offset: Length,
    pub line_height: Length,
    /// The lines composed so far, for geometry that depends on them (23).
    pub previous: &'a [LineFragment],
}

/// The room for one line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Available {
    /// Intervals on the inline axis, in inline order and not overlapping. The
    /// line fills them in turn. An empty list means no room on this line, the
    /// same as skipping one line height.
    Room(Vec<Interval>),
    /// No room at this offset; ask again at `next`, which must be further
    /// down the block axis.
    Skip { next: Length },
    /// The region ends here. The rest of the text continues in another region.
    End,
}

/// Answers how much room each line gets: shapes, runarounds and other
/// non-rectangular measures (23). Must be a pure function of the query.
pub trait GeometryProvider {
    fn available(&self, query: &LineQuery<'_>) -> Available;
}

/// A plain rectangular measure of unlimited depth.
pub struct Measure(pub Length);

impl GeometryProvider for Measure {
    fn available(&self, _: &LineQuery<'_>) -> Available {
        Available::Room(vec![Interval::new(Length::ZERO, self.0)])
    }
}

/// A place the text may break a line (UAX #14, plus authored breaks).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Break {
    /// The byte offset the next line would start at.
    pub at: usize,
    pub kind: BreakKind,
    /// Extra cost of breaking here, for composers that score breaks.
    /// 0 is neutral; higher is worse.
    pub penalty: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum BreakKind {
    /// The line may break here.
    Allowed,
    /// The line must break here (11): a forced line break in the text.
    Forced,
}

/// The break opportunities in `text` after position 0, from the Unicode line
/// breaking algorithm. The end of the text is always the last one.
pub fn break_opportunities(text: &str) -> Vec<Break> {
    let segmenter = LineSegmenter::new_auto(LineBreakOptions::default());
    // UAX #14 can allow a break inside a grapheme cluster: a space followed by
    // a ZWJ or a combining mark isn't a combining sequence (LB9 excludes SP),
    // so LB18 breaks between them. A line must start on a grapheme boundary,
    // so those opportunities are not offered.
    let graphemes: Vec<usize> = GraphemeClusterSegmenter::new().segment_str(text).collect();
    segmenter
        .segment_str(text)
        .filter(|&b| b > 0 && graphemes.binary_search(&b).is_ok())
        .map(|at| Break {
            at,
            kind: if is_forced(text, at) {
                BreakKind::Forced
            } else {
                BreakKind::Allowed
            },
            penalty: 0,
        })
        .collect()
}

/// True after a mandatory break character (UAX #14 classes BK, CR, LF, NL).
fn is_forced(text: &str, at: usize) -> bool {
    matches!(
        text[..at].chars().next_back(),
        Some('\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}')
    )
}

/// Why a line ended where it did (39).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum BreakReason {
    /// The next word didn't fit; broke at the last opportunity that did.
    Opportunity,
    /// A forced line break.
    Forced,
    /// The text ended.
    End,
    /// Nothing fit; the line holds one unbreakable run that overflows.
    Overflow,
}

/// Spacing a composer added to a line, for example to justify it (23, 39).
/// Zero means the line is set at its natural width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adjustment {
    /// Added to every inter-word space.
    pub word_spacing: Length,
    /// Added after every grapheme cluster.
    pub letter_spacing: Length,
}

/// Why a fragment is the way it is (39).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub reason: BreakReason,
    /// The composer's cost for this line, lower is better, or `None` for
    /// composers that don't score lines.
    pub score: Option<i64>,
    pub adjustment: Adjustment,
    /// True when the line's text was shaped again because a break landed
    /// where shaping depended on text across it.
    pub reshaped: bool,
}

/// One line's text in one interval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineFragment {
    /// Source bytes, including trailing whitespace.
    pub text: Range<usize>,
    /// The fragment's glyphs, in logical run order. Reshaped when needed, so
    /// they can differ from the paragraph's [`ShapedText`].
    pub runs: Vec<ShapedRun>,
    /// Width without trailing whitespace, which hangs past the end.
    pub width: Length,
    pub available: Interval,
    /// The line this fragment is on. Fragments in different intervals of one
    /// line share it.
    pub line: u32,
    /// The line's top on the block axis.
    pub block_offset: Length,
    pub height: Length,
    pub explanation: Explanation,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Composition {
    pub lines: Vec<LineFragment>,
    pub notes: Vec<Note>,
    /// Where the text continues when the region ended before it did, or
    /// `None` when all of it was composed.
    pub rest: Option<usize>,
    /// Where the block axis ends: below the last line, or at the last offset
    /// geometry skipped to.
    pub block_end: Length,
}

pub struct ComposeRequest<'a> {
    pub text: &'a str,
    pub shaped: &'a ShapedText,
    pub reshape: &'a dyn Reshape,
    /// From [`break_opportunities`], plus any authored breaks, sorted by `at`.
    pub breaks: &'a [Break],
    pub line_height: Length,
    pub geometry: &'a dyn GeometryProvider,
    /// The byte offset to start at: 0, or an earlier composition's `rest`.
    pub start: usize,
    /// The first line's top on the block axis.
    pub block_start: Length,
}

/// A line-breaking algorithm (23).
///
/// Every composer guarantees, for any input:
///
/// -   Fragments cover the text from `start` contiguously and in order, up to
///     `rest` or the end. An empty text still gets one empty line, so it has
///     a place for a caret.
/// -   Lines break only at `breaks`, except a line holding one unbreakable
///     run that overflows, which is reported with [`codes::OVERFLOW`].
/// -   Forced breaks always end a line.
/// -   A fragment whose start or end isn't safe to break is reshaped.
/// -   Composition terminates and never panics, whatever the geometry provider
///     answers.
/// -   Output is a pure function of the request: integer arithmetic only.
pub trait Composer: Send + Sync {
    fn name(&self) -> &'static str;
    fn compose(&self, request: &ComposeRequest<'_>) -> Composition;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mandatory_break_characters_are_forced() {
        let text = "a\nb\r\nc\u{2028}d e";
        let kinds: Vec<(usize, BreakKind)> = break_opportunities(text)
            .into_iter()
            .map(|b| (b.at, b.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                (2, BreakKind::Forced),
                (5, BreakKind::Forced),
                (9, BreakKind::Forced),
                (11, BreakKind::Allowed),
                (12, BreakKind::Allowed),
            ]
        );
    }

    /// Found by the cross-crate fuzzer: UAX #14 breaks between a space and a
    /// following ZWJ or combining mark, which splits a grapheme cluster.
    #[test]
    fn breaks_never_fall_inside_a_grapheme_cluster() {
        for text in [
            "a \u{200d}\u{1f467} b",
            "a \u{301}b",
            "x  \u{200d}\u{200d}y",
            "\u{1f469}  \u{200d}\u{1f467} family ",
        ] {
            let clusters: Vec<usize> = GraphemeClusterSegmenter::new().segment_str(text).collect();
            for b in break_opportunities(text) {
                assert!(
                    clusters.contains(&b.at),
                    "{text:?}: break at {} is inside a cluster",
                    b.at
                );
            }
        }
    }

    #[test]
    fn reversed_intervals_have_no_room() {
        let i = Interval::new(Length::from_pt(10), Length::from_pt(5));
        assert_eq!(i.width(), Length::ZERO);
    }
}
