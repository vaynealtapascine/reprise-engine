//! Pass 1: flow (24). Text threads through the frames of the main flow in the
//! order the template declares them, page after page. A paragraph that doesn't
//! fit what is left of a frame continues, from the line where it stopped, in
//! the next one. Annotations are composed and left for the relation pass to
//! place.
//!
//! Termination is bounded twice over. Every pass round the loop either places
//! a line or moves on to the next frame, and the number of pages is capped by
//! the engine's page limit, which is reported when it is hit. A line taller
//! than every frame is placed overflowing, so it can't make the flow look for
//! a frame forever.

use std::ops::Range;

use reprise_compose::{
    Adjustment, Break, ComposeRequest, Composer, GeometryProvider, Indented, Indents, LineFragment,
    Measure, Optimal, Polygon, Runaround, break_opportunities, is_word_space,
};
use reprise_diag::Severity;
use reprise_doc::context::{Extent, ResolutionContext};
use reprise_doc::{BlockKind, ComputedStyle, Document, NodeId};
use reprise_font::FaceId;
use reprise_geom::{FrameSpace, Length, Point, Rect};
use reprise_shape::{
    Item, ParagraphInput, ShapedRun, ShapedText, Shaper, StyleRun, itemize, itemize_families,
    reorder_line, visual_order,
};

pub(crate) mod bands;
mod keeps;
pub(crate) mod paragraph;
pub(crate) mod stage;

use crate::region::Bounded;
use crate::regions::Plan;
use crate::template::{ResolvedFrame, ResolvedTemplate};
use crate::{
    BlockLayout, Diagnostic, Engine, FrameLayout, LayoutSnapshot, LineLayout, PageLayout,
    PositionedRun, Subject, codes,
};

/// A block composed but not yet placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pending {
    pub block: BlockLayout,
    /// How far the block extends down the block axis.
    pub extent: Length,
}

/// Lays out every block, places the paragraphs and returns the blocks that
/// wait for a relation to place them.
pub(crate) fn run_with_sections(
    engine: &Engine,
    doc: &Document,
    template: &ResolvedTemplate,
    snapshot: &mut LayoutSnapshot,
    plan: &Plan,
    sections: std::sync::Arc<crate::sections::Table>,
    running: std::rc::Rc<crate::running::compose::Shared>,
) -> Vec<Pending> {
    let continuous = sections.has_continuous;
    let mut cursor =
        Cursor::new_with_sections(engine, doc, template, snapshot, plan, sections, running);
    if continuous {
        let nodes = doc.top(reprise_doc::Domain::Body);
        // A unit either consumes input, advances a frame, or advances one of
        // the bounded preparation/search phases. Include table descendants.
        let input = doc
            .order(reprise_doc::Domain::Body)
            .iter()
            .fold(0usize, |n, node| {
                n.saturating_add(
                    doc.block(*node)
                        .map_or(1, |b| b.text.len().saturating_add(64)),
                )
            });
        let bound = input
            .saturating_add((engine.flow.max_pages.max(1) as usize).saturating_mul(64))
            .saturating_mul(25 * 32)
            .max(1);
        let mut next = 0usize;
        for _ in 0..bound {
            let start = (!cursor.busy()).then(|| nodes.get(next).copied()).flatten();
            if !cursor.busy() && start.is_none() {
                return cursor.finish(engine, doc, template, snapshot, plan);
            }
            if start.is_some() {
                next = next.saturating_add(1);
            }
            (cursor, _) = cursor.unit(engine, doc, template, snapshot, plan, None, start, &[], 1);
        }
        snapshot.diagnostics.push(Diagnostic::new(
            Severity::Error,
            codes::TEXT_UNPLACED,
            Subject::Document,
            "bounded section flow exhausted its work units",
        ));
        return cursor.finish(engine, doc, template, snapshot, plan);
    }
    for node in doc.top(reprise_doc::Domain::Body) {
        cursor = cursor.step(engine, doc, template, snapshot, plan, node, None);
    }
    cursor.finish(engine, doc, template, snapshot, plan)
}

/// Owned continuation state; no borrowed snapshot survives a yield.
#[derive(Clone)]
pub(crate) struct Cursor {
    thread: Vec<usize>,
    max_depth: Length,
    page_base: Vec<usize>,
    fills: Vec<Fill>,
    page: usize,
    pos: usize,
    limit_hit: bool,
    pending: Vec<Pending>,
    region_owners: std::collections::BTreeSet<NodeId>,
    previous: Option<NodeId>,
    /// The block in progress, between units of an incremental job.
    active: Option<Active>,
    keeps: Keeps,
    catalog: crate::pages::Catalog,
    source: crate::pages::FrameSource,
    domain: Option<reprise_doc::Domain>,
    pub(crate) section_table: Option<std::sync::Arc<crate::sections::Table>>,
    section: usize,
    template: std::sync::Arc<ResolvedTemplate>,
    boundary_node: Option<NodeId>,
    band: usize,
    band_start: Option<std::rc::Rc<bands::BandStart>>,
    band_nodes: Vec<NodeId>,
    balance: Option<Box<bands::Balance>>,
    queued_node: Option<NodeId>,
    open_band: bool,
    /// The running regions' memo, for a body cursor of a document with any.
    running: Option<std::rc::Rc<crate::running::compose::Shared>>,
    /// Set for the cursor that composes a running region's story.
    fields: Option<FieldContext>,
}

/// The field environment of a cursor that composes something other than the
/// body: which placement it is, the page it stands for and where its reads go.
#[derive(Clone)]
pub(crate) struct FieldContext {
    pub instance: crate::inline::FieldInstance,
    pub page: usize,
    pub pages: usize,
    pub trace: Option<crate::inline::Trace>,
}

/// The running regions' composition context, when the document has any.
fn running_ctx<'x>(
    running: &'x Option<std::rc::Rc<crate::running::compose::Shared>>,
    table: &crate::sections::Table,
    engine: &'x Engine,
    doc: &'x Document,
    plan: &'x Plan,
) -> Option<crate::running::attach::Ctx<'x>> {
    let shared = running.as_deref()?;
    (table.enabled && table.running.any()).then_some(crate::running::attach::Ctx {
        engine,
        doc,
        plan,
        shared,
        filler_regions: false,
    })
}

/// What keep lookahead reuses between paragraphs of one layout: the block
/// order, and the last few trial preparations (exact `(node, context)`
/// matches only, so it can't change output).
#[derive(Clone, Default)]
pub(crate) struct Keeps {
    order: Option<std::sync::Arc<BlockOrder>>,
    trials: Vec<(NodeId, ResolutionContext, std::sync::Arc<Prepared>)>,
}

/// The document's top-level blocks in order, with each one's position.
pub(crate) struct BlockOrder {
    nodes: Vec<NodeId>,
    index: std::collections::BTreeMap<NodeId, usize>,
}

/// Trial preparations kept for keep lookahead.
const MAX_TRIALS: usize = 64;
/// The most blocks one keep-with-next chain looks ahead through.
pub const MAX_KEEP_CHAIN: usize = 32;

/// The room a frame offers one paragraph: its measure less the paragraph's
/// indents, less float exclusions, to `depth`.
pub(crate) struct FrameRoom {
    runaround: Runaround<Indented<Measure>>,
    pub(crate) depth: Length,
}

impl FrameRoom {
    pub(crate) fn inner(&self) -> &dyn GeometryProvider {
        if self.runaround.exclusions.is_empty() {
            &self.runaround.base
        } else {
            &self.runaround
        }
    }
}

/// How a keep-with-next chain fares in the rest of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chain {
    Fits,
    Fails,
    TooLong,
}

/// A block that takes more than one unit.
#[derive(Clone)]
pub(crate) enum Active {
    Paragraph(Box<paragraph::Paragraph>),
    Table(Box<TableStep>),
}

impl Active {
    fn node(&self) -> NodeId {
        match self {
            Active::Paragraph(p) => p.node(),
            Active::Table(t) => t.node(),
        }
    }
}

/// A table's phases, one unit at a time (see `Flow::table`).
#[derive(Clone)]
pub(crate) enum TableStep {
    Start(NodeId, reprise_doc::TableColumns),
    Rows(crate::table::TableBuild),
    Place {
        node: NodeId,
        headers: Vec<crate::table_flow::Group>,
        bodies: std::collections::VecDeque<crate::table_flow::Group>,
        next_header: usize,
        widths: Vec<Length>,
        repeat: Option<crate::table_flow::Repeat>,
        current: Option<(usize, crate::table_flow::GroupStep)>,
    },
}

impl TableStep {
    fn node(&self) -> NodeId {
        match self {
            TableStep::Start(node, _) | TableStep::Place { node, .. } => *node,
            TableStep::Rows(build) => build.node(),
        }
    }
}

/// Rows a table prepares in one unit, at most.
pub const TABLE_ROW_GROUP: usize = 16;

