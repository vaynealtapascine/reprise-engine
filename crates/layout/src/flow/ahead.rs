//! The look-ahead of a stepped job (sections §11, "Scheduling").
//!
//! A page's running regions are composed stories, and composing a story is
//! work: up to 4,096 blocks and 1 MiB of text each, three slots to a page.
//! Done inside the unit that makes the page, one charged unit would hold up to
//! three whole stories. So a stepped job makes the regions *before* the unit
//! that needs them, in units of its own, like S5's balance search:
//!
//! - **Pages.** Every unit that can make a page, or a section start that will,
//!   first asks the memo for that page's regions. If one is missing, a small
//!   machine makes it: the story's digest a chunk of blocks at a time, then
//!   the composition a flow unit at a time, then the memo entry. The unit that
//!   makes the page then only finds them in the memo.
//! - **The first page** waits for its regions the same way, so
//!   `LayoutJob::new` composes nothing.
//! - **Memoised transitions.** A transition that made pages is replayed when
//!   what the regions of those pages come to is unchanged. Judging that needs
//!   those regions, so the block does not start until the machine has made
//!   them.
//!
//! The machine runs the ordinary flow over the story's blocks (the same code
//! the synchronous path uses, to the end), so what it makes is what composing
//! the story whole makes. A page the look-ahead did not foresee is made the
//! slow way, still correctly, and counted in `WorkCounters::running_inline`;
//! the tests hold that count at zero.

use super::*;
use crate::running::attach::{self, PageProbe};
use crate::running::compose::{DIGEST_CHUNK, DigestBuild, Need, Shared, StoryRun};
use reprise_doc::sections::{BreakKind, SectionId};

/// The need being made, and how far along it is.
#[derive(Clone)]
pub(crate) struct Ahead {
    need: Need,
    digest: Option<DigestBuild>,
    run: Option<Box<StoryRun>>,
    /// What it makes is kept for the region allocation that asked for it,
    /// rather than stored in the memo, which forgets all but the last 64.
    hold: bool,
}

/// What asking for a page's regions came to.
pub(super) enum Looked {
    /// The memo has them: the page can be made.
    Ready,
    /// A look-ahead unit ran, and charged this many units.
    Working(usize),
}

impl Ahead {
    pub fn new(need: Need) -> Self {
        Self {
            need,
            digest: None,
            run: None,
            hold: false,
        }
    }

    /// A need of the region allocation, which wants all its compositions at
    /// once: what is made is held until the allocation has consumed it.
    pub fn held(need: Need) -> Self {
        Self {
            hold: true,
            ..Self::new(need)
        }
    }

    /// The need this look-ahead is making.
    pub fn need(&self) -> &Need {
        &self.need
    }

    /// One unit. Returns the units charged and whether the need is met.
    pub(crate) fn step(
        &mut self,
        shared: &Shared,
        engine: &Engine,
        doc: &Document,
        plan: &Plan,
        evaluation: Option<&crate::incremental::Evaluation>,
        allowance: usize,
    ) -> (usize, bool) {
        match &self.need {
            Need::Digest(story) => {
                let build = self
                    .digest
                    .get_or_insert_with(|| DigestBuild::new(doc, *story));
                let read = build.step(shared, engine, doc, DIGEST_CHUNK);
                let done = build.is_done();
                if let Some(e) = evaluation {
                    e.work(|w| {
                        w.region_units = w.region_units.saturating_add(1);
                        w.digest_blocks = w.digest_blocks.saturating_add(read.blocks);
                        w.digest_inspected = w.digest_inspected.saturating_add(read.looked);
                        w.digest_decoded = w.digest_decoded.saturating_add(read.decoded);
                    });
                }
                if done && let Some(build) = self.digest.take() {
                    shared.set_digest(build);
                }
                (1, done)
            }
            Need::Story(want) => {
                let Some(digest) = shared.ready_digest(want.story) else {
                    // The digest went away (the memo was cleared): ask again.
                    return (1, true);
                };
                let run = self.run.get_or_insert_with(|| {
                    Box::new(StoryRun::new(engine, doc, plan, want.clone(), digest))
                });
                let before = run.started;
                let (units, composed) =
                    run.step(engine, doc, plan, shared.evaluation(), allowance.max(1));
                if let Some(e) = evaluation {
                    let started = run.started.saturating_sub(before);
                    e.work(|w| {
                        w.region_units = w.region_units.saturating_add(units);
                        w.region_blocks = w.region_blocks.saturating_add(started);
                    });
                }
                if let Some(composed) = composed {
                    if self.hold {
                        shared.hold(run, composed);
                    } else {
                        shared.install(run, composed);
                    }
                    return (units, true);
                }
                (units, false)
            }
        }
    }
}

