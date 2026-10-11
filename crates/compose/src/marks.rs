//! First-fit tabs with a per-composition work budget. The public composer
//! request is unchanged; ordinary paragraphs still use the configured composer.
use std::collections::BTreeMap;
use std::ops::Range;

use reprise_diag::Note;
use reprise_doc::marks::{Alignment, TabStops, is_line_break};
use reprise_geom::Length;
use reprise_shape::{Reshape, ShapedRun, ShapedText};

use crate::greedy::{FirstFit, first_fit};
use crate::para::Para;
use crate::walk::Walk;
use crate::{ComposeRequest, Composer, Composition, codes};

pub const MAX_TAB_WORK: usize = 1_000_000;
/// Internal advance-only cell; display consumes its advance without painting.
pub const INVISIBLE_GLYPH: u32 = u32::MAX;
/// An inline object's advance cell (see `docs/inline.md`): composers measure
/// it like a glyph; display paints the object instead.
pub const ATOM_GLYPH: u32 = u32::MAX - 1;

pub fn hide_controls(text: &str, runs: &mut [ShapedRun]) {
    for run in runs {
        for glyph in &mut run.glyphs {
            if text
                .get(glyph.cluster as usize..)
                .and_then(|s| s.chars().next())
                .is_some_and(|c| c == '\t' || is_line_break(c))
            {
                glyph.id = INVISIBLE_GLYPH;
                glyph.advance = Length::ZERO;
                glyph.x_offset = Length::ZERO;
                glyph.y_offset = Length::ZERO;
                glyph.unsafe_to_break = false;
            }
        }
    }
}
struct Clean<'a> {
    text: &'a str,
    reshape: &'a dyn Reshape,
}
impl Reshape for Clean<'_> {
    fn reshape(&self, range: Range<usize>) -> Vec<ShapedRun> {
        let mut runs = self.reshape.reshape(range);
        hide_controls(self.text, &mut runs);
        runs
    }
}

pub fn compose_marks(
    request: &ComposeRequest<'_>,
    tabs: &TabStops,
    composer: &dyn Composer,
) -> Composition {
    compose_marks_with(request, tabs, composer, None)
}

/// [`compose_marks`] for a paragraph with inline objects. When an object
/// needs more room than the strut, lines are set first-fit with per-line
/// heights (see [`crate::LineExtents`]); otherwise this is `compose_marks`.
pub fn compose_marks_with(
    request: &ComposeRequest<'_>,
    tabs: &TabStops,
    composer: &dyn Composer,
    extents: Option<&crate::LineExtents>,
) -> Composition {
    let extents = extents.filter(|e| e.varies());
    let mut shaped: ShapedText = request.shaped.clone();
    hide_controls(request.text, &mut shaped.runs);
    let clean = Clean {
        text: request.text,
        reshape: request.reshape,
    };
    let request = ComposeRequest {
        shaped: &shaped,
        reshape: &clean,
        ..*request
    };
    let mut out = if request.text.contains('\t') || extents.is_some() {
        let mut para = Para::new(&request);
        if request.text.contains('\t') {
            para.tabs = Some(tabs.clone());
            para.tab_positions = request
                .text
                .char_indices()
                .filter_map(|(at, c)| (c == '\t').then_some(at))
                .collect();
        }
        para.extents = extents.cloned();
        let mut out = Composition::default();
        let mut walk = Walk::new(request.block_start);
        first_fit(&para, &mut walk, para.start, &mut out, FirstFit::default());
        out
    } else {
        composer.compose(&request)
    };
    if out.rest.is_none()
        && request.text.chars().next_back().is_some_and(is_line_break)
        && (out.lines.last().is_some_and(|l| !l.text.is_empty())
            || (request.start >= request.text.len() && out.lines.is_empty()))
    {
        // Compose a genuinely empty paragraph, then rebase its caret line.
        // Ordinary composers keep their frozen end-of-text resume semantics.
        let empty = ShapedText::default();
        let tail_request = ComposeRequest {
            text: "",
            shaped: &empty,
            breaks: &[],
            start: 0,
            block_start: if out.lines.is_empty() {
                request.block_start
            } else {
                out.block_end
            },
            ..request
        };
        let tail_para = Para::new(&tail_request);
        let mut tail = Composition::default();
        let mut walk = Walk::new(tail_request.block_start);
        first_fit(
            &tail_para,
            &mut walk,
            tail_para.start,
            &mut tail,
            FirstFit::default(),
        );
        for line in &mut tail.lines {
            line.text = request.text.len()..request.text.len();
        }
        out.lines.extend(tail.lines);
        out.notes.extend(tail.notes);
        out.rest = tail.rest.map(|_| request.text.len());
        out.block_end = tail.block_end;
    }
    out
}

