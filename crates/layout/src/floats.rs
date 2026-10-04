//! Side/edge float placement and stacked exclusion geometry (24).
use crate::regions::{Plan, ensure_page, prepare};
use crate::template::ResolvedTemplate;
use crate::{Diagnostic, Engine, LayoutSnapshot, LineRef, Subject, codes};
use reprise_compose::Measure;
use reprise_diag::Severity;
use reprise_doc::{Document, FloatSide, Param, Relation, RelationId};
use reprise_geom::{Length, Point, Rect};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Floats {
    next: BTreeMap<usize, Length>,
}

impl Floats {
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
        let Some(line) = scratch.line(anchor).cloned() else {
            return;
        };
        let Some(source) = scratch.frame(line.frame).cloned() else {
            return;
        };
        let thread = template.main_thread();
        let Some(first) = thread
            .iter()
            .position(|&i| i == line.frame % template.frames.len().max(1))
        else {
            out.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::RELATION_NO_FRAME,
                Subject::Relation(id),
                "float anchor must be in a body frame",
            ));
            return;
        };
        let side = match relation.params.get("side") {
            Some(Param::Text(s)) => FloatSide::parse(s),
            None => Some(FloatSide::Right),
            _ => None,
        };
        let Some(side) = side else {
            out.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::REGION_PARAMETER,
                Subject::Relation(id),
                "invalid float side; owner omitted",
            ));
            return;
        };
        let mut page = source.page;
        let mut pos = first;
        // One template cycle is enough to know whether an unoccupied frame can
        // ever hold it. An occupied candidate gets one fresh page to try.
        for attempt in 0..thread.len().saturating_mul(2).saturating_add(1) {
            let Some(&ti) = thread.get(pos) else { return };
            let Some(frame) = template.frames.get(ti) else {
                return;
            };
            let index = page
                .saturating_mul(template.frames.len())
                .saturating_add(ti);
            let context =
                crate::flow::resolution_context(engine, template, Some(frame), frame.width);
            let size = doc
                .computed_style_with(owner, &context, &engine.functions)
                .map_or(Length::ZERO, |s| s.size);
            let width = if matches!(side, FloatSide::Top | FloatSide::Bottom) {
                frame.width
            } else {
                match relation.params.get("width") {
                    Some(Param::Length(e)) => e.resolve(size),
                    _ => frame.width.mul_ratio(1, 3),
                }
            };
            let margin = match relation.params.get("margin") {
                Some(Param::Length(e)) => e.resolve(size),
                _ => engine.flow.annotation_spacing,
            }
            .max(Length::ZERO);
            if width <= Length::ZERO {
                break;
            }
            if width <= frame.width {
                let mut candidate_diagnostics = Vec::new();
                let Some(prepared) = prepare(
                    engine,
                    doc,
                    template,
                    owner,
                    ti,
                    width,
                    &mut candidate_diagnostics,
                    evaluation,
                ) else {
                    out.diagnostics.extend(candidate_diagnostics);
                    return;
                };
                let composed = prepared.compose(
                    engine,
                    &Measure(width),
                    index,
                    0,
                    Length::ZERO,
                    &Subject::Node(owner),
                    &mut candidate_diagnostics,
                    evaluation,
                );
                let height = composed.block_end;
                let free = self.next.get(&index).copied().unwrap_or_default();
                let desired = if attempt == 0 && matches!(side, FloatSide::Left | FloatSide::Right)
                {
                    line.rect.origin.y
                } else {
                    Length::ZERO
                };
                let at = if side == FloatSide::Bottom {
                    frame.depth - height
                } else {
                    desired.max(free)
                };
                if composed.rest.is_none()
                    && at >= free
                    && at >= Length::ZERO
                    && height <= frame.depth - at
                    && ensure_page(engine, template, scratch, page)
                {
                    let x = if side == FloatSide::Right {
                        frame.width - width
                    } else {
                        Length::ZERO
                    };
                    let mut block =
                        crate::flow::place(prepared.into_block(composed.lines), index, at);
                    for l in &mut block.lines {
                        l.rect.origin.x += x;
                        for run in &mut l.runs {
                            run.x += x;
                        }
                    }
                    block.sync_image();
                    // Clip padded endpoints before subtracting: saturating a
                    // doubled extreme margin must not shrink the blocked box.
                    let left = (x - margin).max(Length::ZERO);
                    let top = (at - margin).max(Length::ZERO);
                    let right = (x + width + margin).min(frame.width);
                    let bottom = (at + height + margin).min(frame.depth);
                    let exclusion = Rect::new(Point::new(left, top), right - left, bottom - top);
                    out.diagnostics.extend(candidate_diagnostics);
                    out.exclusions.entry(index).or_default().push(exclusion);
                    self.next.insert(index, at + height + margin);
                    scratch.blocks.push(block.clone());
                    out.blocks.push(block);
                    out.applied.insert(id);
                    if attempt > 0 {
                        out.diagnostics.push(Diagnostic::new(
                            Severity::Warning,
                            codes::FLOAT_DEFERRED,
                            Subject::Relation(id),
                            "float did not fit at its anchor; deferred to the next fitting frame",
                        ));
                    }
                    return;
                }
            }
            pos = pos.saturating_add(1);
            if pos >= thread.len() {
                pos = 0;
                page = page.saturating_add(1);
            }
        }
        out.diagnostics.push(Diagnostic::new(
            Severity::Error,
            codes::FLOAT_UNPLACEABLE,
            Subject::Relation(id),
            "float fits no available frame within one fresh template cycle; owner omitted",
        ));
    }
}
