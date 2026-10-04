//! The room a frame offers a block (23, 24).

use reprise_compose::{Available, GeometryProvider, LineQuery};
use reprise_geom::Length;

/// Bounds any geometry provider to a frame's depth: once the next line would
/// reach past the bottom, the region has ended and the block continues in the
/// next frame (`Available::End`).
///
/// This is where a frame can carry a non-rectangular provider: it wraps
/// whatever provider describes the frame's shape (`Measure` for a plain
/// column, a polygon or runaround later) and only adds the bottom edge.
pub(crate) struct Bounded<'a> {
    pub inner: &'a dyn GeometryProvider,
    pub depth: Length,
}

impl GeometryProvider for Bounded<'_> {
    fn available(&self, query: &LineQuery<'_>) -> Available {
        // Compared as a difference so a line near `Length::MAX` can't wrap.
        if query.block_offset > self.depth || self.depth - query.block_offset < query.line_height {
            return Available::End;
        }
        match self.inner.available(query) {
            Available::Skip { next } if next > self.depth => Available::End,
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use reprise_compose::Measure;

    use super::*;

    fn ask(depth: i32, offset: i32, height: i32) -> Available {
        let measure = Measure(Length::from_pt(100));
        Bounded {
            inner: &measure,
            depth: Length::from_pt(depth),
        }
        .available(&LineQuery {
            line: 0,
            block_offset: Length::from_pt(offset),
            line_height: Length::from_pt(height),
            previous: &[],
        })
    }

    #[test]
    fn a_line_must_fit_entirely() {
        assert!(
            matches!(ask(100, 88, 12), Available::Room(_)),
            "exactly fits"
        );
        assert_eq!(ask(100, 89, 12), Available::End);
        assert_eq!(ask(100, 101, 0), Available::End, "already past the bottom");
        assert!(matches!(ask(100, 100, 0), Available::Room(_)));
    }

    #[test]
    fn extreme_offsets_do_not_wrap() {
        let measure = Measure(Length::from_pt(1));
        let frame = Bounded {
            inner: &measure,
            depth: Length::MAX,
        };
        let q = |offset, height| LineQuery {
            line: 0,
            block_offset: offset,
            line_height: height,
            previous: &[],
        };
        assert_eq!(
            frame.available(&q(Length::MAX, Length::from_pt(1))),
            Available::End,
            "saturated offsets still end the region"
        );
        assert!(matches!(
            frame.available(&q(Length::ZERO, Length::MAX)),
            Available::Room(_)
        ));
        assert!(matches!(
            frame.available(&q(Length::MIN, Length::from_pt(1))),
            Available::Room(_)
        ));
    }

    #[test]
    fn a_skip_past_the_bottom_ends_the_region() {
        struct Skipper;
        impl GeometryProvider for Skipper {
            fn available(&self, q: &LineQuery<'_>) -> Available {
                Available::Skip {
                    next: q.block_offset + Length::from_pt(50),
                }
            }
        }
        let frame = Bounded {
            inner: &Skipper,
            depth: Length::from_pt(100),
        };
        let q = |offset| LineQuery {
            line: 0,
            block_offset: Length::from_pt(offset),
            line_height: Length::from_pt(10),
            previous: &[],
        };
        assert!(matches!(frame.available(&q(0)), Available::Skip { .. }));
        assert_eq!(frame.available(&q(60)), Available::End);
    }
}
