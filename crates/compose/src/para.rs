//! The paragraph as composers see it: normalised breaks, widths, and the
//! fragments built from them. Shared by every composer in this crate.
//!
//! Nothing here slices the text with a panicking index: every position comes
//! from a break that was checked to be on a character boundary, or is the
//! start (snapped to one) or the end of the text.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ops::Range;

use reprise_geom::Length;
use reprise_shape::{ShapedGlyph, ShapedRun};

use crate::{
    Adjustment, Break, BreakKind, BreakReason, ComposeRequest, Explanation, Interval, LineFragment,
};

/// True for the characters justification stretches: the word-separator
/// characters of CSS Text 3 (`word-spacing`), so layout and composition
/// agree on what [`Adjustment::word_spacing`] applies to.
pub fn is_word_space(c: char) -> bool {
    matches!(
        c,
        ' ' | '\u{A0}' | '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039F}' | '\u{1091F}'
    )
}

pub(crate) struct Para<'r, 'a> {
    pub request: &'r ComposeRequest<'a>,
    pub(crate) tabs: Option<reprise_doc::marks::TabStops>,
    pub(crate) tab_positions: Vec<usize>,
    pub(crate) tab_work: Cell<usize>,
    pub text: &'a str,
    pub len: usize,
    /// `request.start`, clamped to the text and snapped down to a character
    /// boundary, so a bad start can never split a character.
    pub start: usize,
    /// Valid breaks after `start`, sorted and unique, ending at the end of
    /// the text. Where the request has several breaks at one offset, a forced
    /// one wins, so a duplicate can never hide a forced break.
    pub breaks: Vec<Break>,
    /// For each break, where its line's content ends: the break minus any
    /// whitespace before it (that whitespace hangs).
    trimmed: Vec<usize>,
    /// True when `start` begins an authored line: it is 0, or the request has
    /// a forced break there.
    start_begins_line: bool,
    /// Prefix sums by byte of the advances of the glyphs whose cluster starts
    /// at that byte, so a width is two lookups. `None` when some advance is
    /// negative: then saturating sums are not prefix-additive, and widths come
    /// from [`reprise_shape::ShapedText::width`] instead, so they are exactly
    /// what it would say either way.
    advance: Option<Vec<i64>>,
    /// Prefix sums by byte of word-space advances and counts.
    space_advance: Vec<i64>,
    space_count: Vec<u32>,
    /// Bytes where a glyph cluster starts that is unsafe to break.
    unsafe_at: Vec<bool>,
    /// Each run's glyphs, sorted by cluster, for slicing a line out of a run
    /// without scanning all of it.
    by_cluster: Vec<Vec<(usize, usize)>>,
    /// Widths of lines that had to be reshaped, by byte range.
    reshaped: RefCell<BTreeMap<(usize, usize), Length>>,
}

