//! The greedy composer: fills each line with as many words as fit, then moves on.

use reprise_diag::Note;
use reprise_geom::Length;

use crate::para::{Para, indented};
use crate::walk::{Step, Walk, stalled};
use crate::{
    Adjustment, BreakKind, BreakReason, ComposeRequest, Composer, Composition, Interval, codes,
};

/// How many times in a row a geometry provider may answer `Skip` before
/// composition gives up. Each skip must move down, but a provider that moves
/// one sub-unit at a time would otherwise take billions of steps.
pub const MAX_CONSECUTIVE_SKIPS: u32 = 1024;

pub struct Greedy;

/// How one interval was filled.
enum Fill {
    /// A fragment ending at this byte, for this reason. `ends_line` is set
    /// when the break was forced, so the line can't continue in a later interval.
    Fragment {
        end: usize,
        reason: BreakReason,
        ends_line: bool,
    },
    /// Nothing fits here but the next word fits a later interval on this line.
    Pass,
}

/// How first-fit composition sets lines, beyond what [`Greedy`] does.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FirstFit {
    /// Moves the first interval of a line in by this much when the line
    /// continues an authored line (a turnover).
    pub indent: Length,
    /// Gives an overflowing line that ends at a forced break the reason
    /// `Forced` rather than `Overflow`, so `Forced` always marks the end of
    /// an authored line. [`Greedy`] keeps `Overflow`.
    pub forced_reason_wins: bool,
}

fn fill(
    para: &Para<'_, '_>,
    pos: usize,
    interval: Interval,
    later: &[Interval],
    style: FirstFit,
) -> (Fill, Option<Note>) {
    let width = interval.width();
    let len = para.len;
    let mut last_fit = None;
    for (i, b) in para
        .breaks
        .iter()
        .enumerate()
        .skip(para.first_break_after(pos))
    {
        let need = para.fitted_width(pos, i, width);
        let forced = b.kind == BreakKind::Forced;
        if need <= width {
            if forced || b.at == len {
                let reason = if forced {
                    BreakReason::Forced
                } else {
                    BreakReason::End
                };
                let fill = Fill::Fragment {
                    end: b.at,
                    reason,
                    ends_line: forced,
                };
                return (fill, None);
            }
            last_fit = Some(b.at);
            continue;
        }
        if let Some(end) = last_fit {
            let fill = Fill::Fragment {
                end,
                reason: BreakReason::Opportunity,
                ends_line: false,
            };
            return (fill, None);
        }
        if later.iter().any(|l| need <= l.width()) {
            return (Fill::Pass, None);
        }
        let note = Note::warning(
            codes::OVERFLOW,
            format!("text at bytes {pos}..{} overflows its line", b.at),
        )
        .at(pos..b.at);
        let fill = Fill::Fragment {
            end: b.at,
            reason: if forced && style.forced_reason_wins {
                BreakReason::Forced
            } else {
                BreakReason::Overflow
            },
            ends_line: forced,
        };
        return (fill, Some(note));
    }
    // Only reached for empty text: the one empty line.
    let fill = Fill::Fragment {
        end: pos,
        reason: BreakReason::End,
        ends_line: true,
    };
    (fill, None)
}