impl Para<'_, '_> {
    pub(crate) fn tab_limited(&self) -> bool {
        self.tab_work.get() > MAX_TAB_WORK
    }
    pub(crate) fn fitted_width(&self, pos: usize, index: usize, width: Length) -> Length {
        if self.tabs.is_none() {
            return self.natural_width(pos, index);
        }
        let end = self.breaks.get(index).map_or(self.len, |b| b.at);
        self.tab_sizes(pos..self.tab_content_end(pos, end), width).0
    }
    fn tab_content_end(&self, pos: usize, end: usize) -> usize {
        pos.saturating_add(self.text.get(pos..end).map_or(0, |s| {
            s.trim_end_matches(|c: char| c.is_whitespace() && c != '\t')
                .len()
        }))
    }
    /// Measure in authored order from inline start. L2 subsequently mirrors the
    /// tab fields with their paragraph; no physical left/right enters tab fitting.
    fn tab_sizes(
        &self,
        bytes: Range<usize>,
        measure: Length,
    ) -> (Length, BTreeMap<usize, Length>, Vec<usize>) {
        let Some(tabs) = &self.tabs else {
            return (self.width(bytes), BTreeMap::new(), Vec::new());
        };
        let first = self.tab_positions.partition_point(|&p| p < bytes.start);
        let last = self.tab_positions.partition_point(|&p| p < bytes.end);
        let positions = self.tab_positions.get(first..last).unwrap_or_default();
        let mut sizes = BTreeMap::new();
        let mut missed = Vec::new();
        let mut cursor = Length::ZERO;
        let mut at = bytes.start;
        for (index, &tab) in positions.iter().enumerate() {
            self.tab_work.set(self.tab_work.get().saturating_add(1));
            if self.tab_limited() {
                return (Length::MAX, sizes, missed);
            }
            cursor += self.width(at..tab);
            let after = tab.saturating_add(1);
            let next = positions
                .get(index.saturating_add(1))
                .copied()
                .unwrap_or(bytes.end);
            let field = self.width(after..next);
            let stop = tabs
                .stops
                .iter()
                .find(|s| s.position.unwrap_or(measure) > cursor);
            let (position, alignment) = match stop {
                Some(s) => (s.position.unwrap_or(measure), s.alignment),
                None => {
                    let interval = i64::from(tabs.interval.0.max(1));
                    let n = i64::from(cursor.0.max(0)) / interval + 1;
                    (
                        Length(n.saturating_mul(interval).min(i64::from(i32::MAX)) as i32),
                        Alignment::Start,
                    )
                }
            };
            let desired = position
                - match alignment {
                    Alignment::Start => Length::ZERO,
                    Alignment::Centre => field.mul_ratio(1, 2),
                    Alignment::End => field,
                };
            let reached = desired >= cursor && position <= measure;
            let end = desired.max(cursor).min(measure.max(cursor));
            sizes.insert(tab, end - cursor);
            if !reached {
                missed.push(tab);
            }
            cursor = end;
            at = after;
        }
        cursor += self.width(at..bytes.end);
        (cursor, sizes, missed)
    }
    pub(crate) fn adjust_tabs(
        &self,
        runs: &mut [ShapedRun],
        content: Range<usize>,
        measure: Length,
    ) -> Length {
        // Include trailing tabs: they carry authored positions even without a field.
        let end = self.tab_content_end(
            content.start,
            runs.iter()
                .map(|r| r.range.end)
                .max()
                .unwrap_or(content.end),
        );
        let (width, mut sizes, _) = self.tab_sizes(content.start..end, measure);
        for glyph in runs.iter_mut().flat_map(|r| &mut r.glyphs) {
            if let Some(advance) = sizes.remove(&(glyph.cluster as usize)) {
                glyph.advance = advance;
            }
        }
        width
    }
    pub(crate) fn tab_notes(&self, bytes: Range<usize>, measure: Length) -> Vec<Note> {
        if self.tabs.is_none() {
            return Vec::new();
        }
        let (_, _, missed) = self.tab_sizes(
            bytes.start..self.tab_content_end(bytes.start, bytes.end),
            measure,
        );
        if let (Some(&first), Some(&last)) = (missed.first(), missed.last()) {
            vec![Note::warning(codes::TAB_UNREACHABLE, "tab cannot reach its stop in this interval; gap clamped without reversing text").at(first..last.saturating_add(1))]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Greedy, Measure, break_opportunities, testing::Shaped};

    #[test]
    fn huge_candidate_search_stops_at_the_explicit_tab_budget() {
        let text = Shaped::new(&"\tx ".repeat(2000));
        let shaper = text.shaper();
        let shaped = shaper.shape();
        let request = ComposeRequest {
            text: &text.text,
            shaped: &shaped,
            reshape: &shaper,
            breaks: &break_opportunities(&text.text),
            line_height: Length(1),
            geometry: &Measure(Length::MAX),
            start: 0,
            block_start: Length::MIN,
        };
        let tabs = TabStops {
            interval: Length(1),
            stops: Vec::new(),
        };
        let out = compose_marks(&request, &tabs, &Greedy);
        assert_eq!(out.rest, Some(0));
        assert!(out.notes.iter().any(|n| n.code == codes::TAB_LIMIT));
    }

    #[test]
    fn empty_breaks_and_tabs_with_extreme_geometry_still_terminate() {
        for raw in ["", "\n", "\t", "\t\n\t"] {
            let text = Shaped::new(raw);
            let shaper = text.shaper();
            let shaped = shaper.shape();
            for size in [Length::MIN, Length::ZERO, Length::MAX] {
                let request = ComposeRequest {
                    text: &text.text,
                    shaped: &shaped,
                    reshape: &shaper,
                    breaks: &break_opportunities(&text.text),
                    line_height: size,
                    geometry: &Measure(size),
                    start: 0,
                    block_start: Length::MAX,
                };
                let out = compose_marks(&request, &TabStops::default(), &Greedy);
                assert!(out.lines.len() <= raw.len().saturating_add(1));
                assert!(out.rest.is_none_or(|at| at <= raw.len()));
            }
        }
    }

    #[test]
    fn tab_materialization_cannot_silently_exhaust_the_budget() {
        let text = Shaped::new("\tx");
        let shaper = text.shaper();
        let mut shaped = shaper.shape();
        hide_controls(&text.text, &mut shaped.runs);
        let request = ComposeRequest {
            text: &text.text,
            shaped: &shaped,
            reshape: &shaper,
            breaks: &break_opportunities(&text.text),
            line_height: Length::from_pt(12),
            geometry: &Measure(Length::MAX),
            start: 0,
            block_start: Length::ZERO,
        };
        let mut para = Para::new(&request);
        para.tabs = Some(TabStops::default());
        para.tab_positions = vec![0];
        para.tab_work.set(MAX_TAB_WORK - 2);
        let mut out = Composition::default();
        first_fit(
            &para,
            &mut Walk::new(Length::ZERO),
            0,
            &mut out,
            FirstFit::default(),
        );
        assert!(out.lines.is_empty());
        assert_eq!(out.rest, Some(0));
        assert!(out.notes.iter().any(|n| n.code == codes::TAB_LIMIT));
    }

    fn extents_request<'a>(
        text: &'a Shaped,
        shaped: &'a ShapedText,
        shaper: &'a reprise_shape::Shaper<'a>,
        breaks: &'a [crate::Break],
        geometry: &'a dyn crate::GeometryProvider,
    ) -> ComposeRequest<'a> {
        ComposeRequest {
            text: &text.text,
            shaped,
            reshape: shaper,
            breaks,
            line_height: Length::from_pt(12),
            geometry,
            start: 0,
            block_start: Length::ZERO,
        }
    }

    /// Records the line heights geometry is asked about.
    struct Asked(std::cell::RefCell<Vec<Length>>);
    impl crate::GeometryProvider for Asked {
        fn available(&self, query: &crate::LineQuery<'_>) -> crate::Available {
            self.0.borrow_mut().push(query.line_height);
            crate::Available::Room(vec![crate::Interval::new(
                Length::ZERO,
                Length::from_pt(20),
            )])
        }
    }

    #[test]
    fn a_tall_object_grows_only_its_own_line() {
        let text = Shaped::new("aaa bbb ccc");
        let shaper = text.shaper();
        let shaped = shaper.shape();
        let breaks = break_opportunities(&text.text);
        let asked = Asked(Default::default());
        let request = extents_request(&text, &shaped, &shaper, &breaks, &asked);
        let pt = Length::from_pt;
        let extents = crate::LineExtents {
            strut_top: pt(9),
            strut_bottom: pt(3),
            objects: vec![(5, pt(30), pt(1))],
        };
        let tabs = TabStops::default();
        let out = compose_marks_with(&request, &tabs, &Greedy, Some(&extents));
        let heights: Vec<_> = out.lines.iter().map(|l| l.height).collect();
        assert_eq!(heights, [pt(12), pt(33), pt(12)]);
        let tops: Vec<_> = out.lines.iter().map(|l| l.block_offset).collect();
        assert_eq!(tops, [pt(0), pt(12), pt(45)]);
        assert_eq!(out.block_end, pt(57));
        // The second line was asked about again at the height it needs.
        assert_eq!(*asked.0.borrow(), [pt(12), pt(12), pt(33), pt(12)]);
        // Objects that fit the strut change nothing.
        let fits = crate::LineExtents {
            objects: vec![(5, pt(9), pt(3))],
            ..extents
        };
        assert!(!fits.varies());
        let plain = compose_marks_with(&request, &tabs, &Greedy, Some(&fits));
        assert_eq!(plain, Greedy.compose(&request));
    }

    #[test]
    fn extreme_object_extents_saturate_and_terminate() {
        let text = Shaped::new("a b c d");
        let shaper = text.shaper();
        let shaped = shaper.shape();
        let breaks = break_opportunities(&text.text);
        for (a, d) in [
            (Length::MAX, Length::MAX),
            (Length::MIN, Length::MIN),
            (Length::ZERO, Length::MAX),
        ] {
            let geometry = Measure(Length::from_pt(5));
            let request = extents_request(&text, &shaped, &shaper, &breaks, &geometry);
            let extents = crate::LineExtents {
                strut_top: Length::from_pt(9),
                strut_bottom: Length::from_pt(3),
                objects: (0..7).map(|at| (at, a, d)).collect(),
            };
            let out = compose_marks_with(&request, &TabStops::default(), &Greedy, Some(&extents));
            assert_eq!(text.joined(&out), text.text);
            assert!(out.lines.iter().all(|l| l.height >= Length::from_pt(12)));
        }
    }
}
