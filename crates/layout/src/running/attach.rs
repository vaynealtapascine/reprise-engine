//! Making a page's running regions: choose the variant of each slot, compose
//! its story, take the reservation from the body, and add the region frames
//! and instances to the snapshot (sections §9).
//!
//! Everything here is a function of the page's owner, its position and the
//! stories, so the factory can do it when it makes the page, and a replay can
//! do it again and get the same answer.

use super::compose::{self, Composed, Request, Shared};
use super::{Budget, PageFacts, Region, SLOTS, SlotId, Variant, role_of, select};
use crate::regions::Plan;
use crate::sections::{Section, Table};
use crate::template::ResolvedTemplate;
use crate::{Diagnostic, Engine, FrameLayout, LayoutSnapshot, PageKind, RunningInstance, Subject};
use reprise_diag::{Code, Severity};
use reprise_doc::sections::SectionId;
use reprise_doc::{Document, FrameRole, MAIN_FLOW, NodeId, WritingMode};
use reprise_geom::{FrameSpace, Length, Matrix, PageSpace, Rect, Transform};
use std::rc::Rc;
use std::sync::Arc;

/// What composing regions needs from the layout in progress.
#[derive(Clone, Copy)]
pub(crate) struct Ctx<'a> {
    pub engine: &'a Engine,
    pub doc: &'a Document,
    pub plan: &'a Plan,
    pub shared: &'a Shared,
}

struct Part {
    slot: SlotId,
    variant: Variant,
    story: NodeId,
    rect: Rect<PageSpace>,
    composed: Rc<Composed>,
    /// Bottom-aligned content (a footer) is shifted down inside its frame.
    shift: Length,
    clipped: bool,
}

/// A page's regions, composed but not yet in the snapshot.
pub(crate) struct PageRegions {
    /// The owner's template with its main frames made room for, when that
    /// changes anything.
    pub template: Option<Arc<ResolvedTemplate>>,
    /// The extra top and bottom margin the regions took from the body.
    pub reserve: (Length, Length),
    parts: Vec<Part>,
    notes: Vec<Diagnostic>,
}

/// The first Content page of `section` is its first owned page.
pub(crate) fn first_owned(snapshot: &LayoutSnapshot, section: SectionId, kind: PageKind) -> bool {
    kind == PageKind::Content
        && snapshot
            .sections
            .as_ref()
            .and_then(|s| {
                s.pages
                    .iter()
                    .rev()
                    .find(|p| p.kind == PageKind::Content)
                    .map(|p| p.section)
            })
            .is_none_or(|last| last != section)
}

fn diagnostic(severity: Severity, code: &'static str, subject: Subject, why: String) -> Diagnostic {
    Diagnostic::new(severity, Code::new(code), subject, why)
}

fn subject_of(section: SectionId) -> Subject {
    match section {
        SectionId::First => Subject::Document,
        SectionId::Block(node) => Subject::Node(node),
    }
}

/// Composes the regions of a page that `section` (index `index` in `table`)
/// owns. `page` is its 0-based physical index; `first_owned` says whether it
/// is the section's first content page.
pub(crate) fn prepare(
    ctx: &Ctx<'_>,
    table: &Table,
    index: usize,
    section: &Section,
    page: usize,
    kind: PageKind,
    first_owned: bool,
) -> Option<PageRegions> {
    let bindings = table.running.of(index)?;
    if !table.running.any() {
        return None;
    }
    let filler = kind == PageKind::Filler;
    if filler && !section.filler_running {
        return None;
    }
    let facts = PageFacts {
        index: page,
        first_owned,
        use_first: section.region_first,
        use_even: section.region_even,
        filler,
    };
    let d = section.geometry.dimensions;
    let (width, height) = (section.template.width, section.template.height);
    let budget = Budget::new(
        d.height,
        d.top,
        d.bottom,
        section.geometry.header_distance,
        section.geometry.footer_distance,
    );
    let (mut top, mut bottom) = (budget.top, budget.bottom);
    let mut parts = Vec::new();
    let mut notes = Vec::new();
    for (slot_index, spec) in SLOTS.iter().enumerate() {
        let slot = SlotId(u16::try_from(slot_index).unwrap_or(u16::MAX));
        let variant = select(bindings, slot_index, &facts);
        let (effective, from) = bindings.get(slot_index, variant);
        let story = match effective {
            super::Effective::Unset | super::Effective::None => continue,
            super::Effective::Missing(raw) => {
                notes.push(diagnostic(
                    Severity::Warning,
                    "layout.running-missing",
                    subject_of(from),
                    format!(
                        "region.{}.{}: {raw} is not a live story; the region is empty",
                        spec.name,
                        variant.token()
                    ),
                ));
                continue;
            }
            super::Effective::Story(story) => *story,
        };
        let inline = (width - d.left - d.right).max(Length::ZERO);
        let (frame_width, cap) = match spec.region {
            Region::HeaderBand => (inline, budget.header_cap()),
            Region::FooterBand => (inline, budget.footer_cap(top)),
            Region::Page => (width, height),
        };
        let composed = ctx.shared.compose(&Request {
            engine: ctx.engine,
            doc: ctx.doc,
            plan: ctx.plan,
            story,
            slot,
            page,
            pages: page.saturating_add(1),
            width: frame_width,
            depth: cap,
            page_size: (width, height),
        });
        let used = composed.used.min(cap);
        let mut capped = false;
        let depth = match spec.region {
            Region::HeaderBand => {
                let need = budget.need(budget.header_distance, used, composed.empty);
                top = budget.top_for(need);
                capped = need > top;
                if capped { cap } else { used }
            }
            Region::FooterBand => {
                let need = budget.need(budget.footer_distance, used, composed.empty);
                bottom = budget.bottom_for(top, need);
                capped = need > bottom;
                if capped { cap } else { used }
            }
            Region::Page => height,
        };
        let rect = super::region_rect(
            spec.region,
            width,
            height,
            d.left,
            d.right,
            match spec.region {
                Region::HeaderBand => section.geometry.header_distance,
                Region::FooterBand => section.geometry.footer_distance,
                Region::Page => Length::ZERO,
            },
            depth,
        );
        let shift = if spec.region == Region::FooterBand {
            (depth - used).max(Length::ZERO)
        } else {
            Length::ZERO
        };
        let clipped = composed.clipped || composed.used > cap || (capped && composed.used > depth);
        if clipped {
            notes.push(diagnostic(
                Severity::Error,
                "layout.running-clipped",
                Subject::Node(story),
                format!(
                    "{} region ({}): content does not fit its frame and was omitted",
                    spec.name,
                    variant.token()
                ),
            ));
        }
        for node in &composed.unsupported {
            notes.push(diagnostic(
                Severity::Error,
                "layout.story-unsupported",
                Subject::Node(*node),
                "floats, footnotes and relation-placed blocks are not laid out in a story".into(),
            ));
        }
        notes.extend(composed.notes.iter().cloned());
        parts.push(Part {
            slot,
            variant,
            story,
            rect,
            composed,
            shift,
            clipped,
        });
    }
    let reserve = (
        length(top - i64::from(d.top.0)),
        length(bottom - i64::from(d.bottom.0)),
    );
    let template = (!filler && (reserve.0 > Length::ZERO || reserve.1 > Length::ZERO))
        .then(|| {
            cut(
                &section.template,
                height,
                d.top + reserve.0,
                height - d.bottom - reserve.1,
            )
        })
        .flatten()
        .map(Arc::new);
    if parts.is_empty() && notes.is_empty() && template.is_none() {
        return None;
    }
    Some(PageRegions {
        template,
        reserve,
        parts,
        notes,
    })
}