/// Composes first-fit from `pos` and `walk` to the end of the text, the end
/// of the region or a stall, appending to `out`. Lines carry no score.
pub(crate) fn first_fit(
    para: &Para<'_, '_>,
    walk: &mut Walk,
    mut pos: usize,
    out: &mut Composition,
    style: FirstFit,
) {
    let request = para.request;
    let len = para.len;
    // Empty text still gets one line; otherwise stop when the text runs out.
    let unfinished =
        |out: &Composition, pos: usize| pos < len || (len == 0 && out.lines.is_empty());
    while unfinished(out, pos) {
        let intervals = match walk.next_line(request, &out.lines) {
            Step::Room(intervals) => intervals,
            Step::Moved => continue,
            Step::End => {
                out.rest = Some(pos);
                break;
            }
            Step::Stalled => {
                out.notes.push(stalled(walk, pos, len));
                out.rest = Some(pos);
                break;
            }
        };
        for (k, &interval) in intervals.iter().enumerate() {
            if !unfinished(out, pos) {
                break;
            }
            let interval = if k == 0 && !para.begins_line(pos) {
                indented(interval, style.indent)
            } else {
                interval
            };
            let later = intervals.get(k + 1..).unwrap_or_default();
            let (fill, note) = fill(para, pos, interval, later, style);
            out.notes.extend(note);
            let Fill::Fragment {
                end,
                reason,
                ends_line,
            } = fill
            else {
                continue;
            };
            if para.tab_limited() {
                out.notes.push(
                    Note::error(
                        codes::TAB_LIMIT,
                        "tab fitting exceeded its explicit work budget",
                    )
                    .at(pos..len),
                );
                out.rest = Some(pos);
                out.block_end = walk.y;
                return;
            }
            out.lines.push(para.fragment(
                pos..end,
                interval,
                walk.line,
                walk.y,
                reason,
                None,
                Adjustment::default(),
            ));
            out.notes.extend(para.tab_notes(pos..end, interval.width()));
            pos = end;
            if ends_line {
                break;
            }
        }
        walk.advance(request.line_height);
    }
    out.block_end = walk.y;
}

