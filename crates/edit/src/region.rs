//! Navigation in running regions (sections §12).
//!
//! A running region is an *instance*: one placement of a story, on one page, in
//! one slot. Its lines are not in the snapshot's body blocks, so the body
//! queries never see it, and the instance itself is view state the app holds:
//! never document state. `hit_placed`, `caret_in` and `selection_rects_in` are
//! additive; `Navigator::hit`, `Caret` and `Selection` stay body-only.
//!
//! Each question is answered by a navigator over a small snapshot made from the
//! instance alone (its frame, as page 0, and its blocks), so the ordinary
//! caret model, line models and selection geometry apply unchanged, and the
//! answer is mapped back to the real page.

use reprise_geom::{PageSpace, Point, Rect};
use reprise_layout::{LayoutSnapshot, PageLayout, RunningInstance, SlotId};

use crate::caret::{Caret, CaretRect, Hit, Selection, SelectionRect};
use crate::navigator::Navigator;

/// One placement of a story: the page and the slot (by its stable token). An
/// instance only stays valid while a layout still shows the story there, so the
/// app asks again after a relayout (see [`Navigator::caret_in`]).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionInstance {
    pub page: usize,
    pub slot: String,
}

/// Which regions a hit test may land in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HitMode {
    /// Headers and footers win over the body in their rectangles.
    #[default]
    Normal,
    /// A background is hit too, when nothing above it is.
    Background,
}

/// What a point on a page means.
///
/// Non-exhaustive: a frame chain's frame is a further placement (the frames
/// design adds `Chain`), so match with a wildcard.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlacedHit {
    Body(Hit),
    Running {
        position: Caret,
        instance: RegionInstance,
    },
}

/// An instance's own small snapshot, and where it really is.
struct View {
    snapshot: LayoutSnapshot,
    page: usize,
}

