//! Page/frame allocation and stable logical addresses. Snapshot indices are output tags.
use crate::template::ResolvedTemplate;
use crate::{Engine, FrameLayout, LayoutSnapshot, PageLayout};
use reprise_doc::sections::SectionId;
use reprise_doc::{FrameRole, MAIN_FLOW, WritingMode};
use reprise_geom::{FrameSpace, PageSpace, Rect, Transform};
use std::collections::BTreeMap;
mod bands;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FrameAddr {
    Main {
        page: usize,
        band: usize,
        column: usize,
    },
    Other {
        page: usize,
        role: String,
        ordinal: usize,
    },
    #[allow(dead_code)] // Running slot frames are connected in S6a.
    Running { page: usize, slot: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Provenance {
    pub section: SectionId,
    pub life: String,
    pub rect: Rect<FrameSpace>,
    pub to_page: Transform<FrameSpace, PageSpace>,
    pub writing_mode: WritingMode,
    pub running: Option<(String, reprise_doc::NodeId)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrameRef {
    pub address: FrameAddr,
    pub provenance: Provenance,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry<T> {
    pub provenance: Provenance,
    pub value: T,
}
impl<T> Entry<T> {
    pub fn new(frame: &FrameRef, value: T) -> Self {
        Self {
            provenance: frame.provenance.clone(),
            value,
        }
    }
    pub fn for_frame(&self, frame: &FrameRef) -> Option<&T> {
        (self.provenance == frame.provenance).then_some(&self.value)
    }
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Catalog {
    by_index: Vec<FrameRef>,
    by_address: BTreeMap<FrameAddr, usize>,
    ordinals: BTreeMap<(usize, usize), usize>,
    mains: BTreeMap<usize, usize>,
    others: BTreeMap<(usize, String), usize>,
    counts: BTreeMap<usize, usize>,
    lives: BTreeMap<SectionId, String>,
}
fn role_key(role: &FrameRole) -> String {
    match role {
        FrameRole::Flow(name) => format!("flow:{name}"),
        FrameRole::Margin => "margin".into(),
        FrameRole::Notes => "notes".into(),
    }
}
impl Catalog {
    pub fn from_snapshot(snapshot: &LayoutSnapshot) -> Self {
        let mut catalog = Self::default();
        catalog.extend(snapshot);
        catalog
    }
    pub fn extend(&mut self, snapshot: &LayoutSnapshot) {
        for (index, frame) in snapshot.frames.iter().enumerate().skip(self.by_index.len()) {
            let page = frame.page;
            let t = self.counts.entry(page).or_default();
            self.ordinals.insert((page, *t), index);
            *t = t.saturating_add(1);
            let descriptor = snapshot.sections.as_ref().and_then(|s| s.pages.get(page));
            let band = descriptor.and_then(|p| {
                p.bands.iter().enumerate().find_map(|(b, band)| {
                    band.frames
                        .iter()
                        .position(|i| *i == index)
                        .map(|column| (b, column, band))
                })
            });
            let address = if frame.role == FrameRole::Flow(MAIN_FLOW.into()) {
                let column = self.mains.entry(page).or_default();
                let address = FrameAddr::Main {
                    page,
                    band: band.map_or(0, |(b, _, _)| b),
                    column: band.map_or(*column, |(_, c, _)| c),
                };
                *column = column.saturating_add(1);
                address
            } else {
                let role = role_key(&frame.role);
                let ordinal = self.others.entry((page, role.clone())).or_default();
                let address = FrameAddr::Other {
                    page,
                    role,
                    ordinal: *ordinal,
                };
                *ordinal = ordinal.saturating_add(1);
                address
            };
            if let Some((_, _, band)) = band {
                self.lives.insert(band.section, band.life.clone());
            }
            let section = band
                .map(|(_, _, b)| b.section)
                .or_else(|| descriptor.map(|p| p.section))
                .unwrap_or(SectionId::First);
            let provenance = Provenance {
                section: band
                    .map(|(_, _, b)| b.section)
                    .or_else(|| descriptor.map(|p| p.section))
                    .unwrap_or(SectionId::First),
                life: self
                    .lives
                    .get(&section)
                    .cloned()
                    .or_else(|| {
                        descriptor
                            .and_then(|p| p.bands.first())
                            .map(|b| b.life.clone())
                    })
                    .unwrap_or_else(|| "0".into()),
                rect: frame.rect,
                to_page: frame.to_page,
                writing_mode: frame.writing_mode,
                running: None,
            };
            self.by_address.insert(address.clone(), index);
            self.by_index.push(FrameRef {
                address,
                provenance,
            });
        }
    }
    pub fn reference(&self, index: usize) -> Option<&FrameRef> {
        self.by_index.get(index)
    }
    pub fn index(&self, address: &FrameAddr) -> Option<usize> {
        self.by_address.get(address).copied()
    }
    pub fn ordinal(&self, page: usize, ordinal: usize) -> Option<usize> {
        self.ordinals.get(&(page, ordinal)).copied()
    }
}

/// One limit and one append path, including memo replay. Diagnostics stay at
/// their existing call sites so the legacy path retains its exact output.
pub(crate) struct PageFactory;
impl PageFactory {
    pub fn push(
        engine: &Engine,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
    ) -> bool {
        if snapshot.pages.len() >= engine.flow.max_pages.max(1) as usize {
            return false;
        }
        let page = snapshot.pages.len();
        Self::append(
            snapshot,
            PageLayout {
                width: template.width,
                height: template.height,
            },
            template.frames.iter().map(|f| f.layout(page)),
        );
        true
    }
    pub fn push_section(
        engine: &Engine,
        sections: &crate::sections::Table,
        section: &crate::sections::Section,
        kind: crate::PageKind,
        snapshot: &mut LayoutSnapshot,
    ) -> bool {
        if snapshot.pages.len() >= engine.flow.max_pages.max(1) as usize {
            return false;
        }
        let page = snapshot.pages.len();
        let first_content = section.restart.is_some()
            && snapshot.sections.as_ref().is_none_or(|s| {
                !s.pages
                    .iter()
                    .any(|p| p.section == section.id() && p.kind == crate::PageKind::Content)
            });
        let previous = snapshot
            .sections
            .as_ref()
            .and_then(|s| s.pages.last())
            .map(|p| p.counter);
        let counter =
            if kind == crate::PageKind::Content && first_content && section.restart.is_some() {
                section.restart.unwrap_or(1)
            } else if let Some(previous) = previous {
                if previous == i64::MAX {
                    let note = crate::sections::diagnostic(
                        section.id(),
                        "numbering.restart",
                        "section.page-counter",
                        reprise_diag::Severity::Warning,
                        "page counter saturates at i64::MAX",
                    );
                    if !snapshot.diagnostics.contains(&note) {
                        snapshot.diagnostics.push(note);
                    }
                }
                previous.saturating_add(1)
            } else {
                sections
                    .sections
                    .first()
                    .and_then(|s| s.restart)
                    .unwrap_or(1)
            };
        let base = snapshot.frames.len();
        let frames: Vec<_> = if kind == crate::PageKind::Filler {
            Vec::new()
        } else {
            section
                .template
                .frames
                .iter()
                .map(|f| f.layout(page))
                .collect()
        };
        let body: Vec<_> = frames
            .iter()
            .enumerate()
            .filter(|(_, f)| f.role == FrameRole::Flow(MAIN_FLOW.into()))
            .map(|(i, _)| base.saturating_add(i))
            .collect();
        Self::append(
            snapshot,
            PageLayout {
                width: section.template.width,
                height: section.template.height,
            },
            frames,
        );
        let metadata = snapshot
            .sections
            .get_or_insert_with(crate::SectionsLayout::default);
        metadata.pages.push(crate::PageDescriptor {
            section: section.id(),
            kind,
            counter,
            label: counter.to_string(),
            bands: if kind == crate::PageKind::Content {
                vec![crate::Band {
                    section: section.id(),
                    life: section.life().into(),
                    frames: body,
                    body: section.geometry.body_rect(),
                    offset: reprise_geom::Length::ZERO,
                    depth: section.geometry.block_extent(),
                    balanced: false,
                }]
            } else {
                Vec::new()
            },
        });
        if kind == crate::PageKind::Content {
            metadata
                .rules
                .extend(section.rules.iter().copied().map(|rect| crate::ColumnRule {
                    page,
                    band: 0,
                    rect,
                }));
        }
        if kind != crate::PageKind::Filler {
            for prototype in sections.float_spaces.for_section(section) {
                let mut frame = prototype.clone();
                frame.page = page;
                Self::append_region(snapshot, frame);
            }
        }
        true
    }
    pub fn ensure_section(
        engine: &Engine,
        sections: &crate::sections::Table,
        owner: SectionId,
        kind: crate::PageKind,
        snapshot: &mut LayoutSnapshot,
        page: usize,
    ) -> bool {
        if page >= engine.flow.max_pages.max(1) as usize {
            return false;
        }
        let Some(section) = sections.get(owner) else {
            return false;
        };
        while snapshot.pages.len() <= page {
            if !Self::push_section(engine, sections, section, kind, snapshot) {
                return false;
            }
        }
        true
    }
    fn append(
        snapshot: &mut LayoutSnapshot,
        page: PageLayout,
        frames: impl IntoIterator<Item = FrameLayout>,
    ) {
        snapshot.pages.push(page);
        snapshot.frames.extend(frames);
    }
    pub fn ensure(
        engine: &Engine,
        template: &ResolvedTemplate,
        snapshot: &mut LayoutSnapshot,
        page: usize,
    ) -> bool {
        if page >= engine.flow.max_pages.max(1) as usize {
            return false;
        }
        while snapshot.pages.len() <= page {
            if !Self::push(engine, template, snapshot) {
                return false;
            }
        }
        true
    }
    pub fn replay(
        engine: &Engine,
        snapshot: &mut LayoutSnapshot,
        pages: Vec<PageLayout>,
        frames: Vec<FrameLayout>,
    ) -> bool {
        let Some(end) = snapshot.pages.len().checked_add(pages.len()) else {
            return false;
        };
        if end > engine.flow.max_pages.max(1) as usize {
            return false;
        }
        // A delta can include existing-page frame extensions (bands). Indices
        // were checked by its exact position key before this factory is called.
        snapshot.pages.extend(pages);
        snapshot.frames.extend(frames);
        true
    }
    pub fn replay_sections(snapshot: &mut LayoutSnapshot, metadata: Option<crate::SectionsLayout>) {
        if let Some(metadata) = metadata {
            let current = snapshot
                .sections
                .get_or_insert_with(crate::SectionsLayout::default);
            current.pages.extend(metadata.pages);
            current.running.extend(metadata.running);
            current.rules.extend(metadata.rules);
        }
    }
    pub fn index(snapshot: &LayoutSnapshot, page: usize, ordinal: usize) -> Option<usize> {
        snapshot
            .frames
            .iter()
            .enumerate()
            .filter(|(_, f)| f.page == page)
            .nth(ordinal)
            .map(|(i, _)| i)
    }
    pub fn preview(
        template: &ResolvedTemplate,
        snapshot: &LayoutSnapshot,
        page: usize,
        ordinal: usize,
    ) -> Option<usize> {
        if page < snapshot.pages.len() {
            return Self::index(snapshot, page, ordinal);
        }
        let offset = page
            .checked_sub(snapshot.pages.len())?
            .checked_mul(template.frames.len())?;
        snapshot
            .frames
            .len()
            .checked_add(offset)?
            .checked_add(ordinal)
    }
    pub fn future_reference(
        template: &ResolvedTemplate,
        snapshot: &LayoutSnapshot,
        index: usize,
    ) -> Option<FrameRef> {
        // Current frames are served by the cursor's catalog. This branch only
        // previews a frame that has not been allocated yet.
        let len = template.frames.len();
        if len == 0 {
            return None;
        }
        let relative = index.checked_sub(snapshot.frames.len())?;
        let page = snapshot.pages.len().checked_add(relative / len)?;
        let ordinal = relative % len;
        let frame = template.frames.get(ordinal)?.layout(page);
        let address = if frame.role == FrameRole::Flow(MAIN_FLOW.into()) {
            let column = template
                .frames
                .iter()
                .take(ordinal)
                .filter(|f| f.is_main_flow())
                .count();
            FrameAddr::Main {
                page,
                band: 0,
                column,
            }
        } else {
            let earlier = template
                .frames
                .iter()
                .take(ordinal)
                .filter(|f| f.role == frame.role)
                .count();
            FrameAddr::Other {
                page,
                role: role_key(&frame.role),
                ordinal: earlier,
            }
        };
        Some(FrameRef {
            address,
            provenance: Provenance {
                section: SectionId::First,
                life: "0".into(),
                rect: frame.rect,
                to_page: frame.to_page,
                writing_mode: frame.writing_mode,
                running: None,
            },
        })
    }
    pub fn body_frames(
        snapshot: &LayoutSnapshot,
        page: usize,
    ) -> impl Iterator<Item = (usize, &FrameLayout)> {
        snapshot
            .frames
            .iter()
            .enumerate()
            .filter(move |(_, f)| f.page == page && f.role == FrameRole::Flow(MAIN_FLOW.into()))
    }
}

/// Resumable source policy. The one-frame adapter never allocates a page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum FrameSource {
    #[default]
    Paged,
    Band,
    #[allow(dead_code)] // The plain running-story runner connects this in S6a.
    OneFrame {
        index: usize,
        exhausted: bool,
    },
}
impl FrameSource {
    pub fn next(&mut self) -> bool {
        match self {
            Self::Paged | Self::Band => true,
            Self::OneFrame { exhausted, .. } => {
                *exhausted = true;
                false
            }
        }
    }
    pub fn is_exhausted(&self) -> bool {
        matches!(
            self,
            Self::OneFrame {
                exhausted: true,
                ..
            }
        )
    }
    pub fn can_page(&self) -> bool {
        matches!(self, Self::Paged)
    }
    pub fn single_index(&self) -> Option<usize> {
        match self {
            Self::OneFrame { index, .. } => Some(*index),
            _ => None,
        }
    }
}