impl Composer for Greedy {
    fn name(&self) -> &'static str {
        "greedy"
    }

    fn compose(&self, request: &ComposeRequest<'_>) -> Composition {
        let para = Para::new(request);
        let mut out = Composition::default();
        let mut walk = Walk::new(request.block_start);
        first_fit(&para, &mut walk, para.start, &mut out, FirstFit::default());
        out
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use reprise_font::{Face, FontStore};
    use reprise_shape::{
        HarfRust, Item, ParagraphInput, Reshape, ShapedRun, ShapedText, Shaper, StyleRun, itemize,
    };

    use std::ops::Range;

    use super::*;
    use crate::{Available, Break, LineQuery};
    use crate::{GeometryProvider, Measure, break_opportunities};

    const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");

    struct Fixture {
        fonts: FontStore,
        items: Vec<Item>,
    }

    fn fixture(text: &str) -> Fixture {
        let mut fonts = FontStore::default();
        fonts.add(Face::from_bytes(SERIF).unwrap());
        let styles = [StyleRun {
            range: 0..text.len(),
            families: vec!["Source Serif Pro".into()],
            size: Length::from_pt(10),
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
        Fixture { fonts, items }
    }

    fn compose_with(text: &str, geometry: &dyn GeometryProvider) -> Composition {
        let f = fixture(text);
        let shaper = Shaper {
            text,
            items: &f.items,
            fonts: &f.fonts,
            adapter: &HarfRust,
        };
        let shaped = shaper.shape();
        Greedy.compose(&ComposeRequest {
            text,
            shaped: &shaped,
            reshape: &shaper,
            breaks: &break_opportunities(text),
            line_height: Length::from_pt(12),
            geometry,
            start: 0,
            block_start: Length::ZERO,
        })
    }

    fn compose(text: &str, measure_pt: i32) -> Composition {
        compose_with(text, &Measure(Length::from_pt(measure_pt)))
    }

    fn line_texts<'a>(text: &'a str, c: &Composition) -> Vec<&'a str> {
        c.lines
            .iter()
            .map(|l| text[l.text.clone()].trim_end())
            .collect()
    }

    fn assert_covers(text: &str, c: &Composition) {
        let mut at = 0;
        for l in &c.lines {
            assert_eq!(l.text.start, at, "fragments are contiguous");
            at = l.text.end;
        }
        assert_eq!(at, c.rest.unwrap_or(text.len()), "fragments reach the end");
    }

    #[test]
    fn wraps_at_word_boundaries_within_the_measure() {
        let text = "the quick brown fox jumps over the lazy dog";
        let c = compose(text, 60);
        assert!(c.lines.len() > 1);
        assert_covers(text, &c);
        assert_eq!(line_texts(text, &c).join(" "), text);
        for line in &c.lines {
            assert!(line.width <= line.available.width());
        }
        assert_eq!(c.lines.last().unwrap().explanation.reason, BreakReason::End);
        assert_eq!(
            c.block_end,
            Length::from_pt(12).mul_ratio(c.lines.len() as i32, 1)
        );
    }

    #[test]
    fn forced_breaks_end_lines() {
        let text = "one\u{2028}two";
        let c = compose(text, 500);
        assert_eq!(line_texts(text, &c), ["one", "two"]);
        assert_eq!(c.lines[0].explanation.reason, BreakReason::Forced);
    }

    #[test]
    fn overlong_words_overflow_with_a_diagnostic() {
        let c = compose("incomprehensibilities", 10);
        assert_eq!(c.lines.len(), 1);
        assert_eq!(c.lines[0].explanation.reason, BreakReason::Overflow);
        assert_eq!(c.notes[0].code, "compose.overflow");
    }

    #[test]
    fn empty_text_gets_one_empty_line() {
        let c = compose("", 100);
        assert_eq!(c.lines.len(), 1);
        assert_eq!(c.lines[0].text, 0..0);
        assert!(c.lines[0].runs.is_empty());
        assert!(c.notes.is_empty());
    }

    #[test]
    fn zero_and_negative_measures_overflow_every_word_but_terminate() {
        let text = "a few short words";
        for measure in [0, -50] {
            let c = compose(text, measure);
            assert_covers(text, &c);
            assert_eq!(c.lines.len(), 4);
            assert!(
                c.lines
                    .iter()
                    .all(|l| l.explanation.reason == BreakReason::Overflow
                        || l.explanation.reason == BreakReason::End)
            );
        }
    }

    /// Two columns of room per line, as around a figure in the middle.
    struct Split;

    impl GeometryProvider for Split {
        fn available(&self, _: &LineQuery<'_>) -> Available {
            let pt = Length::from_pt;
            Available::Room(vec![
                Interval::new(pt(0), pt(4)),
                Interval::new(pt(10), pt(60)),
                Interval::new(pt(70), pt(120)),
            ])
        }
    }

    #[test]
    fn lines_fill_several_intervals_in_order() {
        let text = "the quick brown fox jumps over the lazy dog";
        let c = compose_with(text, &Split);
        assert_covers(text, &c);
        assert_eq!(c.lines[0].line, 0);
        assert_eq!(
            c.lines[0].available.start,
            Length::from_pt(10),
            "too narrow, passed"
        );
        assert_eq!(
            c.lines[1].line, 0,
            "the second fragment shares the first line"
        );
        assert_eq!(c.lines[1].available.start, Length::from_pt(70));
        assert!(c.lines.iter().all(|l| l.width <= l.available.width()));
    }

    /// Room only below 30pt, ending at 60pt.
    struct Window;

    impl GeometryProvider for Window {
        fn available(&self, q: &LineQuery<'_>) -> Available {
            let pt = Length::from_pt;
            if q.block_offset < pt(30) {
                Available::Skip { next: pt(30) }
            } else if q.block_offset + q.line_height > pt(60) {
                Available::End
            } else {
                Available::Room(vec![Interval::new(pt(0), pt(50))])
            }
        }
    }

    #[test]
    fn skips_and_region_ends_are_honoured() {
        let text = "the quick brown fox jumps over the lazy dog";
        let c = compose_with(text, &Window);
        assert_eq!(c.lines[0].block_offset, Length::from_pt(30));
        assert_eq!(c.lines.len(), 2, "two 12pt lines fit between 30 and 60");
        assert_covers(text, &c);
        let rest = c.rest.expect("the region ended first");
        assert!(rest < text.len());

        // Continuing from `rest` picks up exactly where it stopped.
        let f = fixture(text);
        let shaper = Shaper {
            text,
            items: &f.items,
            fonts: &f.fonts,
            adapter: &HarfRust,
        };
        let shaped = shaper.shape();
        let more = Greedy.compose(&ComposeRequest {
            text,
            shaped: &shaped,
            reshape: &shaper,
            breaks: &break_opportunities(text),
            line_height: Length::from_pt(12),
            geometry: &Measure(Length::from_pt(50)),
            start: rest,
            block_start: Length::ZERO,
        });
        assert_eq!(more.lines[0].text.start, rest);
        assert_eq!(more.rest, None);
    }

    /// A provider that never moves down.
    struct Stuck;

    impl GeometryProvider for Stuck {
        fn available(&self, q: &LineQuery<'_>) -> Available {
            Available::Skip {
                next: q.block_offset,
            }
        }
    }

    /// A provider that creeps down one sub-unit at a time.
    struct Creep;

    impl GeometryProvider for Creep {
        fn available(&self, q: &LineQuery<'_>) -> Available {
            Available::Skip {
                next: q.block_offset + Length(1),
            }
        }
    }

    #[test]
    fn stalled_geometry_stops_with_a_diagnostic() {
        for geometry in [&Stuck as &dyn GeometryProvider, &Creep] {
            let c = compose_with("text", geometry);
            assert!(c.lines.is_empty());
            assert_eq!(c.rest, Some(0));
            assert_eq!(c.notes[0].code, "compose.geometry-stalled");
        }
    }

    /// Marks every glyph unsafe to break and records what it was asked to
    /// reshape, so the composer's side of the reshaping contract is visible.
    struct Unsafe<'a> {
        inner: Shaper<'a>,
        asked: RefCell<Vec<Range<usize>>>,
    }

    impl Reshape for Unsafe<'_> {
        fn reshape(&self, range: Range<usize>) -> Vec<ShapedRun> {
            self.asked.borrow_mut().push(range.clone());
            self.inner.reshape(range)
        }
    }

    #[test]
    fn breaks_that_are_unsafe_get_reshaped() {
        let text = "the quick brown fox";
        let f = fixture(text);
        let shaper = Shaper {
            text,
            items: &f.items,
            fonts: &f.fonts,
            adapter: &HarfRust,
        };
        let mut shaped: ShapedText = shaper.shape();
        for g in shaped.runs.iter_mut().flat_map(|r| r.glyphs.iter_mut()) {
            g.unsafe_to_break = true;
        }
        let unsafe_shaper = Unsafe {
            inner: shaper,
            asked: RefCell::new(Vec::new()),
        };
        let c = Greedy.compose(&ComposeRequest {
            text,
            shaped: &shaped,
            reshape: &unsafe_shaper,
            breaks: &break_opportunities(text),
            line_height: Length::from_pt(12),
            geometry: &Measure(Length::from_pt(40)),
            start: 0,
            block_start: Length::ZERO,
        });
        assert!(c.lines.len() > 1);
        assert!(c.lines.iter().all(|l| l.explanation.reshaped));
        let asked = unsafe_shaper.asked.borrow();
        let lines: Vec<_> = c.lines.iter().map(|l| l.text.clone()).collect();
        assert_eq!(*asked, lines, "each line was reshaped as itself");
    }

    #[test]
    fn breaks_off_character_boundaries_are_ignored() {
        let text = "héllo wörld";
        let f = fixture(text);
        let shaper = Shaper {
            text,
            items: &f.items,
            fonts: &f.fonts,
            adapter: &HarfRust,
        };
        let shaped = shaper.shape();
        let bogus = [
            Break {
                at: 2, // inside the é
                kind: BreakKind::Forced,
                penalty: 0,
            },
            Break {
                at: 999,
                kind: BreakKind::Forced,
                penalty: 0,
            },
        ];
        let c = Greedy.compose(&ComposeRequest {
            text,
            shaped: &shaped,
            reshape: &shaper,
            breaks: &bogus,
            line_height: Length::from_pt(12),
            geometry: &Measure(Length::from_pt(500)),
            start: 0,
            block_start: Length::ZERO,
        });
        assert_eq!(c.lines.len(), 1);
        assert_eq!(c.lines[0].text, 0..text.len());
    }
}
