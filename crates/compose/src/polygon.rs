//! Non-rectangular geometry providers (decision 23): shapes to fill, and
//! shapes to run around. Floats will use [`Runaround`] (24); rotated frames
//! put these inside their own logical space (20).
//!
//! # Integer edge intersection
//!
//! A line occupies the band `[block_offset, block_offset + line_height]`.
//! Between consecutive vertex heights inside that band no edge starts or
//! ends, and the edges of a simple polygon don't cross, so each sub-band has
//! a fixed left-to-right order of edges. Each edge is linear, so its extremes
//! over a sub-band are at the sub-band's top and bottom: the x where an edge
//! meets height `y` is
//!
//! ```text
//! x = x₀ + (x₁ − x₀) · (y − y₀) / (y₁ − y₀)
//! ```
//!
//! rounded half away from zero, like [`reprise_geom::div_round`]. The product
//! can need 65 bits when vertices sit near [`Length::MIN`] and [`Length::MAX`],
//! so it is computed in `i128` with the same rounding rule; the result lies
//! between `x₀` and `x₁`, so it always fits a [`Length`]. Nothing here can
//! overflow or panic, whatever the vertices.
//!
//! The interior is the even-odd interior. For a polygon whose edges cross,
//! sub-bands are still split only at vertices, so the result is the even-odd
//! interior of each sub-band's edge order: deterministic, but approximate
//! where edges cross inside a sub-band.

use reprise_geom::{FrameSpace, Length, Point, Rect};

use crate::{Available, GeometryProvider, Interval, LineQuery};

/// Turns a unit vector `(c, s)` by a multiple of 90°.
type Turn = fn(i64, i64) -> (i64, i64);

/// A polygon in frame space, as a geometry provider: each line gets the
/// intervals where the whole line band lies inside it.
///
/// -   Above the polygon's top, it answers `Skip` to the top.
/// -   A band reaching below the bottom answers `End`.
/// -   Otherwise `Room` with the intervals inside the polygon over the
///     *whole* band (the intersection over every height in it), in inline
///     order. Concave shapes give several. A band with no room gives an empty
///     list, which composers skip by one line height.
/// -   A line height below one sub-unit counts as one sub-unit.
/// -   A polygon with fewer than three vertices or no height answers `End`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Polygon {
    points: Vec<(i64, i64)>,
    top: Length,
    bottom: Length,
}

/// One non-horizontal edge.
#[derive(Clone, Copy, Debug)]
struct Edge {
    /// Endpoints, with `a.1 < b.1`.
    a: (i64, i64),
    b: (i64, i64),
}

impl Edge {
    /// The edge's x at height `y`, which must be within its span.
    fn x_at(&self, y: i64) -> i64 {
        let (dx, dy) = (self.b.0 - self.a.0, self.b.1 - self.a.1);
        let num = dx as i128 * (y - self.a.1) as i128;
        self.a.0 + div_round_wide(num, dy as i128) as i64
    }
}

/// `num / den` rounded half away from zero, as [`reprise_geom::div_round`],
/// for products that need more than 64 bits. `den` is positive here.
fn div_round_wide(num: i128, den: i128) -> i128 {
    if den == 0 {
        return 0;
    }
    let magnitude = (num.abs() + den.abs() / 2) / den.abs();
    if (num < 0) != (den < 0) {
        -magnitude
    } else {
        magnitude
    }
}

