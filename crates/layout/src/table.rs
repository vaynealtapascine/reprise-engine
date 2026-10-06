//! Tables allocate columns in a declared solver domain, then fragment row
//! groups synchronously across the body thread (see `table_flow`). Cell text
//! uses the ordinary composer. What a cell spans is decided once, by
//! `Document::table_structure`, so layout, export and accessibility agree.
use crate::flow::{Flow, Prepared, prepare_cached, resolution_context};
use crate::solver::{SolverDomain, Variable};
use crate::table_flow::{Cell, Group};
use crate::{Diagnostic, Subject, codes};
use reprise_diag::Severity;
use reprise_doc::{
    ColumnWidth, Document, GridCell, GridIssueKind, NodeId, TableColumns, TableGrid,
};
use reprise_geom::Length;

pub const MAX_TABLE_BLOCKS: usize = 65536;

/// Min/max content width of one cell, from its shaped blocks.
fn measure(blocks: &[Prepared]) -> (Length, Length) {
    let mut min = Length::ZERO;
    let mut max = Length::ZERO;
    for p in blocks {
        let mut start = 0;
        let mut minimum = Length::ZERO;
        let mut maximum = Length::ZERO;
        let mut forced_start = 0;
        for b in &p.breaks {
            minimum = minimum.max(p.shaped.width(start..b.at));
            start = b.at;
            if b.kind == reprise_compose::BreakKind::Forced {
                maximum = maximum.max(p.shaped.width(forced_start..b.at));
                forced_start = b.at;
            }
        }
        maximum = maximum.max(p.shaped.width(forced_start..p.text.len()));
        if let Some(width) = p.image_width() {
            minimum = width;
            maximum = width;
        }
        min = min.max(minimum);
        max = max.max(maximum.max(minimum));
    }
    (min, max)
}

/// Raises the sum of `bounds[column..column + span]` to at least `need`, sharing
/// the deficit equally (remainder to the first columns) among the spanned
/// content columns. Columns of other kinds don't read these bounds.
fn grow(bounds: &mut [Length], eligible: &[bool], column: usize, span: usize, need: Length) {
    let end = column.saturating_add(span).min(bounds.len());
    let have: i64 = bounds
        .get(column..end)
        .unwrap_or(&[])
        .iter()
        .map(|l| i64::from(l.0))
        .sum();
    let deficit = i64::from(need.0) - have;
    let targets: Vec<usize> = (column..end)
        .filter(|&i| eligible.get(i).copied().unwrap_or(false))
        .collect();
    if deficit <= 0 || targets.is_empty() {
        return;
    }
    let count = targets.len() as i64;
    for (k, &i) in targets.iter().enumerate() {
        let share = deficit / count + i64::from((k as i64) < deficit % count);
        if let Some(b) = bounds.get_mut(i) {
            *b = Length((i64::from(b.0) + share).min(i64::from(i32::MAX)) as i32);
        }
    }
}

