//! Staged region feedback (26). A plan is derived and never written to the doc.
use crate::flow;
use crate::relations::resolve::Resolver;
use crate::template::ResolvedTemplate;
use crate::{BlockLayout, Diagnostic, Engine, LayoutSnapshot, LineRef, Resolution, Subject, codes};
use reprise_diag::Severity;
use reprise_doc::relation::builtin;
use reprise_doc::{BlockKind, Document, Domain, NodeId, Relation, RelationId, ResolutionContext};
use reprise_geom::{FrameSpace, Length, Rect};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_REGION_ITERATIONS: usize = 16;
pub const MAX_NOTE_DEPTH: usize = 32;
pub const MAX_REGION_RELATIONS: usize = 4096;
mod physical;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub exclusions: BTreeMap<crate::pages::FrameAddr, crate::pages::Entry<Vec<Rect<FrameSpace>>>>,
    pub reservations: BTreeMap<crate::pages::FrameAddr, crate::pages::Entry<Length>>,
    pub physical_exclusions: physical::Boxes,
    pub note_areas: physical::Areas,
    pub placement_owners: BTreeMap<RelationId, NodeId>,
    pub frame_sources: BTreeMap<crate::pages::FrameAddr, (crate::pages::Provenance, usize)>,
    pub blocks: Vec<BlockLayout>,
    pub applied: BTreeSet<RelationId>,
    pub pages: usize,
    pub page_requests: BTreeMap<usize, (reprise_doc::sections::SectionId, crate::PageKind)>,
    pub diagnostics: Vec<Diagnostic>,
    /// Field texts a finished pass resolved, for the next pass to compose
    /// with (see `docs/inline.md`). Derived; never stored in the document.
    pub fields: crate::inline::FieldValues,
}

impl Plan {
    pub(crate) fn equivalent(&self, other: &Self) -> bool {
        self.regions_equivalent(other) && self.fields == other.fields
    }

    fn regions_equivalent(&self, other: &Self) -> bool {
        self.exclusions == other.exclusions
            && self.reservations == other.reservations
            && self.physical_exclusions == other.physical_exclusions
            && self.note_areas == other.note_areas
            && self.canonical_blocks() == other.canonical_blocks()
            && self
                .frame_sources
                .iter()
                .map(|(a, (p, _))| (a, p))
                .eq(other.frame_sources.iter().map(|(a, (p, _))| (a, p)))
            && self.applied == other.applied
            && self.pages == other.pages
            && self.page_requests == other.page_requests
    }
    pub fn exclusions_for(&self, frame: &crate::pages::FrameRef) -> Option<&Vec<Rect<FrameSpace>>> {
        self.exclusions.get(&frame.address)?.for_frame(frame)
    }
    pub fn reservation_for(&self, frame: &crate::pages::FrameRef) -> Option<Length> {
        self.reservations
            .get(&frame.address)?
            .for_frame(frame)
            .copied()
    }
    pub fn exclude(&mut self, frame: &crate::pages::FrameRef, rect: Rect<FrameSpace>) {
        let entry = self
            .exclusions
            .entry(frame.address.clone())
            .or_insert_with(|| crate::pages::Entry::new(frame, Vec::new()));
        if entry.provenance != frame.provenance {
            *entry = crate::pages::Entry::new(frame, Vec::new());
        }
        entry.value.push(rect);
    }
    pub fn reserve(&mut self, frame: &crate::pages::FrameRef, depth: Length) {
        self.reservations.insert(
            frame.address.clone(),
            crate::pages::Entry::new(frame, depth),
        );
    }
    fn capture_frames(&mut self, snapshot: &LayoutSnapshot) {
        let catalog = crate::pages::Catalog::from_snapshot(snapshot);
        for block in &self.blocks {
            for line in &block.lines {
                if let Some(frame) = catalog.reference(line.frame) {
                    self.frame_sources.insert(
                        frame.address.clone(),
                        (frame.provenance.clone(), line.frame),
                    );
                }
            }
        }
    }
    fn canonical_blocks(&self) -> Vec<BlockLayout> {
        let indices: BTreeMap<_, _> = self
            .frame_sources
            .iter()
            .enumerate()
            .map(|(canonical, (_, (_, index)))| (*index, canonical))
            .collect();
        let mut blocks = self.blocks.clone();
        for block in &mut blocks {
            for line in &mut block.lines {
                if let Some(&index) = indices.get(&line.frame) {
                    line.frame = index;
                }
            }
            block.sync_image();
        }
        blocks
    }
    fn blocks_for(&self, snapshot: &LayoutSnapshot) -> Vec<BlockLayout> {
        let current = crate::pages::Catalog::from_snapshot(snapshot);
        let indices: BTreeMap<_, _> = self
            .frame_sources
            .iter()
            .filter_map(|(address, (provenance, old))| {
                let index = current.index(address)?;
                (current.reference(index)?.provenance == *provenance).then_some((*old, index))
            })
            .collect();
        self.blocks
            .iter()
            .filter_map(|block| {
                let mut block = block.clone();
                for line in &mut block.lines {
                    line.frame = *indices.get(&line.frame)?;
                }
                block.sync_image();
                Some(block)
            })
            .collect()
    }
    pub(crate) fn applied_for(&self, snapshot: &LayoutSnapshot) -> BTreeSet<RelationId> {
        let valid: BTreeSet<_> = self.blocks_for(snapshot).iter().map(|b| b.node).collect();
        // A relation owner is unique in one region allocation. Invalid frame
        // provenance must not suppress the ordinary unplaced-owner diagnostic.
        self.applied
            .iter()
            .filter(|id| {
                self.placement_owners
                    .get(id)
                    .is_some_and(|n| valid.contains(n))
            })
            .copied()
            .collect()
    }
}

