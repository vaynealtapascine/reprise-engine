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
    Adjustment, Break, ComposeRequest, GeometryProvider, LineFragment, Measure, Polygon, Runaround,
    break_opportunities, is_word_space,
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
pub(crate) fn run(
    engine: &Engine,
    doc: &Document,
    template: &ResolvedTemplate,
    snapshot: &mut LayoutSnapshot,
    plan: &Plan,
) -> Vec<Pending> {
    let mut cursor = Cursor::new(engine, doc, template, snapshot, plan);
    for node in doc.blocks() {
        cursor = cursor.step(engine, doc, template, snapshot, plan, node, None);
    }
    cursor.finish(engine, template, snapshot, plan)
}

/// Owned continuation state; no borrowed snapshot survives a yield.
#[derive(Clone, Default)]
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
    pub(crate) fn new(
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        plan: &Plan,
    ) -> Self {
        let thread = template.main_thread();
        let max_depth = thread
            .iter()
            .map(|&t| template.frames[t].depth)
            .max()
            .unwrap_or_default();
        let thread_is_empty = thread.is_empty();
        let mut flow = Flow {
            engine,
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
        };
        // Even an empty document gets a page.
        flow.new_page();

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
        let mut flow = self.restore(engine, template, snapshot, plan, evaluation);
        flow.node(doc, node);
        flow.previous = Some(node);
        flow.cursor()
    }
    /// A block is in progress.
    pub(crate) fn busy(&self) -> bool {
        self.active.is_some()
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
    pub(crate) fn unit(
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
        let mut flow = self.restore(engine, template, snapshot, plan, evaluation);
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
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        plan: &Plan,
    ) -> Vec<Pending> {
        let mut flow = self.restore(engine, template, snapshot, plan, None);
        while flow.snapshot.pages.len() < plan.pages {
            if !flow.new_page() {
                break;
            }
        }
        flow.pending
    }
    pub(crate) fn page(&self) -> usize {
        self.page
    }
    fn restore<'a>(
        self,
        engine: &'a Engine,
        template: &'a ResolvedTemplate,
        snapshot: &'a mut LayoutSnapshot,
        plan: &'a Plan,
        evaluation: Option<&'a crate::incremental::Evaluation>,
    ) -> Flow<'a> {
        Flow {
            engine,
            template,
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
}

pub(crate) struct Flow<'a> {
    pub(crate) pending: Vec<Pending>,
    pub(crate) region_owners: std::collections::BTreeSet<NodeId>,
    pub(crate) previous: Option<NodeId>,
    /// Blocks after the current one, for speculative preparation.
    pub(crate) upcoming: &'a [NodeId],
    pub(crate) engine: &'a Engine,
    pub(crate) evaluation: Option<&'a crate::incremental::Evaluation>,
    pub(crate) template: &'a ResolvedTemplate,
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
}

