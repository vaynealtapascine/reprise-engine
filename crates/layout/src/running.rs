//! Running regions (sections §9): the slot registry, variant selection and the
//! integer reservation budget. Composition is in [`compose`]; attaching a
//! region to a page is in [`attach`].
//!
//! `RunningSlots` is an ordered registry. A slot supplies its name, the
//! variants it supports, its box from the page geometry, its reservation side,
//! its paint order, its hit priority, its artifact type and its export form. A
//! later slot (a margin note, a line-number gutter) is one more entry; nothing
//! here changes for it.

use reprise_doc::NodeId;
use reprise_doc::sections::{BindingView, SectionId, SectionView};
use reprise_geom::{Length, PageSpace, Point, Rect};
use serde::{Serialize, Serializer};

#[cfg(test)]
mod accept_tests;
pub(crate) mod attach;
#[cfg(test)]
mod combo_tests;
pub(crate) mod compose;
#[cfg(test)]
mod field_tests;
#[cfg(test)]
mod incremental_tests;
#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod stepped_tests;
#[cfg(test)]
mod tests;

/// The most blocks of one story read per layout (sections §9).
pub(crate) const MAX_STORY_BLOCKS: usize = 4096;
/// The most visible text bytes of one story read per layout.
pub(crate) const MAX_STORY_TEXT: usize = 1 << 20;
/// The role prefix of a running region's frame.
pub(crate) const FRAME_PREFIX: &str = "reprise.running.";

/// A dense index into the slot registry. In-process only: every JSON form
/// names a slot by its stable token, so adding a slot never renumbers stored
/// or exchanged data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotId(pub u16);

impl SlotId {
    /// The stable token of this slot, or `"?"` for an index past the registry.
    pub fn token(self) -> &'static str {
        SLOTS.get(usize::from(self.0)).map_or("?", |s| s.name)
    }
    fn spec(self) -> Option<&'static SlotSpec> {
        SLOTS.get(usize::from(self.0))
    }
    /// Larger wins a hit where regions overlap; the body is below all of them.
    pub fn hit_priority(self) -> u8 {
        self.spec().map_or(0, |s| s.hit_priority)
    }
    /// The slot takes its room from the top of the body.
    pub fn reserves_top(self) -> bool {
        self.spec().is_some_and(|s| s.side == Side::Top)
    }
    /// The slot takes its room from the bottom of the body.
    pub fn reserves_bottom(self) -> bool {
        self.spec().is_some_and(|s| s.side == Side::Bottom)
    }
    /// The slot is drawn below the body (a background).
    pub fn paints_below(self) -> bool {
        self.spec().is_some_and(|s| s.paint == Paint::Below)
    }
    /// How exports mark the slot's content (sections §13).
    pub fn artifact(self) -> Option<Artifact> {
        self.spec().map(|s| s.artifact)
    }
    /// The slot with stable token `name`.
    pub fn parse(name: &str) -> Option<SlotId> {
        SLOTS
            .iter()
            .position(|s| s.name == name)
            .and_then(|i| u16::try_from(i).ok())
            .map(SlotId)
    }
}

impl Serialize for SlotId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.token())
    }
}

/// Which edge of the page a slot reserves from the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Top,
    Bottom,
    None,
}

/// Where a slot paints relative to the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Paint {
    Below,
    Above,
}

/// How an export marks a slot's content (sections §13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Artifact {
    Header,
    Footer,
    Watermark,
}

/// Where a slot's box is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Region {
    /// The page's inline margins, `header.distance` from the top edge.
    HeaderBand,
    /// The same width, ending `footer.distance` from the bottom edge.
    FooterBand,
    /// The whole page.
    Page,
}

/// The variants a slot can have, in selection order of preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Variant {
    Default,
    First,
    Even,
}

impl Variant {
    pub const ALL: [Variant; 3] = [Variant::Default, Variant::First, Variant::Even];
    pub fn token(self) -> &'static str {
        match self {
            Variant::Default => "default",
            Variant::First => "first",
            Variant::Even => "even",
        }
    }
    fn index(self) -> usize {
        match self {
            Variant::Default => 0,
            Variant::First => 1,
            Variant::Even => 2,
        }
    }
}

/// One entry of the registry.
#[derive(Debug)]
pub(crate) struct SlotSpec {
    pub name: &'static str,
    pub region: Region,
    pub side: Side,
    pub paint: Paint,
    /// Larger wins a hit where regions overlap; the body is below all of them.
    pub hit_priority: u8,
    pub artifact: Artifact,
}