impl Navigator<'_> {
    fn instances(&self) -> impl Iterator<Item = &RunningInstance> {
        self.snapshot.sections.iter().flat_map(|s| s.running.iter())
    }

    fn instance(&self, at: &RegionInstance) -> Option<&RunningInstance> {
        self.instances()
            .find(|r| r.page == at.page && r.slot == at.slot)
    }

    /// The instance as a one-frame, one-page snapshot.
    fn view(&self, instance: &RunningInstance) -> Option<View> {
        let frame = self.snapshot.frame(instance.frame)?.clone();
        let page = frame.page;
        let size = self
            .snapshot
            .pages
            .get(page)
            .copied()
            .unwrap_or(PageLayout {
                width: reprise_geom::Length::ZERO,
                height: reprise_geom::Length::ZERO,
            });
        let mut single = frame;
        single.page = 0;
        let mut blocks = instance.blocks.clone();
        for line in blocks.iter_mut().flat_map(|b| b.lines.iter_mut()) {
            line.frame = 0;
        }
        let s = self.snapshot;
        let snapshot = LayoutSnapshot {
            font_settings: s.font_settings.clone(),
            revision: s.revision.clone(),
            adapter: s.adapter.clone(),
            composer: s.composer.clone(),
            medium: s.medium,
            settings: s.settings,
            template: s.template.clone(),
            pages: vec![size],
            frames: vec![single],
            blocks,
            relations: Vec::new(),
            repeated_headers: Vec::new(),
            diagnostics: Vec::new(),
            sections: None,
        };
        Some(View { snapshot, page })
    }

    /// The page rectangle a region answers hits in: its frame, widened over the
    /// margin band it sits in, up to the body (a header) or from it (a footer),
    /// so a click anywhere in the margin edits the region.
    fn hit_area(&self, instance: &RunningInstance) -> Option<Rect<PageSpace>> {
        let frame = self.snapshot.frame(instance.frame)?;
        let own = frame.to_page.bounds(&frame.rect);
        let descriptor = self.snapshot.sections.as_ref()?.pages.get(instance.page)?;
        let top = descriptor.bands.first().map(|b| b.body.origin.y);
        let bottom = descriptor
            .bands
            .last()
            .map(|b| b.body.origin.y + b.body.height);
        let spec = SlotId::parse(&instance.slot)?;
        let (y0, y1) = match (spec.reserves_top(), spec.reserves_bottom()) {
            (true, _) => (
                reprise_geom::Length::ZERO,
                top.map_or(own.max_y(), |t| t.max(own.max_y())),
            ),
            (_, true) => (
                bottom.map_or(own.origin.y, |b| b.min(own.origin.y)),
                self.snapshot.pages.get(instance.page)?.height,
            ),
            _ => (own.origin.y, own.max_y()),
        };
        Some(Rect::new(
            Point::new(own.origin.x, y0),
            own.width,
            (y1 - y0).max(reprise_geom::Length::ZERO),
        ))
    }

    /// Hit testing across the body and the running regions of `page`. A header
    /// or footer wins over the body inside its rectangle; a background answers
    /// only in [`HitMode::Background`], and only where nothing above it does.
    /// A region whose story has no laid-out text cannot take a caret and is
    /// skipped. `Navigator::hit` is unchanged and body-only.
    pub fn hit_placed(
        &self,
        page: usize,
        point: Point<PageSpace>,
        mode: HitMode,
    ) -> Option<PlacedHit> {
        let mut candidates: Vec<(u8, &RunningInstance)> = self
            .instances()
            .filter(|r| r.page == page && r.blocks.iter().any(|b| !b.lines.is_empty()))
            .filter_map(|r| {
                let slot = SlotId::parse(&r.slot)?;
                (mode == HitMode::Background || !slot.paints_below())
                    .then_some((slot.hit_priority(), r))
            })
            .collect();
        // Higher priority first; the snapshot's order breaks ties.
        candidates.sort_by_key(|(p, _)| std::cmp::Reverse(*p));
        for (_, instance) in candidates {
            let inside = self.hit_area(instance).is_some_and(|a| {
                point.x >= a.origin.x
                    && point.x <= a.max_x()
                    && point.y >= a.origin.y
                    && point.y <= a.max_y()
            });
            if !inside {
                continue;
            }
            let view = self.view(instance)?;
            let local = Navigator::new(&view.snapshot, view.snapshot.blocks.iter().map(|b| b.node));
            if let Some(hit) = local.hit(0, point) {
                return Some(PlacedHit::Running {
                    position: hit.caret,
                    instance: RegionInstance {
                        page,
                        slot: instance.slot.clone(),
                    },
                });
            }
        }
        self.hit(page, point).map(PlacedHit::Body)
    }

    /// The instance that shows `caret`'s block now: `at` itself if its page
    /// still shows the block in that slot, else the nearest page that does, by
    /// physical distance and lower on ties. `None` when no page does.
    fn resolve(&self, at: &RegionInstance, caret: &Caret) -> Option<&RunningInstance> {
        let shows = |r: &&RunningInstance| {
            r.slot == at.slot && r.blocks.iter().any(|b| b.node == caret.node)
        };
        self.instances()
            .filter(shows)
            .min_by_key(|r| (r.page.abs_diff(at.page), r.page))
    }

    /// Where `position` is drawn in `instance`, on its page. If a relayout
    /// moved the story off that page, the nearest page that still shows it in
    /// that slot answers (lower on ties); with none, there is no geometry. The
    /// caret itself stays valid: it is a position in the story.
    pub fn caret_in(&self, instance: &RegionInstance, position: Caret) -> Option<CaretRect> {
        let shown = self.resolve(instance, &position)?;
        let view = self.view(shown)?;
        let local = Navigator::new(&view.snapshot, view.snapshot.blocks.iter().map(|b| b.node));
        let mut rect = local.caret_rect(position)?;
        rect.page = view.page;
        Some(rect)
    }

    /// The page rectangles covering `selection` in `instance`, resolved like
    /// [`Navigator::caret_in`]. Empty when the story is not shown.
    pub fn selection_rects_in(
        &self,
        instance: &RegionInstance,
        selection: &Selection,
    ) -> Vec<SelectionRect> {
        let Some(shown) = self.resolve(instance, &selection.focus) else {
            return Vec::new();
        };
        let Some(view) = self.view(shown) else {
            return Vec::new();
        };
        let local = Navigator::new(&view.snapshot, view.snapshot.blocks.iter().map(|b| b.node));
        local
            .selection_rects(selection)
            .into_iter()
            .map(|mut r| {
                r.page = view.page;
                r
            })
            .collect()
    }

    /// Whether a layout shows `instance` as it is.
    pub fn shows(&self, instance: &RegionInstance) -> bool {
        self.instance(instance).is_some()
    }
}
