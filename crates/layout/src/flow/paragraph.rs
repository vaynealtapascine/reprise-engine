//! A paragraph (or an image in the flow) as a resumable machine.
//!
//! This is the former monolithic `paragraph_inner`, with its locals moved into
//! [`Paragraph`] so a job can stop between loop iterations. `Engine::layout`
//! runs the machine to completion, so both paths make the same transitions;
//! see `docs/incremental.md` ("Why finer steps preserve the reference
//! result"). One unit is at most one preparation stage followed, when no
//! preparation is pending, by one iteration of the composing loop: one composer
//! call or one move to the next frame.

use std::sync::Arc;

use super::stage::{self, Begin, Finished, Staged};
use super::*;
use crate::incremental::{FlowKey, PrepareMiss, PrepareStart};
use crate::shaping::SHAPE_CHUNK_BYTES;
use crate::workers::{Serial, Workers};

/// Every value of the composing loop that is live across a unit boundary.
#[derive(Clone)]
pub(crate) struct Paragraph {
    node: NodeId,
    started: bool,
    finished: bool,
    /// The flow memo key and snapshot mark, captured when the block started.
    memo: Option<(FlowKey, Mark)>,
    ctx: ResolutionContext,
    preparation_notes: Vec<Diagnostic>,
    prepared: Option<Prepared>,
    preparing: Option<Preparing>,
    after: After,
    len: usize,
    lines: Vec<LineLayout>,
    start: usize,
    done: bool,
    stalls: usize,
    forced: bool,
    overflowed: bool,
}

/// Which preparation is running: the block's first, or a retry against a
/// later starting frame before any line was placed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum After {
    Initial,
    Retry,
}

#[derive(Clone)]
struct Preparing {
    staged: Box<Staged>,
    miss: Option<PrepareMiss>,
}

impl Paragraph {
    pub(crate) fn new(node: NodeId) -> Self {
        Self {
            node,
            started: false,
            finished: false,
            memo: None,
            ctx: ResolutionContext::default(),
            preparation_notes: Vec::new(),
            prepared: None,
            preparing: None,
            after: After::Initial,
            len: 0,
            lines: Vec::new(),
            start: 0,
            done: false,
            stalls: 0,
            forced: false,
            overflowed: false,
        }
    }
    pub(crate) fn node(&self) -> NodeId {
        self.node
    }
    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }
    /// The lines placed so far, as a block, for partial views.
    pub(crate) fn in_progress(&self) -> Option<BlockLayout> {
        if self.finished || self.lines.is_empty() {
            return None;
        }
        self.prepared
            .clone()
            .map(|p| p.into_block(self.lines.clone()))
    }
}