impl Flow<'_> {
    pub(crate) fn table(&mut self, doc: &Document, node: NodeId, columns: &TableColumns) {
        let subject = Subject::Node(node);
        let count = columns.columns.len();
        let grid = match doc.table_structure(node) {
            Ok(Some(grid)) => grid,
            _ => {
                self.snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    codes::TABLE_INVALID,
                    subject,
                    "table must declare 1..256 columns; table omitted",
                ));
                return;
            }
        };
        self.report_grid(&grid);
        let Some(frame) = self.snapshot.frame(self.frame_index()).cloned() else {
            return;
        };
        let starting_frame = self
            .template
            .frames
            .get(self.frame_index() % self.template.frames.len().max(1));
        let ctx = resolution_context(self.engine, self.template, starting_frame, frame.rect.width);
        let mut min = vec![Length::ZERO; count];
        let mut max = vec![Length::ZERO; count];
        let mut blocks = 0usize;
        // Prepared blocks and measurements per placed cell, in grid order.
        let mut prepared: Vec<Vec<Prepared>> = Vec::new();
        let mut kept_rows = 0usize;
        'rows: for row in &grid.rows {
            for &index in &row.cells {
                let Some(cell) = grid.cells.get(index) else {
                    prepared.push(Vec::new());
                    continue;
                };
                let mut cell_blocks = Vec::new();
                for block in doc.children(Some(cell.node)) {
                    blocks = blocks.saturating_add(1);
                    if blocks > MAX_TABLE_BLOCKS {
                        break;
                    }
                    if !matches!(doc.table_role(block), Ok(None)) {
                        self.invalid(block, "nested table containers are unsupported");
                        continue;
                    }
                    if doc.kind_of(block) == Some(reprise_doc::BlockKind::Annotation) {
                        if !self.region_owners.contains(&block)
                            && let Some(annotation) = self.annotation(doc, block)
                        {
                            self.pending.push(annotation);
                        }
                        continue;
                    }
                    if let Some(p) = prepare_cached(
                        self.engine,
                        doc,
                        block,
                        &ctx,
                        &mut self.snapshot.diagnostics,
                        self.evaluation,
                    ) {
                        cell_blocks.push(p);
                    }
                }
                prepared.push(cell_blocks);
            }
            kept_rows += 1;
            if blocks > MAX_TABLE_BLOCKS {
                self.snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    codes::TABLE_LIMIT,
                    subject.clone(),
                    "table exceeds 65536 content blocks; remainder omitted",
                ));
                break 'rows;
            }
        }
        // `prepared` follows the rows' cell order, which is grid order.
        let placed: Vec<&GridCell> = grid
            .rows
            .iter()
            .take(kept_rows)
            .flat_map(|r| r.cells.iter())
            .filter_map(|&i| grid.cells.get(i))
            .collect();
        let eligible: Vec<bool> = columns
            .columns
            .iter()
            .map(|c| matches!(c.width, ColumnWidth::Content))
            .collect();
        let measured: Vec<(Length, Length)> = prepared.iter().map(|b| measure(b)).collect();
        for (cell, &(lo, hi)) in placed.iter().zip(&measured) {
            if cell.colspan == 1 {
                if let Some(m) = min.get_mut(cell.column) {
                    *m = (*m).max(lo);
                }
                if let Some(m) = max.get_mut(cell.column) {
                    *m = (*m).max(hi);
                }
            }
        }
        for (cell, &(lo, hi)) in placed.iter().zip(&measured) {
            if cell.colspan > 1 {
                grow(&mut min, &eligible, cell.column, cell.colspan, lo);
                grow(&mut max, &eligible, cell.column, cell.colspan, hi);
            }
        }
        for (hi, &lo) in max.iter_mut().zip(&min) {
            *hi = (*hi).max(lo);
        }
        let variables = columns
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| match c.width {
                ColumnWidth::Fixed(w) => Variable {
                    min: w,
                    max: w,
                    weight: 0,
                },
                ColumnWidth::Proportional(w) => Variable {
                    min: Length::ZERO,
                    max: Length::MAX,
                    weight: w,
                },
                ColumnWidth::Content => Variable {
                    min: min.get(i).copied().unwrap_or_default(),
                    max: max.get(i).copied().unwrap_or_default(),
                    weight: 1,
                },
            })
            .collect();
        let solution = SolverDomain {
            budget: frame.rect.width,
            variables,
        }
        .solve();
        self.snapshot.diagnostics.extend(
            solution
                .notes
                .into_iter()
                .map(|n| Diagnostic::from_note(n, subject.clone())),
        );
        // Assemble row groups: rows joined by row spans are composed together.
        let mut cells = prepared.into_iter();
        let mut by_row: Vec<Vec<(&GridCell, Vec<Prepared>)>> = Vec::new();
        for row in grid.rows.iter().take(kept_rows) {
            let mut in_row = Vec::new();
            for &i in &row.cells {
                if let (Some(cell), Some(blocks)) = (grid.cells.get(i), cells.next()) {
                    in_row.push((cell, blocks));
                }
            }
            by_row.push(in_row);
        }
        let mut groups = Vec::new();
        let mut first = 0usize;
        while first < by_row.len() {
            let mut end = first;
            let mut r = first;
            while r <= end && r < by_row.len() {
                for (cell, _) in by_row.get(r).into_iter().flatten() {
                    end = end.max(cell.row.saturating_add(cell.rowspan).saturating_sub(1));
                }
                r += 1;
            }
            let end = end.min(by_row.len() - 1);
            let mut group = Group {
                rows: end - first + 1,
                header: grid.rows.get(first).is_some_and(|r| r.header)
                    && first < grid.repeating_header_rows,
                cells: Vec::new(),
            };
            for row in by_row.iter_mut().take(end + 1).skip(first) {
                for (cell, blocks) in std::mem::take(row) {
                    let width: Length = solution
                        .widths
                        .iter()
                        .skip(cell.column)
                        .take(cell.colspan)
                        .copied()
                        .sum();
                    let mut blocks = blocks;
                    for block in &mut blocks {
                        block.fit_image_width(width, &mut self.snapshot.diagnostics);
                    }
                    group.cells.push(Cell {
                        node: cell.node,
                        row: cell.row - first,
                        last: (cell.row + cell.rowspan - 1).min(end) - first,
                        column: cell.column,
                        colspan: cell.colspan,
                        blocks,
                    });
                }
            }
            groups.push(group);
            first = end + 1;
        }
        self.place_groups(node, groups, &solution.widths);
    }

    /// Reports what the grid had to cut, drop or limit, in document order.
    fn report_grid(&mut self, grid: &TableGrid) {
        for issue in &grid.issues {
            let (severity, code, message) = match issue.kind {
                GridIssueKind::RowLimit => (
                    Severity::Error,
                    codes::TABLE_LIMIT,
                    "table exceeds 4096 rows; remaining rows omitted",
                ),
                GridIssueKind::MisplacedRow => (
                    Severity::Error,
                    codes::TABLE_INVALID,
                    "table child is not a row",
                ),
                GridIssueKind::MisplacedCell => (
                    Severity::Error,
                    codes::TABLE_INVALID,
                    "row child is not a cell",
                ),
                GridIssueKind::TooManyCells => (
                    Severity::Error,
                    codes::TABLE_INVALID,
                    "too many cells; extra cells omitted",
                ),
                GridIssueKind::ColumnOutside | GridIssueKind::Overlap => (
                    Severity::Error,
                    codes::TABLE_INVALID,
                    "invalid or duplicate cell column",
                ),
                GridIssueKind::SpanZero => (
                    Severity::Warning,
                    codes::TABLE_SPAN,
                    "a zero span was read as one",
                ),
                GridIssueKind::SpanEdge => (
                    Severity::Warning,
                    codes::TABLE_SPAN,
                    "span reaches past the table edge; clamped",
                ),
                GridIssueKind::SpanOverlap => (
                    Severity::Warning,
                    codes::TABLE_SPAN,
                    "span overlaps an earlier cell; clamped",
                ),
                GridIssueKind::SpanHeader => (
                    Severity::Warning,
                    codes::TABLE_SPAN,
                    "row span reaches out of the header rows; clamped",
                ),
            };
            self.snapshot.diagnostics.push(Diagnostic::new(
                severity,
                code,
                Subject::Node(issue.node),
                message,
            ));
        }
    }

    fn invalid(&mut self, node: NodeId, message: &str) {
        self.snapshot.diagnostics.push(Diagnostic::new(
            Severity::Error,
            codes::TABLE_INVALID,
            Subject::Node(node),
            message,
        ));
    }
}
