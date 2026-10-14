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
        sections: &crate::sections::Table,
        fields: &crate::inline::FieldValues,
    ) {
        let Some(owner) = relation.owner else { return };
        let Some(anchor_line) = scratch.line(anchor).cloned() else {
            return;
        };
        let Some(anchor_frame) = scratch.frame(anchor_line.frame).cloned() else {
            return;
        };
        let anchor_owner = scratch
            .sections
            .as_ref()
            .and_then(|s| s.pages.get(anchor_frame.page))
            .map(|p| p.section);
        let placement_hint = match relation.params.get("placement") {
            Some(Param::Text(p)) => NotePlacement::parse(p),
            _ => None,
        };
        let initial_owner = if placement_hint == Some(NotePlacement::End) {
            let last = scratch
                .blocks
                .iter()
                .flat_map(|b| &b.lines)
                .filter_map(|l| scratch.frame(l.frame))
                .filter(|f| matches!(&f.role, FrameRole::Flow(_)))
                .map(|f| f.page)
                .max()
                .unwrap_or(0);
            scratch
                .sections
                .as_ref()
                .and_then(|s| s.pages.get(last))
                .map(|p| p.section)
        } else {
            anchor_owner
        };
        let section_template = initial_owner
            .and_then(|id| sections.get(id))
            .map(|s| s.template.clone());
        let template = section_template.as_deref().unwrap_or(template);
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
        let mut area = area.clone();
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
        let mut carry_owner = if placement == NotePlacement::End {
            scratch
                .sections
                .as_ref()
                .and_then(|s| s.pages.get(page.saturating_sub(1)))
                .map(|p| p.section)
        } else {
            anchor_owner
        };

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
        loop {
            if sections.enabled
                && scratch
                    .sections
                    .as_ref()
                    .and_then(|s| s.pages.get(page))
                    .is_some_and(|p| p.kind == crate::PageKind::Filler)
            {
                page = page.saturating_add(1);
                continue;
            }
            let allocated = if sections.enabled {
                carry_owner.is_some_and(|owner| {
                    crate::pages::PageFactory::ensure_section(
                        engine,
                        sections,
                        owner,
                        crate::PageKind::NoteContinuation,
                        scratch,
                        page,
                    )
                })
            } else {
                ensure_page(engine, template, scratch, page)
            };
            if !allocated {
                break;
            }
            if sections.enabled {
                carry_owner = scratch
                    .sections
                    .as_ref()
                    .and_then(|s| s.pages.get(page))
                    .map(|p| p.section);
            }
            let index = if sections.enabled {
                scratch
                    .frames
                    .iter()
                    .enumerate()
                    .find(|(_, f)| {
                        f.page == page && f.role == FrameRole::Notes && f.rect.height > Length::ZERO
                    })
                    .map(|(i, _)| i)
            } else {
                crate::pages::PageFactory::index(scratch, page, ti)
            };
            let Some(index) = index else {
                break;
            };
            if sections.enabled
                && let Some(frame) = scratch.frame(index)
            {
                area = crate::template::ResolvedFrame::from_layout(frame);
            }
            let previous = self.used.get(&index).copied().unwrap_or_default();
            let used = previous
                + if previous > Length::ZERO {
                    engine.flow.annotation_spacing.max(Length::ZERO)
                } else {
                    Length::ZERO
                };
            let mut capacity = area.depth;
            if let Some(inverse) = area.to_page().inverse() {
                let catalog = crate::pages::Catalog::from_snapshot(scratch);
                for (float_frame, entry) in out.exclusions.iter().chain(&out.physical_exclusions) {
                    let Some(float_index) = catalog.index(float_frame) else {
                        continue;
                    };
                    let Some(reference) = catalog.reference(float_index) else {
                        continue;
                    };
                    let Some(boxes) = entry.for_frame(reference) else {
                        continue;
                    };
                    let Some(frame) = scratch.frame(float_index).filter(|f| f.page == page) else {
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
            let overflow =
                prepared.line_height() > area.depth && page > first_page && used == Length::ZERO;
            let bounded = Bounded {
                inner: &measure,
                depth: if overflow {
                    prepared.line_height()
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
                &crate::inline::FieldEnv {
                    page,
                    pages: page.saturating_add(1),
                    instance: crate::FieldInstance::Single,
                    values: Some(fields),
                },
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
    pub fn finish(self, snapshot: &LayoutSnapshot, out: &mut Plan) {
        let catalog = crate::pages::Catalog::from_snapshot(snapshot);
        for (index, used) in self.used {
            let Some(area) = snapshot.frame(index) else {
                continue;
            };
            let occupied = used.min(area.rect.height);
            let by = (area.rect.height - occupied).max(Length::ZERO);
            for block in &mut out.blocks {
                for line in &mut block.lines {
                    if line.frame == index {
                        line.rect.origin.y += by;
                        line.baseline += by;
                        for atom in &mut line.atoms {
                            atom.rect.origin.y += by;
                            atom.baseline += by;
                        }
                    }
                }
                block.sync_image();
            }
            let page = area.page;
            if snapshot.sections.is_some() {
                if let Some(source) = catalog.reference(index) {
                    out.note_areas.insert(
                        source.address.clone(),
                        crate::pages::Entry::new(
                            source,
                            reprise_geom::Rect::new(
                                Point::new(Length::ZERO, by),
                                area.rect.width,
                                occupied,
                            ),
                        ),
                    );
                }
                continue;
            }
            // Project the occupied notes box to each body's logical space.
            // Conservatively reserve from the earliest intersecting corner.
            for (i, body) in crate::pages::PageFactory::body_frames(snapshot, page) {
                let Some(inverse) = body.to_page.inverse() else {
                    continue;
                };
                let corners = [
                    Point::new(Length::ZERO, by),
                    Point::new(area.rect.width, by),
                    Point::new(Length::ZERO, area.rect.height),
                    Point::new(area.rect.width, area.rect.height),
                ];
                let mapped: Vec<_> = corners
                    .into_iter()
                    .map(|p| inverse.apply(area.to_page.apply(p)))
                    .collect();
                let left = mapped.iter().map(|p| p.x).min().unwrap_or_default();
                let right = mapped.iter().map(|p| p.x).max().unwrap_or_default();
                let top = mapped.iter().map(|p| p.y).min().unwrap_or_default();
                let bottom = mapped.iter().map(|p| p.y).max().unwrap_or_default();
                if right > Length::ZERO
                    && left < body.rect.width
                    && bottom > Length::ZERO
                    && top < body.rect.height
                    && let Some(frame) = catalog.reference(i)
                {
                    out.reserve(frame, body.rect.height - top.max(Length::ZERO));
                }
            }
        }
    }
}
