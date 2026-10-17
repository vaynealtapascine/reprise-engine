//! Paragraph geometry and breaking lookahead (`docs/paragraph-styles.md`):
//! the room a frame offers a paragraph (indents, exclusions, depth), the
//! next frame without creating it, and the keep-with-next chain check.
//!
//! Everything here is a trial: it composes into scratch output that is
//! thrown away, so nothing it does reaches the snapshot except the decision.
//! Trial preparations are cached by exact `(node, context)`, so the cache
//! can't change output.

use std::sync::Arc;

use super::*;
use crate::template::ResolvedFrame;

impl Flow<'_> {
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

    /// The blocks after `node` in document order, built once per layout.
    fn following(&mut self, doc: &Document, node: NodeId) -> Vec<NodeId> {
        let order = self
            .keeps
            .order
            .get_or_insert_with(|| {
                let nodes = doc.top(self.domain.unwrap_or(reprise_doc::Domain::Body));
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

    /// `node` prepared against `ctx`, for a trial only: its diagnostics are
    /// dropped.
    fn trial(
        &mut self,
        doc: &Document,
        node: NodeId,
        ctx: &ResolutionContext,
    ) -> Option<Arc<Prepared>> {
        if let Some((_, _, p)) = self
            .keeps
            .trials
            .iter()
            .find(|(n, c, _)| *n == node && c == ctx)
        {
            return Some(p.clone());
        }
        let mut scratch = Vec::new();
        let prepared = Arc::new(prepare_cached(
            self.engine,
            doc,
            node,
            ctx,
            &mut scratch,
            self.evaluation,
        )?);
        if self.keeps.trials.len() >= MAX_TRIALS {
            self.keeps.trials.remove(0);
        }
        self.keeps
            .trials
            .push((node, ctx.clone(), prepared.clone()));
        Some(prepared)
    }

    /// Whether the blocks after `node`, which keeps with them, fit the rest
    /// of the frame below `end` (the paragraph's block end): each
    /// keep-with-next member whole, then the first other block's
    /// `max(orphans, 1)` lines (all of them if it keeps together). Tables
    /// and flowed images end a chain as fitting; annotations placed by
    /// relations are skipped.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn chain_fits(
        &mut self,
        doc: &Document,
        node: NodeId,
        after: Length,
        end: Length,
        index: usize,
        frame: &ResolvedFrame,
        ctx: &ResolutionContext,
    ) -> Chain {
        let spacing = self.engine.flow.paragraph_spacing;
        let mut used = end + spacing;
        let mut after = after;
        for (count, next) in self.following(doc, node).into_iter().enumerate() {
            if count >= MAX_KEEP_CHAIN {
                return Chain::TooLong;
            }
            if self.region_owners.contains(&next) {
                continue;
            }
            match doc.table_role(next) {
                Ok(None) => {}
                _ => return Chain::Fits,
            }
            match doc.block(next).map(|b| b.kind) {
                Ok(BlockKind::Paragraph) => {}
                Ok(BlockKind::Annotation) => continue,
                _ => return Chain::Fits,
            }
            let Some(prepared) = self.trial(doc, next, ctx) else {
                return Chain::Fits;
            };
            if prepared.image.is_some() {
                return Chain::Fits;
            }
            let style = &prepared.style;
            let top = used + after.max(style.space_before);
            let room = self.room(index, frame, &prepared, 0);
            if top > room.depth {
                return Chain::Fails;
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
                None,
                &self.fields(),
            );
            let lines = rows(&composed.lines).len();
            if lines == 0 {
                return Chain::Fails;
            }
            let whole = composed.rest.is_none();
            if (style.keep_with_next || style.keep_together) && !whole {
                return Chain::Fails;
            }
            if !style.keep_with_next {
                return if whole || lines >= style.orphans() {
                    Chain::Fits
                } else {
                    Chain::Fails
                };
            }
            used = composed.block_end + spacing;
            after = style.space_after;
        }
        Chain::Fits
    }
}
