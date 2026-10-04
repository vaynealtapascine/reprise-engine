//! Exact-input incremental evaluation and single-threaded resumable jobs.
//! See `docs/incremental.md` for the reuse proof and scheduling guarantees.
use crate::flow::{self, Cursor, Delta, PositionKey, Prepared};
use crate::regions::Plan;
use crate::template::ResolvedTemplate;
use crate::{Diagnostic, Engine, LayoutSnapshot, TemplateSource, TemplateUsed};
use reprise_doc::{
    Authored, BlockKind, Document, NodeId, Property, RangeId, RelationId, ResolutionContext,
    Revision, Style,
};
use reprise_geom::{PageSpace, Rect};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

/// A derived unit, separate from authored identities and relations.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Computation {
    Style(NodeId),
    Shape(NodeId),
    Compose(NodeId),
    ComposeInFrame { node: NodeId, frame: usize },
    Relation(RelationId),
    Regions,
    ReadingOrder,
    Page(usize),
    Line(crate::LineRef),
}
/// Unified read vocabulary. Existing doc dependency types retain their semantics.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dependency {
    Text(NodeId),
    Node(NodeId),
    Style(String),
    Range(RangeId),
    Relation(RelationId),
    LayoutOfRange(RangeId),
    LayoutOfNode(NodeId),
    History(Revision),
    Tree,
    Template,
    EngineConfiguration,
    Revision,
    Expression(reprise_doc::Dependency),
    AuthoredRelation(reprise_doc::relation::Dependency),
    Computed(Computation),
}
impl From<reprise_doc::Dependency> for Dependency {
    fn from(value: reprise_doc::Dependency) -> Self {
        Self::Expression(value)
    }
}
impl From<reprise_doc::relation::Dependency> for Dependency {
    fn from(value: reprise_doc::relation::Dependency) -> Self {
        match value {
            reprise_doc::relation::Dependency::Node(n) => Self::Node(n),
            reprise_doc::relation::Dependency::Range(r) => Self::Range(r),
            reprise_doc::relation::Dependency::Tree => Self::Tree,
            reprise_doc::relation::Dependency::LayoutOfRange(r) => Self::LayoutOfRange(r),
            reprise_doc::relation::Dependency::LayoutOfNode(n) => Self::LayoutOfNode(n),
            reprise_doc::relation::Dependency::History(r) => Self::History(r),
            other => Self::AuthoredRelation(other),
        }
    }
}
/// Deterministic transitive dependency inspection. Cycles are visited once.
#[derive(Clone, Debug, Default)]
pub struct DependencyGraph {
    reads: BTreeMap<Computation, BTreeSet<Dependency>>,
    reasons: BTreeMap<Computation, BTreeSet<Dependency>>,
}
impl DependencyGraph {
    pub fn direct(&self, unit: &Computation) -> BTreeSet<Dependency> {
        self.reads.get(unit).cloned().unwrap_or_default()
    }
    pub fn dependencies(&self, unit: &Computation) -> BTreeSet<Dependency> {
        let mut out = BTreeSet::new();
        let mut stack = vec![unit.clone()];
        let mut visited = BTreeSet::new();
        while let Some(unit) = stack.pop() {
            if !visited.insert(unit.clone()) {
                continue;
            }
            for dep in self.direct(&unit) {
                if let Dependency::Computed(next) = &dep {
                    stack.push(next.clone());
                }
                out.insert(dep);
            }
        }
        out
    }
    pub fn dependents(&self, input: &Dependency) -> Vec<Computation> {
        let mut reverse: BTreeMap<Dependency, BTreeSet<Computation>> = BTreeMap::new();
        for (unit, reads) in &self.reads {
            for read in reads {
                reverse
                    .entry(read.clone())
                    .or_default()
                    .insert(unit.clone());
            }
        }
        let mut found = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![input.clone()];
        while let Some(input) = pending.pop() {
            if !seen.insert(input.clone()) {
                continue;
            }
            for unit in reverse.get(&input).into_iter().flatten() {
                if found.insert(unit.clone()) {
                    pending.push(Dependency::Computed(unit.clone()));
                }
            }
        }
        found.into_iter().collect()
    }
    pub fn why_recomputed(&self, unit: &Computation) -> BTreeSet<Dependency> {
        self.reasons.get(unit).cloned().unwrap_or_default()
    }
    fn record(
        &mut self,
        unit: Computation,
        reads: BTreeSet<Dependency>,
        reasons: BTreeSet<Dependency>,
    ) {
        self.reads.insert(unit.clone(), reads);
        if !reasons.is_empty() {
            self.reasons.entry(unit).or_default().extend(reasons);
        }
    }
}
/// Counts actual cache misses and pass executions, never elapsed time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkCounters {
    pub units: usize,
    pub style_resolutions: usize,
    pub itemizations: usize,
    pub shapes: usize,
    /// Paragraph transition misses, including diagnosed empty/unplaced results.
    pub compositions: usize,
    /// Pages touched by paragraph transition misses, including old and new placement.
    pub reflowed_pages: BTreeSet<usize>,
    pub composer_calls: usize,
    pub reused_compositions: usize,
    pub region_passes: usize,
    pub relation_passes: usize,
    pub reading_order_passes: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Inputs {
    kind: BlockKind,
    text: String,
    overrides: Style,
    styles: Vec<(String, Option<Style>)>,
}
impl Inputs {
    fn read(doc: &Document, node: NodeId) -> Option<Self> {
        let block = doc.block(node).ok()?;
        let mut styles = Vec::new();
        let mut name = block.style.clone();
        let mut visited = BTreeSet::new();
        // Mirrors the reference resolver's 32-level boundary, including the
        // next missing/cyclic name so diagnostic changes cannot hide in reuse.
        for _ in 0..=32 {
            let Some(n) = name.take() else { break };
            let style = doc.style(&n);
            name = style.as_ref().and_then(|s| s.parent.clone());
            styles.push((n.clone(), style));
            if !visited.insert(n) {
                break;
            }
        }
        Some(Self {
            kind: block.kind,
            text: block.text.to_string(),
            overrides: block.overrides,
            styles,
        })
    }
    fn dependencies(&self, engine: &Engine, node: NodeId) -> BTreeSet<Dependency> {
        let mut out = BTreeSet::from([
            Dependency::Node(node),
            Dependency::Text(node),
            Dependency::EngineConfiguration,
        ]);
        for (name, _) in &self.styles {
            out.insert(Dependency::Style(name.clone()));
        }
        for style in self
            .styles
            .iter()
            .filter_map(|(_, s)| s.as_ref())
            .chain(std::iter::once(&self.overrides))
        {
            for prop in [Property::Size, Property::LineHeight] {
                if let Some(Authored::Expr(expr)) = style.get(prop) {
                    out.extend(
                        expr.dependencies(&engine.functions)
                            .into_iter()
                            .map(Dependency::from),
                    );
                }
            }
        }
        out
    }
    fn changes(&self, old: &Self, node: NodeId) -> BTreeSet<Dependency> {
        let mut out = BTreeSet::new();
        if self.text != old.text {
            out.insert(Dependency::Text(node));
        }
        if self.kind != old.kind || self.overrides != old.overrides {
            out.insert(Dependency::Node(node));
        }
        for (name, value) in self.styles.iter().chain(&old.styles) {
            if self.styles.iter().find(|(n, _)| n == name)
                != old.styles.iter().find(|(n, _)| n == name)
            {
                let _ = value;
                out.insert(Dependency::Style(name.clone()));
            }
        }
        out
    }
}
/// Owned directly from the complete shaping request. Exhaustive destructuring
/// forces future request fields to be considered rather than silently omitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShapingKey {
    text: String,
    styles: Vec<reprise_shape::StyleRun>,
    direction: Option<reprise_geom::InlineDirection>,
    fallback: Option<(reprise_font::FaceId, reprise_geom::Length)>,
}
impl ShapingKey {
    pub(crate) fn new(
        input: &reprise_shape::ParagraphInput<'_>,
        fallback: Option<(reprise_font::FaceId, reprise_geom::Length)>,
    ) -> Self {
        let reprise_shape::ParagraphInput {
            text,
            styles,
            direction,
        } = input;
        Self {
            text: (*text).to_owned(),
            styles: styles.to_vec(),
            direction: *direction,
            fallback,
        }
    }
}
#[derive(Clone)]
struct ShapingEntry {
    key: ShapingKey,
    value: Option<Prepared>,
    diagnostics: Vec<Diagnostic>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct PrepareKey {
    inputs: Inputs,
    context: ResolutionContext,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FlowKey {
    inputs: Option<Inputs>,
    previous: Option<NodeId>,
    position: PositionKey,
    template: ResolvedTemplate,
    // Geometry is the entire frozen input plan. It includes query results and
    // relation-driven placements, not merely the authored relation IDs.
    plan: Plan,
}
impl FlowKey {
    pub(crate) fn new(flow: &flow::Flow<'_>, doc: &Document, node: NodeId) -> Self {
        Self {
            inputs: Inputs::read(doc, node),
            previous: flow.previous,
            position: flow.position_key(),
            template: flow.template.clone(),
            plan: flow.plan().clone(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AnnotationKey {
    inputs: Option<Inputs>,
    template: ResolvedTemplate,
}
impl AnnotationKey {
    pub(crate) fn new(doc: &Document, node: NodeId, template: &ResolvedTemplate) -> Self {
        Self {
            inputs: Inputs::read(doc, node),
            template: template.clone(),
        }
    }
}
#[derive(Clone)]
struct AnnotationEntry {
    key: AnnotationKey,
    pending: Option<flow::Pending>,
    notes: Vec<Diagnostic>,
}
#[derive(Clone)]
struct PreparedEntry {
    key: PrepareKey,
    value: Option<Prepared>,
    diagnostics: Vec<Diagnostic>,
}
#[derive(Clone)]
struct FlowEntry {
    key: FlowKey,
    delta: Delta,
}
// Resolve authored inputs when deriving keys, rather than attempting to
// enumerate every tombstone/history/anchor read manually. Layout-query answers
// are a pure function of these outcomes and the exact flowed snapshot.
type RelationInput = (
    RelationId,
    Result<reprise_doc::Relation, String>,
    bool,
    Option<reprise_doc::ResolvedRelation>,
);
#[derive(Clone, Debug, PartialEq, Eq)]
struct PassKey {
    snapshot: LayoutSnapshot,
    pending: Vec<flow::Pending>,
    relations: Vec<RelationInput>,
    order: Vec<NodeId>,
    owners: Vec<(NodeId, Option<Inputs>)>,
    template: ResolvedTemplate,
    applied: BTreeSet<RelationId>,
}
impl PassKey {
    fn changes(&self, old: Option<&Self>) -> BTreeSet<Dependency> {
        let Some(old) = old else {
            return BTreeSet::from([Dependency::Revision]);
        };
        let mut changed = BTreeSet::new();
        for (id, ..) in self.relations.iter().chain(&old.relations) {
            if self.relations.iter().find(|r| r.0 == *id)
                != old.relations.iter().find(|r| r.0 == *id)
            {
                changed.insert(Dependency::Relation(*id));
            }
        }
        if self.order != old.order {
            changed.insert(Dependency::Tree);
        }
        if self.template != old.template {
            changed.insert(Dependency::Template);
        }
        for (node, input) in &self.owners {
            if let (Some(input), Some(Some(previous))) = (
                input,
                old.owners.iter().find(|(n, _)| n == node).map(|(_, v)| v),
            ) {
                changed.extend(input.changes(previous, *node));
            }
        }
        let new_blocks: BTreeMap<_, _> = self.snapshot.blocks.iter().map(|b| (b.node, b)).collect();
        let old_blocks: BTreeMap<_, _> = old.snapshot.blocks.iter().map(|b| (b.node, b)).collect();
        for node in new_blocks.keys().chain(old_blocks.keys()) {
            if new_blocks.get(node) != old_blocks.get(node) {
                changed.insert(Dependency::Computed(Computation::Compose(*node)));
            }
        }
        if changed.is_empty() {
            changed.insert(Dependency::Revision);
        }
        changed
    }
    fn new(
        engine: &Engine,
        doc: &Document,
        snapshot: &LayoutSnapshot,
        pending: &[flow::Pending],
        template: &ResolvedTemplate,
        applied: &BTreeSet<RelationId>,
    ) -> Self {
        let mut normalized = snapshot.clone();
        normalized.revision = Revision(Vec::new());
        let mut history = reprise_doc::HistoryCache::default();
        let mut owners = Vec::new();
        let relations = doc
            .relations()
            .into_iter()
            .map(|(id, r)| {
                let resolved = r.as_ref().ok().and_then(|r| {
                    engine
                        .schemas
                        .get(&r.schema)
                        .map(|s| doc.resolve_relation(s, r, &mut history))
                });
                let owner_live = r
                    .as_ref()
                    .ok()
                    .and_then(|r| r.owner)
                    .is_some_and(|n| doc.block(n).is_ok());
                if let Some(n) = r.as_ref().ok().and_then(|r| r.owner) {
                    owners.push((n, Inputs::read(doc, n)));
                }
                (id, r, owner_live, resolved)
            })
            .collect();
        Self {
            snapshot: normalized,
            pending: pending.to_vec(),
            relations,
            order: doc.document_order(),
            owners,
            template: template.clone(),
            applied: applied.clone(),
        }
    }
}
#[derive(Clone)]
struct RegionEntry {
    key: PassKey,
    plan: Plan,
}
#[derive(Clone)]
struct FinalEntry {
    key: PassKey,
    snapshot: LayoutSnapshot,
}
#[derive(Default)]
struct Cache {
    prepared: BTreeMap<NodeId, Vec<PreparedEntry>>,
    shaping: BTreeMap<NodeId, Vec<ShapingEntry>>,
    flow: BTreeMap<NodeId, Vec<FlowEntry>>,
    annotations: BTreeMap<NodeId, AnnotationEntry>,
    regions: Vec<RegionEntry>,
    final_pass: Option<FinalEntry>,
    counters: WorkCounters,
    graph: DependencyGraph,
}
#[derive(Default)]
pub(crate) struct Evaluation {
    cache: RefCell<Cache>,
}
impl Evaluation {
    pub(crate) fn shaping_hit(
        &self,
        node: NodeId,
        key: &ShapingKey,
    ) -> Option<(Option<Prepared>, Vec<Diagnostic>)> {
        self.cache
            .borrow()
            .shaping
            .get(&node)?
            .iter()
            .find(|e| &e.key == key)
            .map(|e| (e.value.clone(), e.diagnostics.clone()))
    }
    pub(crate) fn shaping_miss(
        &self,
        node: NodeId,
        key: ShapingKey,
        value: Option<Prepared>,
        diagnostics: Vec<Diagnostic>,
    ) {
        let mut cache = self.cache.borrow_mut();
        let entries = cache.shaping.entry(node).or_default();
        if entries.len() >= 32 {
            entries.remove(0);
        }
        entries.push(ShapingEntry {
            key,
            value,
            diagnostics,
        });
    }

    pub(crate) fn style_resolution(&self) {
        let mut c = self.cache.borrow_mut();
        c.counters.style_resolutions = c.counters.style_resolutions.saturating_add(1);
    }
    pub(crate) fn itemization(&self) {
        let mut c = self.cache.borrow_mut();
        c.counters.itemizations = c.counters.itemizations.saturating_add(1);
    }
    pub(crate) fn shape(&self) {
        let mut c = self.cache.borrow_mut();
        c.counters.shapes = c.counters.shapes.saturating_add(1);
    }

    pub(crate) fn composer_call(&self) {
        let mut cache = self.cache.borrow_mut();
        cache.counters.composer_calls = cache.counters.composer_calls.saturating_add(1);
    }

    pub(crate) fn prepare(
        &self,
        engine: &Engine,
        doc: &Document,
        node: NodeId,
        ctx: &ResolutionContext,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Prepared> {
        let Some(inputs) = Inputs::read(doc, node) else {
            return flow::prepare(engine, doc, node, ctx, diagnostics);
        };
        let key = PrepareKey {
            inputs,
            context: ctx.clone(),
        };
        let reads = key.inputs.dependencies(engine, node);
        let mut cache = self.cache.borrow_mut();
        if let Some(entry) = cache
            .prepared
            .get(&node)
            .and_then(|v| v.iter().find(|e| e.key == key))
            .cloned()
        {
            cache
                .graph
                .record(Computation::Style(node), reads, BTreeSet::new());
            cache.graph.record(
                Computation::Shape(node),
                BTreeSet::from([
                    Dependency::Computed(Computation::Style(node)),
                    Dependency::Text(node),
                ]),
                BTreeSet::new(),
            );
            diagnostics.extend(entry.diagnostics);
            return entry.value;
        }
        let mut reasons = cache
            .prepared
            .get(&node)
            .and_then(|v| v.last())
            .map_or_else(
                || reads.clone(),
                |e| key.inputs.changes(&e.key.inputs, node),
            );
        if cache
            .prepared
            .get(&node)
            .and_then(|v| v.last())
            .is_some_and(|e| e.key.context != key.context)
        {
            reasons.insert(Dependency::Template);
        }
        cache
            .graph
            .record(Computation::Style(node), reads, reasons.clone());
        cache.graph.record(
            Computation::Shape(node),
            BTreeSet::from([
                Dependency::Computed(Computation::Style(node)),
                Dependency::Text(node),
            ]),
            BTreeSet::new(),
        );
        let itemizations_before = cache.counters.itemizations;
        drop(cache);
        let mut notes = Vec::new();
        let value = flow::prepare_with(engine, doc, node, ctx, &mut notes, Some(self));
        let mut cache = self.cache.borrow_mut();
        if cache.counters.itemizations > itemizations_before {
            cache
                .graph
                .reasons
                .entry(Computation::Shape(node))
                .or_default()
                .extend(reasons);
        }
        let entries = cache.prepared.entry(node).or_default();
        if entries.len() >= 32 {
            entries.remove(0);
        }
        entries.push(PreparedEntry {
            key,
            value: value.clone(),
            diagnostics: notes.clone(),
        });
        diagnostics.extend(notes);
        value
    }
    pub(crate) fn annotation_hit(
        &self,
        node: NodeId,
        key: &AnnotationKey,
        engine: &Engine,
    ) -> Option<(Option<flow::Pending>, Vec<Diagnostic>)> {
        let mut cache = self.cache.borrow_mut();
        let entry = cache
            .annotations
            .get(&node)
            .filter(|e| &e.key == key)?
            .clone();
        if let Some(inputs) = &key.inputs {
            cache.graph.record(
                Computation::Style(node),
                inputs.dependencies(engine, node),
                BTreeSet::new(),
            );
            cache.graph.record(
                Computation::Shape(node),
                BTreeSet::from([
                    Dependency::Computed(Computation::Style(node)),
                    Dependency::Text(node),
                ]),
                BTreeSet::new(),
            );
        }
        cache.graph.record(
            Computation::Compose(node),
            BTreeSet::from([
                Dependency::Computed(Computation::Shape(node)),
                Dependency::Template,
            ]),
            BTreeSet::new(),
        );
        Some((entry.pending, entry.notes))
    }
    pub(crate) fn annotation_miss(
        &self,
        node: NodeId,
        key: AnnotationKey,
        pending: Option<flow::Pending>,
        notes: Vec<Diagnostic>,
    ) {
        let mut cache = self.cache.borrow_mut();
        cache.graph.record(
            Computation::Compose(node),
            BTreeSet::from([
                Dependency::Computed(Computation::Shape(node)),
                Dependency::Template,
            ]),
            BTreeSet::from([Dependency::Computed(Computation::Shape(node))]),
        );
        cache.annotations.insert(
            node,
            AnnotationEntry {
                key,
                pending,
                notes,
            },
        );
    }
    fn allocate(
        &self,
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        snapshot: &LayoutSnapshot,
    ) -> Plan {
        let key = PassKey::new(engine, doc, snapshot, &[], template, &BTreeSet::new());
        if let Some(plan) = self
            .cache
            .borrow()
            .regions
            .iter()
            .find(|e| e.key == key)
            .map(|e| e.plan.clone())
        {
            return plan;
        }
        {
            let mut cache = self.cache.borrow_mut();
            cache.counters.region_passes = cache.counters.region_passes.saturating_add(1);
        }
        let plan = crate::regions::allocate(engine, doc, template, snapshot, Some(self));
        let mut cache = self.cache.borrow_mut();
        if cache.regions.len() >= 16 {
            cache.regions.remove(0);
        }
        cache.regions.push(RegionEntry {
            key,
            plan: plan.clone(),
        });
        plan
    }
    fn final_hit(&self, key: &PassKey) -> Option<LayoutSnapshot> {
        self.cache
            .borrow()
            .final_pass
            .as_ref()
            .filter(|e| &e.key == key)
            .map(|e| e.snapshot.clone())
    }
    fn finish(&self, key: PassKey, snapshot: &LayoutSnapshot) {
        self.cache.borrow_mut().final_pass = Some(FinalEntry {
            key,
            snapshot: snapshot.clone(),
        });
    }
    pub(crate) fn flow_hit(&self, node: NodeId, key: &FlowKey, engine: &Engine) -> Option<Delta> {
        let mut cache = self.cache.borrow_mut();
        let delta = cache
            .flow
            .get(&node)?
            .iter()
            .find(|e| &e.key == key)?
            .delta
            .clone();
        if let Some(inputs) = &key.inputs {
            cache.graph.record(
                Computation::Style(node),
                inputs.dependencies(engine, node),
                BTreeSet::new(),
            );
            cache.graph.record(
                Computation::Shape(node),
                BTreeSet::from([
                    Dependency::Computed(Computation::Style(node)),
                    Dependency::Text(node),
                ]),
                BTreeSet::new(),
            );
        }
        cache.graph.record(
            Computation::Compose(node),
            BTreeSet::from([
                Dependency::Computed(Computation::Shape(node)),
                Dependency::Template,
                Dependency::Computed(Computation::Regions),
                Dependency::Tree,
            ]),
            BTreeSet::new(),
        );
        if let Some(previous) = key.previous {
            cache
                .graph
                .reads
                .entry(Computation::Compose(node))
                .or_default()
                .insert(Dependency::Computed(Computation::Compose(previous)));
        }
        cache.counters.reused_compositions = cache.counters.reused_compositions.saturating_add(1);
        Some(delta)
    }
    pub(crate) fn flow_miss(&self, node: NodeId, key: FlowKey, delta: &Delta) {
        let mut cache = self.cache.borrow_mut();
        let mut reasons = BTreeSet::new();
        if let Some(old) = cache.flow.get(&node).and_then(|v| v.last()) {
            if let (Some(new), Some(old)) = (&key.inputs, &old.key.inputs) {
                reasons.extend(new.changes(old, node));
            }
            if key.position != old.key.position {
                reasons.insert(key.previous.map_or(Dependency::Tree, |n| {
                    Dependency::Computed(Computation::Compose(n))
                }));
            }
            if key.template != old.key.template {
                reasons.insert(Dependency::Template);
            }
            if key.plan != old.key.plan {
                reasons.insert(Dependency::Computed(Computation::Regions));
            }
        } else {
            reasons.insert(Dependency::Node(node));
        }
        let frame_count = key.template.frames.len().max(1);
        let mut pages: BTreeSet<_> = delta
            .blocks
            .iter()
            .flat_map(|b| &b.lines)
            .map(|l| l.frame / frame_count)
            .collect();
        if let Some(old) = cache.flow.get(&node).and_then(|v| v.last()) {
            let old_count = old.key.template.frames.len().max(1);
            pages.extend(
                old.delta
                    .blocks
                    .iter()
                    .flat_map(|b| &b.lines)
                    .map(|l| l.frame / old_count),
            );
        }
        cache.counters.reflowed_pages.extend(pages);
        cache.counters.compositions = cache.counters.compositions.saturating_add(1);
        cache.graph.record(
            Computation::Compose(node),
            BTreeSet::from([
                Dependency::Computed(Computation::Shape(node)),
                Dependency::Template,
                Dependency::Computed(Computation::Regions),
                Dependency::Tree,
            ]),
            reasons,
        );
        if let Some(previous) = key.previous {
            cache
                .graph
                .reads
                .entry(Computation::Compose(node))
                .or_default()
                .insert(Dependency::Computed(Computation::Compose(previous)));
        }
        let entries = cache.flow.entry(node).or_default();
        if entries.len() >= 16 {
            entries.remove(0);
        }
        entries.push(FlowEntry {
            key,
            delta: delta.clone(),
        });
    }
}

/// Viewport demand. Page numbers are zero based; empty/reversed ranges demand nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Viewport {
    Pages(std::ops::Range<usize>),
    Rect { page: usize, rect: Rect<PageSpace> },
}
impl Viewport {
    fn end(&self) -> usize {
        match self {
            Self::Pages(r) => {
                if r.start < r.end {
                    r.end
                } else {
                    0
                }
            }
            Self::Rect { page, .. } => page.saturating_add(1),
        }
    }
}
/// Explicit stage boundaries; feedback shares the full reference's bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    Flow,
    RegionFeedback,
    Relations,
    ReadingOrder,
    Complete,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobError {
    Cancelled,
    Stale,
}
impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for JobError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    /// Pages with fresh geometry in the advertised pass. `settled` states
    /// whether subsequent passes can still add or move content.
    pub pages: BTreeSet<usize>,
    pub complete: bool,
    /// Which stage produced this view. Region/relation stages may still add or move content.
    pub pass: Pass,
    pub settled: bool,
    /// True when demand extends past the final document (including empty docs).
    pub outside_document: bool,
}
/// An explicitly incomplete view. Frame indices retain their original addresses.
/// No `LayoutSnapshot` is returned as complete until all passes have finished.
#[derive(Clone)]
pub struct PartialLayout<'doc> {
    revision: Revision,
    coverage: Coverage,
    snapshot: LayoutSnapshot,
    document: &'doc Document,
    terminal: Rc<Cell<Option<JobError>>>,
}
impl std::fmt::Debug for PartialLayout<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PartialLayout")
            .field("revision", &self.revision)
            .field("coverage", &self.coverage)
            .field("snapshot", &self.snapshot)
            .finish_non_exhaustive()
    }
}
impl PartialLayout<'_> {
    pub fn revision(&self) -> &Revision {
        &self.revision
    }
    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }
    /// Inspect geometry inside an explicitly partial wrapper.
    pub fn snapshot(&self) -> &LayoutSnapshot {
        &self.snapshot
    }
    /// Publication gate: compare against the original document and its live frontier.
    pub fn publish(&self, current: &Document) -> Result<&Self, JobError> {
        if let Some(error) = self.terminal.get() {
            return Err(error);
        }
        current.commit();
        if !std::ptr::eq(self.document, current) || current.revision() != self.revision {
            Err(JobError::Stale)
        } else {
            Ok(self)
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub used: usize,
    pub pass: Pass,
    pub viewport_ready: bool,
    pub complete: bool,
}
/// State bound to an immutable engine, including font identities and plugin settings.
/// Reconfigure by dropping this session and constructing a new one.
pub struct LayoutSession<'engine> {
    engine: &'engine Engine,
    evaluation: Evaluation,
    document: Option<*const Document>,
}
impl<'engine> LayoutSession<'engine> {
    pub fn new(engine: &'engine Engine) -> Self {
        Self {
            engine,
            evaluation: Evaluation::default(),
            document: None,
        }
    }
    pub fn counters(&self) -> WorkCounters {
        self.evaluation.cache.borrow().counters.clone()
    }
    pub fn graph(&self) -> DependencyGraph {
        self.evaluation.cache.borrow().graph.clone()
    }
    pub fn start<'job>(
        &'job mut self,
        doc: &'job Document,
        viewport: Viewport,
    ) -> LayoutJob<'job, 'engine> {
        doc.commit();
        if self.document.is_some_and(|p| !std::ptr::eq(p, doc)) {
            self.evaluation = Evaluation::default();
        }
        self.document = Some(doc);
        {
            let mut cache = self.evaluation.cache.borrow_mut();
            cache.counters = WorkCounters::default();
            cache.graph = DependencyGraph::default();
        }
        LayoutJob::new(self, doc, viewport)
    }
    pub fn layout(&mut self, doc: &Document) -> Result<LayoutSnapshot, JobError> {
        let mut job = self.start(doc, Viewport::Pages(0..0));
        // A synchronous caller cannot edit this document between steps. Units
        // still go through the same resumable coordinator used by UI callers.
        while job.pass != Pass::Complete {
            job.step(usize::MAX)?;
        }
        job.current()?;
        Ok(job.snapshot)
    }
}
/// Single-threaded continuation. The authored document may be edited via shared
/// references; every step and every publication rejects a changed frontier.
pub struct LayoutJob<'job, 'engine> {
    session: &'job mut LayoutSession<'engine>,
    doc: &'job Document,
    revision: Revision,
    viewport: Viewport,
    base: LayoutSnapshot,
    snapshot: LayoutSnapshot,
    template: ResolvedTemplate,
    nodes: Vec<NodeId>,
    next_node: usize,
    cursor: Option<Cursor>,
    pending: Vec<flow::Pending>,
    final_key: Option<PassKey>,
    final_reasons: BTreeSet<Dependency>,
    plan: Plan,
    history: Vec<Plan>,
    active: bool,
    iteration: usize,
    pass: Pass,
    cancelled: bool,
    terminal: Rc<Cell<Option<JobError>>>,
}
impl<'job, 'engine> LayoutJob<'job, 'engine> {
    fn new(
        session: &'job mut LayoutSession<'engine>,
        doc: &'job Document,
        viewport: Viewport,
    ) -> Self {
        let engine = session.engine;
        let mut base = LayoutSnapshot {
            revision: doc.revision(),
            adapter: engine.shaper.info(),
            composer: engine.composer.name().into(),
            medium: engine.medium,
            settings: engine.flow,
            template: TemplateUsed {
                name: String::new(),
                source: TemplateSource::Builtin,
            },
            pages: Vec::new(),
            frames: Vec::new(),
            blocks: Vec::new(),
            relations: Vec::new(),
            diagnostics: Vec::new(),
        };
        let template = crate::template::resolve(engine, doc, &mut base.diagnostics);
        base.template = TemplateUsed {
            name: template.name.clone(),
            source: template.source,
        };
        let mut snapshot = base.clone();
        let plan = Plan::default();
        let cursor = Cursor::new(engine, doc, &template, &mut snapshot, &plan);
        let active = crate::regions::active(engine, doc);
        Self {
            session,
            doc,
            revision: doc.revision(),
            viewport,
            base,
            snapshot,
            template,
            nodes: doc.blocks(),
            next_node: 0,
            cursor: Some(cursor),
            pending: Vec::new(),
            final_key: None,
            final_reasons: BTreeSet::new(),
            plan,
            history: Vec::new(),
            active,
            iteration: 0,
            pass: Pass::Flow,
            cancelled: false,
            terminal: Rc::new(Cell::new(None)),
        }
    }
    pub fn revision(&self) -> &Revision {
        &self.revision
    }
    pub fn counters(&self) -> WorkCounters {
        self.session.counters()
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.terminal.set(Some(JobError::Cancelled));
    }
    fn current(&mut self) -> Result<(), JobError> {
        if let Some(error) = self.terminal.get() {
            return Err(error);
        }
        if self.cancelled {
            self.terminal.set(Some(JobError::Cancelled));
            return Err(JobError::Cancelled);
        }
        self.doc.commit();
        if self.doc.revision() != self.revision {
            self.terminal.set(Some(JobError::Stale));
            // A concurrent edit can race key capture and a unit's reads. Reject
            // both publication and every cache entry from that job.
            self.session.evaluation = Evaluation::default();
            return Err(JobError::Stale);
        }
        Ok(())
    }
    fn sealed_pages(&self) -> usize {
        if self.pass != Pass::Flow {
            self.snapshot.pages.len()
        } else {
            self.cursor.as_ref().map_or(0, Cursor::page)
        }
    }
    fn viewport_ready(&self) -> bool {
        self.pass == Pass::Complete || self.sealed_pages() >= self.viewport.end()
    }
    /// A unit is a top-level flow block, a complete region allocation, or the
    /// final relations or reading-order pass. Stops as soon as the viewport becomes ready.
    /// Zero budget is a checked no-op; no pass runs without a charged unit.
    pub fn step(&mut self, budget: usize) -> Result<Step, JobError> {
        self.current()?;
        let was_ready = self.viewport_ready();
        let mut used = 0usize;
        while used < budget && self.pass != Pass::Complete {
            let engine = self.session.engine;
            match self.pass {
                Pass::Flow => {
                    if let Some(&node) = self.nodes.get(self.next_node) {
                        if let Some(cursor) = self.cursor.take() {
                            self.cursor = Some(cursor.step(
                                engine,
                                self.doc,
                                &self.template,
                                &mut self.snapshot,
                                &self.plan,
                                node,
                                Some(&self.session.evaluation),
                            ));
                        }
                        self.next_node = self.next_node.saturating_add(1);
                    } else {
                        if let Some(cursor) = self.cursor.take() {
                            self.pending = cursor.finish(
                                engine,
                                &self.template,
                                &mut self.snapshot,
                                &self.plan,
                            );
                        }
                        self.pass = if self.active {
                            Pass::RegionFeedback
                        } else {
                            Pass::Relations
                        };
                    }
                }
                Pass::RegionFeedback => {
                    let next = self.session.evaluation.allocate(
                        engine,
                        self.doc,
                        &self.template,
                        &self.snapshot,
                    );
                    let outcome =
                        crate::regions::feedback(&self.plan, &next, &self.history, self.iteration);
                    if outcome == crate::regions::Feedback::Continue {
                        self.history.push(std::mem::replace(&mut self.plan, next));
                        self.iteration = self.iteration.saturating_add(1);
                        self.snapshot = self.base.clone();
                        self.cursor = Some(Cursor::new(
                            engine,
                            self.doc,
                            &self.template,
                            &mut self.snapshot,
                            &self.plan,
                        ));
                        self.next_node = 0;
                        self.pass = Pass::Flow;
                    } else {
                        crate::regions::finish_feedback(
                            &mut self.snapshot,
                            &self.plan,
                            next,
                            outcome,
                        );
                        self.pass = Pass::Relations;
                    }
                }
                Pass::Relations => {
                    let key = PassKey::new(
                        engine,
                        self.doc,
                        &self.snapshot,
                        &self.pending,
                        &self.template,
                        &self.plan.applied,
                    );
                    self.final_reasons = key.changes(
                        self.session
                            .evaluation
                            .cache
                            .borrow()
                            .final_pass
                            .as_ref()
                            .map(|e| &e.key),
                    );
                    if let Some(snapshot) = self.session.evaluation.final_hit(&key) {
                        self.snapshot = snapshot;
                        self.snapshot.revision = self.revision.clone();
                        self.pending.clear();
                        self.pass = Pass::Complete;
                        self.record_graph(false);
                    } else {
                        {
                            let mut cache = self.session.evaluation.cache.borrow_mut();
                            cache.counters.relation_passes =
                                cache.counters.relation_passes.saturating_add(1);
                        }
                        self.pending = crate::relations::begin(
                            engine,
                            self.doc,
                            &mut self.snapshot,
                            std::mem::take(&mut self.pending),
                            &self.plan.applied,
                        );
                        self.final_key = Some(key);
                        self.pass = Pass::ReadingOrder;
                    }
                }
                Pass::ReadingOrder => {
                    {
                        let mut cache = self.session.evaluation.cache.borrow_mut();
                        cache.counters.reading_order_passes =
                            cache.counters.reading_order_passes.saturating_add(1);
                    }
                    crate::relations::finish_reading(
                        self.doc,
                        &mut self.snapshot,
                        std::mem::take(&mut self.pending),
                    );
                    if let Some(key) = self.final_key.take() {
                        self.session.evaluation.finish(key, &self.snapshot);
                    }
                    self.pass = Pass::Complete;
                    self.record_graph(true);
                }
                Pass::Complete => {}
            }
            used = used.saturating_add(1);
            {
                let mut cache = self.session.evaluation.cache.borrow_mut();
                cache.counters.units = cache.counters.units.saturating_add(1);
            }
            if !was_ready && self.viewport_ready() {
                break;
            }
        }
        self.current()?;
        Ok(Step {
            used,
            pass: self.pass,
            viewport_ready: self.viewport_ready(),
            complete: self.pass == Pass::Complete,
        })
    }
    pub fn partial(&mut self) -> Result<PartialLayout<'job>, JobError> {
        self.current()?;
        let sealed = self.sealed_pages();
        let mut snapshot = self.snapshot.clone();
        if self.pass != Pass::Complete {
            snapshot.pages.truncate(sealed);
            snapshot.frames.retain(|f| f.page < sealed);
            snapshot.blocks.retain_mut(|b| {
                b.lines.retain(|l| snapshot.frames.get(l.frame).is_some());
                !b.lines.is_empty()
            });
            snapshot.relations.clear();
            // Provisional diagnostics may depend on future placement; only
            // complete evaluation publishes the ordered diagnostic stream.
            snapshot.diagnostics.clear();
        }
        Ok(PartialLayout {
            revision: self.revision.clone(),
            coverage: Coverage {
                pages: (0..sealed).collect(),
                complete: self.pass == Pass::Complete,
                pass: self.pass,
                settled: self.pass == Pass::Complete
                    || (!self.active && self.doc.relations().is_empty()),
                outside_document: self.pass == Pass::Complete && self.viewport.end() > sealed,
            },
            snapshot,
            document: self.doc,
            terminal: self.terminal.clone(),
        })
    }
    /// Returns a complete snapshot only after all passes and the revision gate.
    pub fn complete(&mut self) -> Result<Option<LayoutSnapshot>, JobError> {
        self.current()?;
        Ok((self.pass == Pass::Complete).then(|| self.snapshot.clone()))
    }
    fn record_graph(&self, recomputed: bool) {
        let mut cache = self.session.evaluation.cache.borrow_mut();
        for (id, relation) in self.doc.relations() {
            let mut reads =
                BTreeSet::from([Dependency::Relation(id), Dependency::EngineConfiguration]);
            if let Ok(relation) = relation {
                for dep in relation.dependencies() {
                    match &dep {
                        reprise_doc::relation::Dependency::LayoutOfNode(n) => {
                            reads.insert(Dependency::Computed(Computation::Compose(*n)));
                        }
                        reprise_doc::relation::Dependency::LayoutOfRange(r) => {
                            reads.insert(Dependency::Range(*r));
                            if let reprise_doc::RangeState::Valid { node, .. }
                            | reprise_doc::RangeState::Rebound { node, .. } =
                                self.doc.resolve_range(*r)
                            {
                                reads.insert(Dependency::Computed(Computation::Compose(node)));
                            }
                        }
                        _ => {}
                    }
                    reads.insert(dep.into());
                }
            }
            cache.graph.record(
                Computation::Relation(id),
                reads,
                if recomputed {
                    self.final_reasons.clone()
                } else {
                    BTreeSet::new()
                },
            );
        }
        let region_reads: BTreeSet<_> = self
            .doc
            .relations()
            .into_iter()
            .filter(|(_, r)| {
                r.as_ref().is_ok_and(|r| {
                    r.schema == reprise_doc::relation::builtin::FLOAT
                        || r.schema == reprise_doc::relation::builtin::NOTE
                })
            })
            .map(|(id, _)| Dependency::Computed(Computation::Relation(id)))
            .collect();
        let region_recomputed = self.active && cache.counters.region_passes > 0;
        cache.graph.record(
            Computation::Regions,
            region_reads,
            if region_recomputed {
                self.final_reasons.clone()
            } else {
                BTreeSet::new()
            },
        );
        let mut reading_reads = BTreeSet::from([Dependency::Tree]);
        for page in 0..self.snapshot.pages.len() {
            cache.graph.record(
                Computation::Page(page),
                BTreeSet::from([Dependency::Template, Dependency::EngineConfiguration]),
                BTreeSet::new(),
            );
        }
        for block in &self.snapshot.blocks {
            if !cache
                .graph
                .reads
                .contains_key(&Computation::Style(block.node))
                && let Some(inputs) = Inputs::read(self.doc, block.node)
            {
                cache.graph.record(
                    Computation::Style(block.node),
                    inputs.dependencies(self.session.engine, block.node),
                    BTreeSet::new(),
                );
            }
            cache
                .graph
                .reads
                .entry(Computation::Shape(block.node))
                .or_insert_with(|| {
                    BTreeSet::from([
                        Dependency::Text(block.node),
                        Dependency::Computed(Computation::Style(block.node)),
                    ])
                });
            cache
                .graph
                .reads
                .entry(Computation::Compose(block.node))
                .or_insert_with(|| {
                    BTreeSet::from([
                        Dependency::Template,
                        Dependency::Computed(Computation::Shape(block.node)),
                        Dependency::Computed(Computation::Regions),
                    ])
                });
            reading_reads.insert(Dependency::Computed(Computation::Compose(block.node)));
            for (index, line) in block.lines.iter().enumerate() {
                let frame_unit = Computation::ComposeInFrame {
                    node: block.node,
                    frame: line.frame,
                };
                cache
                    .graph
                    .reads
                    .entry(frame_unit.clone())
                    .or_insert_with(|| {
                        BTreeSet::from([
                            Dependency::Computed(Computation::Compose(block.node)),
                            Dependency::Template,
                        ])
                    });
                let unit = Computation::Line(crate::LineRef {
                    node: block.node,
                    line: index,
                });
                cache.graph.record(
                    unit,
                    BTreeSet::from([Dependency::Computed(frame_unit)]),
                    BTreeSet::new(),
                );
                if let Some(frame) = self.snapshot.frame(line.frame) {
                    cache
                        .graph
                        .reads
                        .entry(Computation::Page(frame.page))
                        .or_default()
                        .insert(Dependency::Computed(Computation::Compose(block.node)));
                }
            }
        }
        for (id, relation) in self.doc.relations() {
            if let Some(owner) = relation.ok().and_then(|r| r.owner)
                && let Some(block) = self.snapshot.block(owner)
            {
                for (line_index, line) in block.lines.iter().enumerate() {
                    let input = Dependency::Computed(Computation::Relation(id));
                    cache
                        .graph
                        .reads
                        .entry(Computation::Line(crate::LineRef {
                            node: owner,
                            line: line_index,
                        }))
                        .or_default()
                        .insert(input.clone());
                    if let Some(frame) = self.snapshot.frame(line.frame) {
                        cache
                            .graph
                            .reads
                            .entry(Computation::Page(frame.page))
                            .or_default()
                            .insert(input);
                    }
                }
            }
        }
        for (id, _) in self.doc.relations() {
            reading_reads.insert(Dependency::Computed(Computation::Relation(id)));
        }
        cache.graph.record(
            Computation::ReadingOrder,
            reading_reads,
            if recomputed {
                self.final_reasons.clone()
            } else {
                BTreeSet::new()
            },
        );
    }
}