impl<'r, 'a> Para<'r, 'a> {
    pub fn new(request: &'r ComposeRequest<'a>) -> Para<'r, 'a> {
        let text = request.text;
        let len = text.len();
        let mut start = request.start.min(len);
        while !text.is_char_boundary(start) {
            start -= 1;
        }

        let mut candidates: Vec<Break> = request
            .breaks
            .iter()
            .filter(|b| b.at > start && b.at <= len && text.is_char_boundary(b.at))
            .copied()
            .collect();
        candidates.sort_by_key(|b| b.at);
        let mut breaks: Vec<Break> = Vec::with_capacity(candidates.len() + 1);
        for b in candidates {
            match breaks.last_mut() {
                Some(last) if last.at == b.at => {
                    if last.kind != BreakKind::Forced && b.kind == BreakKind::Forced {
                        *last = b;
                    }
                }
                _ => breaks.push(b),
            }
        }
        if len > start && breaks.last().is_none_or(|b| b.at != len) {
            breaks.push(Break {
                at: len,
                kind: BreakKind::Allowed,
                penalty: 0,
            });
        }
        let trimmed = breaks
            .iter()
            .map(|b| text.get(..b.at).map_or(b.at, |s| s.trim_end().len()))
            .collect();
        let start_begins_line = start == 0
            || request
                .breaks
                .iter()
                .any(|b| b.at == start && b.kind == BreakKind::Forced);

        let glyphs = || request.shaped.runs.iter().flat_map(|r| &r.glyphs);
        let at = |g: &ShapedGlyph| Some(g.cluster as usize).filter(|&c| c < len);
        let mut unsafe_at = vec![false; len + 1];
        let mut per_byte = vec![0i64; len + 1];
        let mut space_per_byte = vec![0i64; len + 1];
        let mut additive = true;
        for g in glyphs() {
            additive &= g.advance >= Length::ZERO;
            let Some(c) = at(g) else { continue };
            unsafe_at[c] |= g.unsafe_to_break;
            per_byte[c] += g.advance.0 as i64;
            if text
                .get(c..)
                .and_then(|s| s.chars().next())
                .is_some_and(is_word_space)
            {
                space_per_byte[c] += g.advance.0 as i64;
            }
        }
        let mut space_count = vec![0u32; len + 1];
        let mut prefix = vec![0i64; len + 1];
        let mut space_prefix = vec![0i64; len + 1];
        let mut count = 0u32;
        let (mut sum, mut space_sum) = (0i64, 0i64);
        for i in 0..len {
            if text.is_char_boundary(i) && text[i..].chars().next().is_some_and(is_word_space) {
                count = count.saturating_add(1);
            }
            sum = sum.saturating_add(per_byte[i]);
            space_sum = space_sum.saturating_add(space_per_byte[i]);
            prefix[i + 1] = sum;
            space_prefix[i + 1] = space_sum;
            space_count[i + 1] = count;
        }
        let by_cluster = request
            .shaped
            .runs
            .iter()
            .map(|run| {
                let mut v: Vec<(usize, usize)> = run
                    .glyphs
                    .iter()
                    .enumerate()
                    .map(|(i, g)| (g.cluster as usize, i))
                    .collect();
                v.sort_unstable();
                v
            })
            .collect();

        Para {
            request,
            tabs: None,
            tab_positions: Vec::new(),
            tab_work: Cell::new(0),
            text,
            len,
            start,
            breaks,
            trimmed,
            start_begins_line,
            advance: additive.then_some(prefix),
            space_advance: space_prefix,
            space_count,
            unsafe_at,
            by_cluster,
            reshaped: RefCell::new(BTreeMap::new()),
        }
    }

    /// The index of the first break after `pos`.
    pub fn first_break_after(&self, pos: usize) -> usize {
        self.breaks.partition_point(|b| b.at <= pos)
    }

    /// Where a line from `pos` to break `index` stops having content.
    fn content_end(&self, pos: usize, index: usize) -> usize {
        self.trimmed.get(index).map_or(pos, |&t| t.max(pos))
    }

    /// Where a line from `pos` to `end` stops having content, for any `end`.
    pub(crate) fn content_end_at(&self, pos: usize, end: usize) -> usize {
        let i = self.breaks.partition_point(|b| b.at < end);
        match self.breaks.get(i) {
            Some(b) if b.at == end => self.content_end(pos, i),
            _ => {
                let trimmed = self.text.get(pos..end).map_or(0, |s| s.trim_end().len());
                pos + trimmed
            }
        }
    }

    /// The paragraph's width for the glyphs whose clusters start in `bytes`,
    /// exactly as [`reprise_shape::ShapedText::width`] computes it.
    pub fn width(&self, bytes: Range<usize>) -> Length {
        match &self.advance {
            Some(prefix) => {
                let at = |i: usize| prefix.get(i.min(self.len)).copied().unwrap_or(0);
                let w = at(bytes.end).saturating_sub(at(bytes.start)).max(0);
                Length(w.min(i32::MAX as i64) as i32)
            }
            None => self.request.shaped.width(bytes),
        }
    }

    /// The width of the line `pos..breaks[index].at` without its trailing
    /// whitespace, from the paragraph's shaping. What [`crate::Greedy`] fits by.
    pub fn natural_width(&self, pos: usize, index: usize) -> Length {
        self.width(pos..self.content_end(pos, index))
    }

    /// The width the fragment `pos..breaks[index].at` will actually have:
    /// [`Para::natural_width`], or the reshaped width when an edge is unsafe
    /// to break. Reshaped widths are cached, so asking twice shapes once.
    pub fn line_width(&self, pos: usize, index: usize) -> Length {
        let end = self.breaks.get(index).map_or(self.len, |b| b.at);
        if !self.needs_reshape(pos..end) {
            return self.natural_width(pos, index);
        }
        if let Some(&w) = self.reshaped.borrow().get(&(pos, end)) {
            return w;
        }
        let runs = self.request.reshape.reshape(pos..end);
        let w = reshaped_width(&runs, pos..self.content_end(pos, index));
        self.reshaped.borrow_mut().insert((pos, end), w);
        w
    }