impl Cursor {
    pub(crate) fn new_with_sections(
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        plan: &Plan,
        sections: std::sync::Arc<crate::sections::Table>,
        running: std::rc::Rc<crate::running::compose::Shared>,
    ) -> Self {
        Self::new_source(
            engine,
            doc,
            template,
            snapshot,
            plan,
            reprise_doc::Domain::Body,
            crate::pages::FrameSource::Paged,
            Some(sections),
            Some(running),
        )
    }
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)] // Plain running adapters connect in S6a; exercised by S3 tests.
    pub(crate) fn new_in(
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        plan: &Plan,
        domain: reprise_doc::Domain,
        source: crate::pages::FrameSource,
    ) -> Self {
        let sections = (domain == reprise_doc::Domain::Body)
            .then(|| crate::sections::Table::resolve(engine, doc, template));
        Self::new_source(
            engine, doc, template, snapshot, plan, domain, source, sections, None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn new_source(
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        plan: &Plan,
        domain: reprise_doc::Domain,
        source: crate::pages::FrameSource,
        section_table: Option<std::sync::Arc<crate::sections::Table>>,
        running: Option<std::rc::Rc<crate::running::compose::Shared>>,
    ) -> Self {
        let section = section_table.as_ref().map_or(0, |s| s.initial);
        let template = section_table
            .as_ref()
            .and_then(|s| s.sections.get(section))
            .map_or_else(
                || std::sync::Arc::new(template.clone()),
                |s| s.template.clone(),
            );
        if section_table.as_ref().is_some_and(|s| s.enabled) {
            snapshot
                .sections
                .get_or_insert_with(crate::SectionsLayout::default);
        }
        let thread = if source.can_page() {
            template.main_thread()
        } else {
            (0..template.frames.len().min(1)).collect()
        };
        let max_depth = thread
            .iter()
            .map(|&t| template.frames[t].depth)
            .max()
            .unwrap_or_default();
        let thread_is_empty = thread.is_empty()
            || source.is_exhausted()
            || source
                .single_index()
                .is_some_and(|i| snapshot.frame(i).is_none());
        let catalog = crate::pages::Catalog::from_snapshot(snapshot);
        let mut flow = Flow {
            engine,
            doc,
            running,
            fields: None,
            log: Default::default(),
            field_log: Default::default(),
            evaluation: None,
            previous: None,
            template,
            snapshot,
            plan,
            thread,
            max_depth,
            page_base: Vec::new(),
            fills: Vec::new(),
            page: 0,
            pos: 0,
            // No frame to flow through: unreachable, but nothing may panic.
            limit_hit: thread_is_empty,
            pending: Vec::new(),
            region_owners: crate::regions::owners(engine, doc),
            upcoming: &[],
            keeps: Keeps::default(),
            catalog,
            source,
            domain: Some(domain),
            section_table,
            section,
            boundary_node: None,
            band: 0,
            open_band: false,
        };
        // Even an empty document gets a page.
        if flow.source.can_page() {
            flow.new_page();
        } else {
            flow.page = flow
                .source
                .single_index()
                .and_then(|i| flow.snapshot.frame(i))
                .map_or(0, |f| f.page);
            flow.page_base.push(flow.source.single_index().unwrap_or(0));
            flow.fills.resize(
                flow.snapshot.frames.len(),
                Fill {
                    used: Length::ZERO,
                    empty: true,
                    after: Length::ZERO,
                    anchored: true,
                },
            );
        }

        flow.cursor()
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step(
        self,
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        plan: &Plan,
        node: NodeId,
        evaluation: Option<&crate::incremental::Evaluation>,
    ) -> Self {
        let mut flow = self.restore(engine, doc, template, snapshot, plan, evaluation);
        flow.node(doc, node);
        flow.previous = Some(node);
        flow.cursor()
    }
    /// The same cursor, composing for the field environment `fields`.
    pub(crate) fn with_fields(mut self, fields: FieldContext) -> Self {
        self.fields = Some(fields);
        self
    }
    /// A one-frame source has run out of frame: nothing more can be placed.
    pub(crate) fn exhausted(&self) -> bool {
        self.limit_hit
    }
    /// A block is in progress.
    pub(crate) fn busy(&self) -> bool {
        self.active.is_some() || self.balance.is_some() || self.queued_node.is_some()
    }
    /// The lines a paragraph in progress has placed so far.
    pub(crate) fn in_progress(&self) -> Option<BlockLayout> {
        match &self.active {
            Some(Active::Paragraph(p)) => p.in_progress(),
            _ => None,
        }
    }
    /// One unit of an incremental job: starts `start` when no block is in
    /// progress, then runs one unit of the block in progress. `upcoming` are
    /// the blocks after it, for speculative preparation. Returns the units
    /// charged (at least 1, at most `allowance.max(1)`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ordinary_unit(
        mut self,
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        plan: &Plan,
        evaluation: Option<&crate::incremental::Evaluation>,
        start: Option<NodeId>,
        upcoming: &[NodeId],
        allowance: usize,
    ) -> (Self, usize) {
        let mut active = self.active.take();
        let mut flow = self.restore(engine, doc, template, snapshot, plan, evaluation);
        flow.upcoming = upcoming;
        let mut used = 1;
        if active.is_none()
            && let Some(node) = start
        {
            active = flow.begin_block(doc, node);
            if active.is_none() {
                flow.previous = Some(node);
            }
        }
        if let Some(mut block) = active.take() {
            let (units, done) = flow.block_unit(doc, &mut block, allowance);
            used = units.max(1);
            if done {
                flow.previous = Some(block.node());
            } else {
                active = Some(block);
            }
        }
        let mut cursor = flow.cursor();
        cursor.active = active;
        (cursor, used)
    }
    pub(crate) fn finish(
        self,
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        plan: &Plan,
    ) -> Vec<Pending> {
        let mut flow = self.restore(engine, doc, template, snapshot, plan, None);
        while flow.snapshot.pages.len() < plan.pages {
            let page = flow.snapshot.pages.len();
            let request = plan.page_requests.get(&page).copied();
            let allocated = if let Some((owner, kind)) = request
                && let Some(table) = flow.section_table.clone().filter(|s| s.enabled)
                && let Some(section) = table.get(owner)
            {
                let index = table.by_id.get(&section.id()).copied();
                index.is_some_and(|index| flow.open_page(&table, index, kind, false).is_some())
            } else {
                flow.new_page()
            };
            if !allocated {
                break;
            }
        }
        flow.merge_running_notes();
        flow.pending
    }
    pub(crate) fn page(&self) -> usize {
        if let Some(Active::Table(t)) = &self.active
            && let TableStep::Place {
                current: Some((_, group)),
                ..
            } = t.as_ref()
            && let Some(page) = group.first_page
        {
            return self.page.min(page);
        }
        self.page
    }
    fn restore<'a>(
        self,
        engine: &'a Engine,
        doc: &'a Document,
        _template: &'a ResolvedTemplate,
        snapshot: &'a mut LayoutSnapshot,
        plan: &'a Plan,
        evaluation: Option<&'a crate::incremental::Evaluation>,
    ) -> Flow<'a> {
        Flow {
            engine,
            doc,
            running: self.running,
            fields: self.fields,
            log: Default::default(),
            field_log: Default::default(),
            template: self.template,
            snapshot,
            plan,
            evaluation,
            thread: self.thread,
            max_depth: self.max_depth,
            page_base: self.page_base,
            fills: self.fills,
            page: self.page,
            pos: self.pos,
            limit_hit: self.limit_hit,
            pending: self.pending,
            region_owners: self.region_owners,
            previous: self.previous,
            upcoming: &[],
            keeps: self.keeps,
            catalog: self.catalog,
            source: self.source,
            domain: self.domain,
            section_table: self.section_table,
            section: self.section,
            boundary_node: self.boundary_node,
            band: self.band,
            open_band: self.open_band,
        }
    }
}

/// How much of a frame the flow has used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fill {
    /// Where the next block starts, spacing included.
    used: Length,
    /// Nothing is in the frame yet.
    empty: bool,
    /// The last paragraph's `space-after`, collapsed with the next one's
    /// `space-before` (the larger wins).
    after: Length,
    /// Everything in the frame is a keep-with-next chain leading into the
    /// next block (true for an empty frame): moving that block to the next
    /// frame would not help it.
    anchored: bool,
}

pub(crate) struct Flow<'a> {
    pub(crate) doc: &'a Document,
    running: Option<std::rc::Rc<crate::running::compose::Shared>>,
    fields: Option<FieldContext>,
    /// What the paragraph in progress has read from the plan, for the memo.
    log: std::cell::RefCell<Option<Vec<crate::reads::PlanRead>>>,
    /// The field values it composed with (folded into `log` when taken).
    field_log: std::cell::RefCell<crate::inline::Reads>,
    pub(crate) pending: Vec<Pending>,
    pub(crate) region_owners: std::collections::BTreeSet<NodeId>,
    pub(crate) previous: Option<NodeId>,
    /// Blocks after the current one, for speculative preparation.
    pub(crate) upcoming: &'a [NodeId],
    pub(crate) engine: &'a Engine,
    pub(crate) evaluation: Option<&'a crate::incremental::Evaluation>,
    pub(crate) template: std::sync::Arc<ResolvedTemplate>,
    pub(crate) snapshot: &'a mut LayoutSnapshot,
    plan: &'a Plan,
    /// Template indices of the main flow's frames, in threading order.
    thread: Vec<usize>,
    /// The deepest frame in the thread: a line taller than this fits nowhere.
    max_depth: Length,
    /// The snapshot index of each page's first frame.
    page_base: Vec<usize>,
    /// By snapshot frame index.
    fills: Vec<Fill>,
    /// Where the flow is: a page and a position in the thread.
    page: usize,
    pos: usize,
    limit_hit: bool,
    pub(crate) keeps: Keeps,
    catalog: crate::pages::Catalog,
    source: crate::pages::FrameSource,
    domain: Option<reprise_doc::Domain>,
    pub(crate) section_table: Option<std::sync::Arc<crate::sections::Table>>,
    section: usize,
    boundary_node: Option<NodeId>,
    band: usize,
    open_band: bool,
}

