//! The resolved table grid: which cell occupies which rows and columns, and
//! which rows are headers (06, 24, 33, 37).
//!
//! Authored spans are unchecked input: zero, past the table edge, overlapping
//! another cell, or huge, and two peers can concurrently author spans that
//! collide. [`resolve_grid`] is the single deterministic interpretation, a pure
//! function of the merged document. Layout, export and accessibility all read
//! it, so none of them can disagree about what a cell spans.
//!
//! Rules, applied to rows and cells in document order (first claim wins):
//! - a zero span counts as one;
//! - a span is cut to the table edge and to the free cells right of / below its
//!   origin, so later cells never overwrite earlier ones;
//! - a cell whose origin is already claimed, or whose column is outside the table,
//!   is dropped;
//! - a rowspan never reaches from a repeating header row into the body.
//!
//! Every cut or drop is reported as a [`GridIssue`]; none panics. Derived data
//! only: nothing here is stored in the Loro document (05).
use crate::{Document, NodeId, TableRole};

/// Rows beyond this are omitted (reported as [`GridIssueKind::RowLimit`]).
pub const MAX_GRID_ROWS: usize = 4096;
/// Columns beyond this make the table unusable.
pub const MAX_GRID_COLUMNS: usize = 256;

/// One cell that was placed in the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridCell {
    pub node: NodeId,
    /// First row and column occupied.
    pub row: usize,
    pub column: usize,
    /// Rows and columns actually covered (at least one), after clamping.
    pub colspan: usize,
    pub rowspan: usize,
    /// The row it starts in is a header row, so the cell is a header cell.
    pub header: bool,
}

/// One accepted row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridRow {
    pub node: NodeId,
    pub header: bool,
    /// Indices into [`TableGrid::cells`], in document order.
    pub cells: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridIssueKind {
    /// A table child that is not a row, or a row child that is not a cell.
    Misplaced,
    /// A cell beyond the number of columns the table declares.
    TooManyCells,
    /// The declared column is outside the table.
    ColumnOutside,
    /// The cell's origin is already occupied by an earlier cell; it was dropped.
    Overlap,
    /// A zero span was read as one.
    SpanZero,
    /// A span was cut to the table edge.
    SpanEdge,
    /// A span was cut so it does not overlap an earlier cell.
    SpanOverlap,
    /// A rowspan was cut so it stays inside the header rows it started in.
    SpanHeader,
    /// Rows past `MAX_GRID_ROWS` were left out.
    RowLimit,
}

