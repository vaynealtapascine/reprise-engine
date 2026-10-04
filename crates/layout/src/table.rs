//! Tables allocate columns in a declared solver domain, then fragment rows
//! synchronously across the body thread. Cell text uses the ordinary composer.
use crate::flow::{Flow, Prepared, prepare, resolution_context, unplaced};
use crate::region::Bounded;
use crate::solver::{MAX_DOMAIN_VARIABLES, SolverDomain, Variable};
use crate::{Diagnostic, Subject, codes};
use reprise_compose::Measure;
use reprise_diag::Severity;
use reprise_doc::{ColumnWidth, Document, NodeId, TableColumns, TableRole};
use reprise_geom::Length;

pub const MAX_TABLE_ROWS: usize = 4096;
pub const MAX_TABLE_BLOCKS: usize = 65536;

struct Cell {
    column: usize,
    blocks: Vec<Prepared>,
}

impl Flow<'_> {
    pub(crate) fn table(&mut self, doc: &Document, node: NodeId, columns: &TableColumns) {
        let subject = Subject::Node(node);
        let count = columns.columns.len();
        if count == 0 || count > MAX_DOMAIN_VARIABLES {
            self.snapshot.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::TABLE_INVALID,
                subject,
                "table must declare 1..256 columns; table omitted",
            ));
            return;
        }
        let Some(frame) = self.snapshot.frame(self.frame_index()).cloned() else {
            return;
        };
        let starting_frame = self
            .template
            .frames
            .get(self.frame_index() % self.template.frames.len().max(1));
        let ctx = resolution_context(self.engine, self.template, starting_frame, frame.rect.width);
        let mut rows = Vec::new();
        let mut min = vec![Length::ZERO; count];
        let mut max = vec![Length::ZERO; count];
        let mut blocks = 0usize;
        let children = doc.children(Some(node));
        if children.len() > MAX_TABLE_ROWS {
            self.snapshot.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::TABLE_LIMIT,
                subject.clone(),
                "table exceeds 4096 rows; remaining rows omitted",
            ));
        }
        for row in children.into_iter().take(MAX_TABLE_ROWS) {
            if !matches!(doc.table_role(row), Ok(Some(TableRole::Row(_)))) {
                self.invalid(row, "table child is not a row");
                continue;
            }
            let mut cells = Vec::new();
            let mut occupied = std::collections::BTreeSet::new();
            let children = doc.children(Some(row));
            if children.len() > count {
                self.invalid(row, "too many cells; extra cells omitted");
            }
            for cell in children.into_iter().take(count) {
                let Ok(Some(TableRole::Cell(info))) = doc.table_role(cell) else {
                    self.invalid(cell, "row child is not a cell");
                    continue;
                };
                let col = info.column as usize;
                if col >= count || !occupied.insert(col) {
                    self.invalid(cell, "invalid or duplicate cell column");
                    continue;
                }
                let mut prepared = Vec::new();
                for block in doc.children(Some(cell)) {
                    blocks = blocks.saturating_add(1);
                    if blocks > MAX_TABLE_BLOCKS {
                        break;
                    }
                    if !matches!(doc.table_role(block), Ok(None)) {
                        self.invalid(block, "nested table containers are unsupported");
                        continue;
                    }
                    if doc.kind_of(block) == Some(reprise_doc::BlockKind::Annotation) {
                        if !crate::regions::owned(self.engine, doc, block)
                            && let Some(annotation) = self.annotation(doc, block)
                        {
                            self.pending.push(annotation);
                        }
                        continue;
                    }
                    if let Some(p) = prepare(
                        self.engine,
                        doc,
                        block,
                        &ctx,
                        &mut self.snapshot.diagnostics,
                    ) {
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
                        if let Some(m) = min.get_mut(col) {
                            *m = (*m).max(minimum);
                        }
                        if let Some(m) = max.get_mut(col) {
                            *m = (*m).max(maximum.max(minimum));
                        }
                        prepared.push(p);
                    }
                }
                cells.push(Cell {
                    column: col,
                    blocks: prepared,
                });
            }
            rows.push(cells);
            if blocks > MAX_TABLE_BLOCKS {
                self.snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    codes::TABLE_LIMIT,
                    subject.clone(),
                    "table exceeds 65536 content blocks; remainder omitted",
                ));
                break;
            }
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
        for cells in rows {
            self.table_row(cells, &solution.widths);
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

    fn table_row(&mut self, cells: Vec<Cell>, widths: &[Length]) {
        // Each cell has its own continuation cursor; all advance to the next
        // frame together. A finished cell stays empty on subsequent fragments.
        let mut cursors = vec![(0usize, 0usize); cells.len()];
        let mut output: Vec<Vec<Vec<crate::LineLayout>>> = cells
            .iter()
            .map(|c| c.blocks.iter().map(|_| Vec::new()).collect())
            .collect();
        loop {
            let frame_index = self.frame_index();
            let Some(frame) = self.snapshot.frame(frame_index).cloned() else {
                break;
            };
            let depth = self.depth(frame_index, frame.rect.height);
            let top = self.table_top();
            let mut bottom = top;
            let mut done = true;
            for (i, cell) in cells.iter().enumerate() {
                let Some(cursor) = cursors.get_mut(i) else {
                    continue;
                };
                let width = widths.get(cell.column).copied().unwrap_or_default();
                let x: Length = widths.iter().take(cell.column).copied().sum();
                let mut y = top;
                while let Some(p) = cell.blocks.get(cursor.0) {
                    let subject = Subject::Node(p.node());
                    if widths.iter().copied().sum::<Length>() > frame.rect.width {
                        self.snapshot.diagnostics.push(Diagnostic::new(
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
                    let full_depth = self
                        .template
                        .frames
                        .iter()
                        .filter(|f| f.is_main_flow())
                        .map(|f| f.depth)
                        .max()
                        .unwrap_or_default();
                    let overflow = p.style.line_height > full_depth && depth > Length::ZERO;
                    let geometry: &dyn reprise_compose::GeometryProvider =
                        if overflow { &measure } else { &bounded };
                    let mut composed = p.compose(
                        self.engine,
                        geometry,
                        frame_index,
                        cursor.1,
                        y,
                        &subject,
                        &mut self.snapshot.diagnostics,
                    );
                    if composed.lines.is_empty() {
                        done = false;
                        break;
                    }
                    if overflow {
                        self.snapshot.diagnostics.push(Diagnostic::new(
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
                    if let Some(lines) = output.get_mut(i).and_then(|o| o.get_mut(cursor.0)) {
                        lines.extend(composed.lines);
                    }
                    y = composed.block_end;
                    bottom = bottom.max(y);
                    if let Some(rest) = composed.rest {
                        cursor.1 = rest;
                        done = false;
                        break;
                    }
                    cursor.0 = cursor.0.saturating_add(1);
                    cursor.1 = 0;
                    y += self.engine.flow.paragraph_spacing;
                }
                done &= cursor.0 >= cell.blocks.len();
            }
            if bottom > top {
                self.set_used(bottom + self.engine.flow.paragraph_spacing);
            }
            if done || !self.advance() {
                break;
            }
        }
        for (i, cell) in cells.into_iter().enumerate() {
            let cursor = cursors.get(i).copied().unwrap_or_default();
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
}
