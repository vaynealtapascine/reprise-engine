//! Constraints whose source geometry is independent of a balanced main depth.
//!
//! Keep full provenance on the physical source frame, then project its frozen
//! occupied box into the *current* main frames. This also reaches earlier bands.
use super::*;
use crate::pages::{Catalog, Entry, FrameAddr};
use reprise_geom::Point;

pub(crate) type Boxes = BTreeMap<FrameAddr, Entry<Vec<Rect<FrameSpace>>>>;
pub(crate) type Areas = BTreeMap<FrameAddr, Entry<Rect<FrameSpace>>>;

fn project(
    source: &crate::FrameLayout,
    target: &crate::FrameLayout,
    rect: Rect<FrameSpace>,
) -> Option<Rect<FrameSpace>> {
    if source.page != target.page {
        return None;
    }
    let inverse = target.to_page.inverse()?;
    let corners = [
        rect.origin,
        Point::new(rect.origin.x + rect.width, rect.origin.y),
        Point::new(rect.origin.x, rect.origin.y + rect.height),
        Point::new(rect.origin.x + rect.width, rect.origin.y + rect.height),
    ];
    let points = corners.map(|p| inverse.apply(source.to_page.apply(p)));
    let left = points.iter().map(|p| p.x).min()?;
    let right = points.iter().map(|p| p.x).max()?;
    let top = points.iter().map(|p| p.y).min()?;
    let bottom = points.iter().map(|p| p.y).max()?;
    (right > Length::ZERO
        && left < target.rect.width
        && bottom > Length::ZERO
        && top < target.rect.height)
        .then(|| Rect::new(Point::new(left, top), right - left, bottom - top))
}

impl Plan {
    pub(crate) fn physical_exclusions_for(
        &self,
        catalog: &Catalog,
        snapshot: &LayoutSnapshot,
        index: usize,
    ) -> Vec<Rect<FrameSpace>> {
        let Some(target) = snapshot.frame(index) else {
            return Vec::new();
        };
        self.physical_exclusions
            .iter()
            .filter_map(|(address, entry)| {
                let source_index = catalog.index(address)?;
                let boxes = entry.for_frame(catalog.reference(source_index)?)?;
                Some((snapshot.frame(source_index)?, boxes))
            })
            .flat_map(|(source, boxes)| boxes.iter().filter_map(|r| project(source, target, *r)))
            .collect()
    }
    pub(crate) fn physical_reservation_for(
        &self,
        catalog: &Catalog,
        snapshot: &LayoutSnapshot,
        index: usize,
    ) -> Length {
        let Some(target) = snapshot.frame(index) else {
            return Length::ZERO;
        };
        self.note_areas
            .iter()
            .filter_map(|(address, entry)| {
                let source_index = catalog.index(address)?;
                let rect = entry.for_frame(catalog.reference(source_index)?)?;
                project(snapshot.frame(source_index)?, target, *rect)
            })
            .map(|r| (target.rect.height - r.origin.y.max(Length::ZERO)).max(Length::ZERO))
            .max()
            .unwrap_or_default()
    }
    pub(crate) fn exclude_physical(
        &mut self,
        source: &crate::pages::FrameRef,
        rect: Rect<FrameSpace>,
    ) {
        let entry = self
            .physical_exclusions
            .entry(source.address.clone())
            .or_insert_with(|| Entry::new(source, Vec::new()));
        if entry.provenance != source.provenance {
            *entry = Entry::new(source, Vec::new());
        }
        entry.value.push(rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reprise_doc::sections::SectionId;
    use reprise_geom::{Matrix, Transform};

    #[test]
    fn physical_constraints_reproject_current_bands_and_require_full_source_provenance() {
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "x").unwrap();
        let engine = Engine::new(reprise_font::FontStore::default());
        let mut snapshot = engine.layout(&doc);
        // Use the existing non-main frame as a stable physical source plane.
        snapshot.frames[1].to_page = snapshot.frames[0].to_page;
        let catalog = Catalog::from_snapshot(&snapshot);
        let source = catalog.reference(1).unwrap();
        let rect = Rect::new(
            Point::new(Length::ZERO, Length::from_pt(50)),
            Length::from_pt(100),
            Length::from_pt(10),
        );
        let mut plan = Plan::default();
        plan.exclude_physical(source, rect);
        plan.note_areas
            .insert(source.address.clone(), Entry::new(source, rect));
        assert_eq!(plan.physical_exclusions_for(&catalog, &snapshot, 0), [rect]);
        assert_eq!(
            plan.physical_reservation_for(&catalog, &snapshot, 0),
            snapshot.frames[0].rect.height - Length::from_pt(50)
        );
        snapshot.frames[0].rect.height = Length::from_pt(60);
        let mut shifted = *snapshot.frames[0].to_page.matrix();
        shifted.ty += Length::from_pt(30);
        snapshot.frames[0].to_page = Transform::new(shifted);
        let catalog = Catalog::from_snapshot(&snapshot);
        let projected = plan.physical_exclusions_for(&catalog, &snapshot, 0);
        assert_eq!(projected[0].origin.y, Length::from_pt(20));
        assert_eq!(
            plan.physical_reservation_for(&catalog, &snapshot, 0),
            Length::from_pt(40)
        );
        let mut variants = vec![source.provenance.clone(); 6];
        variants[0].section = SectionId::Block(node);
        variants[1].life = "99@1".into();
        variants[2].rect.width += Length(1);
        variants[3].to_page = Transform::new(Matrix::translate(Length(1), Length::ZERO));
        variants[4].writing_mode = reprise_doc::WritingMode::VerticalRl;
        variants[5].running = Some(("header".into(), node));
        for provenance in variants {
            let mut stale = plan.clone();
            stale
                .physical_exclusions
                .get_mut(&source.address)
                .unwrap()
                .provenance = provenance.clone();
            stale
                .note_areas
                .get_mut(&source.address)
                .unwrap()
                .provenance = provenance;
            assert!(
                stale
                    .physical_exclusions_for(&catalog, &snapshot, 0)
                    .is_empty()
            );
            assert_eq!(
                stale.physical_reservation_for(&catalog, &snapshot, 0),
                Length::ZERO
            );
            assert!(!plan.equivalent(&stale));
        }
    }
}