impl GridIssueKind {
    /// True when a cell or row was left out, false when it was only adjusted.
    pub fn omitted(self) -> bool {
        matches!(
            self,
            Self::Misplaced
                | Self::TooManyCells
                | Self::ColumnOutside
                | Self::Overlap
                | Self::RowLimit
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridIssue {
    pub node: NodeId,
    pub kind: GridIssueKind,
}

/// A table's resolved structure. Read-only and derived; recompute after edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableGrid {
    pub table: NodeId,
    pub columns: usize,
    pub rows: Vec<GridRow>,
    pub cells: Vec<GridCell>,
    /// Number of leading header rows. Only these repeat after a break; a header
    /// row after a body row is still a header for accessibility but never repeats.
    pub repeating_header_rows: usize,
    pub issues: Vec<GridIssue>,
}

impl TableGrid {
    pub fn cell(&self, node: NodeId) -> Option<&GridCell> {
        self.cells.iter().find(|c| c.node == node)
    }
}

/// Input to [`resolve_grid`]: raw authored values.
#[derive(Clone, Copy, Debug)]
pub struct CellInput {
    pub node: NodeId,
    pub column: u32,
    pub colspan: u32,
    pub rowspan: u32,
}

#[derive(Clone, Debug)]
pub struct RowInput {
    pub node: NodeId,
    pub header: bool,
    pub cells: Vec<CellInput>,
}

/// The pure resolver. `columns` must be at most [`MAX_GRID_COLUMNS`]; rows past
/// [`MAX_GRID_ROWS`] are ignored (callers report `RowLimit` themselves via
/// [`Document::table_structure`]).
pub fn resolve_grid(table: NodeId, columns: usize, rows: &[RowInput]) -> TableGrid {
    let columns = columns.min(MAX_GRID_COLUMNS);
    let row_count = rows.len().min(MAX_GRID_ROWS);
    let rows = rows.get(..row_count).unwrap_or(&[]);
    let mut repeating = 0;
    while rows.get(repeating).is_some_and(|r| r.header) {
        repeating += 1;
    }
    let mut occupied = vec![false; row_count.saturating_mul(columns)];
    let free = |occupied: &[bool], r: usize, c: usize| {
        occupied
            .get(r.saturating_mul(columns).saturating_add(c))
            .is_some_and(|taken| !*taken)
    };
    let mut grid = TableGrid {
        table,
        columns,
        rows: Vec::new(),
        cells: Vec::new(),
        repeating_header_rows: repeating,
        issues: Vec::new(),
    };
    for (r, row) in rows.iter().enumerate() {
        let mut indices = Vec::new();
        let mut accepted = 0usize;
        for cell in &row.cells {
            let mut issue = |kind| {
                grid.issues.push(GridIssue {
                    node: cell.node,
                    kind,
                })
            };
            if accepted >= columns {
                issue(GridIssueKind::TooManyCells);
                continue;
            }
            let column = cell.column as usize;
            if column >= columns {
                issue(GridIssueKind::ColumnOutside);
                continue;
            }
            if !free(&occupied, r, column) {
                issue(GridIssueKind::Overlap);
                continue;
            }
            let mut colspan = cell.colspan as usize;
            let mut rowspan = cell.rowspan as usize;
            if colspan == 0 || rowspan == 0 {
                issue(GridIssueKind::SpanZero);
                colspan = colspan.max(1);
                rowspan = rowspan.max(1);
            }
            let widest = columns - column;
            if colspan > widest {
                issue(GridIssueKind::SpanEdge);
                colspan = widest;
            }
            let last_row = if r < repeating { repeating } else { row_count };
            let tallest = last_row - r;
            if rowspan > tallest {
                issue(
                    if r < repeating && row_count > repeating && rowspan <= row_count - r {
                        GridIssueKind::SpanHeader
                    } else {
                        GridIssueKind::SpanEdge
                    },
                );
                rowspan = tallest;
            }
            // Cut to the free run on the first row, then to the rows free across it.
            let run = (0..colspan)
                .take_while(|i| free(&occupied, r, column + i))
                .count();
            if run < colspan {
                issue(GridIssueKind::SpanOverlap);
                colspan = run;
            }
            let depth = (0..rowspan)
                .take_while(|j| (0..colspan).all(|i| free(&occupied, r + j, column + i)))
                .count();
            if depth < rowspan {
                issue(GridIssueKind::SpanOverlap);
                rowspan = depth;
            }
            for j in 0..rowspan {
                for i in 0..colspan {
                    if let Some(slot) = occupied.get_mut((r + j) * columns + column + i) {
                        *slot = true;
                    }
                }
            }
            accepted += 1;
            indices.push(grid.cells.len());
            grid.cells.push(GridCell {
                node: cell.node,
                row: r,
                column,
                colspan,
                rowspan,
                header: row.header,
            });
        }
        grid.rows.push(GridRow {
            node: row.node,
            header: row.header,
            cells: indices,
        });
    }
    grid
}

impl Document {
    /// The resolved grid of a table node, the one interpretation every consumer
    /// shares. `Err` only when `table` is not a live table. Misplaced children,
    /// unreadable metadata and out-of-range spans are reported in
    /// [`TableGrid::issues`], never as failures, so a damaged table still has a
    /// structure. A table of zero or over [`MAX_GRID_COLUMNS`] columns has no
    /// grid (`None`).
    pub fn table_structure(&self, table: NodeId) -> Result<Option<TableGrid>, crate::DocError> {
        let Some(TableRole::Table(declared)) = self.table_role(table)? else {
            return Err(crate::DocError::Malformed(table, "table role"));
        };
        let columns = declared.columns.len();
        if columns == 0 || columns > MAX_GRID_COLUMNS {
            return Ok(None);
        }
        let mut issues = Vec::new();
        let mut inputs = Vec::new();
        let children = self.children(Some(table));
        for &row in children.iter().take(MAX_GRID_ROWS) {
            let Ok(Some(TableRole::Row(info))) = self.table_role(row) else {
                issues.push(GridIssue {
                    node: row,
                    kind: GridIssueKind::Misplaced,
                });
                continue;
            };
            let mut cells = Vec::new();
            for cell in self.children(Some(row)) {
                let Ok(Some(TableRole::Cell(c))) = self.table_role(cell) else {
                    issues.push(GridIssue {
                        node: cell,
                        kind: GridIssueKind::Misplaced,
                    });
                    continue;
                };
                cells.push(CellInput {
                    node: cell,
                    column: c.column,
                    colspan: c.colspan,
                    rowspan: c.rowspan,
                });
            }
            inputs.push(RowInput {
                node: row,
                header: info.header,
                cells,
            });
        }
        let mut grid = resolve_grid(table, columns, &inputs);
        if children.len() > MAX_GRID_ROWS {
            issues.push(GridIssue {
                node: table,
                kind: GridIssueKind::RowLimit,
            });
        }
        issues.append(&mut grid.issues);
        grid.issues = issues;
        Ok(Some(grid))
    }

    /// The grid placement of one cell, found through its table. `None` if the
    /// node is not a placed cell of a laid-out table (misplaced, dropped, or
    /// deleted). Accessibility and export use this for "is this a header, and
    /// what does it span?".
    pub fn table_cell_structure(&self, cell: NodeId) -> Option<GridCell> {
        let row = self.parent_of(cell)??;
        let table = self.parent_of(row)??;
        let grid = self.table_structure(table).ok()??;
        grid.cell(cell).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnWidth, TableColumns};

    fn doc_with_table(cols: usize) -> (Document, NodeId) {
        let doc = Document::new(1).unwrap();
        let table = doc
            .append_table(TableColumns {
                columns: (0..cols)
                    .map(|_| Column {
                        width: ColumnWidth::Proportional(1),
                    })
                    .collect(),
            })
            .unwrap();
        (doc, table)
    }

    #[test]
    fn spans_are_clamped_and_overlaps_dropped_deterministically() {
        let (doc, table) = doc_with_table(3);
        let r0 = doc.append_table_row(table, false).unwrap();
        let a = doc.append_table_cell_spanned(r0, 0, 2, 2).unwrap();
        let b = doc.append_table_cell_spanned(r0, 1, 1, 1).unwrap(); // inside a
        let c = doc.append_table_cell_spanned(r0, 2, 9, u32::MAX).unwrap();
        let r1 = doc.append_table_row(table, false).unwrap();
        let d = doc.append_table_cell_spanned(r1, 0, 1, 1).unwrap(); // under a
        let e = doc.append_table_cell(r1, 2).unwrap(); // under c
        let grid = doc.table_structure(table).unwrap().unwrap();
        let cell = |n| grid.cell(n).copied();
        assert_eq!(cell(a).map(|c| (c.colspan, c.rowspan)), Some((2, 2)));
        assert_eq!(cell(b), None);
        assert_eq!(cell(c).map(|c| (c.colspan, c.rowspan)), Some((1, 2)));
        assert_eq!(cell(d), None);
        assert_eq!(cell(e), None);
        let kinds: Vec<_> = grid.issues.iter().map(|i| (i.node, i.kind)).collect();
        assert!(kinds.contains(&(b, GridIssueKind::Overlap)));
        assert!(kinds.contains(&(c, GridIssueKind::SpanEdge)));
        assert!(kinds.contains(&(d, GridIssueKind::Overlap)));
        assert!(kinds.contains(&(e, GridIssueKind::Overlap)));
    }

    #[test]
    fn zero_spans_and_whole_table_spans() {
        let (doc, table) = doc_with_table(4);
        let r = doc.append_table_row(table, false).unwrap();
        let z = doc.append_table_cell_spanned(r, 0, 0, 0).unwrap();
        let grid = doc.table_structure(table).unwrap().unwrap();
        let z = grid.cell(z).unwrap();
        assert_eq!((z.colspan, z.rowspan), (1, 1));
        assert!(
            grid.issues
                .iter()
                .any(|i| i.kind == GridIssueKind::SpanZero)
        );

        let (doc, table) = doc_with_table(4);
        let h = doc.append_table_row(table, true).unwrap();
        let hc = doc.append_table_cell_spanned(h, 0, 4, 99).unwrap();
        let b = doc.append_table_row(table, false).unwrap();
        let bc = doc.append_table_cell_spanned(b, 0, 4, 99).unwrap();
        let grid = doc.table_structure(table).unwrap().unwrap();
        assert_eq!(grid.repeating_header_rows, 1);
        let hc = grid.cell(hc).unwrap();
        assert_eq!((hc.colspan, hc.rowspan, hc.header), (4, 1, true));
        let bc = grid.cell(bc).unwrap();
        assert_eq!((bc.colspan, bc.rowspan, bc.header), (4, 1, false));
    }

    #[test]
    fn detached_header_rows_are_headers_but_do_not_repeat() {
        let (doc, table) = doc_with_table(1);
        let a = doc.append_table_row(table, true).unwrap();
        let b = doc.append_table_row(table, false).unwrap();
        let c = doc.append_table_row(table, true).unwrap();
        for r in [a, b, c] {
            doc.append_table_cell(r, 0).unwrap();
        }
        let grid = doc.table_structure(table).unwrap().unwrap();
        assert_eq!(grid.repeating_header_rows, 1);
        assert_eq!(
            grid.rows.iter().map(|r| r.header).collect::<Vec<_>>(),
            [true, false, true]
        );
    }

    #[test]
    fn grid_is_none_for_unusable_column_counts_and_err_for_non_tables() {
        let (doc, table) = doc_with_table(MAX_GRID_COLUMNS + 1);
        assert_eq!(doc.table_structure(table).unwrap(), None);
        let (doc, table) = doc_with_table(0);
        assert_eq!(doc.table_structure(table).unwrap(), None);
        let p = doc
            .append_block(crate::BlockKind::Paragraph, "", "x")
            .unwrap();
        assert!(doc.table_structure(p).is_err());
    }

    #[test]
    fn concurrent_overlapping_spans_converge() {
        let (doc, table) = doc_with_table(3);
        let r = doc.append_table_row(table, false).unwrap();
        let a = doc.append_table_cell(r, 0).unwrap();
        let b = doc.append_table_cell(r, 1).unwrap();
        let r2 = doc.append_table_row(table, false).unwrap();
        let c = doc.append_table_cell(r2, 0).unwrap();
        let other = doc.fork(2).unwrap();
        doc.set_table_cell_span(a, 3, 2).unwrap();
        other.set_table_cell_span(b, 1, 2).unwrap();
        other.set_table_cell_span(c, 2, 1).unwrap();
        doc.merge(&other).unwrap();
        other.merge(&doc).unwrap();
        assert_eq!(
            doc.table_structure(table).unwrap(),
            other.table_structure(table).unwrap()
        );
    }
}