impl Flow<'_> {
    fn node(&mut self, doc: &Document, node: NodeId) {
        self.transition(doc, node);
        // `table` runs the same phases as `table_unit`, back to back.
        if let Ok(Some(reprise_doc::TableRole::Table(columns))) = doc.table_role(node) {
            self.table(doc, node, &columns);
            return;
        }
        if let Some(mut block) = self.begin_block(doc, node) {
            while !self.block_unit(doc, &mut block, usize::MAX).1 {}
        }
    }

    fn transition(&mut self, doc: &Document, node: NodeId) {
        if self.boundary_node == Some(node) {
            return;
        }
        self.boundary_node = Some(node);
        let Some(table) = self.section_table.clone().filter(|s| s.enabled) else {
            return;
        };
        let Some(kind) = table.starts.get(&node).copied() else {
            return;
        };
        if self.previous.is_none() {
            return;
        }
        if let Some(previous) = self.previous
            && doc
                .specified_style(previous)
                .is_ok_and(|s| s.compute(&self.engine.functions).keep_with_next)
        {
            let note = Diagnostic::new(
                Severity::Warning,
                codes::KEEP_UNSATISFIED,
                Subject::Node(previous),
                "manual boundary overrides keep-with-next",
            );
            if !self.snapshot.diagnostics.contains(&note) {
                self.snapshot.diagnostics.push(note);
            }
        }
        use reprise_doc::sections::{BreakKind, SectionId};
        match kind {
            BreakKind::Column => {
                self.advance();
            }
            BreakKind::Page => {
                self.page_break();
            }
            BreakKind::Section => {
                let Some(&next) = table.by_id.get(&SectionId::Block(node)) else {
                    return;
                };
                let Some(section) = table.sections.get(next) else {
                    return;
                };
                if self.open_band && section.start == crate::sections::Start::Continuous {
                    self.open_band = false;
                    let descriptor = self
                        .snapshot
                        .sections
                        .as_ref()
                        .and_then(|s| s.pages.get(self.page));
                    let owner = descriptor.and_then(|p| table.get(p.section));
                    let offset = descriptor
                        .and_then(|p| p.bands.last())
                        .map_or(Length::ZERO, |b| b.offset + b.depth);
                    let taken = descriptor.map_or(Length::ZERO, |p| p.reserve.0 + p.reserve.1);
                    if let Some(owner) = owner
                        && let Some(template) = crate::pages::PageFactory::append_band(
                            self.snapshot,
                            section,
                            owner,
                            self.page,
                            offset,
                            if owner.geometry.writing_mode == reprise_doc::WritingMode::HorizontalTb
                            {
                                owner.geometry.block_extent() - taken - offset
                            } else {
                                owner.geometry.block_extent() - offset
                            },
                        )
                    {
                        self.section = next;
                        self.template = std::sync::Arc::new(template);
                        self.thread = self.template.main_thread();
                        self.max_depth = self
                            .thread
                            .iter()
                            .filter_map(|i| self.template.frames.get(*i))
                            .map(|f| f.depth)
                            .max()
                            .unwrap_or_default();
                        self.band = self
                            .snapshot
                            .sections
                            .as_ref()
                            .and_then(|s| s.pages.get(self.page))
                            .map_or(0, |p| p.bands.len().saturating_sub(1));
                        self.pos = 0;
                        self.catalog.extend(self.snapshot);
                        self.resize_fills();
                        return;
                    }
                }
                let parity = match section.start {
                    crate::sections::Start::OddPage => Some(1),
                    crate::sections::Start::EvenPage => Some(0),
                    _ => None,
                };
                if parity.is_some_and(|parity| {
                    (self.snapshot.pages.len().saturating_add(1) % 2) != parity
                }) && table.sections.get(self.section).is_some()
                    // The filler policy is the requesting (incoming) section's.
                    && self
                        .open_page(
                            &table,
                            self.section,
                            crate::PageKind::Filler,
                            section.filler_running,
                        )
                        .is_none()
                {
                    self.page_limit();
                    return;
                }
                self.section = next;
                self.template = section.template.clone();
                self.thread = self.template.main_thread();
                self.max_depth = self
                    .thread
                    .iter()
                    .filter_map(|i| self.template.frames.get(*i))
                    .map(|f| f.depth)
                    .max()
                    .unwrap_or_default();
                self.page_break();
            }
        }
    }
    fn page_break(&mut self) {
        if self.new_page() {
            self.page = self.snapshot.pages.len().saturating_sub(1);
            self.pos = 0;
        } else {
            self.page_limit();
        }
    }
    fn page_limit(&mut self) {
        if !self.limit_hit {
            self.limit_hit = true;
            self.snapshot.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::PAGE_LIMIT,
                Subject::Document,
                format!(
                    "the text needs more than {} pages; the rest was left out",
                    self.snapshot.pages.len()
                ),
            ));
        }
    }

    /// Starts a block. Blocks that take one step are laid out here and give
    /// `None`; paragraphs, flowed images and tables give their machine.
    fn begin_block(&mut self, doc: &Document, node: NodeId) -> Option<Active> {
        self.transition(doc, node);
        let flow = self;
        match doc.table_role(node) {
            Ok(Some(reprise_doc::TableRole::Table(columns))) => {
                return Some(Active::Table(Box::new(TableStep::Start(node, columns))));
            }
            Ok(Some(_)) | Err(_) => {
                flow.snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    codes::TABLE_INVALID,
                    Subject::Node(node),
                    "unreadable or misplaced table container",
                ));
                return None;
            }
            Ok(None) => {}
        }
        let kind = match doc.block(node) {
            Ok(b) => b.kind,
            Err(e) => {
                flow.snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    codes::MALFORMED_BLOCK,
                    Subject::Node(node),
                    e.to_string(),
                ));
                return None;
            }
        };
        let paragraph = || Some(Active::Paragraph(Box::new(paragraph::Paragraph::new(node))));
        match kind {
            BlockKind::Paragraph => return paragraph(),
            BlockKind::Image => {
                if !flow.region_owners.contains(&node) {
                    let follows = doc.relations().iter().any(|(_, r)| {
                        r.as_ref().is_ok_and(|r| {
                            r.owner == Some(node)
                                && r.schema == reprise_doc::relation::builtin::FOLLOW
                        })
                    });
                    if !follows {
                        return paragraph();
                    }
                    if let Some(image) = flow.annotation(doc, node) {
                        flow.pending.push(image);
                    }
                }
            }
            BlockKind::Annotation => {
                if !flow.region_owners.contains(&node)
                    && let Some(annotation) = flow.annotation(doc, node)
                {
                    flow.pending.push(annotation);
                }
            }
        }
        None
    }

    /// One unit of a block in progress. Returns the units used and whether
    /// the block is finished.
    fn block_unit(
        &mut self,
        doc: &Document,
        block: &mut Active,
        allowance: usize,
    ) -> (usize, bool) {
        match block {
            Active::Paragraph(p) => {
                let used = self.paragraph_unit(doc, p, allowance);
                (used, p.is_finished())
            }
            Active::Table(t) => (1, self.table_unit(doc, t)),
        }
    }

    /// One phase of a table: its start, the cells of a group of rows, the
    /// columns, or placing one row group. `Flow::table` runs the same phases
    /// back to back.
    fn table_unit(&mut self, doc: &Document, step: &mut TableStep) -> bool {
        let (rows, bytes) = (TABLE_ROW_GROUP, crate::shaping::SHAPE_CHUNK_BYTES);
        match step {
            TableStep::Start(node, columns) => match self.table_begin(doc, *node, columns) {
                Some(build) => *step = TableStep::Rows(build),
                None => return true,
            },
            TableStep::Rows(build) => {
                if self.table_rows(doc, build, rows, bytes) {
                    let node = build.node();
                    let placeholder = TableStep::Place {
                        node,
                        headers: Vec::new(),
                        bodies: Default::default(),
                        next_header: 0,
                        widths: Vec::new(),
                        repeat: None,
                        current: None,
                    };
                    let TableStep::Rows(build) = std::mem::replace(step, placeholder) else {
                        return true;
                    };
                    let (mut groups, widths) = self.table_columns(build);
                    let split = groups.iter().take_while(|g| g.header).count();
                    let bodies = groups.split_off(split).into();
                    *step = TableStep::Place {
                        node,
                        headers: groups,
                        bodies,
                        next_header: 0,
                        widths,
                        repeat: None,
                        current: None,
                    };
                }
            }
            TableStep::Place {
                node,
                headers,
                bodies,
                next_header,
                widths,
                repeat,
                current,
            } => {
                // As `place_groups`: the repeat state starts when placing does.
                let repeat = repeat.get_or_insert_with(|| {
                    crate::table_flow::Repeat::new(headers.len(), self.used() > Length::ZERO)
                });
                if current.is_none() {
                    if let Some(group) = headers.get(*next_header).cloned() {
                        *current = Some((*next_header, crate::table_flow::GroupStep::new(group)));
                        *next_header = next_header.saturating_add(1);
                    } else if let Some(group) = bodies.pop_front() {
                        *current = Some((usize::MAX, crate::table_flow::GroupStep::new(group)));
                    }
                }
                let instance = self.fields().instance;
                if let Some((index, group)) = current
                    && group.unit(self, doc, *node, *index, headers, widths, repeat, instance)
                {
                    *current = None;
                }
                return *next_header >= headers.len() && bodies.is_empty() && current.is_none();
            }
        }
        false
    }

    fn cursor(self) -> Cursor {
        Cursor {
            template: self.template,
            thread: self.thread,
            max_depth: self.max_depth,
            page_base: self.page_base,
            fills: self.fills,
            page: self.page,
            pos: self.pos,
            limit_hit: self.limit_hit,
            pending: self.pending,
            region_owners: self.region_owners,
            previous: self.previous,
            active: None,
            keeps: self.keeps,
            catalog: self.catalog,
            source: self.source,
            domain: self.domain,
            section_table: self.section_table,
            section: self.section,
            boundary_node: self.boundary_node,
            band: self.band,
            band_start: None,
            band_nodes: Vec::new(),
            balance: None,
            queued_node: None,
            open_band: self.open_band,
            running: self.running,
            fields: self.fields,
        }
    }

    /// Adds what running regions reported to the diagnostics, once each.
    fn merge_running_notes(&mut self) {
        let Some(sections) = self.snapshot.sections.as_mut() else {
            return;
        };
        let notes = std::mem::take(&mut sections.notes);
        for note in notes {
            if !self
                .snapshot
                .diagnostics
                .iter()
                .any(|d| crate::running::compose::same(d, &note))
            {
                self.snapshot.diagnostics.push(note);
            }
        }
    }

    /// Makes the next page of section `index`: its regions are composed, what
    /// they came to is noted for the memo, and the factory makes the page. The
    /// template its body frames came from is returned.
    fn open_page(
        &mut self,
        table: &std::sync::Arc<crate::sections::Table>,
        index: usize,
        kind: crate::PageKind,
        filler_regions: bool,
    ) -> Option<std::sync::Arc<ResolvedTemplate>> {
        let section = table.sections.get(index)?;
        let page = self.snapshot.pages.len();
        let first_owned = crate::running::attach::first_owned(self.snapshot, section.id(), kind);
        let ctx = running_ctx(&self.running, table, self.engine, self.doc, self.plan).map(|c| {
            crate::running::attach::Ctx {
                filler_regions,
                ..c
            }
        });
        let regions = ctx.as_ref().and_then(|ctx| {
            crate::running::attach::prepare(ctx, table, index, section, page, kind, first_owned)
        });
        if ctx.is_some() {
            self.read(|| crate::reads::PlanRead::Regions {
                spec: crate::running::attach::PageSpec {
                    section: section.id(),
                    life: section.life().into(),
                    kind,
                    first_owned,
                    page,
                    filler_regions,
                },
                outcome: regions.as_ref().map(|r| r.outcome()),
            });
        }
        crate::pages::PageFactory::push_prepared(
            self.engine,
            table,
            section,
            kind,
            self.snapshot,
            first_owned,
            regions,
        )
    }

    /// Makes the next page from the template, unless the limit is reached.
    fn new_page(&mut self) -> bool {
        if !self.source.can_page() {
            return false;
        }
        let base = self.snapshot.frames.len();
        let mut page_template = None;
        let pushed = if let Some(table) = self.section_table.clone().filter(|s| s.enabled) {
            page_template = self.open_page(&table, self.section, crate::PageKind::Content, false);
            page_template.is_some()
        } else {
            crate::pages::PageFactory::push(self.engine, &self.template, self.snapshot)
        };
        if !pushed {
            return false;
        }
        if let Some(template) = page_template {
            self.template = template;
            self.thread = self.template.main_thread();
            self.max_depth = self
                .thread
                .iter()
                .filter_map(|i| self.template.frames.get(*i))
                .map(|f| f.depth)
                .max()
                .unwrap_or_default();
            self.band = 0;
        }
        while self.page_base.len() < self.snapshot.pages.len().saturating_sub(1) {
            self.page_base.push(base);
        }
        self.page_base.push(base);
        self.catalog.extend(self.snapshot);
        self.resize_fills();
        true
    }
    fn resize_fills(&mut self) {
        self.fills.resize(
            self.snapshot.frames.len(),
            Fill {
                used: Length::ZERO,
                empty: true,
                after: Length::ZERO,
                anchored: true,
            },
        );
    }

    /// The snapshot index of the frame the flow is in.
    pub(crate) fn current_template_index(&self) -> usize {
        self.thread.get(self.pos).copied().unwrap_or(0)
    }
    pub(crate) fn frame_index(&self) -> usize {
        self.source
            .single_index()
            .or_else(|| self.thread.get(self.pos).and_then(|&t| self.main_index(t)))
            .unwrap_or(self.snapshot.frames.len())
    }
    fn main_index(&self, t: usize) -> Option<usize> {
        if self.section_table.as_ref().is_some_and(|s| s.enabled) {
            let column = self
                .template
                .frames
                .iter()
                .take(t)
                .filter(|f| f.is_main_flow())
                .count();
            self.catalog.index(&crate::pages::FrameAddr::Main {
                page: self.page,
                band: self.band,
                column,
            })
        } else {
            self.catalog.ordinal(self.page, t)
        }
    }

    /// Moves on to the next frame of the thread, or the first of a new page.
    /// False when the page limit stops it, which is reported once.
    pub(crate) fn advance(&mut self) -> bool {
        if !self.source.next() {
            self.limit_hit = true;
            return false;
        }
        if self.pos + 1 < self.thread.len() {
            self.pos += 1;
            return true;
        }
        if self.new_page() {
            self.page = self.snapshot.pages.len() - 1;
            self.pos = 0;
            return true;
        }
        if !self.limit_hit {
            self.limit_hit = true;
            self.snapshot.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::PAGE_LIMIT,
                Subject::Document,
                format!(
                    "the text needs more than {} pages; the rest was left out",
                    self.snapshot.pages.len()
                ),
            ));
        }
        false
    }

    fn frame_reference(&self, index: usize) -> Option<crate::pages::FrameRef> {
        self.catalog.reference(index).cloned().or_else(|| {
            let mut frame =
                crate::pages::PageFactory::future_reference(&self.template, self.snapshot, index)?;
            if let Some(section) = self
                .section_table
                .as_ref()
                .and_then(|s| s.sections.get(self.section))
            {
                frame.provenance.section = section.id();
                frame.provenance.life = section.life().into();
            }
            Some(frame)
        })
    }
    pub(crate) fn exclusions(&self, index: usize) -> Option<Vec<Rect<FrameSpace>>> {
        let frame = self.frame_reference(index);
        let own = frame
            .as_ref()
            .and_then(|frame| self.plan.exclusions_for(frame))
            .cloned();
        if let Some(frame) = &frame {
            self.read(|| crate::reads::PlanRead::Exclusions {
                addr: frame.address.clone(),
                provenance: frame.provenance.clone(),
                value: own.clone(),
            });
        }
        let mut boxes = own.unwrap_or_default();
        boxes.extend(
            self.plan
                .physical_exclusions_for(&self.catalog, self.snapshot, index),
        );
        (!boxes.is_empty()).then_some(boxes)
    }
    pub(crate) fn depth(&self, index: usize, full: Length) -> Length {
        let frame = self.frame_reference(index);
        let reserved = frame
            .as_ref()
            .and_then(|frame| self.plan.reservation_for(frame));
        if let Some(frame) = &frame {
            self.read(|| crate::reads::PlanRead::Reservation {
                addr: frame.address.clone(),
                provenance: frame.provenance.clone(),
                value: reserved,
            });
        }
        (full
            - reserved
                .unwrap_or_default()
                .max(
                    self.plan
                        .physical_reservation_for(&self.catalog, self.snapshot, index),
                ))
        .max(Length::ZERO)
    }

    pub(crate) fn used(&self) -> Length {
        self.fills
            .get(self.frame_index())
            .map_or(Length::ZERO, |f| f.used)
    }

    pub(crate) fn set_used(&mut self, used: Length) {
        let index = self.frame_index();
        if let Some(fill) = self.fills.get_mut(index) {
            fill.used = used;
            fill.empty = false;
            fill.after = Length::ZERO;
            fill.anchored = false;
        }
    }

    /// Tables retain rectangular columns: pass below all float exclusions in
    /// this frame instead of overlapping them or changing column widths.
    pub(crate) fn table_top(&self) -> Length {
        self.exclusions(self.frame_index())
            .into_iter()
            .flatten()
            .map(|r| r.origin.y + r.height)
            .fold(self.used(), Length::max)
    }

    /// Style is frozen once the block places its first line. A continuation
    /// keeps that starting frame's context even when later widths differ.
    fn paragraph_context(&self) -> ResolutionContext {
        let frame = self
            .thread
            .get(self.pos)
            .and_then(|&i| self.template.frames.get(i));
        resolution_context(
            self.engine,
            &self.template,
            frame,
            frame.map_or(Length::ZERO, |f| f.width),
        )
    }

    /// Composes an annotation for a relation to place. The page it lands on
    /// isn't known yet, so it is composed at the margin frame's width, or the
    /// main frame's when the template has no margin frame (then no relation
    /// can place it, and that is reported when one tries).
    pub(crate) fn annotation(&mut self, doc: &Document, node: NodeId) -> Option<Pending> {
        let Some(evaluation) = self.evaluation else {
            return self.annotation_inner(doc, node);
        };
        let key = crate::incremental::AnnotationKey::new(doc, node, &self.template);
        if let Some((pending, notes)) = evaluation.annotation_hit(node, &key, self.engine) {
            self.snapshot.diagnostics.extend(notes);
            return pending;
        }
        let from = self.snapshot.diagnostics.len();
        let pending = self.annotation_inner(doc, node);
        let notes = self
            .snapshot
            .diagnostics
            .get(from..)
            .unwrap_or_default()
            .to_vec();
        evaluation.annotation_miss(node, key, pending.clone(), notes);
        pending
    }
    fn annotation_inner(&mut self, doc: &Document, node: NodeId) -> Option<Pending> {
        let engine = self.engine;
        let subject = Subject::Node(node);
        let frame = self
            .template
            .frames
            .iter()
            .find(|f| f.role == reprise_doc::FrameRole::Margin)
            .or_else(|| {
                self.thread
                    .first()
                    .and_then(|&i| self.template.frames.get(i))
            });
        let width = self
            .template
            .margin_width()
            .unwrap_or_else(|| frame.map_or(Length::ZERO, |f| f.width));
        let ctx = resolution_context(engine, &self.template, frame, width);
        let prepared = prepare_cached(
            engine,
            doc,
            node,
            &ctx,
            &mut self.snapshot.diagnostics,
            self.evaluation,
        )?;
        let (indents, reduced) = prepared.indents(width);
        if reduced {
            self.snapshot.diagnostics.push(indent_clamped(&subject));
        }
        let composed = prepared.compose(
            engine,
            &Indented {
                base: Measure(width),
                indents,
                first_line: true,
                reversed: prepared.base_level % 2 == 1,
            },
            0, // Provisional: the relation that places the block sets the frame.
            0,
            Length::ZERO,
            &subject,
            &mut self.snapshot.diagnostics,
            self.evaluation,
            &crate::inline::FieldEnv {
                values: Some(&self.plan.fields),
                ..crate::inline::FieldEnv::UNPLACED
            },
        );
        if let Some(rest) = composed.rest {
            unplaced(
                &mut self.snapshot.diagnostics,
                &subject,
                rest..prepared.text.len(),
            );
        }
        Some(Pending {
            extent: composed.block_end,
            block: prepared.into_block(composed.lines),
        })
    }
}

