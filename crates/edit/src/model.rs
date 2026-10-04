//! What the navigator derives from a laid-out line: grapheme cells and the
//! caret stops at their edges (30).
//!
//! A line's runs are in visual order, left to right along the inline axis, and
//! each run's glyphs are in visual order too. Glyphs name the byte where their
//! cluster starts; a cluster ends where the next one starts in logical order,
//! or where the run ends. A cluster that holds several graphemes (a ligature)
//! is split evenly between them, the usual convention, since a font records no
//! caret positions inside it.
//!
//! Everything is integer arithmetic, rounded half away from zero.

use std::collections::BTreeSet;
use std::ops::Range;

use reprise_doc::text::segment;
use reprise_geom::{Length, div_round};
use reprise_layout::{BlockLayout, LineLayout};

use crate::caret::{Affinity, GraphemeCell};

/// What the navigator needs to know about a block's text.
pub(crate) struct BlockInfo {
    /// Every grapheme boundary, `0` and the length included.
    pub boundaries: Vec<usize>,
    /// The paragraph direction actually used by layout, from its base level.
    pub rtl: bool,
    /// Word-like segments, for word movement.
    pub words: Vec<Range<usize>>,
}

impl BlockInfo {
    pub fn new(text: &str, base_level: u8) -> BlockInfo {
        BlockInfo {
            boundaries: segment::grapheme_boundaries(text),
            rtl: base_level % 2 == 1,
            words: segment::words(text),
        }
    }

    /// The last boundary strictly before `at`.
    pub fn prev_boundary(&self, at: usize) -> Option<usize> {
        let i = self.boundaries.partition_point(|&b| b < at);
        i.checked_sub(1).map(|i| self.boundaries[i])
    }

    /// The first boundary strictly after `at`.
    pub fn next_boundary(&self, at: usize) -> Option<usize> {
        let i = self.boundaries.partition_point(|&b| b <= at);
        self.boundaries.get(i).copied()
    }

    pub fn is_boundary(&self, at: usize) -> bool {
        self.boundaries.binary_search(&at).is_ok()
    }
}

/// A hard line break: it ends a line, and a caret is never drawn after it on
/// that line.
fn is_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// The offset past which a caret can't be drawn on line `index` of a block:
/// the line's end, or its start of the hard break that ends it when another
/// line follows (the position after the break belongs to that line).
pub(crate) fn end_limit(block: &BlockLayout, info: &BlockInfo, index: usize) -> usize {
    let Some(line) = block.lines.get(index) else {
        return 0;
    };
    let end = line.text.end;
    if index + 1 >= block.lines.len() {
        return end;
    }
    match info.prev_boundary(end) {
        Some(start)
            if start >= line.text.start
                && block
                    .text
                    .get(start..end)
                    .is_some_and(|g| !g.is_empty() && g.chars().all(is_break)) =>
        {
            start
        }
        _ => end,
    }
}

/// One place a caret can be drawn on a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stop {
    pub x: Length,
    pub offset: usize,
    pub affinity: Affinity,
    /// The stop is a cell's left edge (as opposed to its right edge).
    pub left_edge: bool,
    pub level: u8,
    /// Between which cells it is: `j` is between cell `j - 1` and cell `j`.
    pub junction: usize,
}

/// A line as the navigator sees it.
pub(crate) struct LineModel {
    /// The cells of the graphemes a caret can sit beside, in visual order.
    pub cells: Vec<GraphemeCell>,
    /// Both edges of every cell, in visual order.
    pub stops: Vec<Stop>,
    pub start: usize,
    pub end_limit: usize,
    /// Where an empty line's caret goes.
    pub empty_x: Length,
    pub empty_level: u8,
}