/// Header, footer, then the background. The order is also the order instances
/// are made and listed in.
pub(crate) const SLOTS: &[SlotSpec] = &[
    SlotSpec {
        name: "header",
        region: Region::HeaderBand,
        side: Side::Top,
        paint: Paint::Above,
        hit_priority: 2,
        artifact: Artifact::Header,
    },
    SlotSpec {
        name: "footer",
        region: Region::FooterBand,
        side: Side::Bottom,
        paint: Paint::Above,
        hit_priority: 2,
        artifact: Artifact::Footer,
    },
    SlotSpec {
        name: "background",
        region: Region::Page,
        side: Side::None,
        paint: Paint::Below,
        hit_priority: 1,
        artifact: Artifact::Watermark,
    },
];

/// The slot whose frame role is `role`, if it is a running frame's.
pub(crate) fn slot_of_role(role: &str) -> Option<SlotId> {
    SlotId::parse(role.strip_prefix(FRAME_PREFIX)?)
}

/// The frame role of a slot.
pub(crate) fn role_of(slot: SlotId) -> String {
    format!("{FRAME_PREFIX}{}", slot.token())
}

/// A binding after "same as previous" is followed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effective {
    /// Nothing ever bound it along the chain.
    Unset,
    /// Explicitly no region.
    None,
    /// A live, root-level story.
    Story(NodeId),
    /// Names something that is not a live story: renders empty, reported.
    Missing(String),
}

impl Effective {
    pub fn is_unset(&self) -> bool {
        *self == Effective::Unset
    }
}

/// One section's effective binding of every `(slot, variant)`, and where each
/// was authored, for diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SectionBindings {
    cells: Vec<(Effective, SectionId)>,
}

impl SectionBindings {
    pub fn get(&self, slot: usize, variant: Variant) -> (&Effective, SectionId) {
        static UNSET: Effective = Effective::Unset;
        self.cells
            .get(slot * 3 + variant.index())
            .map_or((&UNSET, SectionId::First), |(e, s)| (e, *s))
    }
    /// Whether any cell shows something (or reports something missing).
    fn active(&self) -> bool {
        self.cells
            .iter()
            .any(|(e, _)| matches!(e, Effective::Story(_) | Effective::Missing(_)))
    }
}

/// Every section's effective bindings, resolved in one forward scan.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct Bindings {
    sections: Vec<SectionBindings>,
    any: bool,
}

impl Bindings {
    /// `views` are the section table's views, in body order, first section
    /// first. Inherit follows the nearest earlier section's same cell, and the
    /// first section inheriting is unset; an explicit story that is not live
    /// is `Missing` and never falls through to an earlier story (§6).
    pub fn resolve(views: &[SectionView]) -> Self {
        let mut chain: Vec<(Effective, SectionId)> =
            vec![(Effective::Unset, SectionId::First); SLOTS.len() * 3];
        let mut sections = Vec::with_capacity(views.len());
        for view in views {
            for (slot, spec) in SLOTS.iter().enumerate() {
                for variant in Variant::ALL {
                    let cell = chain.get_mut(slot * 3 + variant.index());
                    let binding = view
                        .bindings
                        .iter()
                        .find(|b| b.slot == spec.name && b.variant == variant.token());
                    if let (Some(cell), Some(binding)) = (cell, binding) {
                        apply(cell, binding, view.id);
                    }
                }
            }
            sections.push(SectionBindings {
                cells: chain.clone(),
            });
        }
        let any = sections.iter().any(SectionBindings::active);
        Self { sections, any }
    }
    /// Whether any section shows or reports a running region at all. Documents
    /// where this is false lay out exactly as they did before regions.
    pub fn any(&self) -> bool {
        self.any
    }
    pub fn of(&self, section: usize) -> Option<&SectionBindings> {
        self.sections.get(section)
    }
}

fn apply(cell: &mut (Effective, SectionId), binding: &BindingView, at: SectionId) {
    match binding.authored.as_str() {
        "none" => *cell = (Effective::None, at),
        "story" => {
            *cell = (
                match binding.resolved {
                    Some(story) => Effective::Story(story),
                    None => Effective::Missing(
                        binding
                            .story
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "unreadable".into()),
                    ),
                },
                at,
            );
        }
        _ => {}
    }
}