#[cfg(test)]
mod tests;

/// Opaque memo storage for hosts that cannot retain a borrowed session.
/// Keep it bound to the same document and immutable engine configuration.
#[derive(Default)]
pub struct LayoutCache {
    evaluation: Evaluation,
}
/// Opaque suspended job. Resume only against its original document/configuration.
pub struct LayoutContinuation {
    document: *const Document,
    revision: Revision,
    viewport: Viewport,
    base: LayoutSnapshot,
    snapshot: LayoutSnapshot,
    template: ResolvedTemplate,
    nodes: Vec<NodeId>,
    next_node: usize,
    cursor: Option<Cursor>,
    pending: Vec<flow::Pending>,
    final_key: Option<PassKey>,
    final_reasons: BTreeSet<Dependency>,
    plan: Plan,
    history: Vec<Plan>,
    active: bool,
    iteration: usize,
    pass: Pass,
    cancelled: bool,
    terminal: Rc<Cell<Option<JobError>>>,
}
impl<'engine> LayoutSession<'engine> {
    pub fn from_cache(engine: &'engine Engine, cache: LayoutCache) -> Self {
        Self {
            engine,
            evaluation: cache.evaluation,
            document: None,
        }
    }
    pub fn into_cache(self) -> LayoutCache {
        LayoutCache {
            evaluation: self.evaluation,
        }
    }
    pub fn resume<'job>(
        &'job mut self,
        doc: &'job Document,
        state: LayoutContinuation,
    ) -> Result<LayoutJob<'job, 'engine>, JobError> {
        if !std::ptr::eq(state.document, doc) || state.revision != doc.revision() {
            self.evaluation = Evaluation::default();
            return Err(JobError::Stale);
        }
        self.document = Some(doc);
        Ok(LayoutJob {
            session: self,
            doc,
            revision: state.revision,
            viewport: state.viewport,
            base: state.base,
            snapshot: state.snapshot,
            template: state.template,
            nodes: state.nodes,
            next_node: state.next_node,
            cursor: state.cursor,
            pending: state.pending,
            final_key: state.final_key,
            final_reasons: state.final_reasons,
            plan: state.plan,
            history: state.history,
            active: state.active,
            iteration: state.iteration,
            pass: state.pass,
            cancelled: state.cancelled,
            terminal: state.terminal,
        })
    }
}
impl LayoutJob<'_, '_> {
    pub fn suspend(self) -> LayoutContinuation {
        LayoutContinuation {
            document: self.doc,
            revision: self.revision,
            viewport: self.viewport,
            base: self.base,
            snapshot: self.snapshot,
            template: self.template,
            nodes: self.nodes,
            next_node: self.next_node,
            cursor: self.cursor,
            pending: self.pending,
            final_key: self.final_key,
            final_reasons: self.final_reasons,
            plan: self.plan,
            history: self.history,
            active: self.active,
            iteration: self.iteration,
            pass: self.pass,
            cancelled: self.cancelled,
            terminal: self.terminal,
        }
    }
}