impl LineModel {
    pub fn new(
        block: &BlockLayout,
        info: &BlockInfo,
        index: usize,
        line: &LineLayout,
    ) -> LineModel {
        let limit = end_limit(block, info, index);
        let mut cells = Vec::new();
        for run in &line.runs {
            run_cells(run, info, &mut cells);
        }
        cells.retain(|c| {
            c.range.start >= line.text.start
                && c.range.end <= limit
                && info.is_boundary(c.range.start)
                && info.is_boundary(c.range.end)
        });
        // Missing-font spans may have no runs at all. Keep their logical
        // grapheme stops, with zero width at the nearest mapped text edge.
        let covered: BTreeSet<usize> = cells.iter().map(|c| c.range.start).collect();
        let mut missing = Vec::new();
        let first = info.boundaries.partition_point(|&b| b < line.text.start);
        for edge in info.boundaries.get(first..).unwrap_or_default().windows(2) {
            if edge[1] > limit {
                break;
            }
            if covered.contains(&edge[0]) {
                continue;
            }
            let nearest = cells.iter().min_by_key(|c| c.range.start.abs_diff(edge[0]));
            let x = nearest.map_or(line.rect.origin.x, |c| {
                let before = edge[1] <= c.range.start;
                if before != (c.level % 2 == 1) {
                    c.x0
                } else {
                    c.x1
                }
            });
            missing.push(GraphemeCell {
                range: edge[0]..edge[1],
                x0: x,
                x1: x,
                level: u8::from(info.rtl),
            });
        }
        if !missing.is_empty() {
            if info.rtl {
                missing.reverse();
            }
            cells.extend(missing);
            cells.sort_by_key(|c| c.x0);
        }
        let mut stops = Vec::with_capacity(cells.len() * 2);
        for (i, cell) in cells.iter().enumerate() {
            let rtl = cell.level % 2 == 1;
            let (start, end) = (cell.range.start, cell.range.end);
            stops.push(Stop {
                x: cell.x0,
                offset: if rtl { end } else { start },
                affinity: if rtl {
                    Affinity::Upstream
                } else {
                    Affinity::Downstream
                },
                left_edge: true,
                level: cell.level,
                junction: i,
            });
            stops.push(Stop {
                x: cell.x1,
                offset: if rtl { start } else { end },
                affinity: if rtl {
                    Affinity::Downstream
                } else {
                    Affinity::Upstream
                },
                left_edge: false,
                level: cell.level,
                junction: i + 1,
            });
        }
        let empty_x = if info.rtl {
            line.rect.origin.x + line.rect.width
        } else {
            line.rect.origin.x
        };
        LineModel {
            cells,
            stops,
            start: line.text.start,
            end_limit: limit.max(line.text.start),
            empty_x,
            empty_level: block.base_level,
        }
    }

    /// The stop for a caret. The stop with the same offset and affinity, else
    /// the same offset with the other affinity, else (when the offset sits in
    /// text with no glyphs, such as a missing font's) the nearest offset.
    pub fn locate(&self, offset: usize, affinity: Affinity) -> Stop {
        let same = |s: &&Stop| s.offset == offset;
        let exact = self
            .stops
            .iter()
            .filter(same)
            .find(|s| s.affinity == affinity)
            .or_else(|| self.stops.iter().find(same));
        if let Some(stop) = exact {
            return *stop;
        }
        let nearest = self
            .stops
            .iter()
            .min_by_key(|s| (s.offset.abs_diff(offset), s.offset > offset));
        match nearest {
            Some(stop) => *stop,
            None => self.empty_stop(),
        }
    }

    /// The caret of a line with nothing on it.
    pub fn empty_stop(&self) -> Stop {
        Stop {
            x: self.empty_x,
            offset: self.start,
            affinity: Affinity::Downstream,
            left_edge: true,
            level: self.empty_level,
            junction: 0,
        }
    }

    /// Normalizes affinity only when the logical offset also agrees. Equal x
    /// alone does not identify a caret: zero-width text and bidi junctions
    /// can put distinct logical positions at the same visual location.
    pub fn canonical(&self, stop: Stop) -> Stop {
        let at: Vec<&Stop> = self
            .stops
            .iter()
            .filter(|s| s.x == stop.x && s.offset == stop.offset)
            .collect();
        at.iter()
            .find(|s| s.left_edge)
            .or_else(|| at.last())
            .map_or(stop, |s| **s)
    }

    /// The stops at a junction: one when its two sides agree, two (a split
    /// caret) when a bidi boundary or a gap makes them differ.
    pub fn at_junction(&self, junction: usize) -> Vec<Stop> {
        self.stops
            .iter()
            .filter(|s| s.junction == junction)
            .copied()
            .collect()
    }

    /// The stop to land on at `junction`. A junction has one stop, or two
    /// when it joins the right edge of one cell to the left edge of the next.
    /// If the two agree on the offset, the left edge of the cell on the right
    /// is the one (as for a hit). If they don't (a bidi boundary, or a gap),
    /// it is a split caret, and `prefer` says which, so that a step lands where
    /// it crossed to; without a preference it is the same left edge.
    ///
    /// This picks by junction, not by x: zero-width cells share an x, and a
    /// step must still get past them.
    pub fn arrive(&self, junction: usize, prefer: Option<Stop>) -> Stop {
        let here = self.at_junction(junction);
        let left = here.iter().find(|s| s.left_edge).or(here.first()).copied();
        let Some(left) = left else {
            return self.empty_stop();
        };
        prefer
            .filter(|p| here.iter().any(|s| s == p) && here.iter().any(|s| s.offset != left.offset))
            .unwrap_or(left)
    }