fn length(v: i64) -> Length {
    super::length(v)
}

impl PageRegions {
    /// Adds the region frames, instances and notes of `page` to the snapshot.
    /// The body frames are the template's, made by the caller.
    pub fn install(self, snapshot: &mut LayoutSnapshot, page: usize) {
        let mut instances = Vec::new();
        for part in self.parts {
            let role = role_of(part.slot);
            let index = snapshot.frames.len();
            snapshot.frames.push(FrameLayout {
                name: role.clone(),
                role: FrameRole::Flow(role),
                page,
                to_page: Transform::new(Matrix::translate(part.rect.origin.x, part.rect.origin.y)),
                rect: Rect::new(
                    reprise_geom::Point::origin(),
                    part.rect.width,
                    part.rect.height,
                ),
                writing_mode: WritingMode::HorizontalTb,
            });
            let instance = crate::FieldInstance::Running {
                page: u32::try_from(page).unwrap_or(u32::MAX),
                slot: part.slot,
                repeat: 0,
            };
            instances.push(RunningInstance {
                page,
                slot: part.slot.token().into(),
                variant: part.variant.token().into(),
                story: part.story,
                frame: index,
                blocks: compose::place(&part.composed.blocks, index, instance, part.shift),
                clipped: part.clipped,
            });
        }
        let sections = snapshot
            .sections
            .get_or_insert_with(crate::SectionsLayout::default);
        sections.running.extend(instances);
        sections.notes.extend(self.notes);
    }
}

/// `template` with its main frames limited to the physical band
/// `top..bottom` of a page `height` high. Only frames whose block axis is the
/// page's vertical (horizontal text) or whose inline axis is (vertical text)
/// are limited: a rotated, mirrored or shaped spatial frame keeps its authored
/// geometry, and the region simply paints over it.
pub(crate) fn cut(
    template: &ResolvedTemplate,
    height: Length,
    top: Length,
    bottom: Length,
) -> Option<ResolvedTemplate> {
    let unit = Length(1024);
    let bottom = bottom.min(height);
    let mut out = template.clone();
    let mut changed = false;
    for frame in &mut out.frames {
        if frame.role != FrameRole::Flow(MAIN_FLOW.into()) {
            continue;
        }
        let m = frame.to_page();
        let m = m.matrix();
        let (ox, oy) = m.apply(Length::ZERO, Length::ZERO);
        let (ix, iy) = m.apply(unit, Length::ZERO);
        let (bx, by) = m.apply(Length::ZERO, unit);
        let (ix, iy, bx, by) = (ix - ox, iy - oy, bx - ox, by - oy);
        let vertical_block =
            ix.0.abs() == unit.0 && iy == Length::ZERO && bx == Length::ZERO && by == unit;
        let vertical_inline =
            iy == unit && ix == Length::ZERO && by == Length::ZERO && bx.0.abs() == unit.0;
        let (start, extent) = if vertical_block {
            (oy, frame.depth)
        } else if vertical_inline {
            (oy, frame.width)
        } else {
            continue;
        };
        let end = start + extent;
        let new_start = start.max(top);
        let new_end = end.min(bottom).max(new_start);
        if new_start == start && new_end == end {
            continue;
        }
        changed = true;
        frame.y = frame.y + (new_start - start);
        let extent = new_end - new_start;
        if vertical_block {
            frame.depth = extent;
        } else {
            frame.width = extent;
        }
    }
    changed.then_some(out)
}

#[allow(dead_code)]
fn _types(_: Rect<FrameSpace>) {}
