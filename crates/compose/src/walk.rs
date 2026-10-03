//! Walking a geometry provider down the block axis: skips, the end of the
//! region, and stalls. Every composer here walks the same way, so they agree
//! on where lines go and on when composition stops.

use reprise_diag::Note;
use reprise_geom::Length;

use crate::greedy::MAX_CONSECUTIVE_SKIPS;
use crate::{
    Available, ComposeRequest, GeometryProvider, Interval, LineFragment, LineQuery, codes,
};

/// Where the walk is: the next line's index and top, and how many times in a
/// row geometry has skipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Walk {
    pub line: u32,
    pub y: Length,
    pub skips: u32,
}

/// What one answer from the provider means for the walk.
pub(crate) enum Step {
    /// A line with room in these (non-empty) intervals.
    Room(Vec<Interval>),
    /// The region ended.
    End,
    /// Geometry stopped moving down; composition must stop.
    Stalled,
    /// Moved down; ask again.
    Moved,
}

impl Walk {
    pub fn new(block_start: Length) -> Walk {
        Walk {
            line: 0,
            y: block_start,
            skips: 0,
        }
    }

    pub fn query(
        &self,
        geometry: &dyn GeometryProvider,
        line_height: Length,
        previous: &[LineFragment],
    ) -> Available {
        geometry.available(&LineQuery {
            line: self.line,
            block_offset: self.y,
            line_height,
            previous,
        })
    }

    /// Applies one answer. An empty `Room` skips one line height. A skip that
    /// doesn't move down, or more than [`MAX_CONSECUTIVE_SKIPS`] in a row,
    /// stalls.
    pub fn apply(&mut self, answer: Available, line_height: Length) -> Step {
        match answer {
            Available::End => Step::End,
            Available::Room(intervals) if !intervals.is_empty() => {
                self.skips = 0;
                Step::Room(intervals)
            }
            skip => {
                let next = match skip {
                    Available::Skip { next } => next,
                    _ => self.y + line_height,
                };
                self.skips = self.skips.saturating_add(1);
                if next <= self.y || self.skips > MAX_CONSECUTIVE_SKIPS {
                    return Step::Stalled;
                }
                self.y = next;
                Step::Moved
            }
        }
    }

    /// Moves past a line that was placed.
    pub fn advance(&mut self, line_height: Length) {
        self.y += line_height;
        self.line = self.line.saturating_add(1);
    }

    /// Asks until geometry gives a line, ends or stalls.
    pub fn next_line(&mut self, request: &ComposeRequest<'_>, previous: &[LineFragment]) -> Step {
        loop {
            let answer = self.query(request.geometry, request.line_height, previous);
            match self.apply(answer, request.line_height) {
                Step::Moved => continue,
                step => return step,
            }
        }
    }
}

/// The note for a stall at `walk`, with `pos..len` left uncomposed.
pub(crate) fn stalled(walk: &Walk, pos: usize, len: usize) -> Note {
    let y = walk.y;
    Note::error(
        codes::GEOMETRY_STALLED,
        format!(
            "geometry made no progress at block offset {y:?}; \
             bytes {pos}..{len} not composed"
        ),
    )
    .at(pos..len)
}
