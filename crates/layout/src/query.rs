//! Layout queries over a snapshot (13, 16).
//!
//! The queries the contract lists (`line_containing`, `previous_line`,
//! `next_line`, `lines_in`) live in `snapshot.rs`. These are the ones that
//! relations added, and [`LayoutSnapshot::answer`], which turns a
//! [`LayoutQuery`] into a [`Resolution`].
//!
//! Every query is a pure function of the snapshot. Each says what zero and
//! several matches mean in its documentation: zero is `None` or an empty
//! list, never a panic.

use std::ops::Range;

use reprise_doc::{LayoutQuery, NodeId};

use crate::{LayoutSnapshot, LineRef, Resolution};

/// Where a layout query starts, once the document has resolved its anchor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutAt {
    /// A range, as it is now: its block and bytes.
    Range { node: NodeId, bytes: Range<usize> },
    /// A block.
    Node(NodeId),
}

impl LayoutSnapshot {
    /// The first line of a block. `None` if the block wasn't laid out.
    pub fn first_line(&self, node: NodeId) -> Option<LineRef> {
        let block = self.block(node)?;
        (!block.lines.is_empty()).then_some(LineRef { node, line: 0 })
    }

    /// The last line of a block. `None` if the block wasn't laid out.
    pub fn last_line(&self, node: NodeId) -> Option<LineRef> {
        let line = self.block(node)?.lines.len().checked_sub(1)?;
        Some(LineRef { node, line })
    }

    /// The frame holding a line, as an index into `frames`.
    pub fn frame_of(&self, at: LineRef) -> Option<usize> {
        let frame = self.line(at)?.frame;
        self.frame(frame).map(|_| frame)
    }

    /// The page holding a line, as an index into `pages`.
    pub fn page_of(&self, at: LineRef) -> Option<usize> {
        let page = self.frame(self.line(at)?.frame)?.page;
        self.pages.get(page).map(|_| page)
    }

    /// The line before the one containing byte `at` of `node`, in the same
    /// block. `None` on the first line, or if no line contains `at`.
    pub fn line_before(&self, node: NodeId, at: usize) -> Option<LineRef> {
        self.previous_line(self.line_containing(node, at)?)
    }

    /// The line after the one containing byte `at` of `node`, in the same
    /// block. `None` on the last line, or if no line contains `at`.
    pub fn line_after(&self, node: NodeId, at: usize) -> Option<LineRef> {
        self.next_line(self.line_containing(node, at)?)
    }

    /// Answers a layout query from a resolved anchor. `None` is zero matches:
    /// the block wasn't laid out, or the query reaches past the first or last
    /// line, or `at` isn't the kind of anchor the query takes. A list answer
    /// is never empty. Which answer a query gives depends only on the
    /// query: [`LayoutQuery::LinesIn`] always answers with
    /// [`Resolution::Lines`], and the rest with a single resolution.
    pub fn answer(&self, query: &LayoutQuery, at: &LayoutAt) -> Option<Resolution> {
        match (query, at) {
            (LayoutQuery::FirstLine { .. }, LayoutAt::Node(node)) => {
                self.first_line(*node).map(Resolution::Line)
            }
            (LayoutQuery::LastLine { .. }, LayoutAt::Node(node)) => {
                self.last_line(*node).map(Resolution::Line)
            }
            (_, LayoutAt::Range { node, bytes }) => {
                let start = bytes.start;
                match query {
                    LayoutQuery::LineContaining { .. } => {
                        self.line_containing(*node, start).map(Resolution::Line)
                    }
                    LayoutQuery::PreviousLine { .. } => {
                        self.line_before(*node, start).map(Resolution::Line)
                    }
                    LayoutQuery::NextLine { .. } => {
                        self.line_after(*node, start).map(Resolution::Line)
                    }
                    LayoutQuery::LinesIn { .. } => {
                        let lines = self.lines_in(*node, bytes.clone());
                        (!lines.is_empty()).then_some(Resolution::Lines(lines))
                    }
                    LayoutQuery::FrameContaining { .. } => self
                        .line_containing(*node, start)
                        .and_then(|l| self.frame_of(l))
                        .map(Resolution::Frame),
                    LayoutQuery::PageContaining { .. } => self
                        .line_containing(*node, start)
                        .and_then(|l| self.page_of(l))
                        .map(Resolution::Page),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