pub(crate) fn owners(engine: &Engine, doc: &Document) -> BTreeSet<NodeId> {
    doc.relations()
        .into_iter()
        .filter_map(|(_, r)| {
            let r = r.ok()?;
            (is_region(&r) && engine.schemas.get(&r.schema).is_some())
                .then_some(r.owner)
                .flatten()
        })
        .collect()
}

fn is_region(r: &Relation) -> bool {
    r.schema == builtin::FLOAT || r.schema == builtin::NOTE
}

/// Only region-bearing documents iterate; old snapshots keep their exact output.
pub(crate) fn run(
    engine: &Engine,
    doc: &Document,
    template: &ResolvedTemplate,
    mut base: LayoutSnapshot,
) -> LayoutSnapshot {
    let sections = crate::sections::Table::resolve(engine, doc, template);
    base.diagnostics.extend(sections.diagnostics.clone());
    let running = std::rc::Rc::new(crate::running::compose::Shared::default());
    let active = active(engine, doc);
    let mut plan = Plan::default();
    let mut history: Vec<Plan> = Vec::new();
    for iteration in 0..MAX_REGION_ITERATIONS {
        let mut snapshot = base.clone();
        let pending = flow::run_with_sections(
            engine,
            doc,
            template,
            &mut snapshot,
            &plan,
            sections.clone(),
            running.clone(),
        );
        if !active {
            crate::relations::run(engine, doc, &mut snapshot, pending, &plan.applied);
            sections.sort_diagnostics(&mut snapshot);
            return snapshot;
        }
        let next = allocate_with_sections(
            engine, doc, template, &snapshot, None, &sections, &plan, &running,
        );
        let outcome = feedback(&plan, &next, &history, iteration);
        if outcome != Feedback::Continue {
            let applied = if outcome == Feedback::Converged {
                next.applied_for(&snapshot)
            } else {
                plan.applied_for(&snapshot)
            };
            finish_feedback(&mut snapshot, &plan, next, outcome);
            crate::inline::field_notes(doc, &mut snapshot);
            crate::relations::run(engine, doc, &mut snapshot, pending, &applied);
            sections.sort_diagnostics(&mut snapshot);
            return snapshot;
        }
        history.push(plan);
        plan = next;
    }
    base
}

