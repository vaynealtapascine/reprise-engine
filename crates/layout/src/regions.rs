//! Staged region feedback (26). A plan is derived and never written to the doc.
use crate::flow;
use crate::relations::resolve::Resolver;
use crate::template::ResolvedTemplate;
use crate::{
    BlockLayout, Diagnostic, Engine, FrameLayout, LayoutSnapshot, LineRef, PageLayout, Resolution,
    Subject, codes,
};
use reprise_diag::Severity;
use reprise_doc::relation::builtin;
use reprise_doc::{BlockKind, Document, NodeId, Relation, RelationId, ResolutionContext};
use reprise_geom::{FrameSpace, Length, Rect};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_REGION_ITERATIONS: usize = 16;
pub const MAX_NOTE_DEPTH: usize = 32;
pub const MAX_REGION_RELATIONS: usize = 4096;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub exclusions: BTreeMap<usize, Vec<Rect<FrameSpace>>>,
    pub reservations: BTreeMap<usize, Length>,
    pub blocks: Vec<BlockLayout>,
    pub applied: BTreeSet<RelationId>,
    pub pages: usize,
    pub diagnostics: Vec<Diagnostic>,
}

impl Plan {
    fn equivalent(&self, other: &Self) -> bool {
        self.exclusions == other.exclusions
            && self.reservations == other.reservations
            && self.blocks == other.blocks
            && self.applied == other.applied
            && self.pages == other.pages
    }
}

pub(crate) fn owned(engine: &Engine, doc: &Document, node: NodeId) -> bool {
    doc.relations().into_iter().any(|(_, r)| {
        r.is_ok_and(|r| {
            r.owner == Some(node) && is_region(&r) && engine.schemas.get(&r.schema).is_some()
        })
    })
}

fn is_region(r: &Relation) -> bool {
    r.schema == builtin::FLOAT || r.schema == builtin::NOTE
}

/// Only region-bearing documents iterate; old snapshots keep their exact output.
pub(crate) fn run(
    engine: &Engine,
    doc: &Document,
    template: &ResolvedTemplate,
    base: LayoutSnapshot,
) -> LayoutSnapshot {
    let active = doc.relations().iter().any(|(_, r)| {
        r.as_ref()
            .is_ok_and(|r| is_region(r) && engine.schemas.get(&r.schema).is_some())
    });
    let mut plan = Plan::default();
    let mut history: Vec<Plan> = Vec::new();
    for iteration in 0..MAX_REGION_ITERATIONS {
        let mut snapshot = base.clone();
        let pending = flow::run(engine, doc, template, &mut snapshot, &plan);
        if !active {
            crate::relations::run(engine, doc, &mut snapshot, pending, &plan.applied);
            return snapshot;
        }
        let next = allocate(engine, doc, template, &snapshot);
        if next.equivalent(&plan) {
            snapshot.blocks.extend(next.blocks);
            snapshot.diagnostics.extend(next.diagnostics);
            crate::relations::run(engine, doc, &mut snapshot, pending, &next.applied);
            return snapshot;
        }
        let oscillates = history.iter().any(|p| next.equivalent(p));
        if oscillates || iteration + 1 == MAX_REGION_ITERATIONS {
            // Freeze the INPUT plan: this flow was composed with precisely its
            // exclusions and reservations. Never publish a mismatched pair.
            snapshot.blocks.extend(plan.blocks);
            snapshot.diagnostics.extend(plan.diagnostics);
            snapshot.diagnostics.push(Diagnostic::new(Severity::Warning, if oscillates { codes::REGION_CYCLE } else { codes::REGION_LIMIT }, Subject::Document, "region feedback did not converge; last complete input allocation frozen, anchors may differ"));
            crate::relations::run(engine, doc, &mut snapshot, pending, &plan.applied);
            return snapshot;
        }
        history.push(plan);
        plan = next;
    }
    base
}

fn allocate(
    engine: &Engine,
    doc: &Document,
    template: &ResolvedTemplate,
    flowed: &LayoutSnapshot,
) -> Plan {
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
            || doc.kind_of(owner) != Some(BlockKind::Annotation)
            || flowed.block(owner).is_some()
        {
            out.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::RELATION_OWNER,
                Subject::Relation(*id),
                "region owner must be a unique annotation",
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
    notes.finish(template, &mut out);
    out.pages = out
        .blocks
        .iter()
        .flat_map(|b| &b.lines)
        .filter_map(|l| scratch.frame(l.frame))
        .map(|f| f.page.saturating_add(1))
        .max()
        .unwrap_or(0);
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
    if page >= engine.flow.max_pages.max(1) as usize {
        return false;
    }
    while snapshot.pages.len() <= page {
        let page = snapshot.pages.len();
        snapshot.pages.push(PageLayout {
            width: template.width,
            height: template.height,
        });
        for frame in &template.frames {
            snapshot.frames.push(FrameLayout {
                name: frame.name.clone(),
                role: frame.role.clone(),
                page,
                to_page: frame.to_page(),
                rect: frame.rect(),
            });
        }
    }
    true
}

pub(crate) fn prepare(
    engine: &Engine,
    doc: &Document,
    template: &ResolvedTemplate,
    node: NodeId,
    template_index: usize,
    width: Length,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<flow::Prepared> {
    let ctx: ResolutionContext =
        flow::resolution_context(engine, template, template.frames.get(template_index), width);
    flow::prepare(engine, doc, node, &ctx, diagnostics)
}
