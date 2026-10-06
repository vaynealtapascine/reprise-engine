//! Row-group fragmentation and repeated header rows (24, 26, 33, 37).
//!
//! A table is a sequence of row groups: rows linked by row-spanning cells form
//! one group, every other row is a group of its own. A group is composed one
//! frame fragment at a time by [`Flow::compose_fragment`], which is pure with
//! respect to the snapshot: it only returns lines, so the same code answers
//! "would this group fit here?" and "place this group here".
//!
//! Within a fragment rows are stacked top to bottom; each cell composes from
//! its own block/byte cursor. A cell that spans rows composes once at the first
//! row of the fragment it covers, and its bottom extends the last row it spans.
//! A fragment ends when any cell that must finish in the current row runs out
//! of frame; unfinished cells continue together at the top of the next frame,
//! exactly like single-row fragmentation.
//!
//! A multi-row group is kept together when it cannot fit the rest of this frame
//! but would fit a fresh one. When it cannot fit a fresh frame it fragments and
//! reports `layout.table-rowspan-split`.
//!
//! Repeating headers are derived: after the table continues on a new frame the
//! leading header groups are composed again at its top and recorded in
//! `LayoutSnapshot::repeated_headers`, never in `blocks` and never in the
//! document (05). A copy is skipped, with `layout.table-header-unrepeated`,
//! when it does not fit or would leave the body no room.
use crate::flow::{Flow, Prepared, unplaced};
use crate::region::Bounded;
use crate::{BlockLayout, Diagnostic, LineLayout, Subject, codes};
use reprise_compose::Measure;
use reprise_diag::Severity;
use reprise_doc::NodeId;
use reprise_geom::Length;

#[derive(Clone)]
pub(crate) struct Cell {
    pub node: NodeId,
    /// First and last row the cell covers, relative to its group.
    pub row: usize,
    pub last: usize,
    pub column: usize,
    pub colspan: usize,
    pub blocks: Vec<Prepared>,
}

#[derive(Clone)]
pub(crate) struct Group {
    pub rows: usize,
    pub header: bool,
    pub cells: Vec<Cell>,
}

impl Group {
    fn spans_rows(&self) -> bool {
        self.cells.iter().any(|c| c.last > c.row)
    }
}

/// Where a group's composition has reached.
#[derive(Clone, PartialEq, Eq)]
struct State {
    /// Per cell: block index and byte offset.
    cursors: Vec<(usize, usize)>,
    /// The first row with a cell that must still finish.
    row: usize,
}

impl State {
    fn start(group: &Group) -> State {
        State {
            cursors: vec![(0, 0); group.cells.len()],
            row: 0,
        }
    }
}

struct Fragment {
    /// Cell, block and the lines appended to it.
    lines: Vec<(usize, usize, Vec<LineLayout>)>,
    bottom: Length,
    placed: bool,
    done: bool,
    diagnostics: Vec<Diagnostic>,
}

struct CellOut {
    lines: Vec<(usize, Vec<LineLayout>)>,
    bottom: Length,
    cursor: (usize, usize),
}

/// A derived copy of the header rows, ready to commit.
struct Copy {
    blocks: Vec<BlockLayout>,
    bottom: Length,
}

/// What repeating headers need to remember across a table.
pub(crate) struct Repeat {
    /// Leading groups that are header groups.
    pub groups: usize,
    /// Every header group was placed together in one frame.
    placed: bool,
    /// Headers can still repeat.
    enabled: bool,
    /// The last advance was inside the table: the next fragment needs a copy.
    pending: bool,
    warned: bool,
    /// Height the header took where it was first placed.
    extent: Length,
    header_top: Option<Length>,
    /// Whether the current frame already has table body content.
    body: bool,
}

impl Repeat {
    pub(crate) fn new(groups: usize, body: bool) -> Repeat {
        Repeat {
            groups,
            placed: false,
            enabled: groups > 0,
            pending: false,
            warned: false,
            extent: Length::ZERO,
            header_top: None,
            body,
        }
    }
}

fn span_sum(widths: &[Length], column: usize, span: usize) -> Length {
    widths.iter().skip(column).take(span).copied().sum()
}

