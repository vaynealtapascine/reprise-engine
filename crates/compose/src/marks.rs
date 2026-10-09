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
    let mut out = if request.text.contains('\t') {
        let mut para = Para::new(&request);
        para.tabs = Some(tabs.clone());
        para.tab_positions = request
            .text
            .char_indices()
            .filter_map(|(at, c)| (c == '\t').then_some(at))
            .collect();
        let mut out = Composition::default();
        let mut walk = Walk::new(request.block_start);
        first_fit(&para, &mut walk, para.start, &mut out, FirstFit::default());
        out
    } else {
        composer.compose(&request)
    };
    if out.rest.is_none()
        && request.text.chars().next_back().is_some_and(is_line_break)
        && out.lines.last().is_some_and(|l| !l.text.is_empty())
    {
        let tail_request = ComposeRequest {
            start: request.text.len(),
            block_start: out.block_end,
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
        out.lines.extend(tail.lines);
        out.notes.extend(tail.notes);
        out.rest = tail.rest;
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
}
