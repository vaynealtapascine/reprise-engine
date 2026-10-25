//! Paragraph geometry and breaking lookahead (`docs/paragraph-styles.md`):
//! the room a frame offers a paragraph (indents, exclusions, depth), the
//! next frame without creating it, and the keep-with-next chain check.
//!
//! Everything here is a trial: it composes into scratch output that is
//! thrown away, so nothing it does reaches the snapshot except the decision.
//! Trial preparations are cached by exact `(node, context)`, so the cache
//! can't change output.
//!
//! The keep-with-next check is a machine ([`ChainRun`]): a stage of the
//! preparation of a block it keeps with, and the composition of each, are
//! steps of their own, which the paragraph's unit charges and which a job can
//! suspend between. It looks only at the blocks it is given. For a running
//! story those are the blocks of the story's digest, in its order (the
//! cursor's `Keeps::scope`): a block the digest left out (past a read limit, or
//! a host too long to decode) is not looked at, so it is not prepared, and
//! everything the check reads of the blocks it does look at is in the digest
//! the story's memo is keyed by: their inputs, and the [`Link`] each makes,
//! which is how the check reads the relations and table roles.

use std::sync::Arc;

use super::paragraph::{Advance, Preparing, Started};
use super::*;
use crate::template::ResolvedFrame;

impl Link {
    /// The link `node` makes. The flow lays out a paragraph, and a table,
    /// whatever relation names it owner (the region pass reports such an owner
    /// as invalid and places nothing), so only the kinds the flow itself leaves
    /// to regions ask `owned`: an annotation is always passed over, an image
    /// only when a float or note owns it.
    pub(crate) fn of(doc: &Document, node: NodeId, owned: impl FnOnce() -> bool) -> Link {
        if !matches!(doc.table_role(node), Ok(None)) {
            return Link::End;
        }
        match doc.block(node).map(|b| b.kind) {
            Ok(BlockKind::Paragraph) => Link::Check,
            Ok(BlockKind::Annotation) => Link::Pass,
            Ok(BlockKind::Image) if owned() => Link::Pass,
            _ => Link::End,
        }
    }
}

