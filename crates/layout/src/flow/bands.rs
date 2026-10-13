//! Continuous bands and the bounded, resumable search in sections §8.
//!
//! Checkpoints contain geometry and the active block, never completed prefix
//! text. A successful trial is retained as output; committing never reflows it.
use super::*;
use std::sync::Arc;

pub(crate) const MAX_BANDS: usize = 32;
const MAX_BALANCE_TRIALS: usize = 24;

#[derive(Clone)]
pub(super) struct BandStart {
    cursor: Cursor,
    header: LayoutSnapshot,
    blocks: usize,
    diagnostics: usize,
    repeats: usize,
}

#[derive(Clone)]
struct Output {
    cursor: Cursor,
    snapshot: LayoutSnapshot,
}

#[derive(Clone)]
struct Trial {
    output: Output,
    next: usize,
    depth: Length,
    ended: bool,
}

#[derive(Clone)]
pub(super) struct Balance {
    checkpoint: Arc<BandStart>,
    nodes: Arc<Vec<NodeId>>,
    endpoint: NodeId,
    lo: i32,
    hi: i32,
    trials: usize,
    kept: Option<Output>,
    trial: Option<Trial>,
}

/// Clone only geometry and pass identity. Completed text stays in the caller's
/// snapshot while a trial owns the suffix it produces.
fn header(snapshot: &LayoutSnapshot) -> LayoutSnapshot {
    LayoutSnapshot {
        font_settings: snapshot.font_settings.clone(),
        revision: snapshot.revision.clone(),
        adapter: snapshot.adapter.clone(),
        composer: snapshot.composer.clone(),
        medium: snapshot.medium.clone(),
        settings: snapshot.settings.clone(),
        template: snapshot.template.clone(),
        pages: snapshot.pages.clone(),
        frames: snapshot.frames.clone(),
        blocks: Vec::new(),
        relations: Vec::new(),
        repeated_headers: Vec::new(),
        diagnostics: Vec::new(),
        sections: snapshot.sections.clone(),
    }
}

impl Cursor {
    fn checkpoint(&mut self, snapshot: &LayoutSnapshot) {
        if !self.section_table.as_ref().is_some_and(|s| s.enabled) {
            return;
        }
        self.band_start = None;
        self.balance = None;
        self.band_nodes.clear();
        let mut cursor = self.clone();
        cursor.queued_node = None;
        self.band_start = Some(Arc::new(BandStart {
            cursor,
            header: header(snapshot),
            blocks: snapshot.blocks.len(),
            diagnostics: snapshot.diagnostics.len(),
            repeats: snapshot.repeated_headers.len(),
        }));
    }

    fn continuous(&self, node: NodeId) -> Option<&crate::sections::Section> {
        let table = self.section_table.as_ref().filter(|s| s.enabled)?;
        if table.starts.get(&node) != Some(&reprise_doc::sections::BreakKind::Section) {
            return None;
        }
        let section = table.get(reprise_doc::sections::SectionId::Block(node))?;
        (section.start == crate::sections::Start::Continuous && self.previous.is_some())
            .then_some(section)
    }

    fn compatible(&self, section: &crate::sections::Section, snapshot: &LayoutSnapshot) -> bool {
        let owner = snapshot.sections.as_ref().and_then(|s| s.pages.get(self.page));
        let owner = owner.and_then(|p| self.section_table.as_ref()?.get(p.section));
        owner.is_some_and(|owner| {
            owner.template.width == section.template.width
                && owner.template.height == section.template.height
                && owner.geometry.writing_mode == section.geometry.writing_mode
                && owner.template.name == section.template.name
                && owner.template.source == section.template.source
        })
    }

