//! A row group's ordinary placement loop, suspended after each fragment/move.
use super::*;

#[derive(Clone)]
pub(crate) struct GroupStep {
    group: Group,
    state: State,
    output: Vec<Vec<Vec<LineLayout>>>,
    split_reported: bool,
    first_top: Option<Length>,
    pub(crate) first_page: Option<usize>,
    finished: bool,
}
impl GroupStep {
    pub(crate) fn new(group: Group) -> Self {
        Self {
            state: State::start(&group),
            output: group
                .cells
                .iter()
                .map(|c| c.blocks.iter().map(|_| Vec::new()).collect())
                .collect(),
            group,
            split_reported: false,
            first_top: None,
            first_page: None,
            finished: false,
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn unit(
        &mut self,
        flow: &mut Flow<'_>,
        doc: &Document,
        table: NodeId,
        index: usize,
        groups: &[Group],
        widths: &[Length],
        repeat: &mut Repeat,
    ) -> bool {
        if self.finished {
            return true;
        }
        let group = &self.group;
        let spacing = flow.engine.flow.paragraph_spacing;
        let frame_index = flow.frame_index();
        let Some(frame) = flow.snapshot.frame(frame_index).cloned() else {
            return self.finish(flow);
        };
        self.first_page.get_or_insert(frame.page);
        let depth = flow.depth(frame_index, frame.rect.height);
        let plain_top = flow.table_top();
        self.first_top.get_or_insert(plain_top);
        let mut top = plain_top;
        let mut copy = None;
        if repeat.pending && !group.header {
            repeat.pending = false;
            if let Some(c) = flow.compose_header(
                doc,
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
                flow.header_unrepeated(table, repeat, "header rows do not fit this frame");
            }
        }
        let (mut next, mut fragment) = flow.compose_fragment(
            doc,
            group,
            widths,
            &self.state,
            frame_index,
            frame.rect.width,
            top,
            depth,
        );
        if copy.is_some() && !fragment.placed && !fragment.done {
            copy = None;
            top = plain_top;
            flow.header_unrepeated(table, repeat, "a repeated header would leave no room");
            (next, fragment) = flow.compose_fragment(
                doc,
                group,
                widths,
                &self.state,
                frame_index,
                frame.rect.width,
                top,
                depth,
            );
        }
        if !fragment.done
            && group.spans_rows()
            && repeat.body
            && self.state == State::start(group)
            && !self.split_reported
        {
            let head = if repeat.enabled && repeat.placed && !group.header {
                repeat.extent + spacing
            } else {
                Length::ZERO
            };
            let (_, fresh) = flow.compose_fragment(
                doc,
                group,
                widths,
                &self.state,
                frame_index,
                frame.rect.width,
                head,
                flow.fresh_depth(),
            );
            if fresh.done {
                if !flow.advance() {
                    return self.finish(flow);
                }
                repeat.body = false;
                repeat.pending = repeat.enabled && repeat.placed && !group.header;
                return false;
            }
        }
        if !fragment.done && group.spans_rows() && !self.split_reported {
            self.split_reported = true;
            flow.snapshot.diagnostics.push(Diagnostic::new(
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
        flow.snapshot.diagnostics.append(&mut fragment.diagnostics);
        if let Some(c) = copy {
            flow.commit_copy(table, frame_index, c, spacing);
        }
        for (cell, block, lines) in fragment.lines {
            if let Some(out) = self.output.get_mut(cell).and_then(|o| o.get_mut(block)) {
                out.extend(lines);
            }
        }
        self.state = next;
        if fragment.bottom > top {
            flow.set_used(fragment.bottom + spacing);
        }
        if fragment.placed {
            repeat.body = true;
        }
        if fragment.done {
            if group.header && repeat.header_top.is_none() {
                repeat.header_top = self.first_top;
            }
            if group.header && index.saturating_add(1) == repeat.groups {
                if let Some(start) = repeat.header_top {
                    repeat.extent = (fragment.bottom - start).max(Length::ZERO);
                }
                repeat.placed = repeat.enabled;
            }
            return self.finish(flow);
        }
        if group.header {
            repeat.enabled = false;
            if !repeat.warned {
                repeat.warned = true;
                flow.snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Warning,
                    codes::TABLE_HEADER_UNREPEATED,
                    Subject::Node(table),
                    "header rows continue across frames; they are not repeated",
                ));
            }
        }
        if !flow.advance() {
            return self.finish(flow);
        }
        repeat.body = false;
        repeat.pending = repeat.enabled && repeat.placed && !group.header;
        false
    }
    fn finish(&mut self, flow: &mut Flow<'_>) -> bool {
        self.finished = true;
        for (i, cell) in std::mem::take(&mut self.group.cells)
            .into_iter()
            .enumerate()
        {
            let cursor = self.state.cursors.get(i).copied().unwrap_or_default();
            let mut lines = self
                .output
                .get_mut(i)
                .map(std::mem::take)
                .unwrap_or_default()
                .into_iter();
            for (j, p) in cell.blocks.into_iter().enumerate() {
                if j >= cursor.0 {
                    unplaced(
                        &mut flow.snapshot.diagnostics,
                        &Subject::Node(p.node()),
                        if j == cursor.0 { cursor.1 } else { 0 }..p.text.len(),
                    );
                }
                let placed = lines.next().unwrap_or_default();
                if !placed.is_empty() {
                    flow.snapshot.blocks.push(p.into_block(placed));
                }
            }
        }
        true
    }
}
