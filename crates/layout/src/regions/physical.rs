//! Constraints whose source geometry is independent of a balanced main depth.
//!
//! Keep full provenance on the physical source frame, then project its frozen
//! occupied box into the *current* main frames. This also reaches earlier bands.
use super::*;
use crate::pages::{Catalog, Entry, FrameAddr};
use reprise_geom::Point;

pub(crate) type Boxes = BTreeMap<FrameAddr, Entry<Vec<Rect<FrameSpace>>>>;
pub(crate) type Areas = BTreeMap<FrameAddr, Entry<Rect<FrameSpace>>>;

fn project(source: &crate::FrameLayout, target: &crate::FrameLayout, rect: Rect<FrameSpace>) -> Option<Rect<FrameSpace>> {
    if source.page != target.page { return None; }
    let inverse = target.to_page.inverse()?;
    let corners = [rect.origin, Point::new(rect.origin.x + rect.width, rect.origin.y),
        Point::new(rect.origin.x, rect.origin.y + rect.height),
        Point::new(rect.origin.x + rect.width, rect.origin.y + rect.height)];
    let points = corners.map(|p| inverse.apply(source.to_page.apply(p)));
    let left = points.iter().map(|p| p.x).min()?;
    let right = points.iter().map(|p| p.x).max()?;
    let top = points.iter().map(|p| p.y).min()?;
    let bottom = points.iter().map(|p| p.y).max()?;
    (right > Length::ZERO && left < target.rect.width
        && bottom > Length::ZERO && top < target.rect.height)
        .then(|| Rect::new(Point::new(left, top), right - left, bottom - top))
}

impl Plan {
    pub(crate) fn physical_exclusions_for(&self, catalog: &Catalog, snapshot: &LayoutSnapshot, index: usize) -> Vec<Rect<FrameSpace>> {
        let Some(target) = snapshot.frame(index) else { return Vec::new(); };
        self.physical_exclusions.iter().filter_map(|(address, entry)| {
            let source_index = catalog.index(address)?;
            let boxes = entry.for_frame(catalog.reference(source_index)?)?;
            Some((snapshot.frame(source_index)?, boxes))
        }).flat_map(|(source, boxes)| boxes.iter().filter_map(|r| project(source, target, *r))).collect()
    }
    pub(crate) fn physical_reservation_for(&self, catalog: &Catalog, snapshot: &LayoutSnapshot, index: usize) -> Length {
        let Some(target) = snapshot.frame(index) else { return Length::ZERO; };
        self.note_areas.iter().filter_map(|(address, entry)| {
            let source_index = catalog.index(address)?;
            let rect = entry.for_frame(catalog.reference(source_index)?)?;
            project(snapshot.frame(source_index)?, target, *rect)
        }).map(|r| (target.rect.height - r.origin.y.max(Length::ZERO)).max(Length::ZERO))
            .max().unwrap_or_default()
    }
    pub(crate) fn exclude_physical(&mut self, source: &crate::pages::FrameRef, rect: Rect<FrameSpace>) {
        let entry = self.physical_exclusions.entry(source.address.clone()).or_insert_with(|| Entry::new(source, Vec::new()));
        if entry.provenance != source.provenance { *entry = Entry::new(source, Vec::new()); }
        entry.value.push(rect);
    }
}