    /// The total advance and number of word spaces in a line's content,
    /// from the paragraph's shaping.
    pub fn spaces(&self, pos: usize, index: usize) -> (i64, u32) {
        let end = self.content_end(pos, index).min(self.len);
        let pos = pos.min(end);
        let w = self.space_advance[end].saturating_sub(self.space_advance[pos]);
        let n = self.space_count[end].saturating_sub(self.space_count[pos]);
        (w.max(0), n)
    }

    /// True when a line starting at `pos` starts an authored line: it is the
    /// start of the text (or of a request that resumes after a forced break),
    /// or the previous line ended at a forced break.
    pub fn begins_line(&self, pos: usize) -> bool {
        if pos == self.start {
            return self.start_begins_line;
        }
        let i = self.breaks.partition_point(|b| b.at < pos);
        self.breaks
            .get(i)
            .is_some_and(|b| b.at == pos && b.kind == BreakKind::Forced)
    }

    fn edge_unsafe(&self, at: usize) -> bool {
        at > 0 && at < self.len && self.unsafe_at.get(at).copied().unwrap_or(false)
    }

    fn needs_reshape(&self, text: Range<usize>) -> bool {
        self.edge_unsafe(text.start) || self.edge_unsafe(text.end)
    }

    /// The paragraph's runs cut down to the glyphs whose clusters start in
    /// `bytes`; the same result as [`reprise_shape::ShapedText::slice`].
    fn slice(&self, bytes: Range<usize>) -> Vec<ShapedRun> {
        let runs = &self.request.shaped.runs;
        runs.iter()
            .zip(&self.by_cluster)
            .filter_map(|(run, sorted)| {
                let range = run.range.start.max(bytes.start)..run.range.end.min(bytes.end);
                if range.is_empty() {
                    return None;
                }
                let from = sorted.partition_point(|&(c, _)| c < range.start);
                let to = sorted.partition_point(|&(c, _)| c < range.end);
                let mut picked: Vec<usize> =
                    sorted.get(from..to)?.iter().map(|&(_, i)| i).collect();
                picked.sort_unstable();
                let glyphs: Vec<ShapedGlyph> = picked
                    .iter()
                    .filter_map(|&i| run.glyphs.get(i))
                    .copied()
                    .collect();
                (!glyphs.is_empty()).then(|| ShapedRun {
                    upright: run.upright,
                    combined: run.combined,
                    horizontal_scale: run.horizontal_scale,
                    range,
                    face: run.face.clone(),
                    size: run.size,
                    level: run.level,
                    glyphs,
                })
            })
            .collect()
    }

    /// Builds the fragment for `text` in `available`, reshaping it when an
    /// edge is unsafe to break. `width` is [`Para::line_width`] of the range.
    #[allow(clippy::too_many_arguments)]
    pub fn fragment(
        &self,
        text: Range<usize>,
        available: Interval,
        line: u32,
        block_offset: Length,
        reason: BreakReason,
        score: Option<i64>,
        adjustment: Adjustment,
    ) -> LineFragment {
        let reshaped = self.needs_reshape(text.clone());
        let content = text.start..self.content_end_at(text.start, text.end);
        let (mut runs, mut width) = if reshaped {
            let runs = self.request.reshape.reshape(text.clone());
            let width = reshaped_width(&runs, content.clone());
            (runs, width)
        } else {
            (self.slice(text.clone()), self.width(content.clone()))
        };
        if self.tabs.is_some() {
            width = self.adjust_tabs(&mut runs, content.clone(), available.width());
        }
        LineFragment {
            text,
            runs,
            width,
            available,
            line,
            block_offset,
            height: self.request.line_height,
            explanation: Explanation {
                reason,
                score,
                adjustment,
                reshaped,
            },
        }
    }
}

/// The width of reshaped runs: the advances of the glyphs whose clusters
/// start in `content`, summed in glyph order with saturation.
fn reshaped_width(runs: &[ShapedRun], content: Range<usize>) -> Length {
    runs.iter()
        .flat_map(|r| &r.glyphs)
        .filter(|g| content.contains(&(g.cluster as usize)))
        .map(|g| g.advance)
        .sum()
}

/// `interval` with its inline start moved in by `indent`, never past its end.
pub(crate) fn indented(interval: Interval, indent: Length) -> Interval {
    if indent <= Length::ZERO {
        return interval;
    }
    let start = (interval.start + indent).min(interval.end.max(interval.start));
    Interval::new(start, interval.end)
}
