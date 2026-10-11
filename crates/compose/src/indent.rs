//! Paragraph indents as a geometry provider (`docs/paragraph-styles.md`).
//!
//! [`Indented`] wraps the provider describing a frame's measure and takes the
//! paragraph's indents off each line's room, before float exclusions are
//! subtracted. Everything is logical: `start` is the inline start of the
//! paragraph's base direction, so a right-to-left paragraph loses it on the
//! right (`reversed`), and a vertical frame's inline axis needs nothing extra.

use reprise_geom::Length;

use crate::{Available, GeometryProvider, Interval, LineQuery};

/// Used indents, already bounded by the caller (see [`Indents::bounded`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Indents {
    pub start: Length,
    pub end: Length,
    /// Added to `start` on the paragraph's first line; negative hangs.
    pub first: Length,
}

impl Indents {
    pub fn is_zero(&self) -> bool {
        *self == Indents::default()
    }

    /// Bounds authored indents for a measure of `width`: `start` and `end`
    /// are at least zero, a hanging `first` never takes the first line past
    /// the measure's start, and together they leave at least a quarter of
    /// `width` (reducing `end`, then `first`, then `start`). The flag says
    /// something had to be reduced beyond the sign rules.
    pub fn bounded(start: Length, end: Length, first: Length, width: Length) -> (Indents, bool) {
        let width = width.max(Length::ZERO);
        let mut start = start.max(Length::ZERO);
        let mut end = end.max(Length::ZERO);
        let mut first = first.max(-start);
        let budget = width - width.mul_ratio(1, 4);
        let mut reduced = false;
        let lead = |start: Length, first: Length| start + first.max(Length::ZERO);
        if lead(start, first) + end > budget {
            reduced = true;
            end = (budget - lead(start, first)).max(Length::ZERO);
            if lead(start, first) > budget {
                first = first.min((budget - start).max(Length::ZERO));
                start = start.min(budget);
                first = first.max(-start);
            }
        }
        (Indents { start, end, first }, reduced)
    }
}

/// A provider whose intervals lose the paragraph's indents.
pub struct Indented<G> {
    pub base: G,
    pub indents: Indents,
    /// Line 0 of this composition is the paragraph's first line.
    pub first_line: bool,
    /// The paragraph runs right to left: its start is the intervals' end.
    pub reversed: bool,
}

impl<G: GeometryProvider> GeometryProvider for Indented<G> {
    fn available(&self, query: &LineQuery<'_>) -> Available {
        let mut intervals = match self.base.available(query) {
            Available::Room(intervals) => intervals,
            other => return other,
        };
        if self.indents.is_zero() {
            return Available::Room(intervals);
        }
        let mut lead = self.indents.start;
        if self.first_line && query.line == 0 {
            lead += self.indents.first;
        }
        let (left, right) = if self.reversed {
            (self.indents.end, lead)
        } else {
            (lead, self.indents.end)
        };
        if let Some(first) = intervals.first_mut() {
            first.start += left;
        }
        if let Some(last) = intervals.last_mut() {
            last.end -= right;
        }
        // An interval the indents consumed has no room; never reversed.
        for i in &mut intervals {
            if i.end < i.start {
                *i = Interval::new(i.start, i.start);
            }
        }
        Available::Room(intervals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Measure;

    fn room(p: &dyn GeometryProvider, line: u32) -> Vec<Interval> {
        match p.available(&LineQuery {
            line,
            block_offset: Length::ZERO,
            line_height: Length(10),
            previous: &[],
        }) {
            Available::Room(r) => r,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn first_line_hangs_and_rtl_mirrors() {
        let (indents, reduced) = Indents::bounded(Length(30), Length(10), Length(-50), Length(400));
        assert!(!reduced);
        assert_eq!(indents.first, Length(-30), "bounded by start");
        let ltr = Indented {
            base: Measure(Length(400)),
            indents,
            first_line: true,
            reversed: false,
        };
        assert_eq!(room(&ltr, 0), [Interval::new(Length(0), Length(390))]);
        assert_eq!(room(&ltr, 1), [Interval::new(Length(30), Length(390))]);
        let rtl = Indented {
            reversed: true,
            ..ltr
        };
        assert_eq!(room(&rtl, 1), [Interval::new(Length(10), Length(370))]);
        let continued = Indented {
            first_line: false,
            ..rtl
        };
        assert_eq!(
            room(&continued, 0),
            [Interval::new(Length(10), Length(370))]
        );
    }

    #[test]
    fn absurd_indents_leave_a_quarter_and_never_overflow() {
        for (s, e, f, w) in [
            (Length::MAX, Length::MAX, Length::MAX, Length(400)),
            (Length::MIN, Length::MIN, Length::MIN, Length(400)),
            (Length(1000), Length(0), Length(0), Length(400)),
            (Length(0), Length::MAX, Length(0), Length::MAX),
            (Length(5), Length(5), Length(5), Length::ZERO),
            (Length(5), Length(5), Length(5), Length(-40)),
        ] {
            let (i, _) = Indents::bounded(s, e, f, w);
            let w = w.max(Length::ZERO);
            assert!(i.start >= Length::ZERO && i.end >= Length::ZERO);
            assert!(i.start + i.first >= Length::ZERO);
            assert!(
                i.start + i.first.max(Length::ZERO) + i.end <= w - w.mul_ratio(1, 4),
                "{i:?} for {w:?}"
            );
            let p = Indented {
                base: Measure(w),
                indents: i,
                first_line: true,
                reversed: false,
            };
            for r in room(&p, 0).into_iter().chain(room(&p, 1)) {
                assert!(r.start <= r.end);
            }
        }
        let (_, reduced) = Indents::bounded(Length(1000), Length(0), Length(0), Length(400));
        assert!(reduced);
    }
}
