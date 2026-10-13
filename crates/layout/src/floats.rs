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

struct Candidate {
    page: usize,
    index: usize,
    ti: usize,
    frame: crate::template::ResolvedFrame,
    template: std::sync::Arc<ResolvedTemplate>,
    owner: reprise_doc::sections::SectionId,
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
        sections: &crate::sections::Table,
    ) {
        let Some(owner) = relation.owner else { return };
        let Some(line) = scratch.line(anchor).cloned() else {
            return;
        };
        let Some(source) = scratch.frame(line.frame).cloned() else {
            return;
        };
        let candidates = sections
            .enabled
            .then(|| Self::candidates(sections, scratch, line.frame));
        let thread = template.main_thread();
        let first = if candidates.is_some() {
            Some(0)
        } else {
            thread.iter().position(|&i| {
                crate::pages::PageFactory::index(scratch, source.page, i) == Some(line.frame)
            })
        };
        let Some(first) = first else {
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
        let attempts = candidates.as_ref().map_or_else(
            || thread.len().saturating_mul(2).saturating_add(1),
            Vec::len,
        );
        for attempt in 0..attempts {
            let candidate = candidates.as_ref().and_then(|c| c.get(attempt));
            let template = candidate.map_or(template, |c| c.template.as_ref());
            let ti = if let Some(candidate) = candidate {
                page = candidate.page;
                candidate.ti
            } else {
                let Some(&ti) = thread.get(pos) else { return };
                ti
            };
            let frame = if let Some(candidate) = candidate {
                &candidate.frame
            } else {
                let Some(frame) = template.frames.get(ti) else {
                    return;
                };
                frame
            };
            let index = if let Some(candidate) = candidate {
                candidate.index
            } else {
                let Some(index) = crate::pages::PageFactory::preview(template, scratch, page, ti)
                else {
                    return;
                };
                index
            };
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
                    && if let Some(candidate) = candidate {
                        crate::pages::PageFactory::ensure_section(
                            engine,
                            sections,
                            candidate.owner,
                            crate::PageKind::Content,
                            scratch,
                            page,
                        )
                    } else {
                        ensure_page(engine, template, scratch, page)
                    }
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
                    if let Some(frame) =
                        crate::pages::Catalog::from_snapshot(scratch).reference(index)
                    {
                        out.exclude(frame, exclusion);
                    }
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
    fn candidates(
        sections: &crate::sections::Table,
        snapshot: &LayoutSnapshot,
        anchor: usize,
    ) -> Vec<Candidate> {
        let catalog = crate::pages::Catalog::from_snapshot(snapshot);
        let Some(source) = snapshot.frame(anchor) else {
            return Vec::new();
        };
        let Some(reference) = catalog.reference(anchor) else {
            return Vec::new();
        };
        if !matches!(reference.address, crate::pages::FrameAddr::Main { .. }) {
            return Vec::new();
        }
        let mut pages = vec![source.page];
        let mut next = source.page.saturating_add(1);
        while snapshot
            .sections
            .as_ref()
            .and_then(|s| s.pages.get(next))
            .is_some_and(|p| p.kind == crate::PageKind::Filler)
        {
            next = next.saturating_add(1);
        }
        pages.push(next);
        let mut result = Vec::new();
        for page in pages {
            if page < snapshot.pages.len() {
                for (index, frame) in crate::pages::PageFactory::body_frames(snapshot, page) {
                    if page == source.page && index < anchor {
                        continue;
                    }
                    let Some(reference) = catalog.reference(index) else {
                        continue;
                    };
                    let Some(section) = sections.get(reference.provenance.section) else {
                        continue;
                    };
                    if frame.rect.height <= Length::ZERO
                        || (section.columns.is_some() && frame.rect.width < Length::from_pt(1))
                    {
                        continue;
                    }
                    let ti = section
                        .template
                        .frames
                        .iter()
                        .position(|f| f.name == frame.name)
                        .unwrap_or(0);
                    result.push(Candidate {
                        page,
                        index,
                        ti,
                        frame: crate::template::ResolvedFrame::from_layout(frame),
                        template: section.template.clone(),
                        owner: section.id(),
                    });
                }
            } else if let Some(section) = sections.get(reference.provenance.section) {
                for ti in section.template.main_thread() {
                    if let Some(frame) = section.template.frames.get(ti)
                        && let Some(index) = crate::pages::PageFactory::preview(
                            &section.template,
                            snapshot,
                            page,
                            ti,
                        )
                    {
                        result.push(Candidate {
                            page,
                            index,
                            ti,
                            frame: frame.clone(),
                            template: section.template.clone(),
                            owner: section.id(),
                        });
                    }
                }
            }
        }
        result
    }
}