/// Region relations and inline objects (whose fields depend on pagination)
/// take part in feedback.
pub(crate) fn active(engine: &Engine, doc: &Document) -> bool {
    doc.has_inline_objects()
        || doc.relations().iter().any(|(_, r)| {
            r.as_ref()
                .is_ok_and(|r| is_region(r) && engine.schemas.get(&r.schema).is_some())
        })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Feedback {
    Continue,
    Converged,
    Oscillated,
    Exhausted,
}
pub(crate) fn feedback(plan: &Plan, next: &Plan, history: &[Plan], iteration: usize) -> Feedback {
    if next.equivalent(plan) {
        Feedback::Converged
    } else if history.iter().any(|p| next.equivalent(p)) {
        Feedback::Oscillated
    } else if iteration.saturating_add(1) >= MAX_REGION_ITERATIONS {
        Feedback::Exhausted
    } else {
        Feedback::Continue
    }
}
pub(crate) fn finish_feedback(
    snapshot: &mut LayoutSnapshot,
    plan: &Plan,
    next: Plan,
    outcome: Feedback,
) {
    if outcome == Feedback::Converged {
        snapshot.blocks.extend(next.blocks_for(snapshot));
        snapshot.diagnostics.extend(next.diagnostics);
    } else {
        let fields = plan.fields != next.fields;
        snapshot.blocks.extend(plan.blocks_for(snapshot));
        snapshot.diagnostics.extend(plan.diagnostics.clone());
        if !plan.regions_equivalent(&next) || !fields {
            snapshot.diagnostics.push(Diagnostic::new(Severity::Warning,
                if outcome == Feedback::Oscillated { codes::REGION_CYCLE } else { codes::REGION_LIMIT },
                Subject::Document, "region feedback did not converge; last complete input allocation frozen, anchors may differ"));
        }
        if fields {
            snapshot.diagnostics.push(Diagnostic::new(
                Severity::Warning,
                codes::FIELD_UNSETTLED,
                Subject::Document,
                "field text and pagination did not settle; the last complete pass is published",
            ));
        }
    }
}

#[cfg(test)]
pub(crate) fn allocate(
    engine: &Engine,
    doc: &Document,
    template: &ResolvedTemplate,
    flowed: &LayoutSnapshot,
    evaluation: Option<&crate::incremental::Evaluation>,
    fields: &crate::inline::FieldValues,
) -> Plan {
    let sections = crate::sections::Table::resolve(engine, doc, template);
    let previous = Plan {
        fields: fields.clone(),
        ..Plan::default()
    };
    allocate_with_sections(
        engine,
        doc,
        template,
        flowed,
        evaluation,
        &sections,
        &previous,
        &crate::running::compose::Shared::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn allocate_with_sections(
    engine: &Engine,
    doc: &Document,
    template: &ResolvedTemplate,
    flowed: &LayoutSnapshot,
    evaluation: Option<&crate::incremental::Evaluation>,
    sections: &crate::sections::Table,
    previous: &Plan,
    running: &crate::running::compose::Shared,
) -> Plan {
    let fields = &previous.fields;
    let ctx = (sections.enabled && sections.running.any()).then_some(crate::running::attach::Ctx {
        engine,
        doc,
        plan: previous,
        shared: running,
        filler_regions: false,
    });
    let mut scratch = flowed.clone();
    let mut out = Plan::default();
    let mut relations: Vec<_> = doc
        .relations()
        .into_iter()
        .filter_map(|(id, r)| {
            r.ok()
                .filter(|r| is_region(r) && engine.schemas.get(&r.schema).is_some())
                .map(|r| (id, r))
        })
        .collect();
    // This allocation places the body's regions. A live owner outside the body
    // domain (any story, bound or not) is not the body's to place, and must
    // neither crowd out the body's relations nor register as one of its owners.
    relations.retain(|(id, r)| {
        let outside = r
            .owner
            .is_some_and(|o| doc.block(o).is_ok() && doc.domain_of(o) != Some(Domain::Body));
        if outside {
            out.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::RELATION_OWNER,
                Subject::Relation(*id),
                "region owner must be a body annotation or image; this one belongs to a story",
            ));
        }
        !outside
    });
    if relations.len() > MAX_REGION_RELATIONS {
        out.diagnostics.push(Diagnostic::new(
            Severity::Error,
            codes::REGION_LIMIT,
            Subject::Document,
            "more than 4096 region relations; remainder omitted",
        ));
        relations.truncate(MAX_REGION_RELATIONS);
    }
    // Body-anchored floats allocate before notes so notes leave room for their
    // boxes. Stable IDs order each class.
    relations.sort_by_key(|(id, r)| (r.schema != builtin::FLOAT, *id));
    let mut owners = BTreeSet::new();
    let mut depths: BTreeMap<NodeId, usize> = BTreeMap::new();
    relations.retain(|(id, r)| {
        let Some(owner) = r.owner else { return false };
        if doc.block(owner).is_err() {
            return false;
        }
        if !owners.insert(owner)
            || !matches!(
                doc.kind_of(owner),
                Some(BlockKind::Annotation | BlockKind::Image)
            )
            || flowed.block(owner).is_some()
        {
            out.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::RELATION_OWNER,
                Subject::Relation(*id),
                "region owner must be a unique annotation or image",
            ));
            return false;
        }
        true
    });
    let mut resolver = Resolver::new(doc);
    let mut floats = crate::floats::Floats::default();
    let mut notes = crate::notes::Notes::default();
    // Topological rounds also handle relations in reverse ID order. No recursion.
    for _ in 0..MAX_NOTE_DEPTH {
        let mut waiting = Vec::new();
        let mut progressed = false;
        for (id, r) in relations {
            let Some(schema) = engine.schemas.get(&r.schema) else {
                continue;
            };
            let resolved = resolver.resolve(&scratch, id, schema, &r);
            let anchor = resolved
                .unique("anchor")
                .and_then(|a| anchor_line(&scratch, a));
            let Some(anchor) = anchor else {
                waiting.push((id, r));
                continue;
            };
            let depth = depths
                .get(&anchor.node)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            if depth > MAX_NOTE_DEPTH {
                out.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    codes::NOTE_DEPTH,
                    Subject::Relation(id),
                    "region nesting exceeds 32 levels; owner omitted",
                ));
                progressed = true;
                continue;
            }
            if let Some(owner) = r.owner {
                depths.insert(owner, depth);
            }
            if r.schema == builtin::FLOAT {
                floats.place(
                    engine,
                    doc,
                    template,
                    &mut scratch,
                    &mut out,
                    id,
                    &r,
                    anchor,
                    evaluation,
                    sections,
                    fields,
                    ctx.as_ref(),
                );
            } else {
                notes.place(
                    engine,
                    doc,
                    template,
                    &mut scratch,
                    &mut out,
                    id,
                    &r,
                    anchor,
                    evaluation,
                    sections,
                    fields,
                    ctx.as_ref(),
                );
            }
            progressed = true;
        }
        relations = waiting;
        if relations.is_empty() || !progressed {
            break;
        }
    }
    if !relations.is_empty() {
        for (id, _) in relations {
            out.diagnostics.push(Diagnostic::new(Severity::Error, codes::NOTE_DEPTH, Subject::Relation(id), "unresolved region dependency, cycle, missing anchor or nesting beyond 32 rounds; owner omitted"));
        }
    }
    notes.finish(&scratch, &mut out);
    // What the next pass composes fields with: the body's and the placed
    // notes' and floats' own, which `scratch` now holds too.
    out.fields = crate::inline::next_fields(doc, &scratch, fields);
    out.pages = out
        .blocks
        .iter()
        .flat_map(|b| &b.lines)
        .filter_map(|l| scratch.frame(l.frame))
        .map(|f| f.page.saturating_add(1))
        .max()
        .unwrap_or(0);
    if let Some(metadata) = &scratch.sections {
        out.page_requests.extend(
            metadata
                .pages
                .iter()
                .enumerate()
                .map(|(page, p)| (page, (p.section, p.kind))),
        );
    }
    out.capture_frames(&scratch);
    for (id, relation) in doc.relations() {
        if let Ok(relation) = relation
            && let Some(owner) = relation.owner
            && out.applied.contains(&id)
        {
            out.placement_owners.insert(id, owner);
        }
    }
    out
}

fn anchor_line(snapshot: &LayoutSnapshot, resolution: &Resolution) -> Option<LineRef> {
    match resolution {
        Resolution::Range { node, bytes } => snapshot.line_containing(*node, bytes.start),
        Resolution::Line(l) => Some(*l),
        Resolution::Lines(ls) if ls.len() == 1 => ls.first().copied(),
        _ => None,
    }
}

pub(crate) fn ensure_page(
    engine: &Engine,
    template: &ResolvedTemplate,
    snapshot: &mut LayoutSnapshot,
    page: usize,
) -> bool {
    crate::pages::PageFactory::ensure(engine, template, snapshot, page)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare(
    engine: &Engine,
    doc: &Document,
    template: &ResolvedTemplate,
    node: NodeId,
    template_index: usize,
    width: Length,
    diagnostics: &mut Vec<Diagnostic>,
    evaluation: Option<&crate::incremental::Evaluation>,
) -> Option<flow::Prepared> {
    let ctx: ResolutionContext =
        flow::resolution_context(engine, template, template.frames.get(template_index), width);
    flow::prepare_cached(engine, doc, node, &ctx, diagnostics, evaluation)
}