fn length(v: i64) -> Length {
    Length(v.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

impl Polygon {
    /// A polygon through `points` in order; the last joins the first.
    pub fn new(points: impl IntoIterator<Item = Point<FrameSpace>>) -> Polygon {
        let points: Vec<(i64, i64)> = points
            .into_iter()
            .map(|p| (p.x.0 as i64, p.y.0 as i64))
            .collect();
        let top = points.iter().map(|p| p.1).min().unwrap_or(0);
        let bottom = points.iter().map(|p| p.1).max().unwrap_or(0);
        Polygon {
            points,
            top: length(top),
            bottom: length(bottom),
        }
    }

    /// A rectangle. Negative widths and heights extend the other way.
    pub fn rect(r: Rect<FrameSpace>) -> Polygon {
        let (x0, y0) = (r.origin.x, r.origin.y);
        let (x1, y1) = (r.max_x(), r.max_y());
        Polygon::new([(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| Point::new(x, y)))
    }

    /// An ellipse around `center` with radii `rx` and `ry` (signs ignored),
    /// as `4 · steps` vertices exactly on it, rounded to sub-units. `steps`
    /// is per quarter and clamped to `1..=1024`.
    ///
    /// The vertices come from the rational parametrisation of the circle,
    /// `((n² − k²) / (n² + k²), 2kn / (n² + k²))` for `k` in `0..n`, so they
    /// are exact integers, not floating point.
    pub fn ellipse(center: Point<FrameSpace>, rx: Length, ry: Length, steps: u32) -> Polygon {
        let n = steps.clamp(1, 1024) as i64;
        let (rx, ry) = (rx.0.unsigned_abs() as i64, ry.0.unsigned_abs() as i64);
        let (cx, cy) = (center.x.0 as i64, center.y.0 as i64);
        let quarter: Vec<(i64, i64, i64)> = (0..n)
            .map(|k| (n * n - k * k, 2 * k * n, n * n + k * k))
            .collect();
        let mut points = Vec::with_capacity(4 * quarter.len());
        // Each quarter is the first one turned by 90°: (c, s) → (−s, c).
        let turns: [Turn; 4] = [
            |c, s| (c, s),
            |c, s| (-s, c),
            |c, s| (-c, -s),
            |c, s| (s, -c),
        ];
        for turn in turns {
            for &(c, s, d) in &quarter {
                let (ux, uy) = turn(c, s);
                let x = cx + reprise_geom::div_round(rx * ux, d);
                let y = cy + reprise_geom::div_round(ry * uy, d);
                points.push(Point::new(length(x), length(y)));
            }
        }
        Polygon::new(points)
    }

    /// A circle; see [`Polygon::ellipse`].
    pub fn circle(center: Point<FrameSpace>, radius: Length, steps: u32) -> Polygon {
        Polygon::ellipse(center, radius, radius, steps)
    }

    pub fn top(&self) -> Length {
        self.top
    }

    pub fn bottom(&self) -> Length {
        self.bottom
    }

    fn edges(&self) -> impl Iterator<Item = Edge> + '_ {
        let n = self.points.len();
        (0..n).filter_map(move |i| {
            let p = *self.points.get(i)?;
            let q = *self.points.get((i + 1) % n)?;
            match p.1.cmp(&q.1) {
                std::cmp::Ordering::Less => Some(Edge { a: p, b: q }),
                std::cmp::Ordering::Greater => Some(Edge { a: q, b: p }),
                std::cmp::Ordering::Equal => None,
            }
        })
    }

    /// For each sub-band of `[top, bottom]` split at vertex heights, the
    /// interior's pieces as `(left edge, right edge)` x at the sub-band's top
    /// and bottom: `(l_top, l_bottom, r_top, r_bottom)`.
    fn sub_bands(&self, top: i64, bottom: i64) -> Vec<Vec<(i64, i64, i64, i64)>> {
        if self.points.len() < 3 || top >= bottom {
            return Vec::new();
        }
        let mut cuts: Vec<i64> = self
            .points
            .iter()
            .map(|p| p.1)
            .filter(|&y| y > top && y < bottom)
            .collect();
        cuts.push(top);
        cuts.push(bottom);
        cuts.sort_unstable();
        cuts.dedup();
        let edges: Vec<Edge> = self.edges().collect();
        cuts.windows(2)
            .map(|w| {
                let (ya, yb) = (w[0], w[1]);
                let mut active: Vec<(i64, i64)> = edges
                    .iter()
                    .filter(|e| e.a.1 <= ya && e.b.1 >= yb)
                    .map(|e| (e.x_at(ya), e.x_at(yb)))
                    .collect();
                // Edges don't cross inside a sub-band: order by the midpoint.
                active.sort_by_key(|&(xa, xb)| (xa + xb, xa));
                active
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|[l, r]| (l.0, l.1, r.0, r.1))
                    .collect()
            })
            .collect()
    }

    /// The intervals where the vertical segment from `top` to `bottom` lies
    /// inside the polygon: where a line box over that band fits. Sorted,
    /// disjoint, each wider than zero.
    pub fn inside(&self, top: Length, bottom: Length) -> Vec<Interval> {
        let bands = self.sub_bands(top.0 as i64, bottom.0 as i64);
        let mut result: Option<Vec<(i64, i64)>> = None;
        for pieces in bands {
            let here: Vec<(i64, i64)> = pieces
                .into_iter()
                .map(|(la, lb, ra, rb)| (la.max(lb), ra.min(rb)))
                .filter(|(lo, hi)| lo < hi)
                .collect();
            result = Some(match result {
                None => here,
                Some(so_far) => intersect(&so_far, &here),
            });
        }
        to_intervals(result.unwrap_or_default())
    }

    /// The intervals the polygon's interior reaches anywhere strictly
    /// between `top` and `bottom`: what it blocks on a line over that band.
    /// Sorted, disjoint, each wider than zero.
    pub fn covered(&self, top: Length, bottom: Length) -> Vec<Interval> {
        let mut pieces: Vec<(i64, i64)> = self
            .sub_bands(top.0 as i64, bottom.0 as i64)
            .into_iter()
            .flatten()
            .map(|(la, lb, ra, rb)| (la.min(lb), ra.max(rb)))
            .filter(|(lo, hi)| lo < hi)
            .collect();
        pieces.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::with_capacity(pieces.len());
        for (lo, hi) in pieces {
            match merged.last_mut() {
                Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        to_intervals(merged)
    }
}

/// The intersection of two sorted, disjoint interval lists.
fn intersect(a: &[(i64, i64)], b: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while let (Some(&(a0, a1)), Some(&(b0, b1))) = (a.get(i), b.get(j)) {
        let (lo, hi) = (a0.max(b0), a1.min(b1));
        if lo < hi {
            out.push((lo, hi));
        }
        if a1 < b1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

fn to_intervals(v: Vec<(i64, i64)>) -> Vec<Interval> {
    v.into_iter()
        .map(|(lo, hi)| Interval::new(length(lo), length(hi)))
        .filter(|i| i.start < i.end)
        .collect()
}

/// The band a line covers: at least one sub-unit tall.
fn band(query: &LineQuery<'_>) -> (Length, Length) {
    let top = query.block_offset;
    (top, top + query.line_height.max(Length(1)))
}

impl GeometryProvider for Polygon {
    fn available(&self, query: &LineQuery<'_>) -> Available {
        if self.points.len() < 3 || self.top >= self.bottom {
            return Available::End;
        }
        let (top, bottom) = band(query);
        if top < self.top {
            return Available::Skip { next: self.top };
        }
        if bottom > self.bottom {
            return Available::End;
        }
        Available::Room(self.inside(top, bottom))
    }
}

/// A base provider with shapes cut out of it: text runs around them.
///
/// For each line the base answers; `Skip` and `End` pass through unchanged.
/// From `Room`, every exclusion's [`Polygon::covered`] intervals over the
/// line band grown by `margin` above and below, themselves grown by `margin`
/// on both sides, are removed. (That is the exclusion dilated by a square of
/// side `2 · margin`.) Pieces narrower than `min_width`, and empty ones, are
/// dropped: text never goes into a sliver. If nothing is left, the answer is
/// an empty `Room`, which composers skip by one line height.
///
/// Negative margins and minimum widths count as zero. Arithmetic saturates.
pub struct Runaround<G> {
    pub base: G,
    pub exclusions: Vec<Polygon>,
    pub margin: Length,
    pub min_width: Length,
}

impl<G: GeometryProvider> GeometryProvider for Runaround<G> {
    fn available(&self, query: &LineQuery<'_>) -> Available {
        let intervals = match self.base.available(query) {
            Available::Room(intervals) => intervals,
            other => return other,
        };
        let margin = self.margin.max(Length::ZERO);
        let (top, bottom) = band(query);
        let mut blocked: Vec<(Length, Length)> = self
            .exclusions
            .iter()
            .flat_map(|e| e.covered(top - margin, bottom + margin))
            .map(|i| (i.start - margin, i.end + margin))
            .collect();
        blocked.sort_unstable();
        let min_width = self.min_width.max(Length(1));
        let mut room = Vec::new();
        for interval in intervals {
            let mut from = interval.start;
            for &(lo, hi) in &blocked {
                if hi <= from || lo >= interval.end {
                    continue;
                }
                if lo > from {
                    room.push(Interval::new(from, lo));
                }
                from = from.max(hi);
            }
            if from < interval.end {
                room.push(Interval::new(from, interval.end));
            }
        }
        room.retain(|i| i.width() >= min_width);
        Available::Room(room)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(v: i32) -> Length {
        Length::from_pt(v)
    }

    fn p(x: i32, y: i32) -> Point<FrameSpace> {
        Point::new(pt(x), pt(y))
    }

    fn ask(g: &dyn GeometryProvider, y: i32, lh: i32) -> Available {
        g.available(&LineQuery {
            line: 0,
            block_offset: pt(y),
            line_height: pt(lh),
            previous: &[],
        })
    }

    fn room(spans: &[(i32, i32)]) -> Available {
        Available::Room(
            spans
                .iter()
                .map(|&(a, b)| Interval::new(pt(a), pt(b)))
                .collect(),
        )
    }

    #[test]
    fn a_rectangle_gives_its_width_skips_to_its_top_and_ends_at_its_bottom() {
        let r = Polygon::rect(Rect::new(p(10, 20), pt(100), pt(50)));
        assert_eq!(ask(&r, 0, 10), Available::Skip { next: pt(20) });
        assert_eq!(ask(&r, 20, 10), room(&[(10, 110)]));
        assert_eq!(ask(&r, 60, 10), room(&[(10, 110)]), "the last band fits");
        assert_eq!(ask(&r, 61, 10), Available::End);
    }

    #[test]
    fn a_triangle_gives_the_intersection_over_the_whole_band() {
        // Apex at the top: the band's narrowest height is its top.
        let t = Polygon::new([p(50, 0), p(100, 100), p(0, 100)]);
        // At y = 20 the triangle spans 40..60; at y = 30, 35..65.
        assert_eq!(ask(&t, 20, 10), room(&[(40, 60)]));
        // Upside down: the narrowest height is the band's bottom.
        let v = Polygon::new([p(0, 0), p(100, 0), p(50, 100)]);
        // At y = 70 the triangle spans 35..65; at y = 80, 40..60.
        assert_eq!(ask(&v, 70, 10), room(&[(40, 60)]));
    }

    #[test]
    fn a_band_across_a_vertex_takes_the_narrowest_height_inside_it() {
        // A diamond: widest at y = 50, where the band straddles the vertex.
        let d = Polygon::new([p(50, 0), p(100, 50), p(50, 100), p(0, 50)]);
        // At y = 45 and y = 55 the diamond spans 5..95; at 50, 0..100.
        assert_eq!(ask(&d, 45, 10), room(&[(5, 95)]));
    }

    #[test]
    fn concave_shapes_give_several_intervals() {
        // A U: two prongs from y = 0 to 60, joined below y = 60.
        let u = Polygon::new([
            p(0, 0),
            p(30, 0),
            p(30, 60),
            p(70, 60),
            p(70, 0),
            p(100, 0),
            p(100, 100),
            p(0, 100),
        ]);
        assert_eq!(ask(&u, 10, 10), room(&[(0, 30), (70, 100)]));
        assert_eq!(ask(&u, 60, 10), room(&[(0, 100)]));
        // A band across the notch's bottom only fits where both parts do.
        assert_eq!(ask(&u, 55, 10), room(&[(0, 30), (70, 100)]));
    }

    #[test]
    fn a_band_through_a_pointed_end_has_no_room() {
        // At y = 0 the triangle has no width, so nothing fits the band 0..10.
        let t = Polygon::new([p(50, 0), p(100, 100), p(0, 100)]);
        assert_eq!(ask(&t, 0, 10), room(&[]));
    }

    #[test]
    fn degenerate_polygons_end() {
        assert_eq!(ask(&Polygon::new([]), 0, 10), Available::End);
        assert_eq!(
            ask(&Polygon::new([p(0, 0), p(10, 10)]), 0, 10),
            Available::End
        );
        let flat = Polygon::new([p(0, 5), p(10, 5), p(20, 5)]);
        assert_eq!(ask(&flat, 0, 10), Available::End);
    }

    #[test]
    fn zero_and_negative_line_heights_count_as_one_sub_unit() {
        let r = Polygon::rect(Rect::new(p(0, 0), pt(100), pt(50)));
        for lh in [0, -10] {
            assert_eq!(ask(&r, 10, lh), room(&[(0, 100)]));
            assert_eq!(ask(&r, 50, lh), Available::End);
        }
    }

    #[test]
    fn vertices_at_the_limits_saturate_and_never_panic() {
        let (lo, hi) = (Length::MIN, Length::MAX);
        let huge = Polygon::new([
            Point::new(lo, lo),
            Point::new(hi, lo),
            Point::new(hi, hi),
            Point::new(lo, hi),
        ]);
        assert_eq!(
            ask(&huge, 0, 10),
            Available::Room(vec![Interval::new(lo, hi)])
        );
        // A sliver of a triangle spanning the whole range.
        let slant = Polygon::new([Point::new(lo, lo), Point::new(hi, hi), Point::new(lo, hi)]);
        let Available::Room(r) = ask(&slant, 0, 10) else {
            panic!("room expected")
        };
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].start, lo);
        // At y = 0 the diagonal is at x = 0 (to within rounding).
        assert!(r[0].end.0.abs() <= 1, "{:?}", r[0]);
        for y in [i32::MIN, -1, 0, 1, i32::MAX] {
            for lh in [i32::MIN, -1, 0, 1, i32::MAX] {
                let q = LineQuery {
                    line: 0,
                    block_offset: Length(y),
                    line_height: Length(lh),
                    previous: &[],
                };
                let _ = huge.available(&q);
                let _ = slant.available(&q);
                let _ = huge.covered(Length(y), Length(lh));
            }
        }
    }

    #[test]
    fn ellipses_are_symmetric_integer_polygons() {
        let c = Polygon::circle(p(100, 100), pt(50), 8);
        assert_eq!(c.points.len(), 32);
        assert_eq!(c.top(), pt(50));
        assert_eq!(c.bottom(), pt(150));
        // Every vertex is on the circle to within rounding.
        let r = pt(50).0 as i64;
        for &(x, y) in &c.points {
            let (dx, dy) = (x - pt(100).0 as i64, y - pt(100).0 as i64);
            let d2 = dx * dx + dy * dy;
            assert!((d2 - r * r).abs() <= 2 * r, "({dx}, {dy}) off the circle");
        }
        // The middle band is wide and symmetric about the centre.
        let Available::Room(mid) = ask(&c, 95, 10) else {
            panic!("room expected")
        };
        assert_eq!(mid.len(), 1);
        // Point symmetry about the centre, to within rounding.
        let skew = (pt(100) - mid[0].start) - (mid[0].end - pt(100));
        assert!(skew.abs() <= Length(1), "{mid:?}");
        assert!(mid[0].width() > pt(95));
        let e = Polygon::ellipse(p(0, 0), pt(-40), pt(20), 0);
        assert_eq!(e.points.len(), 4, "steps clamp to 1");
        assert_eq!((e.top(), e.bottom()), (pt(-20), pt(20)));
    }

    #[test]
    fn runarounds_cut_exclusions_with_a_margin_and_drop_slivers() {
        let figure = Polygon::rect(Rect::new(p(40, 20), pt(20), pt(20)));
        let wrap = Runaround {
            base: crate::Measure(pt(100)),
            exclusions: vec![figure],
            margin: pt(5),
            min_width: pt(10),
        };
        // Above the figure's margin: the whole measure.
        assert_eq!(ask(&wrap, 0, 10), room(&[(0, 100)]));
        // The band 10..20 reaches the margin above the figure (15..20).
        assert_eq!(ask(&wrap, 10, 10), room(&[(0, 35), (65, 100)]));
        // A line exactly touching the margin's edge isn't blocked.
        assert_eq!(ask(&wrap, 5, 10), room(&[(0, 100)]));
        assert_eq!(ask(&wrap, 45, 10), room(&[(0, 100)]));
        // A wider margin leaves slivers, which are dropped.
        let tight = Runaround {
            margin: pt(33),
            ..wrap
        };
        assert_eq!(ask(&tight, 20, 10), room(&[]));
    }

    #[test]
    fn runarounds_follow_slanted_exclusions_over_the_whole_band() {
        // A triangle pointing right from x = 0: at y its right edge is at
        // 50 − |y − 50|.
        let wedge = Polygon::new([p(0, 0), p(50, 50), p(0, 100)]);
        let wrap = Runaround {
            base: crate::Measure(pt(100)),
            exclusions: vec![wedge],
            margin: Length::ZERO,
            min_width: Length::ZERO,
        };
        // Over 40..50 the wedge reaches as far as x = 50 (at the band's bottom).
        assert_eq!(ask(&wrap, 40, 10), room(&[(50, 100)]));
        // Passing through: skips and ends of the base go through unchanged.
        let in_shape = Runaround {
            base: Polygon::rect(Rect::new(p(0, 10), pt(100), pt(10))),
            exclusions: Vec::new(),
            margin: pt(-5),
            min_width: pt(-5),
        };
        assert_eq!(ask(&in_shape, 0, 10), Available::Skip { next: pt(10) });
        assert_eq!(ask(&in_shape, 10, 10), room(&[(0, 100)]));
        assert_eq!(ask(&in_shape, 11, 10), Available::End);
    }
}