impl Flow<'_> {
    fn node(&mut self, doc: &Document, node: NodeId) {
        // `table` runs the same phases as `table_unit`, back to back.
        if let Ok(Some(reprise_doc::TableRole::Table(columns))) = doc.table_role(node) {
            self.table(doc, node, &columns);
            return;
        }
        if let Some(mut block) = self.begin_block(doc, node) {
            while !self.block_unit(doc, &mut block, usize::MAX).1 {}
        }
    }

    /// Starts a block. Blocks that take one step are laid out here and give
    /// `None`; paragraphs, flowed images and tables give their machine.
    fn begin_block(&mut self, doc: &Document, node: NodeId) -> Option<Active> {
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
            } => {
                // As `place_groups`: the repeat state starts when placing does.
                let repeat = repeat.get_or_insert_with(|| {
                    crate::table_flow::Repeat::new(headers.len(), self.used() > Length::ZERO)
                });
                if let Some(group) = headers.get(*next_header).cloned() {
                    self.place_group(doc, *node, group, *next_header, headers, widths, repeat);
                    *next_header += 1;
                } else if let Some(group) = bodies.pop_front() {
                    self.place_group(doc, *node, group, usize::MAX, headers, widths, repeat);
                }
                return *next_header >= headers.len() && bodies.is_empty();
            }
        }
        false
    }

    fn cursor(self) -> Cursor {
        Cursor {
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
        }
    }

    /// Makes the next page from the template, unless the limit is reached.
    fn new_page(&mut self) -> bool {
        let limit = self.engine.flow.max_pages.max(1) as usize;
        if self.snapshot.pages.len() >= limit {
            return false;
        }
        let page = self.snapshot.pages.len();
        self.page_base.push(self.snapshot.frames.len());
        self.snapshot.pages.push(PageLayout {
            width: self.template.width,
            height: self.template.height,
        });
        for frame in &self.template.frames {
            self.snapshot.frames.push(frame.layout(page));
            self.fills.push(Fill {
                used: Length::ZERO,
                empty: true,
            });
        }
        true
    }

    /// The snapshot index of the frame the flow is in.
    pub(crate) fn frame_index(&self) -> usize {
        self.page_base[self.page] + self.thread[self.pos]
    }

    /// Moves on to the next frame of the thread, or the first of a new page.
    /// False when the page limit stops it, which is reported once.
    pub(crate) fn advance(&mut self) -> bool {
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

    pub(crate) fn depth(&self, index: usize, full: Length) -> Length {
        (full
            - self
                .plan
                .reservations
                .get(&index)
                .copied()
                .unwrap_or_default())
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
        }
    }

    /// Tables retain rectangular columns: pass below all float exclusions in
    /// this frame instead of overlapping them or changing column widths.
    pub(crate) fn table_top(&self) -> Length {
        self.plan
            .exclusions
            .get(&self.frame_index())
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
            self.template,
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
        let key = crate::incremental::AnnotationKey::new(doc, node, self.template);
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
        let ctx = resolution_context(engine, self.template, frame, width);
        let prepared = prepare_cached(
            engine,
            doc,
            node,
            &ctx,
            &mut self.snapshot.diagnostics,
            self.evaluation,
        )?;
        let composed = prepared.compose(
            engine,
            &Measure(width),
            0, // Provisional: the relation that places the block sets the frame.
            0,
            Length::ZERO,
            &subject,
            &mut self.snapshot.diagnostics,
            self.evaluation,
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
    page: usize,
    pos: usize,
    limit_hit: bool,
    pages: usize,
    frames: usize,
    fills: Vec<Fill>,
}
#[derive(Clone)]
pub(crate) struct Delta {
    pages: Vec<PageLayout>,
    frames: Vec<FrameLayout>,
    pub(crate) blocks: Vec<BlockLayout>,
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
            page: self.page,
            pos: self.pos,
            limit_hit: self.limit_hit,
            pages: self.snapshot.pages.len(),
            frames: self.snapshot.frames.len(),
            fills: self.fills.get(tail..).unwrap_or_default().to_vec(),
        }
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
    fn apply_delta(&mut self, delta: Delta) {
        self.snapshot.pages.extend(delta.pages);
        self.snapshot.frames.extend(delta.frames);
        self.snapshot.blocks.extend(delta.blocks);
        self.snapshot.diagnostics.extend(delta.diagnostics);
        self.fills.truncate(delta.tail);
        self.fills.extend(delta.fills);
        self.page_base.extend(delta.page_base);
        self.page = delta.page;
        self.pos = delta.pos;
        self.limit_hit = delta.limit_hit;
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
                    frame,
                    text: 0..self.text.len(),
                    preview: self.text.clone(),
                    rect,
                    baseline: block_start,
                    width: image.rect.width,
                    runs: Vec::new(),
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
        let composition = engine.composer.compose(&ComposeRequest {
            text: &self.text,
            shaped: &self.shaped,
            reshape: &shaper,
            breaks: &self.breaks,
            line_height: self.style.line_height,
            geometry,
            start,
            block_start,
        });
        diagnostics.extend(
            composition
                .notes
                .into_iter()
                .map(|n| Diagnostic::from_note(n, subject.clone())),
        );
        if let Some(e) = evaluation {
            e.composed(self.text.len(), composition.lines.len());
        }
        Composed {
            lines: composition
                .lines
                .into_iter()
                .map(|l| line_layout(engine, self, frame, l, subject, diagnostics))
                .collect(),
            rest: composition.rest,
            block_end: composition.block_end,
        }
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
    let half_leading = (fragment.height - ascent - descent).mul_ratio(1, 2);

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
    let mut x = fragment.available.start - hanging_left;
    let runs = visual_runs
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
        Point::new(fragment.available.start, fragment.block_offset),
        fragment.available.width(),
        fragment.height,
    );
    LineLayout {
        frame,
        preview: text
            .get(fragment.text.clone())
            .unwrap_or_default()
            .trim_end()
            .to_string(),
        text: fragment.text,
        rect,
        baseline: fragment.block_offset + half_leading + ascent,
        width,
        explanation: fragment.explanation,
        runs,
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