/// All pages currently use the same resolved template. Named bases include
/// every frame on that page; block height stays indefinite until composed.
pub(crate) fn resolution_context(
    engine: &Engine,
    template: &ResolvedTemplate,
    frame: Option<&ResolvedFrame>,
    width: Length,
) -> ResolutionContext {
    let mut ctx = ResolutionContext::default()
        .with_medium(Extent::definite(engine.medium.width, engine.medium.height))
        .with_page(Extent::definite(template.width, template.height));
    for f in &template.frames {
        ctx = ctx.with_named_frame(&f.name, Extent::definite(f.width, f.depth));
    }
    if let Some(f) = frame {
        ctx.writing_mode = f.writing_mode;
        ctx = ctx.with_current_frame(&f.name, Extent::definite(f.width, f.depth));
    }
    ctx.with_block(Extent::auto_height(width))
}

pub(crate) fn unplaced(diagnostics: &mut Vec<Diagnostic>, subject: &Subject, bytes: Range<usize>) {
    let mut d = Diagnostic::new(
        Severity::Error,
        codes::TEXT_UNPLACED,
        subject.clone(),
        format!("bytes {}..{} were not placed", bytes.start, bytes.end),
    );
    d.bytes = Some(bytes);
    diagnostics.push(d);
}

pub(crate) fn indent_clamped(subject: &Subject) -> Diagnostic {
    Diagnostic::new(
        Severity::Warning,
        codes::INDENT_CLAMPED,
        subject.clone(),
        "indents would leave less than a quarter of the frame's inline size; they were reduced",
    )
}

/// The distinct visual lines of some placed fragments, as `(top, bottom)` on
/// the block axis, in order. Fragments of one line share their top.
pub(crate) fn rows(lines: &[LineLayout]) -> Vec<(Length, Length)> {
    let mut out: Vec<(Length, Length)> = Vec::new();
    for l in lines {
        let top = l.rect.origin.y;
        let bottom = top + l.rect.height;
        match out.last_mut() {
            Some(last) if last.0 == top => last.1 = last.1.max(bottom),
            _ => out.push((top, bottom)),
        }
    }
    out
}