    /// The cell nearest to `x` (distance 0 when inside, edges included), the
    /// earlier one on a tie.
    pub fn nearest_cell(&self, x: Length) -> Option<usize> {
        (0..self.cells.len()).min_by_key(|&i| {
            let c = &self.cells[i];
            gap(c.x0, c.x1, x)
        })
    }

    /// The stop nearest `x`: the nearer edge of the nearest cell, made
    /// canonical.
    pub fn stop_at_x(&self, x: Length) -> Stop {
        let Some(i) = self.nearest_cell(x) else {
            return self.empty_stop();
        };
        let c = &self.cells[i];
        // The nearer edge; the right edge wins a tie.
        let left = (i64::from(x.0) - i64::from(c.x0.0)).abs();
        let right = (i64::from(c.x1.0) - i64::from(x.0)).abs();
        let stop = if left < right {
            self.stops[2 * i]
        } else {
            self.stops[2 * i + 1]
        };
        self.canonical(stop)
    }
}

/// The distance from `v` to the interval between `a` and `b`.
pub(crate) fn gap(a: Length, b: Length, v: Length) -> i64 {
    let (lo, hi) = (i64::from(a.0.min(b.0)), i64::from(a.0.max(b.0)));
    let v = i64::from(v.0);
    (lo - v).max(v - hi).max(0)
}

/// The cells of one run, appended in visual order.
fn run_cells(run: &reprise_layout::PositionedRun, info: &BlockInfo, out: &mut Vec<GraphemeCell>) {
    let rtl = run.level % 2 == 1;
    let range = &run.range;
    if range.start >= range.end {
        return;
    }
    // Glyph groups: consecutive glyphs of one cluster, in visual order, each
    // with its extent.
    let mut groups: Vec<(usize, Length, Length)> = Vec::new();
    let mut pen = run.x;
    for glyph in &run.glyphs {
        let cluster = glyph.cluster as usize;
        let from = pen;
        pen += glyph.advance;
        match groups.last_mut() {
            Some((c, _, x1)) if *c == cluster => *x1 = pen,
            _ => groups.push((cluster, from, pen)),
        }
    }
    groups.retain(|(c, ..)| range.contains(c) && info.is_boundary(*c));
    // A cluster runs to the next larger cluster, or to the run's end.
    let starts: BTreeSet<usize> = groups.iter().map(|(c, ..)| *c).collect();
    let end_of = |c: usize| {
        starts
            .range(c + 1..)
            .next()
            .copied()
            .unwrap_or(range.end)
            .min(range.end)
    };
    let mut cells: Vec<GraphemeCell> = Vec::new();
    for &(cluster, a, b) in &groups {
        let (lo, hi) = (a.min(b), a.max(b));
        let end = end_of(cluster);
        // The cluster's graphemes, as boundaries; the cluster's own edges
        // count even if a malformed shaper put them inside a grapheme.
        let mut edges = vec![cluster];
        edges.extend(
            info.boundaries
                .iter()
                .copied()
                .filter(|&bd| bd > cluster && bd < end),
        );
        edges.push(end);
        let n = edges.len() - 1;
        let width = i64::from(hi.0) - i64::from(lo.0);
        let at = |k: usize| {
            let share = div_round(width.saturating_mul(k as i64), n as i64);
            Length(
                i64::from(lo.0)
                    .saturating_add(share)
                    .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            )
        };
        for k in 0..n {
            let grapheme = if rtl { n - 1 - k } else { k };
            cells.push(GraphemeCell {
                range: edges[grapheme]..edges[grapheme + 1],
                x0: at(k),
                x1: at(k + 1),
                level: run.level,
            });
        }
    }
    // Text before the first cluster has no glyph: no width, at the run's
    // logical start edge.
    let first = starts.iter().next().copied().unwrap_or(range.end);
    if first > range.start {
        let edge = if rtl { run.x + run.width } else { run.x };
        let mut bytes = vec![range.start];
        bytes.extend(
            info.boundaries
                .iter()
                .copied()
                .filter(|&bd| bd > range.start && bd < first),
        );
        bytes.push(first);
        let mut leading: Vec<GraphemeCell> = bytes
            .windows(2)
            .map(|w| GraphemeCell {
                range: w[0]..w[1],
                x0: edge,
                x1: edge,
                level: run.level,
            })
            .collect();
        if rtl {
            // Logically first is visually rightmost.
            leading.reverse();
            cells.extend(leading);
        } else {
            leading.append(&mut cells);
            cells = leading;
        }
    }
    out.append(&mut cells);
}