impl Flow<'_> {
    /// Places every group of a table, in order.
    pub(crate) fn place_groups(
        &mut self,
        table: NodeId,
        mut groups: Vec<Group>,
        widths: &[Length],
    ) {
        let split = groups.iter().take_while(|g| g.header).count();
        let bodies = groups.split_off(split);
        let headers = groups;
        let mut repeat = Repeat::new(headers.len(), self.used() > Length::ZERO);
        // Header groups are kept for the copies, so they are placed from clones.
        for (index, group) in headers.iter().enumerate() {
            self.place_group(table, group.clone(), index, &headers, widths, &mut repeat);
        }
        for group in bodies {
            self.place_group(table, group, usize::MAX, &headers, widths, &mut repeat);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn place_group(
        &mut self,
        table: NodeId,
        owned: Group,
        index: usize,
        groups: &[Group],
        widths: &[Length],
        repeat: &mut Repeat,
    ) {
        let group = &owned;
        let spacing = self.engine.flow.paragraph_spacing;
        let mut state = State::start(group);
        let mut output: Vec<Vec<Vec<LineLayout>>> = group
            .cells
            .iter()
            .map(|c| c.blocks.iter().map(|_| Vec::new()).collect())
            .collect();
        let mut split_reported = false;
        let mut first_top = None;
        loop {
            let frame_index = self.frame_index();
            let Some(frame) = self.snapshot.frame(frame_index).cloned() else {
                break;
            };
            let depth = self.depth(frame_index, frame.rect.height);
            let plain_top = self.table_top();
            first_top.get_or_insert(plain_top);
            let mut top = plain_top;
            let mut copy = None;
            if repeat.pending && !group.header {
                repeat.pending = false;
                if let Some(c) = self.compose_header(
                    groups,
                    repeat,
                    widths,
                    frame_index,
                    frame.rect.width,
                    plain_top,
                    depth,
                ) {
                    top = c.bottom + spacing;
                    copy = Some(c);
                } else {
                    self.header_unrepeated(table, repeat, "header rows do not fit this frame");
                }
            }
            let (mut next, mut fragment) = self.compose_fragment(
                group,
                widths,
                &state,
                frame_index,
                frame.rect.width,
                top,
                depth,
            );
            if copy.is_some() && !fragment.placed && !fragment.done {
                // The copy would leave the body no room: the body wins.
                copy = None;
                top = plain_top;
                self.header_unrepeated(table, repeat, "a repeated header would leave no room");
                (next, fragment) = self.compose_fragment(
                    group,
                    widths,
                    &state,
                    frame_index,
                    frame.rect.width,
                    top,
                    depth,
                );
            }
            if !fragment.done
                && group.spans_rows()
                && repeat.body
                && state == State::start(group)
                && !split_reported
            {
                let head = if repeat.enabled && repeat.placed && !group.header {
                    repeat.extent + spacing
                } else {
                    Length::ZERO
                };
                let (_, fresh) = self.compose_fragment(
                    group,
                    widths,
                    &state,
                    frame_index,
                    frame.rect.width,
                    head,
                    self.fresh_depth(),
                );
                if fresh.done {
                    // Keep the group together: start it on a fresh frame.
                    if !self.advance() {
                        break;
                    }
                    repeat.body = false;
                    repeat.pending = repeat.enabled && repeat.placed && !group.header;
                    continue;
                }
            }
            if !fragment.done && group.spans_rows() && !split_reported {
                split_reported = true;
                self.snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Warning,
                    codes::TABLE_ROWSPAN_SPLIT,
                    Subject::Node(
                        group
                            .cells
                            .iter()
                            .find(|c| c.last > c.row)
                            .map_or(table, |c| c.node),
                    ),
                    "rows joined by a row span do not fit one frame; the group continues on the next",
                ));
            }
            self.snapshot.diagnostics.append(&mut fragment.diagnostics);
            if let Some(c) = copy {
                self.commit_copy(table, frame_index, c, spacing);
            }
            for (cell, block, lines) in fragment.lines {
                if let Some(out) = output.get_mut(cell).and_then(|o| o.get_mut(block)) {
                    out.extend(lines);
                }
            }
            state = next;
            if fragment.bottom > top {
                self.set_used(fragment.bottom + spacing);
            }
            if fragment.placed {
                repeat.body = true;
            }
            if fragment.done {
                if group.header && repeat.header_top.is_none() {
                    repeat.header_top = first_top;
                }
                if group.header && index + 1 == repeat.groups {
                    // Record the header's extent for keep-together estimates.
                    if let Some(start) = repeat.header_top {
                        repeat.extent = (fragment.bottom - start).max(Length::ZERO);
                    }
                    repeat.placed = repeat.enabled;
                }
                break;
            }
            if group.header {
                repeat.enabled = false;
                if !repeat.warned {
                    repeat.warned = true;
                    self.snapshot.diagnostics.push(Diagnostic::new(
                        Severity::Warning,
                        codes::TABLE_HEADER_UNREPEATED,
                        Subject::Node(table),
                        "header rows continue across frames; they are not repeated",
                    ));
                }
            }
            if !self.advance() {
                break;
            }
            repeat.body = false;
            repeat.pending = repeat.enabled && repeat.placed && !group.header;
        }
        for (i, cell) in owned.cells.into_iter().enumerate() {
            let cursor = state.cursors.get(i).copied().unwrap_or_default();
            let mut lines = output
                .get_mut(i)
                .map(std::mem::take)
                .unwrap_or_default()
                .into_iter();
            for (j, p) in cell.blocks.into_iter().enumerate() {
                if j >= cursor.0 {
                    unplaced(
                        &mut self.snapshot.diagnostics,
                        &Subject::Node(p.node()),
                        if j == cursor.0 { cursor.1 } else { 0 }..p.text.len(),
                    );
                }
                let placed = lines.next().unwrap_or_default();
                if !placed.is_empty() {
                    self.snapshot.blocks.push(p.into_block(placed));
                }
            }
        }
    }

    fn header_unrepeated(&mut self, table: NodeId, repeat: &mut Repeat, message: &str) {
        if repeat.warned {
            return;
        }
        repeat.warned = true;
        self.snapshot.diagnostics.push(Diagnostic::new(
            Severity::Warning,
            codes::TABLE_HEADER_UNREPEATED,
            Subject::Node(table),
            message,
        ));
    }

    /// The depth of the next frame of the main thread, ignoring reservations
    /// that do not exist yet. Only an estimate for keeping groups together.
    fn fresh_depth(&self) -> Length {
        let thread = self.template.main_thread();
        let here = self.frame_index() % self.template.frames.len().max(1);
        let next = thread
            .iter()
            .position(|&i| i == here)
            .map_or(0, |k| (k + 1) % thread.len().max(1));
        thread
            .get(next)
            .and_then(|&i| self.template.frames.get(i))
            .map_or(Length::ZERO, |f| f.depth)
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_header(
        &self,
        groups: &[Group],
        repeat: &Repeat,
        widths: &[Length],
        frame_index: usize,
        frame_width: Length,
        top: Length,
        depth: Length,
    ) -> Option<Copy> {
        let spacing = self.engine.flow.paragraph_spacing;
        let mut y = top;
        let mut bottom = top;
        let mut blocks = Vec::new();
        for group in groups.iter().take(repeat.groups) {
            let (_, fragment) = self.compose_fragment(
                group,
                widths,
                &State::start(group),
                frame_index,
                frame_width,
                y,
                depth,
            );
            if !fragment.done {
                return None;
            }
            bottom = bottom.max(fragment.bottom);
            y = fragment.bottom + spacing;
            let mut per: Vec<Vec<Vec<LineLayout>>> = group
                .cells
                .iter()
                .map(|c| c.blocks.iter().map(|_| Vec::new()).collect())
                .collect();
            for (cell, block, lines) in fragment.lines {
                if let Some(out) = per.get_mut(cell).and_then(|o| o.get_mut(block)) {
                    out.extend(lines);
                }
            }
            for (cell, lines) in group.cells.iter().zip(per) {
                for (p, placed) in cell.blocks.iter().zip(lines) {
                    if !placed.is_empty() {
                        blocks.push(p.clone().into_block(placed));
                    }
                }
            }
        }
        Some(Copy { blocks, bottom })
    }

    fn commit_copy(&mut self, table: NodeId, frame: usize, copy: Copy, spacing: Length) {
        self.set_used(copy.bottom + spacing);
        self.snapshot.repeated_headers.push(crate::RepeatedHeader {
            table,
            frame,
            blocks: copy.blocks,
        });
    }

    /// Composes one frame's worth of a group from `state`. Pure: the caller
    /// commits the result, or discards it to ask "would this fit?".
    #[allow(clippy::too_many_arguments)]
    fn compose_fragment(
        &self,
        group: &Group,
        widths: &[Length],
        state: &State,
        frame_index: usize,
        frame_width: Length,
        top: Length,
        depth: Length,
    ) -> (State, Fragment) {
        let spacing = self.engine.flow.paragraph_spacing;
        let full_depth = self
            .template
            .frames
            .iter()
            .filter(|f| f.is_main_flow())
            .map(|f| f.depth)
            .max()
            .unwrap_or_default();
        let total: Length = widths.iter().copied().sum();
        let mut next = state.clone();
        let mut fragment = Fragment {
            lines: Vec::new(),
            bottom: top,
            placed: false,
            done: false,
            diagnostics: Vec::new(),
        };
        let mut composed = vec![false; group.cells.len()];
        let mut spanning_bottom = vec![top; group.cells.len()];
        let finished = |cell: &Cell, cursor: (usize, usize)| cursor.0 >= cell.blocks.len();
        let mut y = top;
        let mut row = state.row;
        while row < group.rows {
            let mut row_bottom = y;
            for (i, cell) in group.cells.iter().enumerate() {
                if cell.row > row || cell.last < row || composed.get(i).copied().unwrap_or(true) {
                    continue;
                }
                let cursor = next.cursors.get(i).copied().unwrap_or_default();
                if finished(cell, cursor) {
                    continue;
                }
                if let Some(c) = composed.get_mut(i) {
                    *c = true;
                }
                let out = self.compose_cell(
                    cell,
                    widths,
                    cursor,
                    y,
                    frame_index,
                    frame_width,
                    total,
                    depth,
                    full_depth,
                    spacing,
                    &mut fragment.diagnostics,
                );
                if let Some(c) = next.cursors.get_mut(i) {
                    *c = out.cursor;
                }
                for (block, lines) in out.lines {
                    if !lines.is_empty() {
                        fragment.placed = true;
                    }
                    fragment.lines.push((i, block, lines));
                }
                if cell.last > cell.row {
                    if let Some(b) = spanning_bottom.get_mut(i) {
                        *b = out.bottom;
                    }
                    fragment.bottom = fragment.bottom.max(out.bottom);
                } else {
                    row_bottom = row_bottom.max(out.bottom);
                }
            }
            // A spanning cell stretches the last row it covers.
            for (i, cell) in group.cells.iter().enumerate() {
                if cell.last == row
                    && cell.last > cell.row
                    && composed.get(i).copied().unwrap_or(false)
                {
                    row_bottom = row_bottom.max(spanning_bottom.get(i).copied().unwrap_or(y));
                }
            }
            fragment.bottom = fragment.bottom.max(row_bottom);
            let unfinished = group.cells.iter().enumerate().any(|(i, c)| {
                c.last == row && !finished(c, next.cursors.get(i).copied().unwrap_or_default())
            });
            if unfinished {
                break;
            }
            y = row_bottom + spacing;
            row += 1;
        }
        next.row = row;
        fragment.done = row >= group.rows;
        (next, fragment)
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_cell(
        &self,
        cell: &Cell,
        widths: &[Length],
        mut cursor: (usize, usize),
        y0: Length,
        frame_index: usize,
        frame_width: Length,
        total: Length,
        depth: Length,
        full_depth: Length,
        spacing: Length,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> CellOut {
        let width = span_sum(widths, cell.column, cell.colspan);
        let x: Length = widths.iter().take(cell.column).copied().sum();
        let mut y = y0;
        let mut bottom = y0;
        let mut lines = Vec::new();
        while let Some(p) = cell.blocks.get(cursor.0) {
            let subject = Subject::Node(p.node());
            if total > frame_width {
                diagnostics.push(Diagnostic::new(
                    Severity::Warning,
                    codes::FRAME_OVERFLOW,
                    subject.clone(),
                    "frozen table columns exceed this continuation frame's width",
                ));
            }
            let measure = Measure(width);
            let bounded = Bounded {
                inner: &measure,
                depth,
            };
            let overflow = p.line_height() > full_depth && depth > Length::ZERO;
            let geometry: &dyn reprise_compose::GeometryProvider =
                if overflow { &measure } else { &bounded };
            let mut composed = p.compose(
                self.engine,
                geometry,
                frame_index,
                cursor.1,
                y,
                &subject,
                diagnostics,
                self.evaluation,
            );
            if composed.lines.is_empty() {
                break;
            }
            if overflow {
                diagnostics.push(Diagnostic::new(
                    Severity::Warning,
                    codes::FRAME_OVERFLOW,
                    subject.clone(),
                    "cell line taller than every body frame; placed overflowing",
                ));
            }
            for line in &mut composed.lines {
                line.rect.origin.x += x;
                for run in &mut line.runs {
                    run.x += x;
                }
            }
            lines.push((cursor.0, composed.lines));
            y = composed.block_end;
            bottom = bottom.max(y);
            if let Some(rest) = composed.rest {
                cursor.1 = rest;
                break;
            }
            cursor.0 = cursor.0.saturating_add(1);
            cursor.1 = 0;
            y += spacing;
        }
        CellOut {
            lines,
            bottom,
            cursor,
        }
    }
}
