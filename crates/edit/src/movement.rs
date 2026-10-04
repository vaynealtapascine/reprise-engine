//! Caret movement (30): logical and visual, by grapheme, word, line, block
//! and document, across blocks, lines and frames.
//!
//! **Logical** movement follows the text: next and previous grapheme or word
//! by byte offset. **Visual** movement follows the screen: right is right,
//! and a step crosses one grapheme cell of the line's runs in the order they
//! are drawn, so it passes through right-to-left runs and across bidi
//! boundaries correctly. At a line's visual end it continues on the next line
//! in reading direction.
//!
//! Between blocks, movement follows the [reading order](crate::Navigator)
//! (33), never the geometry. Between the lines of one block it follows the
//! block's lines in logical order, so it crosses from a frame into the next
//! frame of the thread.

use reprise_doc::NodeId;
use reprise_layout::LineRef;

use crate::caret::{Caret, Cursor, Movement};
use crate::navigator::Navigator;

impl Navigator<'_> {
    /// Moves a caret. Where there is nowhere to go (the start or end of the
    /// document) it comes back unchanged. `None` only if `from` isn't a place
    /// in this snapshot.
    pub fn move_caret(&self, from: Caret, movement: Movement) -> Option<Caret> {
        self.move_cursor(&Cursor::from(from), movement)
            .map(|c| c.caret)
    }

    /// Moves a cursor, which also remembers the goal x that vertical movement
    /// keeps across lines of different lengths.
    pub fn move_cursor(&self, from: &Cursor, movement: Movement) -> Option<Cursor> {
        let caret = from.caret;
        // Fails on a caret that isn't a place here.
        let (line, model, stop) = self.locate(&caret)?;
        let node = caret.node;
        let info = self.info(node)?;
        let len = info.boundaries.last().copied().unwrap_or(0);
        let moved = match movement {
            Movement::NextGrapheme => match info.next_boundary(caret.offset) {
                Some(o) => self.place(node, o),
                None => self.node_after(node).and_then(|n| self.place(n, 0)),
            },
            Movement::PreviousGrapheme => match info.prev_boundary(caret.offset) {
                Some(o) => self.place(node, o),
                None => self
                    .node_before(node)
                    .and_then(|n| self.place(n, self.len_of(n))),
            },
            Movement::NextWord => {
                if caret.offset >= len {
                    self.node_after(node).and_then(|n| self.place(n, 0))
                } else {
                    let to = info
                        .words
                        .iter()
                        .find(|w| w.end > caret.offset)
                        .map_or(len, |w| w.end);
                    self.place(node, to)
                }
            }
            Movement::PreviousWord => {
                if caret.offset == 0 {
                    self.node_before(node)
                        .and_then(|n| self.place(n, self.len_of(n)))
                } else {
                    let to = info
                        .words
                        .iter()
                        .rev()
                        .find(|w| w.start < caret.offset)
                        .map_or(0, |w| w.start);
                    self.place(node, to)
                }
            }
            Movement::VisualRight | Movement::VisualLeft => {
                self.visual(caret, line, &model, stop, movement == Movement::VisualRight)
            }
            Movement::LineUp | Movement::LineDown => {
                let goal = from.goal_x.unwrap_or(stop.x);
                let up = movement == Movement::LineUp;
                let target = if up {
                    self.line_before(node, line)
                } else {
                    self.line_after(node, line)
                };
                let caret = match target {
                    Some(t) => self.caret_at_x(t, goal),
                    // The edge of the document: go to the end of the line.
                    None if up => self.place(node, model.start),
                    None => self.place(node, model.end_limit),
                };
                return caret.map(|caret| Cursor {
                    caret,
                    goal_x: Some(goal),
                });
            }
            Movement::LineStart => self.place(node, model.start),
            Movement::LineEnd => self.place_upstream(node, model.end_limit),
            Movement::LineLeftmost => Some(self.caret_of(node, &model.arrive(0, None))),
            Movement::LineRightmost => {
                Some(self.caret_of(node, &model.arrive(model.cells.len(), None)))
            }
            Movement::BlockStart => self.place(node, 0),
            Movement::BlockEnd => self.place(node, len),
            Movement::DocumentStart => self.order.first().and_then(|&n| self.place(n, 0)),
            Movement::DocumentEnd => self
                .order
                .last()
                .and_then(|&n| self.place(n, self.len_of(n))),
        };
        Some(Cursor {
            caret: moved.unwrap_or(caret),
            goal_x: None,
        })
    }

    /// One step right or left on the screen.
    fn visual(
        &self,
        from: Caret,
        line: usize,
        model: &crate::model::LineModel,
        stop: crate::model::Stop,
        right: bool,
    ) -> Option<Caret> {
        let node = from.node;
        let n = model.cells.len();
        let j = stop.junction;
        if right && j < n {
            // Cross cell `j`, landing at its right edge.
            let crossed = model.stops[2 * j + 1];
            return Some(self.caret_of(node, &model.arrive(j + 1, Some(crossed))));
        }
        if !right && j > 0 {
            let crossed = model.stops[2 * (j - 1)];
            return Some(self.caret_of(node, &model.arrive(j - 1, Some(crossed))));
        }
        // The end of the line. Right is the way text reads in a left-to-right
        // paragraph and against it in a right-to-left one.
        let reading_forward = right != self.info(node)?.rtl;
        let target = if reading_forward {
            self.line_after(node, line)
        } else {
            self.line_before(node, line)
        };
        let Some(target) = target else {
            return Some(from);
        };
        let (tm, _) = self.line_model(target.node, target.line)?;
        let stop = if right {
            tm.arrive(0, None)
        } else {
            tm.arrive(tm.cells.len(), None)
        };
        Some(self.caret_of(target.node, &stop))
    }

    /// The caret at `offset`, attached to the character after it (or before
    /// it at the end of the text), normalized.
    pub(crate) fn place(&self, node: NodeId, offset: usize) -> Option<Caret> {
        let caret = if offset >= self.len_of(node) {
            Caret::upstream(node, offset)
        } else {
            Caret::new(node, offset)
        };
        self.normalize(caret)
    }

    pub(crate) fn place_upstream(&self, node: NodeId, offset: usize) -> Option<Caret> {
        self.normalize(Caret::upstream(node, offset))
    }

    pub(crate) fn len_of(&self, node: NodeId) -> usize {
        self.info(node)
            .and_then(|i| i.boundaries.last().copied())
            .unwrap_or(0)
    }

    /// The block after `node` in reading order.
    pub(crate) fn node_after(&self, node: NodeId) -> Option<NodeId> {
        self.order.get(self.rank.get(&node)? + 1).copied()
    }

    pub(crate) fn node_before(&self, node: NodeId) -> Option<NodeId> {
        self.order
            .get(self.rank.get(&node)?.checked_sub(1)?)
            .copied()
    }

    /// The line after `line` of `node`: the block's next line, else the first
    /// line of the next block in reading order.
    pub fn line_after(&self, node: NodeId, line: usize) -> Option<LineRef> {
        let block = self.block(node)?;
        if line + 1 < block.lines.len() {
            return Some(LineRef {
                node,
                line: line + 1,
            });
        }
        self.node_after(node).map(|n| LineRef { node: n, line: 0 })
    }

    pub fn line_before(&self, node: NodeId, line: usize) -> Option<LineRef> {
        if let Some(l) = line.checked_sub(1) {
            return Some(LineRef { node, line: l });
        }
        let before = self.node_before(node)?;
        let last = self.block(before)?.lines.len().checked_sub(1)?;
        Some(LineRef {
            node: before,
            line: last,
        })
    }
}