/// Where a continuous section's band would go on page `page`: the page's
/// owner, the band's offset and its depth.
pub(super) fn band_room<'t>(
    snapshot: &LayoutSnapshot,
    table: &'t crate::sections::Table,
    page: usize,
) -> Option<(&'t crate::sections::Section, Length, Length)> {
    let descriptor = snapshot.sections.as_ref().and_then(|s| s.pages.get(page));
    let owner = descriptor.and_then(|p| table.get(p.section))?;
    let offset = descriptor
        .and_then(|p| p.bands.last())
        .map_or(Length::ZERO, |b| b.offset + b.depth);
    let taken = descriptor.map_or(Length::ZERO, |p| p.reserve.0 + p.reserve.1);
    let depth = if owner.geometry.writing_mode == reprise_doc::WritingMode::HorizontalTb {
        owner.geometry.block_extent() - taken - offset
    } else {
        owner.geometry.block_extent() - offset
    };
    Some((owner, offset, depth))
}

/// What the gate at the top of a stepped unit decided.
pub(super) enum Gate {
    /// Nothing to wait for: the unit goes on.
    Proceed,
    /// The look-ahead ran instead of the unit, and charged this many units.
    Working(usize),
    /// The first page's regions are ready: make it.
    FirstPage,
}

impl Cursor {
    /// Whether this cursor makes running regions for the pages it makes.
    pub(super) fn regions(&self) -> bool {
        self.stepped
            && self.running.is_some()
            && self
                .section_table
                .as_ref()
                .is_some_and(|t| t.enabled && t.running.any())
    }

    /// Whether `node` is a section start the next unit will process, as
    /// opposed to start as a block.
    pub(super) fn starts_a_transition(&self, node: NodeId) -> bool {
        self.active.is_none()
            && self.boundary_node != Some(node)
            && self
                .section_table
                .as_ref()
                .is_some_and(|t| t.enabled && t.starts.contains_key(&node))
    }

    /// The page `probe` describes, looked up in the memo alone.
    fn probe_page(
        &self,
        engine: &Engine,
        doc: &Document,
        plan: &Plan,
        snapshot: &LayoutSnapshot,
        probe: PageProbe,
    ) -> Result<(), Need> {
        let (Some(table), Some(shared)) = (self.section_table.as_ref(), self.running.as_deref())
        else {
            return Ok(());
        };
        let ctx = attach::Ctx {
            engine,
            doc,
            plan,
            shared,
            filler_regions: probe.filler_regions,
            stepped: true,
            collect: None,
        };
        attach::probe(&ctx, table, probe, snapshot)
    }

    /// One look-ahead unit for the need in progress.
    fn work_ahead(
        &mut self,
        engine: &Engine,
        doc: &Document,
        plan: &Plan,
        evaluation: Option<&crate::incremental::Evaluation>,
        allowance: usize,
    ) -> usize {
        let (Some(shared), Some(mut ahead)) = (self.running.clone(), self.ahead.take()) else {
            return 1;
        };
        let (units, done) = ahead.step(&shared, engine, doc, plan, evaluation, allowance);
        if done {
            self.stall = Some(ahead.need);
        } else {
            self.ahead = Some(ahead);
        }
        units.max(1)
    }