impl Flow<'_> {
    fn workers(&self) -> Arc<dyn Workers> {
        self.evaluation
            .map_or_else(|| Arc::new(Serial) as Arc<dyn Workers>, |e| e.workers())
    }

    /// Runs one unit of `p`. `allowance` (at least 1) is how many units the
    /// caller may still charge; only a batch of shaping requests uses more
    /// than one. Returns the units used.
    pub(crate) fn paragraph_unit(
        &mut self,
        doc: &Document,
        p: &mut Paragraph,
        allowance: usize,
    ) -> usize {
        let mut used = 1;
        if !p.started {
            p.started = true;
            if let Some(evaluation) = self.evaluation {
                let key = FlowKey::new(self, doc, p.node);
                if let Some(delta) = evaluation.flow_hit(p.node, &key, self.engine) {
                    self.apply_delta(delta);
                    p.finished = true;
                    return used;
                }
                p.memo = Some((key, self.mark()));
            }
            p.ctx = self.paragraph_context();
            p.after = After::Initial;
            self.start_preparation(doc, p);
        } else if p.preparing.is_some() {
            used = self.advance_preparation(p, allowance);
        }
        if p.preparing.is_none() && !p.finished {
            self.iteration(doc, p);
        }
        used.max(1)
    }

    /// Starts a preparation for `p.ctx`. A memo hit, a failure or a paragraph
    /// of at most `SHAPE_CHUNK_BYTES` finishes here; a longer one is left
    /// itemised, with its shaping requests pending.
    fn start_preparation(&mut self, doc: &Document, p: &mut Paragraph) {
        let engine = self.engine;
        let mut miss = None;
        if let Some(e) = self.evaluation {
            match e.prepare_start(engine, doc, p.node, &p.ctx) {
                PrepareStart::Hit(value, notes) => {
                    self.prepared(p, value, notes);
                    return;
                }
                PrepareStart::Untracked => {
                    let mut notes = Vec::new();
                    let value = prepare(engine, doc, p.node, &p.ctx, &mut notes);
                    self.prepared(p, value, notes);
                    return;
                }
                PrepareStart::Miss(m) => miss = Some(m),
            }
        }
        let staged = match stage::begin(engine, doc, p.node, &p.ctx, self.evaluation) {
            Begin::Done(value, notes) => {
                self.end_preparation(
                    p,
                    miss,
                    Finished {
                        value,
                        notes,
                        itemized: false,
                    },
                );
                return;
            }
            Begin::Shape(staged) => staged,
        };
        if staged.len() <= SHAPE_CHUNK_BYTES {
            let finished = match (self.evaluation, miss.as_ref()) {
                // Speculation joins the first preparation of a block only.
                (Some(e), Some(m)) if p.after == After::Initial => e.prepare_ahead(
                    engine,
                    doc,
                    m,
                    *staged,
                    &p.ctx,
                    self.upcoming,
                    &self.region_owners,
                ),
                _ => self.run_staged(*staged),
            };
            self.end_preparation(p, miss, finished);
            return;
        }
        let mut staged = staged;
        staged.itemize(&engine.fonts);
        if let Some(e) = self.evaluation {
            e.scanned(staged.len());
        }
        p.preparing = Some(Preparing { staged, miss });
    }

    /// A short preparation, inline: every stage, with requests on workers.
    fn run_staged(&self, mut staged: Staged) -> Finished {
        let engine = self.engine;
        staged.run_pure(&engine.fonts, engine.shaper.as_ref(), &*self.workers());
        if let Some(e) = self.evaluation {
            e.shaped(staged.request_bytes());
            e.scanned(staged.len());
            if staged.has_breaks() {
                e.scanned(staged.len());
            }
        }
        staged.finish(engine, self.evaluation)
    }

    /// The next stage of a long preparation: a batch of at most `allowance`
    /// shaping requests, or, once none remain, break analysis and assembly.
    fn advance_preparation(&mut self, p: &mut Paragraph, allowance: usize) -> usize {
        let engine = self.engine;
        let workers = self.workers();
        let Some(preparing) = p.preparing.as_mut() else {
            return 1;
        };
        if preparing.staged.remaining() > 0 {
            let sizes = preparing.staged.shape(
                &engine.fonts,
                engine.shaper.as_ref(),
                &*workers,
                allowance.max(1),
            );
            let count = sizes.len();
            if let Some(e) = self.evaluation {
                e.shaped(sizes);
            }
            return count.max(1);
        }
        let Some(Preparing { mut staged, miss }) = p.preparing.take() else {
            return 1;
        };
        staged.analyse_breaks_if_shapeable();
        if let Some(e) = self.evaluation
            && staged.has_breaks()
        {
            e.scanned(staged.len());
        }
        let finished = staged.finish(engine, self.evaluation);
        self.end_preparation(p, miss, finished);
        1
    }

    fn end_preparation(&mut self, p: &mut Paragraph, miss: Option<PrepareMiss>, f: Finished) {
        if let (Some(e), Some(miss)) = (self.evaluation, miss) {
            e.prepare_end(miss, &f);
        }
        self.prepared(p, f.value, f.notes);
    }

    /// What the monolithic loop did right after each `prepare_cached` call.
    fn prepared(&mut self, p: &mut Paragraph, value: Option<Prepared>, notes: Vec<Diagnostic>) {
        p.preparation_notes = notes;
        let Some(prepared) = value else {
            self.snapshot.diagnostics.append(&mut p.preparation_notes);
            self.finish_paragraph(p);
            return;
        };
        if p.after == After::Initial {
            p.len = prepared.text.len();
            if self.limit_hit {
                self.snapshot.diagnostics.append(&mut p.preparation_notes);
                unplaced(
                    &mut self.snapshot.diagnostics,
                    &Subject::Node(p.node),
                    0..p.len,
                );
                self.finish_paragraph(p);
                return;
            }
        }
        p.prepared = Some(prepared);
    }

    /// One iteration of the composing loop.
    fn iteration(&mut self, doc: &Document, p: &mut Paragraph) {
        let engine = self.engine;
        let subject = Subject::Node(p.node);
        let Some(prepared) = p.prepared.as_ref() else {
            self.finish_paragraph(p);
            return;
        };
        let index = self.frame_index();
        let fill = self.fills[index];
        let template = self.template;
        let frame = &template.frames[self.thread[self.pos]];
        let y = if prepared.image.is_some() {
            self.table_top()
        } else if fill.empty {
            Length::ZERO
        } else {
            fill.used
        };
        let measure = Measure(frame.width);
        let runaround = Runaround {
            base: measure,
            exclusions: self
                .plan
                .exclusions
                .get(&index)
                .into_iter()
                .flatten()
                .copied()
                .map(Polygon::rect)
                .collect(),
            margin: Length::ZERO,
            min_width: Length(1),
        };
        let depth = self.depth(index, frame.depth);
        if prepared.image.is_some()
            && y > Length::ZERO
            && (y > depth || depth - y < prepared.line_height())
        {
            if !self.advance() {
                self.end_loop(p);
                return;
            }
            self.retry(doc, p);
            return;
        }
        let inner: &dyn GeometryProvider = if runaround.exclusions.is_empty() {
            &runaround.base
        } else {
            &runaround
        };
        let bounded = Bounded { inner, depth };
        let unbounded =
            (prepared.line_height() > self.max_depth || p.forced) && depth > Length::ZERO;
        let geometry: &dyn GeometryProvider = if unbounded { inner } else { &bounded };
        let mut composition_notes = Vec::new();
        let composed = prepared.compose(
            engine,
            geometry,
            index,
            p.start,
            y,
            &subject,
            &mut composition_notes,
            self.evaluation,
        );
        if composed.lines.is_empty() {
            self.snapshot.diagnostics.extend(composition_notes);
            // Nothing fit here. That is ordinary for a frame that is
            // already partly full. A whole page of empty frames that can't
            // take a line means the geometry will never allow one: place it
            // anyway, overflowing.
            p.stalls += usize::from(fill.empty && depth > Length::ZERO);
            if p.stalls > self.thread.len() && !p.forced {
                p.forced = true;
            } else if !self.advance() {
                self.end_loop(p);
                return;
            }
            if p.lines.is_empty() {
                // No line has been placed yet: resolve against the new
                // candidate starting frame, discarding provisional notes.
                self.retry(doc, p);
            }
            return;
        }
        self.snapshot.diagnostics.append(&mut p.preparation_notes);
        self.snapshot.diagnostics.extend(composition_notes);
        if unbounded && !p.overflowed {
            p.overflowed = true;
            self.snapshot.diagnostics.push(Diagnostic::new(
                Severity::Warning,
                codes::FRAME_OVERFLOW,
                subject.clone(),
                "a line is taller than every frame of the main flow; placed overflowing",
            ));
        }
        p.stalls = 0;
        p.forced = false;
        p.lines.extend(composed.lines);
        let spacing = engine.flow.paragraph_spacing;
        match composed.rest {
            None => {
                p.done = true;
                self.fills[index] = Fill {
                    used: composed.block_end + spacing,
                    empty: false,
                };
                self.end_loop(p);
            }
            Some(rest) => {
                p.start = rest;
                self.fills[index] = Fill {
                    used: composed.block_end,
                    empty: false,
                };
                if !self.advance() {
                    self.end_loop(p);
                }
            }
        }
    }

    /// Re-prepares against the frame the flow just moved to.
    fn retry(&mut self, doc: &Document, p: &mut Paragraph) {
        p.ctx = self.paragraph_context();
        p.preparation_notes.clear();
        p.after = After::Retry;
        self.start_preparation(doc, p);
    }

    /// After the composing loop: what the monolithic routine did once the
    /// loop ended (rather than returning early).
    fn end_loop(&mut self, p: &mut Paragraph) {
        self.snapshot.diagnostics.append(&mut p.preparation_notes);
        if !p.done {
            unplaced(
                &mut self.snapshot.diagnostics,
                &Subject::Node(p.node),
                p.start..p.len,
            );
        }
        if !p.lines.is_empty()
            && let Some(prepared) = p.prepared.take()
        {
            self.snapshot
                .blocks
                .push(prepared.into_block(std::mem::take(&mut p.lines)));
        }
        self.finish_paragraph(p);
    }

    /// Records the transition for the flow memo, however the block ended.
    fn finish_paragraph(&mut self, p: &mut Paragraph) {
        p.finished = true;
        p.preparing = None;
        if let (Some(evaluation), Some((key, mark))) = (self.evaluation, p.memo.take()) {
            let delta = self.delta(mark);
            evaluation.flow_miss(p.node, key, &delta);
        }
    }
}