/// What picks a variant on one page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PageFacts {
    /// 0-based physical page index.
    pub index: usize,
    /// The owner's first content page.
    pub first_owned: bool,
    /// The section's `region.first` and `region.even` switches.
    pub use_first: bool,
    pub use_even: bool,
    /// A filler never takes the first variant.
    pub filler: bool,
}

impl PageFacts {
    /// Physical, 1-based parity: page 2 is even, whatever its label.
    pub fn is_even(&self) -> bool {
        self.index % 2 == 1
    }
}

/// The variant a slot shows on a page (sections §9, "Variant selection"):
/// `first` when the section's first owned content page and the switch is on and
/// the first binding is defined; else `even` likewise; else `default`. An
/// explicit None is defined, so it selects an empty region and stops fallback.
pub(crate) fn select(bindings: &SectionBindings, slot: usize, page: &PageFacts) -> Variant {
    if page.use_first
        && page.first_owned
        && !page.filler
        && !bindings.get(slot, Variant::First).0.is_unset()
    {
        Variant::First
    } else if page.use_even && page.is_even() && !bindings.get(slot, Variant::Even).0.is_unset() {
        Variant::Even
    } else {
        Variant::Default
    }
}

/// The page-space box of `region` on a page of `width` by `height` with
/// margins `left`/`right` and edge distance `distance`; `depth` is the height
/// a header or footer takes. Footers end at `distance` from the bottom edge.
pub(crate) fn region_rect(
    region: Region,
    width: Length,
    height: Length,
    left: Length,
    right: Length,
    distance: Length,
    depth: Length,
) -> Rect<PageSpace> {
    let inline = (width - left - right).max(Length::ZERO);
    match region {
        Region::HeaderBand => Rect::new(Point::new(left, distance), inline, depth),
        Region::FooterBand => Rect::new(
            Point::new(left, (height - distance - depth).max(Length::ZERO)),
            inline,
            depth,
        ),
        Region::Page => Rect::new(Point::origin(), width, height),
    }
}

/// The reservation budget of one horizontal-tb page (sections §9), or of the
/// physical top/bottom bands of a vertical one. Header first, then footer;
/// integer arithmetic only; no proportions.
///
/// `top = min(max(T, h), H − B − keep)` and
/// `bottom = min(max(B, f), H − top − keep)`, with `keep = max(1, D/4)` and
/// `D = H − T − B`. Hence `H − top − bottom ≥ keep ≥ 1`, and each term is at
/// least its authored margin: a cap only takes from running reservations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Budget {
    pub height: i64,
    pub top: i64,
    pub bottom: i64,
    pub keep: i64,
    pub header_distance: i64,
    pub footer_distance: i64,
}

impl Budget {
    pub fn new(
        height: Length,
        top: Length,
        bottom: Length,
        header_distance: Length,
        footer_distance: Length,
    ) -> Self {
        let (height, top, bottom) = (
            i64::from(height.0),
            i64::from(top.0).max(0),
            i64::from(bottom.0).max(0),
        );
        let authored = (height - top - bottom).max(1);
        Self {
            height,
            top,
            bottom,
            keep: (authored / 4).max(1),
            header_distance: i64::from(header_distance.0).max(0),
            footer_distance: i64::from(footer_distance.0).max(0),
        }
    }
    /// The deepest the header may be composed: what the cap leaves.
    pub fn header_cap(&self) -> Length {
        lengths(self.height - self.bottom - self.keep - self.header_distance)
    }
    /// The page's top reservation for a header that needs `need`
    /// (`header.distance` plus its composed depth; 0 for no header).
    pub fn top_for(&self, need: i64) -> i64 {
        self.top
            .max(need)
            .min(self.height - self.bottom - self.keep)
    }
    /// The deepest the footer may be composed under reservation `top`.
    pub fn footer_cap(&self, top: i64) -> Length {
        lengths(self.height - top - self.keep - self.footer_distance)
    }
    /// The page's bottom reservation for a footer that needs `need`.
    pub fn bottom_for(&self, top: i64, need: i64) -> i64 {
        self.bottom.max(need).min(self.height - top - self.keep)
    }
    pub fn need(&self, distance: i64, composed: Length, empty: bool) -> i64 {
        if empty {
            0
        } else {
            distance.saturating_add(i64::from(composed.0).max(0))
        }
    }
}

fn lengths(v: i64) -> Length {
    Length(i32::try_from(v.clamp(0, i64::from(i32::MAX))).unwrap_or(i32::MAX))
}

/// A clamp of a non-negative budget value to a `Length`.
pub(crate) fn length(v: i64) -> Length {
    lengths(v)
}