    /// One charged transition, trial start, ordinary unit, trial end or commit.
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
        if !self.section_table.as_ref().is_some_and(|s| s.enabled) {
            return self.ordinary_unit(engine, doc, template, snapshot, plan, evaluation,
                start, upcoming, allowance);
        }
        if let Some(balance) = self.balance.take() {
            return (self.balance_unit(*balance, engine, doc, template, snapshot, plan), 1);
        }
        if self.band_start.is_none() {
            self.checkpoint(snapshot);
        }
        let start = self.queued_node.take().or(start);
        if self.active.is_none()
            && let Some(node) = start
            && self.boundary_node != Some(node)
            && self.section_table.as_ref().is_some_and(|s| s.enabled && s.starts.contains_key(&node))
        {
            self.queued_node = Some(node);
            if !self.open_band && let Some(section) = self.continuous(node) {
                let bands = snapshot.sections.as_ref().and_then(|s| s.pages.get(self.page))
                    .map_or(0, |p| p.bands.len());
                if self.compatible(section, snapshot) && bands < MAX_BANDS {
                    if let Some(checkpoint) = self.band_start.clone() {
                        let full = checkpoint.cursor.max_depth;
                        if full > Length::ZERO && fit(snapshot, &checkpoint, full, checkpoint.diagnostics) {
                            self.balance = Some(Box::new(Balance {
                                checkpoint,
                                nodes: Arc::new(std::mem::take(&mut self.band_nodes)),
                                endpoint: node,
                                lo: 1,
                                hi: full.0,
                                trials: 0,
                                kept: None,
                                trial: None,
                            }));
                            return (self, 1);
                        }
                    }
                    snapshot.diagnostics.push(crate::sections::diagnostic(
                        section.id(), "start", "section.continuous-new-page", Severity::Warning,
                        "ordinary reference does not fit; its output is kept at full depth",
                    ));
                } else {
                    snapshot.diagnostics.push(crate::sections::diagnostic(
                        section.id(), "start",
                        if bands >= MAX_BANDS { "section.band-limit" } else { "section.continuous-new-page" },
                        Severity::Warning,
                        if bands >= MAX_BANDS { "at most 32 bands share one page" } else { "page size, writing mode or template differs from the page owner" },
                    ));
                }
            }
            let mut flow = self.restore(engine, template, snapshot, plan, evaluation);
            flow.transition(doc, node);
            let mut cursor = flow.cursor();
            cursor.queued_node = Some(node);
            cursor.checkpoint(snapshot);
            return (cursor, 1);
        }
        if let Some(node) = start {
            self.band_nodes.push(node);
        }
        let before = (self.page, self.band);
        let checkpoint = self.band_start.take();
        let nodes = std::mem::take(&mut self.band_nodes);
        let (mut cursor, used) = self.ordinary_unit(
            engine, doc, template, snapshot, plan, evaluation, start, upcoming, allowance,
        );
        cursor.band_start = checkpoint;
        cursor.band_nodes = nodes;
        if before != (cursor.page, cursor.band) {
            cursor.checkpoint(snapshot);
        }
        (cursor, used)
    }

    #[allow(clippy::too_many_arguments)]
    fn balance_unit(
        mut self,
        mut balance: Balance,
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        plan: &Plan,
    ) -> Self {
        if let Some(mut trial) = balance.trial.take() {
            if trial.ended {
                balance.trials = balance.trials.saturating_add(1);
                if trial.output.cursor.page == balance.checkpoint.cursor.page
                    && !trial.output.cursor.limit_hit
                    && fit(&trial.output.snapshot, &balance.checkpoint, trial.depth, 0)
                {
                    balance.hi = trial.depth.0;
                    balance.kept = Some(trial.output);
                } else {
                    balance.lo = trial.depth.0.saturating_add(1);
                }
            } else {
                let busy = trial.output.cursor.active.is_some();
                let start = (!busy).then(|| balance.nodes.get(trial.next).copied()).flatten();
                if busy || start.is_some() {
                    if start.is_some() { trial.next = trial.next.saturating_add(1); }
                    let (cursor, _) = trial.output.cursor.ordinary_unit(
                        engine, doc, template, &mut trial.output.snapshot, plan, None,
                        start, &[], 1,
                    );
                    trial.output.cursor = cursor;
                } else {
                    trial.ended = true;
                }
                balance.trial = Some(trial);
            }
            self.balance = Some(Box::new(balance));
            return self;
        }
        if balance.lo < balance.hi && balance.trials < MAX_BALANCE_TRIALS {
            let depth = Length(balance.lo.saturating_add((balance.hi - balance.lo) / 2));
            let mut cursor = balance.checkpoint.cursor.clone();
            let mut scratch = balance.checkpoint.header.clone();
            cursor.source = crate::pages::FrameSource::Band;
            cursor.clip_band(&mut scratch, depth, false);
            cursor.catalog = crate::pages::Catalog::from_snapshot(&scratch);
            // Only the active continuation can include text before C. Its memo
            // was for the reference and must never record scratch output.
            if let Some(Active::Paragraph(p)) = &mut cursor.active { p.clear_memo(); }
            balance.trial = Some(Trial {
                output: Output { cursor, snapshot: scratch },
                next: 0,
                depth,
                ended: false,
            });
            self.balance = Some(Box::new(balance));
            return self;
        }
        if let Some(output) = balance.kept.take() {
            snapshot.blocks.truncate(balance.checkpoint.blocks);
            snapshot.blocks.extend(output.snapshot.blocks);
            snapshot.diagnostics.truncate(balance.checkpoint.diagnostics);
            snapshot.diagnostics.extend(output.snapshot.diagnostics);
            snapshot.repeated_headers.truncate(balance.checkpoint.repeats);
            snapshot.repeated_headers.extend(output.snapshot.repeated_headers);
            snapshot.frames = output.snapshot.frames;
            snapshot.sections = output.snapshot.sections;
            self = output.cursor;
        }
        self.source = crate::pages::FrameSource::Paged;
        self.clip_band(snapshot, Length(balance.hi), true);
        self.catalog = crate::pages::Catalog::from_snapshot(snapshot);
        if balance.lo != balance.hi {
            let section = self.section_table.as_ref().and_then(|t| t.sections.get(self.section));
            if let Some(section) = section {
                snapshot.diagnostics.push(crate::sections::diagnostic(
                    section.id(), "columns", "section.balance-approx", Severity::Info,
                    "24 trials exhausted; the shallowest verified depth tested is kept",
                ));
            }
        }
        self.band_start = None;
        self.band_nodes.clear();
        self.queued_node = Some(balance.endpoint);
        // The next unit performs the band transition, without searching again.
        self.open_band = true;
        self
    }

    fn clip_band(&mut self, snapshot: &mut LayoutSnapshot, depth: Length, balanced: bool) {
        let frames = snapshot.sections.as_mut().and_then(|s| s.pages.get_mut(self.page))
            .and_then(|p| p.bands.get_mut(self.band)).map(|b| {
                b.depth = depth;
                b.balanced = balanced;
                b.frames.clone()
            }).unwrap_or_default();
        let mut template = (*self.template).clone();
        for index in frames {
            if let Some(frame) = snapshot.frames.get_mut(index) { frame.rect.height = depth; }
        }
        for frame in &mut template.frames {
            if frame.is_main_flow() { frame.depth = depth; }
        }
        self.template = Arc::new(template);
        self.max_depth = depth;
        crate::pages::PageFactory::clip_rules(snapshot, self.page, self.band, depth);
    }
}

/// The same recorded-outcome predicate checks reference and scratch trials.
fn fit(snapshot: &LayoutSnapshot, checkpoint: &BandStart, depth: Length, diagnostic_start: usize) -> bool {
    let diagnostics = snapshot.diagnostics.get(diagnostic_start..).unwrap_or_default();
    let frames = checkpoint.header.sections.as_ref()
        .and_then(|s| s.pages.get(checkpoint.cursor.page))
        .and_then(|p| p.bands.get(checkpoint.cursor.band)).map(|b| &b.frames);
    !diagnostics.iter().any(|d| d.severity == Severity::Error
        || d.code == codes::FRAME_OVERFLOW || d.code == codes::KEEP_UNSATISFIED)
        && snapshot.blocks.iter().flat_map(|b| &b.lines).all(|line| {
            snapshot.frame(line.frame).is_some()
                && (frames.is_none_or(|frames| !frames.contains(&line.frame))
                    || line.rect.origin.y + line.rect.height <= depth)
        })
}