pub(crate) fn prepare_cached(
    engine: &Engine,
    doc: &Document,
    node: NodeId,
    ctx: &ResolutionContext,
    diagnostics: &mut Vec<Diagnostic>,
    evaluation: Option<&crate::incremental::Evaluation>,
) -> Option<Prepared> {
    match evaluation {
        Some(e) => e.prepare(engine, doc, node, ctx, diagnostics),
        None => prepare(engine, doc, node, ctx, diagnostics),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PositionKey {
    band: usize,
    source: crate::pages::FrameSource,
    page: usize,
    pos: usize,
    limit_hit: bool,
    pages: usize,
    frames: usize,
    fills: Vec<Fill>,
    section: Option<(reprise_doc::sections::SectionId, String)>,
    owner: Option<(reprise_doc::sections::SectionId, i64)>,
}
#[derive(Clone)]
pub(crate) struct Delta {
    band: usize,
    source: crate::pages::FrameSource,
    template: std::sync::Arc<ResolvedTemplate>,
    thread: Vec<usize>,
    max_depth: Length,
    sections: Option<crate::SectionsLayout>,
    pages: Vec<PageLayout>,
    frames: Vec<FrameLayout>,
    pub(crate) blocks: Vec<BlockLayout>,
    pub(crate) block_pages: std::collections::BTreeSet<usize>,
    diagnostics: Vec<Diagnostic>,
    fills: Vec<Fill>,
    page_base: Vec<usize>,
    page: usize,
    pos: usize,
    limit_hit: bool,
    tail: usize,
}
#[derive(Clone)]
pub(crate) struct Mark {
    pages: usize,
    frames: usize,
    blocks: usize,
    diagnostics: usize,
    tail: usize,
}
impl Flow<'_> {
    pub(crate) fn position_key(&self) -> PositionKey {
        let tail = self.page_base.last().copied().unwrap_or(0);
        PositionKey {
            band: self.band,
            source: self.source.clone(),
            page: self.page,
            pos: self.pos,
            limit_hit: self.limit_hit,
            pages: self.snapshot.pages.len(),
            frames: self.snapshot.frames.len(),
            fills: self.fills.get(tail..).unwrap_or_default().to_vec(),
            section: self
                .section_table
                .as_ref()
                .filter(|s| s.enabled)
                .and_then(|s| s.sections.get(self.section))
                .map(|s| (s.id(), s.life().into())),
            owner: self
                .snapshot
                .sections
                .as_ref()
                .and_then(|s| s.pages.get(self.page))
                .map(|p| (p.section, p.counter)),
        }
    }
    /// What fields compose with on the page being filled.
    pub(crate) fn fields(&self) -> crate::inline::FieldEnv<'_> {
        match &self.fields {
            Some(ctx) => {
                let mut env = crate::inline::FieldEnv::at(
                    ctx.page,
                    ctx.pages,
                    ctx.instance,
                    Some(&self.plan.fields),
                );
                env.placed = true;
                env.trace = ctx.trace.as_deref();
                env
            }
            None => {
                let mut env = crate::inline::FieldEnv::at(
                    self.page,
                    self.page.saturating_add(1),
                    crate::inline::FieldInstance::Single,
                    Some(&self.plan.fields),
                );
                env.trace = self.recording().then_some(&self.field_log);
                env
            }
        }
    }

    /// Whether a paragraph's reads are being recorded.
    fn recording(&self) -> bool {
        self.log.borrow().is_some()
    }

    /// Notes one read, built only when recording.
    pub(crate) fn read(&self, read: impl FnOnce() -> crate::reads::PlanRead) {
        if let Some(log) = self.log.borrow_mut().as_mut() {
            crate::reads::note(log, read());
        }
    }

    /// Starts recording, continuing from `earlier` reads.
    pub(crate) fn record_from(&self, earlier: Vec<crate::reads::PlanRead>) {
        *self.log.borrow_mut() = Some(earlier);
    }

    /// Stops recording and returns everything read: plan entries, then field
    /// values.
    pub(crate) fn take_reads(&self) -> Vec<crate::reads::PlanRead> {
        let mut reads = self.log.borrow_mut().take().unwrap_or_default();
        let fields = std::mem::take(&mut *self.field_log.borrow_mut());
        for (key, value) in fields.fields {
            crate::reads::note(&mut reads, crate::reads::PlanRead::Field { key, value });
        }
        reads
    }

    /// Whether every read of a memoised transition still holds: the plan
    /// entries answer as they did, and the regions of the pages it made come to
    /// the same.
    pub(crate) fn reads_hold(&self, reads: &[crate::reads::PlanRead]) -> bool {
        reads.iter().all(|read| match read.holds(self.plan) {
            Some(holds) => holds,
            None => {
                let crate::reads::PlanRead::Regions { spec, outcome } = read else {
                    return false;
                };
                let Some(table) = self.section_table.as_ref() else {
                    return false;
                };
                match running_ctx(&self.running, table, self.engine, self.doc, self.plan) {
                    Some(ctx) => {
                        crate::running::attach::outcome_of(&ctx, table, spec).as_ref()
                            == Some(outcome)
                    }
                    None => outcome.is_none(),
                }
            }
        })
    }

    pub(crate) fn plan(&self) -> &Plan {
        self.plan
    }
    fn mark(&self) -> Mark {
        Mark {
            pages: self.snapshot.pages.len(),
            frames: self.snapshot.frames.len(),
            blocks: self.snapshot.blocks.len(),
            diagnostics: self.snapshot.diagnostics.len(),
            tail: self.page_base.last().copied().unwrap_or(0),
        }
    }
    fn delta(&self, before: Mark) -> Delta {
        Delta {
            band: self.band,
            source: self.source.clone(),
            template: self.template.clone(),
            thread: self.thread.clone(),
            max_depth: self.max_depth,
            sections: self
                .snapshot
                .sections
                .as_ref()
                .map(|s| crate::SectionsLayout {
                    pages: s.pages.get(before.pages..).unwrap_or_default().to_vec(),
                    // Instances and what they reported are made again from the
                    // running memo when the transition is replayed.
                    running: Vec::new(),
                    notes: Vec::new(),
                    rules: s
                        .rules
                        .iter()
                        .filter(|r| r.page >= before.pages)
                        .cloned()
                        .collect(),
                }),
            block_pages: self
                .snapshot
                .blocks
                .get(before.blocks..)
                .unwrap_or_default()
                .iter()
                .flat_map(|b| &b.lines)
                .filter_map(|l| self.snapshot.frame(l.frame))
                .map(|f| f.page)
                .collect(),
            pages: self
                .snapshot
                .pages
                .get(before.pages..)
                .unwrap_or_default()
                .to_vec(),
            frames: self
                .snapshot
                .frames
                .get(before.frames..)
                .unwrap_or_default()
                .to_vec(),
            blocks: self
                .snapshot
                .blocks
                .get(before.blocks..)
                .unwrap_or_default()
                .to_vec(),
            diagnostics: self
                .snapshot
                .diagnostics
                .get(before.diagnostics..)
                .unwrap_or_default()
                .to_vec(),
            fills: self.fills.get(before.tail..).unwrap_or_default().to_vec(),
            page_base: self
                .page_base
                .get(before.pages..)
                .unwrap_or_default()
                .to_vec(),
            page: self.page,
            pos: self.pos,
            limit_hit: self.limit_hit,
            tail: before.tail,
        }
    }
    /// Makes the running instances of replayed pages again, from the running
    /// memo: a transition that made a page is replayed when what the page's
    /// regions came to is unchanged, and their text may not be.
    fn reinstall_regions(&mut self, from: usize) {
        let Some(table) = self.section_table.clone() else {
            return;
        };
        let Some(ctx) = running_ctx(&self.running, &table, self.engine, self.doc, self.plan) else {
            return;
        };
        for page in from..self.snapshot.pages.len() {
            crate::running::attach::reinstall(&ctx, &table, self.snapshot, page);
        }
    }
    fn apply_delta(&mut self, delta: Delta) -> bool {
        let first_replayed = self.snapshot.pages.len();
        if !crate::pages::PageFactory::replay(self.engine, self.snapshot, delta.pages, delta.frames)
        {
            return false;
        }
        crate::pages::PageFactory::replay_sections(self.snapshot, delta.sections);
        self.reinstall_regions(first_replayed);
        self.catalog.extend(self.snapshot);
        self.snapshot.blocks.extend(delta.blocks);
        self.snapshot.diagnostics.extend(delta.diagnostics);
        self.fills.truncate(delta.tail);
        self.fills.extend(delta.fills);
        self.page_base.extend(delta.page_base);
        self.page = delta.page;
        self.pos = delta.pos;
        self.limit_hit = delta.limit_hit;
        self.band = delta.band;
        self.source = delta.source;
        self.template = delta.template;
        self.thread = delta.thread;
        self.max_depth = delta.max_depth;
        true
    }
}

