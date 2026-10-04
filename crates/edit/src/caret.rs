//! Carets, selections and the plain data the navigator returns (30).

use std::ops::Range;

use reprise_doc::NodeId;
use reprise_geom::{Length, PageSpace, Point, Rect};
use reprise_layout::LineRef;

/// Which side of a position a caret is attached to, when one offset can be
/// drawn in two places: at a line break (the end of one line and the start of
/// the next) and at a bidi boundary (where two runs meet).
///
/// This is not `reprise_text::Affinity`, which says what an *anchor* does when
/// text is inserted at it. A caret's affinity is about where it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Affinity {
    /// Attached to the character before the offset: drawn at that character's
    /// trailing edge, which at a soft line break is the end of the line.
    Upstream,
    /// Attached to the character after the offset: drawn at that character's
    /// leading edge, which at a soft line break is the start of the next line.
    Downstream,
}

/// A caret: a block, a byte offset into its text, and an affinity.
///
/// The offset is a UTF-8 byte offset on a grapheme boundary. Carets are
/// plain values; two carets at the same offset that draw in the same place
/// compare unequal if their affinities differ, so compare
/// [`crate::Navigator::normalize`]d carets when that matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Caret {
    pub node: NodeId,
    pub offset: usize,
    pub affinity: Affinity,
}

impl Caret {
    /// A caret attached to the character after `offset`.
    pub fn new(node: NodeId, offset: usize) -> Caret {
        Caret {
            node,
            offset,
            affinity: Affinity::Downstream,
        }
    }

    /// A caret attached to the character before `offset`.
    pub fn upstream(node: NodeId, offset: usize) -> Caret {
        Caret {
            node,
            offset,
            affinity: Affinity::Upstream,
        }
    }
}

/// An anchor and a focus. The focus is the end that moves when the selection
/// is extended; either may come first in the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: Caret,
    pub focus: Caret,
}

impl Selection {
    pub fn collapsed(caret: Caret) -> Selection {
        Selection {
            anchor: caret,
            focus: caret,
        }
    }

    pub fn is_collapsed(&self) -> bool {
        self.anchor.node == self.focus.node && self.anchor.offset == self.focus.offset
    }
}

/// Where a caret is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaretRect {
    /// The page it is on.
    pub page: usize,
    /// The caret as a segment across its line, in page space: its extent
    /// along the line's block axis, and no width along the inline axis (for a
    /// frame turned a quarter turn, the rectangle is wide and flat).
    pub rect: Rect<PageSpace>,
    /// The line it sits on.
    pub line: LineRef,
    /// Where it is on the line's inline axis, in the frame's space.
    pub x: Length,
    /// The bidi level of the text it sits in. A UI that draws split carets
    /// can use the parity to flag the direction.
    pub level: u8,
}

impl CaretRect {
    /// A page point on the caret. Hit testing returns a caret at the same
    /// visual position; coincident caret identities cannot be distinguished.
    pub fn point(&self) -> Point<PageSpace> {
        let r = &self.rect;
        Point::new(
            r.origin.x + r.width.mul_ratio(1, 2),
            r.origin.y + r.height.mul_ratio(1, 2),
        )
    }
}

/// The result of hit testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub caret: Caret,
    pub line: LineRef,
    pub page: usize,
    /// Whether the point was inside the line's box, as opposed to between
    /// lines, outside the frame, or on a page without text, where the nearest
    /// line was used.
    pub inside: bool,
}

/// One grapheme as laid out on a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphemeCell {
    /// Its bytes in the block's text.
    pub range: Range<usize>,
    /// Its extent along the line's inline axis, in the frame's space, with
    /// `x0 <= x1`. A grapheme in a ligature gets an equal share of the
    /// ligature's width; one with no glyph has no width.
    pub x0: Length,
    pub x1: Length,
    /// The bidi level of the run it is in. Odd means right to left.
    pub level: u8,
}

/// A rectangle of a selection on a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRect {
    pub page: usize,
    pub rect: Rect<PageSpace>,
    pub line: LineRef,
}

/// One block's part of a selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRange {
    pub node: NodeId,
    pub bytes: Range<usize>,
}

/// A way to move a caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Movement {
    /// To the next grapheme boundary in logical order.
    NextGrapheme,
    PreviousGrapheme,
    /// To the end of the next word, or the block's end if there is none.
    NextWord,
    /// To the start of the previous word, or the block's start.
    PreviousWord,
    /// One step to the right on the page: a grapheme in visual order along
    /// the inline axis, or a line step for a quarter-turned frame. General
    /// transforms use the dominant inverse-mapped axis (inline on ties).
    VisualRight,
    VisualLeft,
    /// One visual grapheme cell toward increasing/decreasing frame inline x.
    /// Use these to traverse a turned line or follow successive path strips,
    /// independently of the direction of page-horizontal arrows.
    InlineForward,
    InlineBackward,
    /// To the previous line, keeping the goal x. On the first line of the
    /// document, to the start of the line.
    LineUp,
    /// To the next line, keeping the goal x. On the last line of the
    /// document, to the end of the line.
    LineDown,
    /// To the logical start of the line: the left edge of a left-to-right
    /// line, the right edge of a right-to-left one.
    LineStart,
    /// To the logical end of the line, before a hard line break.
    LineEnd,
    /// To the page-left or page-right end of the line. If both have the same
    /// page x (a quarter turn), use the frame's first/last inline edge.
    LineLeftmost,
    LineRightmost,
    /// The first/last visual junction along increasing frame inline x,
    /// irrespective of its page orientation or paragraph reading direction.
    LineInlineStart,
    LineInlineEnd,
    BlockStart,
    BlockEnd,
    DocumentStart,
    DocumentEnd,
}

/// A caret with the horizontal position that vertical movement keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub caret: Caret,
    /// Set by [`Movement::LineUp`] and [`Movement::LineDown`], in the frame
    /// space of the line the movement started on, and cleared by any other
    /// movement.
    pub goal_x: Option<Length>,
}

impl From<Caret> for Cursor {
    fn from(caret: Caret) -> Cursor {
        Cursor {
            caret,
            goal_x: None,
        }
    }
}
