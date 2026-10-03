//! Line composition (decision 23): composers break shaped text into lines,
//! and geometry providers say how much room each line gets.

use std::ops::Range;

use icu_segmenter::LineSegmenter;
use icu_segmenter::options::LineBreakOptions;
use reprise_geom::Length;
use reprise_shape::ShapedText;
use serde::{Deserialize, Serialize};

/// The span of the inline axis a line may use, in frame space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    pub start: Length,
    pub end: Length,
}

impl Interval {
    pub fn width(&self) -> Length {
        self.end - self.start
    }
}

/// Answers how much room a line gets. Shapes, runarounds and other
/// non-rectangular measures are geometry providers.
pub trait GeometryProvider {
    /// `line` is the line's index in the paragraph; `block_offset` is where its
    /// top sits in the frame; it may depend on both.
    fn available(&self, line: usize, block_offset: Length, line_height: Length) -> Interval;
}

/// A plain rectangular measure.
pub struct Measure(pub Length);

impl GeometryProvider for Measure {
    fn available(&self, _: usize, _: Length, _: Length) -> Interval {
        Interval {
            start: Length::ZERO,
            end: self.0,
        }
    }
}

/// Why a line ended where it did (39).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BreakReason {
    /// The next word didn't fit; broke at the last opportunity that did.
    Opportunity,
    /// A forced line break (U+2028 LINE SEPARATOR or a newline).
    Forced,
    /// The text ended.
    End,
    /// Nothing fit; the line holds one unbreakable run that overflows.
    Overflow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineFragment {
    /// Source bytes, including trailing whitespace.
    pub text: Range<usize>,
    /// Indices into [`ShapedText::glyphs`].
    pub glyphs: Range<usize>,
    /// Width without trailing whitespace, which hangs past the end.
    pub width: Length,
    pub available: Interval,
    pub block_offset: Length,
    pub break_reason: BreakReason,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Composition {
    pub lines: Vec<LineFragment>,
    pub diagnostics: Vec<String>,
}

pub struct ComposeRequest<'a> {
    pub text: &'a str,
    pub shaped: &'a ShapedText,
    pub line_height: Length,
    pub geometry: &'a dyn GeometryProvider,
}

pub trait Composer {
    fn name(&self) -> &'static str;
    fn compose(&self, request: &ComposeRequest<'_>) -> Composition;
}

/// Break opportunities after position 0, from the Unicode line breaking algorithm.
pub fn break_opportunities(text: &str) -> Vec<usize> {
    let segmenter = LineSegmenter::new_auto(LineBreakOptions::default());
    segmenter.segment_str(text).filter(|&b| b > 0).collect()
}

fn is_forced(text: &str, at: usize) -> bool {
    matches!(text[..at].chars().next_back(), Some('\u{2028}' | '\n'))
}

/// Fills each line with as many words as fit, then moves on.
pub struct Greedy;

impl Composer for Greedy {
    fn name(&self) -> &'static str {
        "greedy"
    }

    fn compose(&self, request: &ComposeRequest<'_>) -> Composition {
        let text = request.text;
        let shaped = request.shaped;
        let mut out = Composition::default();
        let opportunities = break_opportunities(text);

        let measure = |range: Range<usize>| {
            let trimmed = text[range.clone()].trim_end().len();
            shaped.width(shaped.glyphs_in(range.start..range.start + trimmed))
        };

        let mut start = 0;
        let mut last_fit: Option<usize> = None;
        let mut i = 0;
        while i < opportunities.len() {
            let line = out.lines.len();
            let block_offset = request.line_height.mul_ratio(line as i32, 1);
            let available = request
                .geometry
                .available(line, block_offset, request.line_height);
            let opp = opportunities[i];
            let end_line = |out: &mut Composition, end: usize, break_reason| {
                if shaped
                    .glyphs
                    .get(shaped.glyphs_in(end..text.len()).start)
                    .is_some_and(|g| g.unsafe_to_break)
                {
                    out.diagnostics.push(format!(
                        "break at byte {end} needs reshaping (not yet done)"
                    ));
                }
                out.lines.push(LineFragment {
                    text: start..end,
                    glyphs: shaped.glyphs_in(start..end),
                    width: measure(start..end),
                    available,
                    block_offset,
                    break_reason,
                });
            };

            if measure(start..opp) <= available.width() {
                let forced = is_forced(text, opp);
                if forced || opp == text.len() {
                    end_line(
                        &mut out,
                        opp,
                        if forced {
                            BreakReason::Forced
                        } else {
                            BreakReason::End
                        },
                    );
                    start = opp;
                    last_fit = None;
                } else {
                    last_fit = Some(opp);
                }
                i += 1;
            } else if let Some(fit) = last_fit.take() {
                // Close the line at the last fit and try this opportunity again.
                end_line(&mut out, fit, BreakReason::Opportunity);
                start = fit;
            } else {
                end_line(&mut out, opp, BreakReason::Overflow);
                out.diagnostics
                    .push(format!("text at bytes {start}..{opp} overflows its line"));
                start = opp;
                i += 1;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reprise_font::Face;
    use reprise_geom::InlineDirection;
    use reprise_shape::{HarfRust, ShapeRequest, ShapingAdapter};

    const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");

    fn compose(text: &str, measure_pt: i32) -> Composition {
        let face = Face::from_bytes(SERIF).unwrap();
        let shaped = HarfRust.shape(&ShapeRequest {
            text,
            face: &face,
            size: Length::from_pt(10),
            direction: InlineDirection::Ltr,
        });
        Greedy.compose(&ComposeRequest {
            text,
            shaped: &shaped,
            line_height: Length::from_pt(12),
            geometry: &Measure(Length::from_pt(measure_pt)),
        })
    }

    fn line_texts<'a>(text: &'a str, c: &Composition) -> Vec<&'a str> {
        c.lines
            .iter()
            .map(|l| text[l.text.clone()].trim_end())
            .collect()
    }

    #[test]
    fn wraps_at_word_boundaries_within_the_measure() {
        let text = "the quick brown fox jumps over the lazy dog";
        let c = compose(text, 60);
        assert!(c.lines.len() > 1);
        assert_eq!(line_texts(text, &c).join(" "), text);
        for line in &c.lines {
            assert!(line.width <= line.available.width());
        }
        assert_eq!(c.lines.last().unwrap().break_reason, BreakReason::End);
    }

    #[test]
    fn forced_breaks_end_lines() {
        let text = "one\u{2028}two";
        let c = compose(text, 500);
        assert_eq!(
            line_texts(text, &c),
            ["one\u{2028}", "two"].map(|s| s.trim_end())
        );
        assert_eq!(c.lines[0].break_reason, BreakReason::Forced);
    }

    #[test]
    fn overlong_words_overflow_with_a_diagnostic() {
        let c = compose("incomprehensibilities", 10);
        assert_eq!(c.lines.len(), 1);
        assert_eq!(c.lines[0].break_reason, BreakReason::Overflow);
        assert!(!c.diagnostics.is_empty());
    }
}