/// A block shaped and ready to compose into one or more regions.
#[derive(Clone)]
pub(crate) struct Prepared {
    paint: Vec<PaintRun>,
    node: NodeId,
    kind: BlockKind,
    pub(crate) style: ComputedStyle,
    pub(crate) text: String,
    items: Vec<Item>,
    levels: Vec<u8>,
    base_level: u8,
    pub(crate) shaped: ShapedText,
    pub(crate) breaks: Vec<Break>,
    /// Empty lines take their vertical metrics from the style's own face.
    fallback: Option<(FaceId, Length)>,
    image: Option<crate::ImageLayout>,
    /// Inline objects (see `docs/inline.md`) and the frame they are sized for.
    pub(crate) atoms: Vec<crate::inline::PreparedAtom>,
    pub(crate) atom_frame: crate::inline::AtomFrame,
    /// A list item's marker and indents.
    pub(crate) marker: Option<crate::list_marker::PreparedMarker>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaintRun {
    range: std::ops::Range<usize>,
    color: [u8; 4],
    decoration: reprise_doc::Decoration,
}

/// What composing a block into one region produced.
pub(crate) struct Composed {
    pub(crate) lines: Vec<LineLayout>,
    /// Where the text continues, if the region ended before it did.
    pub(crate) rest: Option<usize>,
    pub(crate) block_end: Length,
}

/// Resolves a block's style and shapes its text. `None` when it can't be laid
/// out at all; the diagnostics say why.
pub(crate) fn prepare(
    engine: &Engine,
    doc: &Document,
    node: NodeId,
    ctx: &ResolutionContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Prepared> {
    prepare_with(engine, doc, node, ctx, diagnostics, None)
}

pub(crate) fn prepare_with(
    engine: &Engine,
    doc: &Document,
    node: NodeId,
    ctx: &ResolutionContext,
    diagnostics: &mut Vec<Diagnostic>,
    evaluation: Option<&crate::incremental::Evaluation>,
) -> Option<Prepared> {
    // The stages live in `stage` so a job can run them in separate units.
    let finished = match evaluation {
        Some(e) => stage::run(engine, doc, node, ctx, Some(e), &*e.workers()),
        None => stage::run(engine, doc, node, ctx, None, &crate::workers::Serial),
    };
    diagnostics.extend(finished.notes);
    finished.value
}

impl Prepared {
    pub(crate) fn image_width(&self) -> Option<Length> {
        self.image.as_ref().map(|i| i.rect.width)
    }

    pub(crate) fn fit_image_width(&mut self, width: Length, diagnostics: &mut Vec<Diagnostic>) {
        if let Some(image) = &mut self.image
            && image.rect.width > width.max(Length::ZERO)
        {
            let limit = width.max(Length::ZERO);
            image.rect.height = image.rect.height.mul_ratio(limit.0, image.rect.width.0);
            image.rect.width = limit;
            diagnostics.push(Diagnostic::new(
                Severity::Warning,
                codes::IMAGE_SIZE,
                Subject::Node(self.node),
                "image scaled proportionally to its table cell's inline extent",
            ));
        }
    }
    /// The paragraph's indents, bounded for a measure of `width`; the flag
    /// says they had to be reduced (`layout.indent-clamped`).
    pub(crate) fn indents(&self, width: Length) -> (Indents, bool) {
        Indents::bounded(
            if self.marker.is_some() {
                Length::ZERO
            } else {
                self.style.indent_start
            },
            self.style.indent_end,
            if self.marker.is_some() {
                Length::ZERO
            } else {
                self.style.indent_first
            },
            width,
        )
    }
    pub(crate) fn line_height(&self) -> Length {
        self.image
            .as_ref()
            .map_or(self.style.line_height, |i| i.rect.height)
    }
    pub(crate) fn node(&self) -> NodeId {
        self.node
    }
    /// Composes the text from byte `start` into one region, with its first
    /// line `block_start` down the frame's block axis. Lines are in frame
    /// `frame`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compose(
        &self,
        engine: &Engine,
        geometry: &dyn GeometryProvider,
        frame: usize,
        start: usize,
        block_start: Length,
        subject: &Subject,
        diagnostics: &mut Vec<Diagnostic>,
        evaluation: Option<&crate::incremental::Evaluation>,
        fields: &crate::inline::FieldEnv<'_>,
    ) -> Composed {
        if let Some(image) = &self.image {
            use reprise_compose::{Available, BreakReason, Explanation, LineQuery};
            let height = image.rect.height;
            let room = geometry.available(&LineQuery {
                line: 0,
                block_offset: block_start,
                line_height: height,
                previous: &[],
            });
            let interval = match room {
                Available::Room(intervals) => intervals
                    .into_iter()
                    .find(|r| r.width() >= image.rect.width),
                _ => None,
            };
            let Some(interval) = interval else {
                return Composed {
                    lines: Vec::new(),
                    rest: Some(start),
                    block_end: block_start,
                };
            };
            let rect = Rect::new(
                Point::new(interval.start, block_start),
                image.rect.width,
                height,
            );
            return Composed {
                lines: vec![LineLayout {
                    available: interval,
                    alignment: None,
                    frame,
                    text: 0..self.text.len(),
                    preview: self.text.clone(),
                    rect,
                    baseline: block_start,
                    width: image.rect.width,
                    runs: Vec::new(),
                    atoms: Vec::new(),
                    marker: None,
                    explanation: Explanation {
                        reason: BreakReason::End,
                        score: None,
                        adjustment: Adjustment::default(),
                        reshaped: false,
                    },
                }],
                rest: None,
                block_end: block_start + height,
            };
        }
        if let Some(e) = evaluation {
            e.composer_call();
        }
        let shaper = Shaper {
            text: &self.text,
            items: &self.items,
            fonts: &engine.fonts,
            adapter: engine.shaper.as_ref(),
        };
        // A list item's text is inset from the room's logical inline start.
        let inset = self
            .marker
            .as_ref()
            .map(|m| crate::list_marker::Inset::new(geometry, m, start == 0, self.style.size));
        let geometry: &dyn GeometryProvider = match &inset {
            Some(inset) => inset,
            None => geometry,
        };
        // Inline objects become advance cells of their boxes' advance.
        let sized = crate::inline::size_atoms(
            engine,
            &self.atoms,
            &self.items,
            self.style.size,
            self.atom_frame,
            fields,
            start,
            subject,
            diagnostics,
        );
        let substituted;
        let shaped = if sized.is_empty() {
            &self.shaped
        } else {
            let mut copy = self.shaped.clone();
            crate::inline::substitute(&mut copy.runs, &sized);
            substituted = copy;
            &substituted
        };
        let reshape = crate::inline::AtomReshape {
            inner: &shaper,
            sized: &sized,
        };
        let request = ComposeRequest {
            text: &self.text,
            shaped,
            reshape: &reshape,
            breaks: &self.breaks,
            line_height: self.style.line_height,
            geometry,
            start,
            block_start,
        };
        let extents = (!sized.is_empty()).then(|| {
            let (top, bottom) = self.strut(engine);
            reprise_compose::LineExtents {
                strut_top: top,
                strut_bottom: bottom,
                objects: sized
                    .iter()
                    .map(|(&at, s)| {
                        let (ascent, descent) = s.line_extent();
                        (at, ascent, descent)
                    })
                    .collect(),
            }
        });
        // Justified paragraphs use the optimal composer's justified mode,
        // whatever the engine's composer is (`docs/paragraph-styles.md`).
        let justified = Optimal::justified();
        let composer: &dyn Composer =
            if self.style.alignment == Some(reprise_doc::marks::Alignment::Justify) {
                &justified
            } else {
                engine.composer.as_ref()
            };
        let composition = if self
            .text
            .chars()
            .any(|c| c == '\t' || reprise_doc::marks::is_line_break(c))
            || extents.as_ref().is_some_and(|e| e.varies())
        {
            reprise_compose::compose_marks_with(
                &request,
                &self.style.tabs.clone().unwrap_or_default(),
                composer,
                extents.as_ref(),
            )
        } else {
            composer.compose(&request)
        };
        diagnostics.extend(
            composition
                .notes
                .into_iter()
                .map(|n| Diagnostic::from_note(n, subject.clone())),
        );
        if let Some(e) = evaluation {
            e.composed(self.text.len(), composition.lines.len());
        }
        let mut lines: Vec<LineLayout> = composition
            .lines
            .into_iter()
            .map(|l| line_layout(engine, self, frame, l, subject, diagnostics, &sized))
            .collect();
        if let (Some(inset), Some(marker)) = (&inset, &self.marker)
            && start == 0
        {
            if inset.clamped.get() {
                diagnostics.push(Diagnostic::new(
                    Severity::Warning,
                    codes::LIST_INDENT,
                    subject.clone(),
                    "a list indent leaves less than one em of room; reduced",
                ));
            }
            if let Some(first) = lines.first_mut() {
                let edges = inset.first_edges(first.rect.origin.y).unwrap_or_else(|| {
                    // The composer asked at another offset: rebuild from the line.
                    let inset = marker.first_inset;
                    if marker.rtl {
                        crate::list_marker::Edges {
                            lo: first.available.start,
                            hi: first.available.end + inset,
                            inset,
                        }
                    } else {
                        crate::list_marker::Edges {
                            lo: first.available.start - inset,
                            hi: first.available.end,
                            inset,
                        }
                    }
                });
                first.marker = crate::list_marker::place(engine, marker, first, edges);
            }
        }
        Composed {
            lines,
            rest: composition.rest,
            block_end: composition.block_end,
        }
    }

    /// The strut's extent above and below the baseline: half-leading
    /// around the paragraph face's ascent and descent.
    fn strut(&self, engine: &Engine) -> (Length, Length) {
        let face = self
            .fallback
            .clone()
            .or_else(|| self.items.first().map(|i| (i.face.clone(), i.size)));
        let (ascent, descent) = face
            .and_then(|(face, size)| {
                let m = engine.fonts.get(&face).ok()?.metrics();
                Some((
                    Length::from_font_units(m.ascent, size, m.units_per_em),
                    Length::from_font_units(m.descent, size, m.units_per_em),
                ))
            })
            .unwrap_or_default();
        let height = self.style.line_height;
        let top = (height - ascent - descent).mul_ratio(1, 2) + ascent;
        (top, height - top)
    }

    pub(crate) fn into_block(self, lines: Vec<LineLayout>) -> BlockLayout {
        let mut image = self.image;
        if let (Some(image), Some(line)) = (&mut image, lines.first()) {
            image.frame = line.frame;
            image.rect = line.rect;
        }
        BlockLayout {
            node: self.node,
            kind: self.kind,
            style: self.style,
            text: self.text,
            lines,
            base_level: self.base_level,
            image,
        }
    }
}

fn line_layout(
    engine: &Engine,
    prepared: &Prepared,
    frame: usize,
    fragment: LineFragment,
    subject: &Subject,
    diagnostics: &mut Vec<Diagnostic>,
    sized: &std::collections::BTreeMap<usize, crate::inline::Sized>,
) -> LineLayout {
    let text = &prepared.text;
    let fallback = prepared.fallback.as_ref();
    // Half-leading: the tallest ascent and descent on the line are centred
    // in the line box.
    let metrics = |face: &FaceId, size: Length| {
        engine.fonts.get(face).ok().map(|f| {
            let m = f.metrics();
            (
                Length::from_font_units(m.ascent, size, m.units_per_em),
                Length::from_font_units(m.descent, size, m.units_per_em),
            )
        })
    };
    let mut extents: Vec<(Length, Length)> = fragment
        .runs
        .iter()
        .filter_map(|r| metrics(&r.face, r.size))
        .collect();
    if extents.is_empty() {
        extents.extend(fallback.and_then(|(face, size)| metrics(face, *size)));
    }
    let ascent = extents.iter().map(|e| e.0).max().unwrap_or_default();
    let descent = extents.iter().map(|e| e.1).max().unwrap_or_default();
    // A line with inline objects keeps the strut's baseline unless an
    // object rises above it; composition made the line tall enough.
    let atom_top = sized
        .range(fragment.text.clone())
        .map(|(_, s)| s.line_extent().0)
        .max();
    let half_leading = match atom_top {
        None => (fragment.height - ascent - descent).mul_ratio(1, 2),
        Some(top) => {
            let strut_top = (prepared.style.line_height - ascent - descent).mul_ratio(1, 2);
            strut_top.max(top - ascent)
        }
    };

    let mut visual_runs = match reorder_line(
        text,
        &fragment.runs,
        &prepared.levels,
        fragment.text.clone(),
        prepared.base_level,
    ) {
        Ok(runs) => runs,
        Err(note) => {
            diagnostics.push(Diagnostic::from_note(note, subject.clone()));
            let levels: Vec<u8> = fragment.runs.iter().map(|r| r.level).collect();
            visual_order(&levels)
                .into_iter()
                .filter_map(|i| fragment.runs.get(i).cloned())
                .collect()
        }
    };
    tab_leaders(
        engine,
        prepared,
        &mut visual_runs,
        fragment.available.width(),
        subject,
        diagnostics,
    );
    let adjustment = fragment.explanation.adjustment;
    let adjusted = adjustment != Adjustment::default();
    let content_end = fragment.text.start.saturating_add(
        text.get(fragment.text.clone())
            .map_or(0, |s| s.trim_end().len()),
    );
    let content = fragment.text.start..content_end;
    let width = if adjusted {
        apply_spacing(text, &content, &mut visual_runs, adjustment);
        visual_runs
            .iter()
            .flat_map(|r| &r.glyphs)
            .filter(|g| content.contains(&(g.cluster as usize)))
            .fold(Length::ZERO, |w, g| w + g.advance)
    } else {
        fragment.width
    };
    // L1 can put logically trailing spaces on the visual left of an RTL
    // line. They hang outside the used interval, rather than shifting its
    // justified content past the interval's end.
    let hanging_left = if adjusted && !content.is_empty() {
        visual_runs
            .iter()
            .flat_map(|r| &r.glyphs)
            .take_while(|g| !content.contains(&(g.cluster as usize)))
            .fold(Length::ZERO, |w, g| w + g.advance)
    } else {
        Length::ZERO
    };
    let alignment = prepared.style.alignment.or_else(|| {
        text.contains('\t')
            .then_some(reprise_doc::marks::Alignment::Start)
    });
    let surplus = (fragment.available.width() - width).max(Length::ZERO);
    let shift = match alignment {
        Some(reprise_doc::marks::Alignment::Centre) => surplus.mul_ratio(1, 2),
        Some(reprise_doc::marks::Alignment::End) if prepared.base_level.is_multiple_of(2) => {
            surplus
        }
        Some(reprise_doc::marks::Alignment::Start | reprise_doc::marks::Alignment::Justify)
            if prepared.base_level % 2 == 1 =>
        {
            surplus
        }
        _ => Length::ZERO,
    };
    let mut x = fragment.available.start + shift - hanging_left;
    let runs: Vec<PositionedRun> = visual_runs
        .into_iter()
        .map(|run| {
            let width = run.width();
            let paint = prepared
                .paint
                .iter()
                .find(|p| p.range.contains(&run.range.start));
            let decorations = engine.fonts.get(&run.face).ok().map(|face| {
                face.decoration_metrics().map(|(position, thickness)| {
                    crate::snapshot::DecorationLine {
                        offset: -run
                            .size
                            .mul_ratio(position, i32::from(face.metrics().units_per_em)),
                        thickness: run
                            .size
                            .mul_ratio(thickness, i32::from(face.metrics().units_per_em))
                            .max(Length(1)),
                    }
                })
            });
            let placed = PositionedRun {
                synthetic_oblique: run.synthetic_oblique,
                color: paint.map_or([0, 0, 0, 255], |p| p.color),
                underline: paint
                    .filter(|p| p.decoration.underline == Some(true))
                    .and(decorations.map(|d| d[0])),
                strike: paint
                    .filter(|p| p.decoration.strike == Some(true))
                    .and(decorations.map(|d| d[1])),
                upright: run.upright,
                horizontal_scale: run.horizontal_scale,
                combined: run.combined,
                range: run.range,
                face: run.face,
                size: run.size,
                level: run.level,
                x,
                width,
                glyphs: run.glyphs,
            };
            x += width;
            placed
        })
        .collect();
    let rect: Rect<FrameSpace> = Rect::new(
        Point::new(fragment.available.start + shift, fragment.block_offset),
        if alignment.is_some() {
            width
        } else {
            fragment.available.width()
        },
        fragment.height,
    );
    let baseline = fragment.block_offset + half_leading + ascent;
    let atoms = place_atoms(prepared, &runs, sized, baseline);
    LineLayout {
        available: fragment.available,
        alignment: prepared.style.alignment,
        frame,
        preview: text
            .get(fragment.text.clone())
            .unwrap_or_default()
            .trim_end()
            .to_string(),
        text: fragment.text,
        rect,
        baseline,
        width,
        explanation: fragment.explanation,
        runs,
        atoms,
        marker: None,
    }
}

/// The objects whose advance cells are on a line, at their cells' pen
/// positions, in visual order.
fn place_atoms(
    prepared: &Prepared,
    runs: &[PositionedRun],
    sized: &std::collections::BTreeMap<usize, crate::inline::Sized>,
    baseline: Length,
) -> Vec<crate::AtomLayout> {
    if sized.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for run in runs {
        let mut pen = run.x;
        for glyph in &run.glyphs {
            let at = glyph.cluster as usize;
            if glyph.id == reprise_compose::ATOM_GLYPH
                && let Some(atom) = sized.get(&at)
            {
                let color = prepared
                    .paint
                    .iter()
                    .find(|p| p.range.contains(&at))
                    .map_or([0, 0, 0, 255], |p| p.color);
                out.push(crate::AtomLayout {
                    id: atom.id,
                    at,
                    kind: atom.kind.clone(),
                    rect: Rect::new(
                        Point::new(pen, baseline - atom.size.ascent),
                        atom.size.advance,
                        atom.size.ascent + atom.size.descent,
                    ),
                    baseline,
                    instance: atom.instance,
                    color,
                    paint: atom.paint.clone(),
                });
            }
            pen += glyph.advance;
        }
    }
    out
}

/// Tab leader glyphs occupy the existing tab cell; they add no source positions.
fn tab_leaders(
    engine: &Engine,
    prepared: &Prepared,
    runs: &mut [ShapedRun],
    measure: Length,
    subject: &Subject,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(tabs) = &prepared.style.tabs else {
        return;
    };
    if !tabs.stops.iter().any(|s| s.leader.is_some()) {
        return;
    }
    let mut widths = std::collections::BTreeMap::<usize, Length>::new();
    for g in runs.iter().flat_map(|r| &r.glyphs) {
        *widths.entry(g.cluster as usize).or_default() += g.advance;
    }
    let mut cursor = Length::ZERO;
    let mut leaders = std::collections::BTreeMap::new();
    for (&at, &width) in &widths {
        if prepared.text.get(at..).is_some_and(|s| s.starts_with('\t'))
            && let Some(leader) = tabs
                .stops
                .iter()
                .find(|s| s.position.unwrap_or(measure) > cursor)
                .and_then(|s| s.leader)
        {
            leaders.insert(at, leader);
        }
        cursor += width;
    }
    let mut remaining = 8192usize;
    let mut limited = false;
    let mut missing = false;
    for run in runs {
        let mut glyphs = Vec::new();
        for glyph in &run.glyphs {
            let Some(&leader) = leaders
                .get(&(glyph.cluster as usize))
                .filter(|_| glyph.advance > Length::ZERO)
            else {
                glyphs.push(*glyph);
                continue;
            };
            let raw = leader.to_string();
            let Some(mut item) = prepared
                .items
                .iter()
                .find(|i| i.range.contains(&(glyph.cluster as usize)))
                .cloned()
            else {
                glyphs.push(*glyph);
                continue;
            };
            item.range = 0..raw.len();
            item.combined = false;
            let leader_runs = Shaper {
                text: &raw,
                items: &[item],
                fonts: &engine.fonts,
                adapter: engine.shaper.as_ref(),
            }
            .shape();
            let width: Length = leader_runs
                .runs
                .iter()
                .flat_map(|r| &r.glyphs)
                .map(|g| g.advance)
                .sum();
            if width <= Length::ZERO
                || leader_runs
                    .runs
                    .iter()
                    .flat_map(|r| &r.glyphs)
                    .any(|g| g.id == 0)
            {
                missing = true;
                glyphs.push(*glyph);
                continue;
            }
            let want = (glyph.advance.0 / width.0) as usize;
            let per = leader_runs
                .runs
                .iter()
                .map(|r| r.glyphs.len())
                .sum::<usize>()
                .max(1);
            let count = want.min(256).min(remaining / per);
            limited |= count < want;
            remaining = remaining.saturating_sub(count.saturating_mul(per));
            for _ in 0..count {
                for g in leader_runs.runs.iter().flat_map(|r| &r.glyphs) {
                    let mut g = *g;
                    g.cluster = glyph.cluster;
                    glyphs.push(g);
                }
            }
            let mut rest = *glyph;
            rest.advance -= width.mul_ratio(i32::try_from(count).unwrap_or(i32::MAX), 1);
            glyphs.push(rest);
        }
        run.glyphs = glyphs;
    }
    if limited {
        diagnostics.push(Diagnostic::new(Severity::Warning, codes::TAB_LEADER_LIMIT, subject.clone(), "tab leaders capped at 256 repeats per gap and 8192 glyphs per fragment; remaining gap stays blank"));
    }
    if missing {
        diagnostics.push(Diagnostic::new(
            Severity::Warning,
            codes::TAB_LEADER_MISSING,
            subject.clone(),
            "tab leader unavailable in its font; gap stays blank",
        ));
    }
}

/// Apply adjustments in visual glyph order using logical source clusters.
/// Multiple glyphs in a cluster get letter spacing only at its visual end.
/// A ligature covering several graphemes accumulates their spacing there:
/// positioning cannot insert a gap inside an indivisible shaped glyph.
fn apply_spacing(
    text: &str,
    content: &Range<usize>,
    runs: &mut [ShapedRun],
    adjustment: Adjustment,
) {
    let boundaries = if adjustment.letter_spacing != Length::ZERO {
        reprise_text::segment::grapheme_boundaries(text.get(content.clone()).unwrap_or_default())
            .into_iter()
            .map(|b| content.start.saturating_add(b))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    for run in runs {
        let mut cluster_ends = std::collections::BTreeMap::new();
        for (i, glyph) in run.glyphs.iter_mut().enumerate() {
            let c = glyph.cluster as usize;
            if !content.contains(&c) {
                continue;
            }
            if text
                .get(c..)
                .and_then(|s| s.chars().next())
                .is_some_and(is_word_space)
            {
                glyph.advance += adjustment.word_spacing;
            }
            if adjustment.letter_spacing != Length::ZERO {
                cluster_ends.insert(c, i);
            }
        }
        let mut clusters = cluster_ends.into_iter().peekable();
        while let Some((c, last_glyph)) = clusters.next() {
            let end = clusters
                .peek()
                .map_or(run.range.end.min(content.end), |&(next, _)| next);
            let from = boundaries.partition_point(|&b| b <= c);
            let to = boundaries.partition_point(|&b| b <= end);
            let count = i64::try_from(to.saturating_sub(from)).unwrap_or(i64::MAX);
            let spacing = i64::from(adjustment.letter_spacing.0).saturating_mul(count);
            let spacing = Length(spacing.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
            if let Some(glyph) = run.glyphs.get_mut(last_glyph) {
                glyph.advance += spacing;
            }
        }
    }
}

/// Puts a block in `frame`, `by` down its block axis.
pub(crate) fn place(mut block: BlockLayout, frame: usize, by: Length) -> BlockLayout {
    for line in &mut block.lines {
        line.frame = frame;
        line.rect.origin.y += by;
        line.baseline += by;
        for atom in &mut line.atoms {
            atom.rect.origin.y += by;
            atom.baseline += by;
        }
    }
    block.sync_image();
    block
}

#[cfg(test)]
mod tests {
    use super::*;
    use reprise_compose::{Adjustment, BreakReason, Explanation, Interval};
    use reprise_font::FontStore;

    fn synthetic_run(clusters: &[u32]) -> ShapedRun {
        ShapedRun {
            synthetic_oblique: false,
            upright: false,
            combined: false,
            horizontal_scale: reprise_geom::Fixed::ONE,
            range: 0..8,
            face: FaceId {
                family: "test".into(),
                hash: "0000000000000000".into(),
            },
            size: Length(10),
            level: 0,
            glyphs: clusters
                .iter()
                .map(|&cluster| reprise_shape::ShapedGlyph {
                    id: 1,
                    cluster,
                    advance: Length(10),
                    x_offset: Length::ZERO,
                    y_offset: Length::ZERO,
                    unsafe_to_break: false,
                    unsafe_to_concat: false,
                })
                .collect(),
        }
    }

    #[test]
    fn spacing_counts_graphemes_once_at_visual_cluster_ends() {
        let text = "fi a\u{301}  ";
        for clusters in [vec![0, 2, 3, 3, 6, 7], vec![7, 6, 3, 3, 2, 0]] {
            let mut runs = [synthetic_run(&clusters)];
            apply_spacing(
                text,
                &(0..6),
                &mut runs,
                Adjustment {
                    word_spacing: Length(4),
                    letter_spacing: Length(2),
                },
            );
            let advances: Vec<_> = runs[0].glyphs.iter().map(|g| g.advance.0).collect();
            if clusters[0] == 0 {
                assert_eq!(advances, [14, 16, 10, 12, 10, 10]);
            } else {
                assert_eq!(advances, [10, 10, 10, 12, 16, 14]);
            }
        }
    }

    #[test]
    fn positioned_widths_include_word_and_letter_spacing_but_not_hanging_spaces() {
        let engine = Engine::new(FontStore::default());
        let doc = Document::new(1).unwrap();
        doc.define_style("test", &reprise_doc::Style::default())
            .unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "test", "").unwrap();
        let mut diagnostics = Vec::new();
        let mut prepared = prepare(
            &engine,
            &doc,
            node,
            &ResolutionContext::default(),
            &mut diagnostics,
        )
        .unwrap();
        prepared.text = "fi a\u{301}  ".into();
        prepared.levels = vec![0; prepared.text.len()];
        let fragment = LineFragment {
            text: 0..8,
            runs: vec![synthetic_run(&[0, 2, 3, 3, 6, 7])],
            width: Length(40),
            available: Interval::new(Length::ZERO, Length(100)),
            line: 0,
            block_offset: Length::ZERO,
            height: Length(20),
            explanation: Explanation {
                reason: BreakReason::Opportunity,
                score: None,
                adjustment: Adjustment {
                    word_spacing: Length(4),
                    letter_spacing: Length(2),
                },
                reshaped: false,
            },
        };
        let line = line_layout(
            &engine,
            &prepared,
            0,
            fragment,
            &Subject::Node(node),
            &mut diagnostics,
            &Default::default(),
        );
        assert_eq!(line.width, Length(52));
        assert_eq!(line.runs[0].width, Length(72));
        assert_eq!(line.runs[0].x, Length::ZERO);
        assert_eq!(
            line.runs[0]
                .glyphs
                .iter()
                .take(4)
                .fold(Length::ZERO, |w, g| w + g.advance),
            line.width
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn spacing_is_bounded_on_extreme_empty_and_malformed_input() {
        let adjustment = Adjustment {
            word_spacing: Length::MAX,
            letter_spacing: Length::MIN,
        };
        apply_spacing("", &(0..0), &mut [], adjustment);
        let mut runs = [synthetic_run(&[0, 1, u32::MAX])];
        runs[0].glyphs[0].advance = Length::MAX;
        runs[0].glyphs[1].advance = Length::MIN;
        apply_spacing(" é", &(0..3), &mut runs, adjustment);
        assert_eq!(runs[0].glyphs[2].advance, Length(10));
        let mut runs = [synthetic_run(&vec![u32::MAX; 8192])];
        apply_spacing(" ", &(0..1), &mut runs, adjustment);
        assert_eq!(runs[0].glyphs.len(), 8192);
        let mut spaces = [synthetic_run(&[0, 1])];
        apply_spacing("  ", &(0..0), &mut spaces, adjustment);
        assert!(spaces[0].glyphs.iter().all(|g| g.advance == Length(10)));
    }

    #[test]
    fn preparation_uses_the_engines_function_registry() {
        use reprise_doc::expr::{Dim, Value};
        use reprise_doc::function::{Builtin, Signature};
        use reprise_doc::{Authored, Expr, Property, Style};
        let mut engine = Engine::new(FontStore::default());
        engine
            .functions
            .register(
                "custom-size",
                Builtin::new(Signature::new(&[], Dim::Length), |_| {
                    Ok(Value::Length(Length::from_pt(12)))
                }),
            )
            .unwrap();
        let doc = Document::new(1).unwrap();
        let mut style = Style::default();
        style.set(
            Property::Size,
            Authored::Expr(Expr::parse("custom-size()").unwrap()),
        );
        doc.define_style("custom", &style).unwrap();
        let node = doc
            .append_block(BlockKind::Paragraph, "custom", "")
            .unwrap();
        let mut notes = Vec::new();
        let prepared = prepare(
            &engine,
            &doc,
            node,
            &ResolutionContext::default(),
            &mut notes,
        )
        .unwrap();
        assert_eq!(prepared.style.size, Length::from_pt(12));
        assert!(notes.is_empty());
    }

    #[test]
    fn context_keeps_extreme_definite_bases_and_indefinite_block_height() {
        use reprise_doc::context::{Axis, Basis, Level, Resolved};
        let mut engine = Engine::new(FontStore::default());
        engine.medium = reprise_doc::Medium::new(Length::MIN, Length::MAX);
        let frame = ResolvedFrame {
            name: "extreme".into(),
            role: reprise_doc::FrameRole::Margin,
            transform: reprise_geom::Matrix::IDENTITY,
            x: Length::MIN,
            y: Length::MAX,
            width: Length::MAX,
            depth: Length::ZERO,
            writing_mode: reprise_doc::WritingMode::HorizontalTb,
        };
        let template = ResolvedTemplate {
            name: "extreme".into(),
            source: crate::TemplateSource::Document,
            width: Length::MAX,
            height: Length::ZERO,
            frames: vec![frame.clone()],
            page_setup: None,
            thread: None,
        };
        let ctx = resolution_context(&engine, &template, Some(&frame), frame.width);
        assert_eq!(
            ctx.basis(&Basis::frame("extreme", Axis::Width)),
            Resolved::Definite(Length::MAX)
        );
        assert_eq!(
            ctx.basis(&Basis::exact(Level::Frame, Axis::Height)),
            Resolved::Definite(Length::ZERO)
        );
        assert_eq!(
            ctx.basis(&Basis::exact(Level::Block, Axis::Height)),
            Resolved::Indefinite
        );
        assert_eq!(
            ctx.basis(&Basis::exact(Level::Medium, Axis::Width)),
            Resolved::Definite(Length::MIN)
        );
        let absent = resolution_context(&engine, &template, None, Length::ZERO);
        assert_eq!(absent.frame, Extent::UNRESOLVED);
    }

    #[test]
    fn malformed_line_levels_report_and_fall_back_without_panicking() {
        let engine = Engine::new(FontStore::default());
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "").unwrap();
        let mut diagnostics = Vec::new();
        let mut prepared = prepare(
            &engine,
            &doc,
            node,
            &ResolutionContext::default(),
            &mut diagnostics,
        )
        .unwrap();
        prepared.levels = vec![127];
        let fragment = LineFragment {
            text: 0..0,
            runs: Vec::new(),
            width: Length::ZERO,
            available: Interval::new(Length::MIN, Length::MAX),
            line: 0,
            block_offset: Length::ZERO,
            height: Length::MAX,
            explanation: Explanation {
                reason: BreakReason::End,
                score: None,
                adjustment: Adjustment::default(),
                reshaped: false,
            },
        };
        let line = line_layout(
            &engine,
            &prepared,
            0,
            fragment,
            &Subject::Node(node),
            &mut diagnostics,
            &Default::default(),
        );
        assert!(line.runs.is_empty());
        let note = diagnostics
            .iter()
            .find(|d| d.code.as_str() == "shape.bad-line")
            .unwrap();
        assert_eq!(note.severity, Severity::Warning);
        assert_eq!(note.subject, Subject::Node(node));
    }

    #[test]
    fn missing_cell_content_preparation_reports_instead_of_disappearing() {
        let engine = Engine::new(FontStore::default());
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "cell").unwrap();
        doc.delete_block(node).unwrap();
        let mut diagnostics = Vec::new();
        assert!(
            prepare(
                &engine,
                &doc,
                node,
                &ResolutionContext::default(),
                &mut diagnostics
            )
            .is_none()
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, codes::MALFORMED_BLOCK);
        assert_eq!(diagnostics[0].severity, Severity::Error);
    }
}
