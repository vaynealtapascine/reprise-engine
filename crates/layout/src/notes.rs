//! Notes have independent byte cursors and may consume whole continuation pages.
use crate::region::Bounded;
use crate::regions::{Plan, ensure_page, prepare};
use crate::template::ResolvedTemplate;
use crate::{Diagnostic, Engine, LayoutSnapshot, LineRef, Subject, codes};
use reprise_compose::Measure;
use reprise_diag::Severity;
use reprise_doc::{Document, FrameRole, NotePlacement, Param, Relation, RelationId};
use reprise_geom::{Length, Point};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Notes {
    used: BTreeMap<usize, Length>,
}

impl Notes {
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &mut self,
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        scratch: &mut LayoutSnapshot,
        out: &mut Plan,
        id: RelationId,
        relation: &Relation,
        anchor: LineRef,
        evaluation: Option<&crate::incremental::Evaluation>,
    ) {
        let Some(owner) = relation.owner else { return };
        let Some(anchor_line) = scratch.line(anchor).cloned() else {
            return;
        };
        let Some(anchor_frame) = scratch.frame(anchor_line.frame).cloned() else {
            return;
        };
        let Some((ti, area)) = template
            .frames
            .iter()
            .enumerate()
            .find(|(_, f)| f.role == FrameRole::Notes && f.takes_text())
        else {
            out.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::RELATION_NO_FRAME,
                Subject::Relation(id),
                "page template has no usable notes frame; owner omitted",
            ));
            return;
        };
        let placement = match relation.params.get("placement") {
            Some(Param::Text(p)) => NotePlacement::parse(p),
            None => Some(NotePlacement::Page),
            _ => None,
        };
        let Some(placement) = placement else {
            out.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::REGION_PARAMETER,
                Subject::Relation(id),
                "invalid note placement; owner omitted",
            ));
            return;
        };
        let mut page = if placement == NotePlacement::End {
            scratch
                .blocks
                .iter()
                .flat_map(|b| &b.lines)
                .filter_map(|l| scratch.frame(l.frame))
                .filter(|f| matches!(&f.role, FrameRole::Flow(_)))
                .map(|f| f.page)
                .max()
                .unwrap_or(0)
                .saturating_add(1)
        } else {
            anchor_frame.page
        };
        let first_page = page;
        let Some(prepared) = prepare(
            engine,
            doc,
            template,
            owner,
            ti,
            area.width,
            &mut out.diagnostics,
            evaluation,
        ) else {
            return;
        };
        let mut start = 0;
        let mut lines = Vec::new();
        let mut completed = false;
        while ensure_page(engine, template, scratch, page) {
            let index = page
                .saturating_mul(template.frames.len())
                .saturating_add(ti);
            let previous = self.used.get(&index).copied().unwrap_or_default();
            let used = previous
                + if previous > Length::ZERO {
                    engine.flow.annotation_spacing.max(Length::ZERO)
                } else {
                    Length::ZERO
                };
            let mut capacity = area.depth;
            if let Some(inverse) = area.to_page().inverse() {
                for (float_frame, boxes) in &out.exclusions {
                    let Some(frame) = scratch.frame(*float_frame).filter(|f| f.page == page) else {
                        continue;
                    };
                    for rect in boxes {
                        let corners = [
                            rect.origin,
                            Point::new(rect.origin.x + rect.width, rect.origin.y),
                            Point::new(rect.origin.x, rect.origin.y + rect.height),
                            Point::new(rect.origin.x + rect.width, rect.origin.y + rect.height),
                        ];
                        let projected: Vec<_> = corners
                            .into_iter()
                            .map(|p| inverse.apply(frame.to_page.apply(p)))
                            .collect();
                        let left = projected.iter().map(|p| p.x).min().unwrap_or_default();
                        let right = projected.iter().map(|p| p.x).max().unwrap_or_default();
                        let bottom = projected.iter().map(|p| p.y).max().unwrap_or_default();
                        if left < area.width && right > Length::ZERO {
                            capacity = capacity
                                .min((area.depth - bottom.max(Length::ZERO)).max(Length::ZERO));
                        }
                    }
                }
            }
            if page == first_page
                && placement == NotePlacement::Page
                && matches!(anchor_frame.role, FrameRole::Flow(_))
            {
                // Leave room for the anchor: a full-page note must not chase
                // its own anchor indefinitely through otherwise empty pages.
                if let Some(inverse) = area.to_page().inverse() {
                    let bottom = anchor_frame.to_page.apply(Point::new(
                        anchor_line.rect.origin.x,
                        anchor_line.rect.origin.y + anchor_line.rect.height,
                    ));
                    let anchor_bottom = inverse.apply(bottom).y.max(Length::ZERO);
                    capacity = capacity.min((area.depth - anchor_bottom).max(Length::ZERO));
                } else {
                    capacity = Length::ZERO;
                }
            }
            let measure = Measure(area.width);
            let overflow = prepared.style.line_height > area.depth
                && page > first_page
                && used == Length::ZERO;
            let bounded = Bounded {
                inner: &measure,
                depth: if overflow {
                    prepared.style.line_height
                } else {
                    capacity
                },
            };
            let composed = prepared.compose(
                engine,
                &bounded,
                index,
                start,
                used,
                &Subject::Node(owner),
                &mut out.diagnostics,
                evaluation,
            );
            if !composed.lines.is_empty() {
                if overflow {
                    out.diagnostics.push(Diagnostic::new(
                        Severity::Warning,
                        codes::FRAME_OVERFLOW,
                        Subject::Node(owner),
                        "note line taller than its entire notes frame; placed overflowing",
                    ));
                }
                let end = composed.block_end;
                lines.extend(composed.lines);
                self.used.insert(index, end);
                if let Some(rest) = composed.rest {
                    start = rest;
                } else {
                    completed = true;
                    break;
                }
            }
            page = page.saturating_add(1);
        }
        if !completed {
            crate::flow::unplaced(
                &mut out.diagnostics,
                &Subject::Node(owner),
                start..prepared.text.len(),
            );
            out.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::PAGE_LIMIT,
                Subject::Relation(id),
                "note continuation reached the page limit; remaining text omitted",
            ));
        }
        if !lines.is_empty() {
            if page > first_page {
                out.diagnostics.push(Diagnostic::new(
                    Severity::Info,
                    codes::NOTE_CONTINUED,
                    Subject::Relation(id),
                    "note continues or starts on a later page",
                ));
            }
            let block = prepared.into_block(lines);
            scratch.blocks.push(block.clone());
            out.blocks.push(block);
            out.applied.insert(id);
        }
    }

    /// Align all notes on a page to the area's bottom and reserve intersected
    /// body depth. This is allocation, not mutation of authored frame geometry.
    pub fn finish(self, template: &ResolvedTemplate, out: &mut Plan) {
        let frames = template.frames.len();
        for (index, used) in self.used {
            let Some(area) = template.frames.get(index % frames.max(1)) else {
                continue;
            };
            let occupied = used.min(area.depth);
            let by = (area.depth - occupied).max(Length::ZERO);
            for block in &mut out.blocks {
                for line in &mut block.lines {
                    if line.frame == index {
                        line.rect.origin.y += by;
                        line.baseline += by;
                    }
                }
            }
            let page = index / frames.max(1);
            // Project the occupied notes box to each body's logical space.
            // Conservatively reserve from the earliest intersecting corner.
            for (i, body) in template
                .frames
                .iter()
                .enumerate()
                .filter(|(_, f)| f.is_main_flow())
            {
                let Some(inverse) = body.to_page().inverse() else {
                    continue;
                };
                let corners = [
                    Point::new(Length::ZERO, by),
                    Point::new(area.width, by),
                    Point::new(Length::ZERO, area.depth),
                    Point::new(area.width, area.depth),
                ];
                let mapped: Vec<_> = corners
                    .into_iter()
                    .map(|p| inverse.apply(area.to_page().apply(p)))
                    .collect();
                let left = mapped.iter().map(|p| p.x).min().unwrap_or_default();
                let right = mapped.iter().map(|p| p.x).max().unwrap_or_default();
                let top = mapped.iter().map(|p| p.y).min().unwrap_or_default();
                let bottom = mapped.iter().map(|p| p.y).max().unwrap_or_default();
                if right > Length::ZERO
                    && left < body.width
                    && bottom > Length::ZERO
                    && top < body.depth
                {
                    out.reservations.insert(
                        page.saturating_mul(frames).saturating_add(i),
                        body.depth - top.max(Length::ZERO),
                    );
                }
            }
        }
    }
}
