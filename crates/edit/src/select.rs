//! Selections (30) and the operations gestures are made of (31).
//!
//! Gesture *policy* (what a double click or a drag means in a mode) is up to
//! the UI. These are the operations it needs: select the word, line or block
//! at a caret, select everything, find a selection's per-block ranges and the
//! page rectangles that cover it.

use std::ops::Range;

use reprise_geom::{Length, Point, Rect};
use reprise_layout::LineRef;

use crate::caret::{BlockRange, Caret, Selection, SelectionRect};
use crate::navigator::Navigator;

impl Navigator<'_> {
    /// The word at `caret`, as a selection. A caret just after a word
    /// selects that word. A caret between words selects the gap between them
    /// (the spaces and punctuation), so double-clicking whitespace selects it.
    pub fn select_word(&self, caret: Caret) -> Option<Selection> {
        let node = caret.node;
        let info = self.info(node)?;
        self.normalize(caret)?;
        let o = caret.offset;
        let len = self.len_of(node);
        let range = match info.words.iter().find(|w| w.start <= o && o <= w.end) {
            Some(w) => w.clone(),
            None => {
                let from = info
                    .words
                    .iter()
                    .rev()
                    .find(|w| w.end <= o)
                    .map_or(0, |w| w.end);
                let to = info
                    .words
                    .iter()
                    .find(|w| w.start >= o)
                    .map_or(len, |w| w.start);
                from..to
            }
        };
        self.selection_over(node, range)
    }

    /// The line `caret` is on, as a selection, without a hard line break that
    /// ends it.
    pub fn select_line(&self, caret: Caret) -> Option<Selection> {
        let (_, model, _) = self.locate(&caret)?;
        self.selection_over(caret.node, model.start..model.end_limit)
    }

    /// The whole block `caret` is in.
    pub fn select_block(&self, caret: Caret) -> Option<Selection> {
        self.normalize(caret)?;
        self.selection_over(caret.node, 0..self.len_of(caret.node))
    }

    /// From the start of the first block to the end of the last, in reading
    /// order. `None` if nothing is laid out.
    pub fn select_all(&self) -> Option<Selection> {
        let first = *self.order.first()?;
        let last = *self.order.last()?;
        Some(Selection {
            anchor: self.place(first, 0)?,
            focus: self.place(last, self.len_of(last))?,
        })
    }

    fn selection_over(&self, node: reprise_doc::NodeId, bytes: Range<usize>) -> Option<Selection> {
        Some(Selection {
            anchor: self.place(node, bytes.start)?,
            focus: self.place_upstream(node, bytes.end)?,
        })
    }

    /// A selection's bytes, block by block, in logical order: the part of the
    /// first block after its start, every block between in full, and the part
    /// of the last before its end, whichever way the selection was made. A
    /// collapsed selection gives one empty range. A caret that isn't in the
    /// snapshot gives nothing.
    pub fn selection_ranges(&self, selection: &Selection) -> Vec<BlockRange> {
        if self.locate(&selection.anchor).is_none() || self.locate(&selection.focus).is_none() {
            return Vec::new();
        }
        let key = |c: &Caret| self.rank.get(&c.node).map(|&r| (r, c.offset));
        let (Some(a), Some(b)) = (key(&selection.anchor), key(&selection.focus)) else {
            return Vec::new();
        };
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        if start.0 == end.0 {
            return self
                .order
                .get(start.0)
                .map(|&node| BlockRange {
                    node,
                    bytes: start.1..end.1,
                })
                .into_iter()
                .collect();
        }
        (start.0..=end.0)
            .filter_map(|r| self.order.get(r).copied().map(|n| (r, n)))
            .map(|(r, node)| BlockRange {
                node,
                bytes: if r == start.0 {
                    start.1..self.len_of(node)
                } else if r == end.0 {
                    0..end.1
                } else {
                    0..self.len_of(node)
                },
            })
            .collect()
    }

    /// The page rectangles covering a selection: one for each stretch of
    /// each line that is selected and drawn contiguously. A line of mixed
    /// direction can give several for one logical range, since the selected
    /// text is not always side by side on screen. In logical order of blocks
    /// and lines, then left to right.
    ///
    /// A hard line break and a selected empty line have no rectangle.
    pub fn selection_rects(&self, selection: &Selection) -> Vec<SelectionRect> {
        let mut out = Vec::new();
        for range in self.selection_ranges(selection) {
            if range.bytes.is_empty() {
                continue;
            }
            let Some(block) = self.block(range.node) else {
                continue;
            };
            for line in 0..block.lines.len() {
                self.rects_on_line(range.node, line, &range.bytes, &mut out);
            }
        }
        out
    }

    fn rects_on_line(
        &self,
        node: reprise_doc::NodeId,
        line: usize,
        bytes: &Range<usize>,
        out: &mut Vec<SelectionRect>,
    ) {
        let Some((model, layout)) = self.line_model(node, line) else {
            return;
        };
        let Some(frame) = self.snapshot.frame(layout.frame) else {
            return;
        };
        // Stretches of selected cells that touch, in visual order.
        let mut stretches: Vec<(Length, Length)> = Vec::new();
        for cell in &model.cells {
            let selected = cell.range.start < bytes.end && bytes.start < cell.range.end;
            if !selected || cell.x0 == cell.x1 {
                continue;
            }
            match stretches.last_mut() {
                Some(last) if cell.x0 <= last.1 && cell.x1 >= last.0 => {
                    last.0 = last.0.min(cell.x0);
                    last.1 = last.1.max(cell.x1);
                }
                _ => stretches.push((cell.x0, cell.x1)),
            }
        }
        for (x0, x1) in stretches {
            let r = Rect::new(
                Point::new(x0, layout.rect.origin.y),
                x1 - x0,
                layout.rect.height,
            );
            out.push(SelectionRect {
                page: frame.page,
                rect: frame.to_page.bounds(&r),
                line: LineRef { node, line },
            });
        }
    }
}