impl Flow<'_> {
    /// What the chain makes of `node`, one of the blocks of [`Flow::following`].
    /// A running story's blocks are the digest's, and so is the link: the
    /// chain reads what the story's memo is keyed by, not the document. The
    /// body's are looked at.
    pub(crate) fn link(&self, doc: &Document, node: NodeId) -> Link {
        match (&self.keeps.scope, &self.keeps.order) {
            (Some(scope), Some(order)) => order
                .index
                .get(&node)
                .and_then(|&at| scope.link_at(at))
                .unwrap_or(Link::End),
            _ => Link::of(doc, node, || self.region_owners.contains(&node)),
        }
    }

    /// The room snapshot frame `index` (template frame `frame`) offers
    /// `prepared` when composing from byte `start`.
    pub(crate) fn room(
        &self,
        index: usize,
        frame: &ResolvedFrame,
        prepared: &Prepared,
        start: usize,
    ) -> FrameRoom {
        FrameRoom {
            runaround: Runaround {
                base: prepared.paragraph_measure(frame.width, start),
                exclusions: self
                    .exclusions(index)
                    .into_iter()
                    .flatten()
                    .map(Polygon::rect)
                    .collect(),
                margin: Length::ZERO,
                min_width: Length(1),
            },
            depth: self.depth(index, frame.depth),
        }
    }

    /// The frame the flow would move to next: its snapshot index and its
    /// template frame. `None` when the page limit would stop it. Pages are
    /// made from the template in order, so a page not made yet has a known
    /// index.
    pub(crate) fn next_frame(&self) -> Option<(usize, usize)> {
        if self.source.single_index().is_some() {
            return None;
        }
        if self.pos + 1 < self.thread.len() {
            let t = *self.thread.get(self.pos + 1)?;
            return Some((self.main_index(t)?, t));
        }
        if !self.source.can_page() {
            return None;
        }
        let full = self
            .section_table
            .as_ref()
            .filter(|s| s.enabled)
            .and_then(|s| s.sections.get(self.section))
            .map_or(self.template.as_ref(), |s| s.template.as_ref());
        let t = *full.main_thread().first()?;
        let next_page = self.page + 1;
        if next_page >= self.snapshot.pages.len()
            && self.snapshot.pages.len() >= self.engine.flow.max_pages.max(1) as usize
        {
            return None;
        }
        Some((
            crate::pages::PageFactory::preview(full, self.snapshot, next_page, t)?,
            t,
        ))
    }
    pub(crate) fn next_template(&self, index: usize) -> &ResolvedTemplate {
        if self
            .snapshot
            .frame(index)
            .is_some_and(|f| f.page == self.page)
        {
            &self.template
        } else {
            self.section_table
                .as_ref()
                .filter(|s| s.enabled)
                .and_then(|s| s.sections.get(self.section))
                .map_or(self.template.as_ref(), |s| s.template.as_ref())
        }
    }

    /// The blocks after `node` in document order, built once per layout. A
    /// running story's are the blocks of its digest and no others (see the top
    /// of this file).
    fn following(&mut self, doc: &Document, node: NodeId) -> Vec<NodeId> {
        let order = self
            .keeps
            .order
            .get_or_insert_with(|| {
                let nodes = match &self.keeps.scope {
                    Some(scope) => scope.nodes().to_vec(),
                    None => doc.top(self.domain.unwrap_or(reprise_doc::Domain::Body)),
                };
                let index = nodes.iter().enumerate().map(|(i, n)| (*n, i)).collect();
                Arc::new(BlockOrder { nodes, index })
            })
            .clone();
        let Some(&at) = order.index.get(&node) else {
            return Vec::new();
        };
        order
            .nodes
            .iter()
            .skip(at + 1)
            .take_while(|node| {
                !self
                    .section_table
                    .as_ref()
                    .is_some_and(|s| s.starts.contains_key(node))
            })
            .take(MAX_KEEP_CHAIN + 1)
            .copied()
            .collect()
    }

    /// The blocks `node`'s placement may read ahead: the following ones a
    /// keep-with-next chain can reach, when `node` keeps with the next
    /// (resolved without a context: keep flags are keywords). Conservative,
    /// for memo keys.
    pub(crate) fn keep_lookahead(&mut self, doc: &Document, node: NodeId) -> Vec<NodeId> {
        let keeps = doc
            .specified_style(node)
            .map(|s| s.compute(&self.engine.functions).keep_with_next)
            .unwrap_or(false);
        if keeps {
            self.following(doc, node)
        } else {
            Vec::new()
        }
    }

    /// The preparation of `node` against `ctx` that an earlier trial kept.
    fn trial_kept(&self, node: NodeId, ctx: &ResolutionContext) -> Option<Arc<Prepared>> {
        self.keeps
            .trials
            .iter()
            .find(|(n, c, _)| *n == node && c == ctx)
            .map(|(_, _, p)| p.clone())
    }

    /// Keeps the preparation of `node` against `ctx` for later trials.
    fn trial_keep(&mut self, node: NodeId, ctx: &ResolutionContext, prepared: Prepared) {
        if self.keeps.trials.len() >= MAX_TRIALS {
            self.keeps.trials.remove(0);
        }
        self.keeps
            .trials
            .push((node, ctx.clone(), Arc::new(prepared)));
    }

    /// Starts the check of the chain `node` begins, once it is composed to end
    /// at `end` (the paragraph's block end) with `after` space after it. No
    /// work is done yet.
    pub(crate) fn chain_begin(
        &mut self,
        doc: &Document,
        node: NodeId,
        after: Length,
        end: Length,
    ) -> ChainRun {
        ChainRun {
            members: self.following(doc, node),
            next: 0,
            used: end + self.engine.flow.paragraph_spacing,
            after,
            preparing: None,
        }
    }

    /// Goes on with the check of `run`, the chain after a paragraph in
    /// snapshot frame `index` (template frame `frame`), whose members are
    /// prepared against `ctx`: each fits the rest of the frame whole, then the
    /// first other block's `max(orphans, 1)` lines (all of them if it keeps
    /// together). The [`Link`] of a block says how it counts: tables and images
    /// laid out in the flow end a chain as fitting; annotations, and images
    /// placed by relations, are skipped.
    ///
    /// A step does at most one piece of work: a stage of a member's
    /// preparation (a batch of at most `allowance` shaping requests, if it is
    /// a long one), or one composition. It returns what it charged to go on
    /// with the next in another unit. `fresh` says the unit has done no work
    /// of its own yet; if it has, the step does none and says so. The trial
    /// preparations are kept, so a member is prepared once however many
    /// paragraphs ask. Reading only the members, which are the digest's for a
    /// running story, it reads nothing a story's memo is not keyed by.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn chain_step(
        &mut self,
        doc: &Document,
        run: &mut ChainRun,
        index: usize,
        frame: &ResolvedFrame,
        ctx: &ResolutionContext,
        allowance: usize,
        mut fresh: bool,
    ) -> ChainStep {
        let spacing = self.engine.flow.paragraph_spacing;
        loop {
            let Some(&next) = run.members.get(run.next) else {
                return ChainStep::Done(Chain::Fits);
            };
            if run.next >= MAX_KEEP_CHAIN {
                return ChainStep::Done(Chain::TooLong);
            }
            match self.link(doc, next) {
                Link::Check => {}
                Link::Pass => {
                    run.next += 1;
                    continue;
                }
                Link::End => return ChainStep::Done(Chain::Fits),
            }
            let Some(prepared) = self.trial_kept(next, ctx) else {
                if !fresh {
                    return ChainStep::Working(1);
                }
                let (units, value) = match run.preparing.take() {
                    Some(preparing) => match self.advance_staged(preparing, allowance) {
                        Advance::More(preparing, units) => {
                            run.preparing = Some(preparing);
                            return ChainStep::Working(units);
                        }
                        Advance::Ready(value, _) => (1, value),
                    },
                    None => match self.begin_preparation(doc, next, ctx, false) {
                        Started::Long(preparing) => {
                            run.preparing = Some(preparing);
                            return ChainStep::Working(1);
                        }
                        Started::Ready(value, _) => (1, value),
                    },
                };
                // The notes of a trial are dropped. A block that can't be
                // prepared ends the chain as fitting.
                let Some(prepared) = value else {
                    return ChainStep::Done(Chain::Fits);
                };
                self.trial_keep(next, ctx, prepared);
                return ChainStep::Working(units);
            };
            if prepared.image.is_some() {
                return ChainStep::Done(Chain::Fits);
            }
            let style = &prepared.style;
            let top = run.used + run.after.max(style.space_before);
            let room = self.room(index, frame, &prepared, 0);
            if top > room.depth {
                return ChainStep::Done(Chain::Fails);
            }
            if !fresh {
                return ChainStep::Working(1);
            }
            let mut scratch = Vec::new();
            let composed = prepared.compose(
                self.engine,
                &crate::region::Bounded {
                    inner: room.inner(),
                    depth: room.depth,
                },
                index,
                0,
                top,
                &Subject::Node(next),
                &mut scratch,
                self.evaluation,
                &self.fields(),
            );
            fresh = false;
            let lines = rows(&composed.lines).len();
            if lines == 0 {
                return ChainStep::Done(Chain::Fails);
            }
            let whole = composed.rest.is_none();
            if (style.keep_with_next || style.keep_together) && !whole {
                return ChainStep::Done(Chain::Fails);
            }
            if !style.keep_with_next {
                return ChainStep::Done(if whole || lines >= style.orphans() {
                    Chain::Fits
                } else {
                    Chain::Fails
                });
            }
            run.used = composed.block_end + spacing;
            run.after = style.space_after;
            run.next += 1;
        }
    }
}

/// A keep-with-next chain being checked, a step at a time (`Flow::chain_step`).
#[derive(Clone)]
pub(crate) struct ChainRun {
    /// The blocks the chain can reach after the paragraph, in order.
    members: Vec<NodeId>,
    /// The member being looked at.
    next: usize,
    /// Where the next member would start in the frame, and the space after the
    /// one before it.
    used: Length,
    after: Length,
    /// The preparation of that member, if it is a long one in progress.
    preparing: Option<Preparing>,
}

/// What a step of a chain check came to.
pub(crate) enum ChainStep {
    Done(Chain),
    /// The check goes on in another unit; this many units were charged.
    Working(usize),
}