    /// Asks for the regions of the page `probe` describes: `Ready` when the
    /// memo has them, else one look-ahead unit towards making them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn look(
        &mut self,
        engine: &Engine,
        doc: &Document,
        plan: &Plan,
        snapshot: &LayoutSnapshot,
        evaluation: Option<&crate::incremental::Evaluation>,
        allowance: usize,
        probe: PageProbe,
    ) -> Looked {
        if self.ahead.is_none() {
            match self.probe_page(engine, doc, plan, snapshot, probe) {
                Ok(()) => {
                    self.stall = None;
                    return Looked::Ready;
                }
                Err(need) => {
                    if self.stall.as_ref() == Some(&need) {
                        // Made, and still not there: the page makes it itself.
                        return Looked::Ready;
                    }
                    self.ahead = Some(Box::new(Ahead::new(need)));
                }
            }
        }
        Looked::Working(self.work_ahead(engine, doc, plan, evaluation, allowance))
    }

    /// The top of a stepped unit: carry on with a look-ahead in progress, or
    /// make sure the regions of the page this unit may make are ready.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn gate(
        &mut self,
        engine: &Engine,
        doc: &Document,
        plan: &Plan,
        snapshot: &LayoutSnapshot,
        evaluation: Option<&crate::incremental::Evaluation>,
        start: Option<NodeId>,
        allowance: usize,
    ) -> Gate {
        if self.ahead.is_some() {
            return Gate::Working(self.work_ahead(engine, doc, plan, evaluation, allowance));
        }
        let content = PageProbe {
            section: self.section,
            kind: crate::PageKind::Content,
            filler_regions: false,
        };
        if self.first_pending {
            return match self.look(engine, doc, plan, snapshot, evaluation, allowance, content) {
                Looked::Working(units) => Gate::Working(units),
                Looked::Ready => Gate::FirstPage,
            };
        }
        // A block unit makes a page only by moving past the last frame of the
        // one it is on, once, and only if it has a block to place.
        let places =
            self.active.is_some() || start.is_some_and(|node| !self.starts_a_transition(node));
        if places
            && self.source.can_page()
            && !self.limit_hit
            && self.pos.saturating_add(1) >= self.thread.len()
            && let Looked::Working(units) =
                self.look(engine, doc, plan, snapshot, evaluation, allowance, content)
        {
            return Gate::Working(units);
        }
        Gate::Proceed
    }

    /// The page the transition at section start `node` makes first, if it
    /// makes one: a parity filler, else the content page. The filler's content
    /// page is the next unit's. `None` when it makes none.
    pub(super) fn transition_probe(
        &self,
        snapshot: &LayoutSnapshot,
        node: NodeId,
    ) -> Option<PageProbe> {
        let table = self
            .section_table
            .as_ref()
            .filter(|t| t.enabled && t.running.any())?;
        if !self.source.can_page() || self.previous.is_none() {
            return None;
        }
        let content = |section| PageProbe {
            section,
            kind: crate::PageKind::Content,
            filler_regions: false,
        };
        match *table.starts.get(&node)? {
            BreakKind::Column => (!self.limit_hit
                && self.pos.saturating_add(1) >= self.thread.len())
            .then(|| content(self.section)),
            BreakKind::Page => Some(content(self.section)),
            BreakKind::Section => {
                let next = *table.by_id.get(&SectionId::Block(node))?;
                let section = table.sections.get(next)?;
                if self.open_band
                    && section.start == crate::sections::Start::Continuous
                    && band_room(snapshot, table, self.page).is_some_and(|(_, _, depth)| {
                        depth > Length::ZERO
                            && snapshot
                                .sections
                                .as_ref()
                                .and_then(|s| s.pages.get(self.page))
                                .is_some_and(|p| p.bands.len() < bands::MAX_BANDS)
                            && section.template.frames.iter().any(|f| f.is_main_flow())
                    })
                {
                    return None;
                }
                let parity = match section.start {
                    crate::sections::Start::OddPage => Some(1),
                    crate::sections::Start::EvenPage => Some(0),
                    _ => None,
                };
                if parity
                    .is_some_and(|parity| (snapshot.pages.len().saturating_add(1) % 2) != parity)
                    && table.sections.get(self.section).is_some()
                {
                    return Some(PageProbe {
                        section: self.section,
                        kind: crate::PageKind::Filler,
                        filler_regions: section.filler_running,
                    });
                }
                Some(content(next))
            }
        }
    }

    /// One unit towards the end of the flow: the look-ahead for the next page
    /// the plan asks for (or the first page, if it is still to come), then
    /// that page. `Err` is the finished flow's pending blocks.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finish_unit(
        mut self,
        engine: &Engine,
        doc: &Document,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        plan: &Plan,
        evaluation: Option<&crate::incremental::Evaluation>,
        allowance: usize,
    ) -> Result<(Self, usize), Vec<Pending>> {
        let page = snapshot.pages.len();
        if !self.first_pending && page >= plan.pages {
            return Err(self.finish(engine, doc, template, snapshot, plan));
        }
        // The plan's request for this page, else the next ordinary page.
        let request = if self.first_pending {
            None
        } else {
            plan.page_requests.get(&page).copied()
        };
        if self.regions() {
            let table = self.section_table.as_ref();
            let section = match request {
                Some((owner, _)) => table.and_then(|t| t.by_id.get(&owner).copied()),
                None => Some(self.section),
            };
            if let Some(section) = section {
                let probe = PageProbe {
                    section,
                    kind: request.map_or(crate::PageKind::Content, |(_, kind)| kind),
                    filler_regions: false,
                };
                if let Looked::Working(units) =
                    self.look(engine, doc, plan, snapshot, evaluation, allowance, probe)
                {
                    return Ok((self, units));
                }
            }
        }
        let mut flow = self.restore(engine, doc, template, snapshot, plan, evaluation);
        flow.first_pending = false;
        let allocated = if let Some((owner, kind)) = request
            && let Some(table) = flow.section_table.clone().filter(|s| s.enabled)
            && let Some(section) = table.get(owner)
        {
            let index = table.by_id.get(&section.id()).copied();
            index.is_some_and(|index| flow.open_page(&table, index, kind, false).is_some())
        } else {
            flow.new_page()
        };
        let cursor = flow.cursor();
        if allocated {
            Ok((cursor, 1))
        } else {
            // The page limit: nothing more can be made.
            Err(cursor.finish(engine, doc, template, snapshot, plan))
        }
    }
}
