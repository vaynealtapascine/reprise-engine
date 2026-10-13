//! The factory's existing-page main-frame append path for continuous bands.
use super::*;
use reprise_geom::{Length, Point};

impl PageFactory {
    pub(crate) fn append_band(
        snapshot: &mut LayoutSnapshot,
        section: &crate::sections::Section,
        owner: &crate::sections::Section,
        page: usize,
        offset: Length,
        depth: Length,
    ) -> Option<ResolvedTemplate> {
        if depth <= Length::ZERO {
            return None;
        }
        let count = snapshot.sections.as_ref()?.pages.get(page)?.bands.len();
        if count >= crate::flow::bands::MAX_BANDS {
            return None;
        }
        let d = section.geometry.dimensions;
        let o = owner.geometry.dimensions;
        let mode = owner.geometry.writing_mode;
        let (dx, dy) = match mode {
            WritingMode::HorizontalTb => (Length::ZERO, o.top + offset - d.top),
            WritingMode::VerticalRl => (
                o.width - o.right - offset - (d.width - d.right),
                Length::ZERO,
            ),
            WritingMode::VerticalLr => (o.left + offset - d.left, Length::ZERO),
        };
        let body = match mode {
            WritingMode::HorizontalTb => Rect::new(
                Point::new(d.left, o.top + offset),
                section.geometry.inline_extent(),
                depth,
            ),
            WritingMode::VerticalRl => Rect::new(
                Point::new(o.width - o.right - offset - depth, d.top),
                depth,
                section.geometry.inline_extent(),
            ),
            WritingMode::VerticalLr => Rect::new(
                Point::new(o.left + offset, d.top),
                depth,
                section.geometry.inline_extent(),
            ),
        };
        let mut template = (*section.template).clone();
        let mut frames = Vec::new();
        for frame in &mut template.frames {
            if frame.is_main_flow() {
                frame.x += dx;
                frame.y += dy;
                frame.depth = depth;
                frames.push(frame.layout(page));
            }
        }
        if frames.is_empty() {
            return None;
        }
        let start = snapshot.frames.len();
        let indices = (start..start.saturating_add(frames.len())).collect();
        snapshot.frames.extend(frames);
        let metadata = snapshot.sections.as_mut()?;
        metadata.pages.get_mut(page)?.bands.push(crate::Band {
            section: section.id(),
            life: section.life().into(),
            frames: indices,
            body,
            offset,
            depth,
            balanced: false,
        });
        metadata
            .rules
            .extend(section.rules.iter().copied().map(|mut rect| {
                rect.origin.x += dx;
                rect.origin.y += dy;
                crate::ColumnRule {
                    page,
                    band: count,
                    rect,
                }
            }));
        Self::clip_rules(snapshot, page, count, depth);
        Some(template)
    }

    pub(crate) fn clip_rules(
        snapshot: &mut LayoutSnapshot,
        page: usize,
        band: usize,
        depth: Length,
    ) {
        let mode = snapshot
            .sections
            .as_ref()
            .and_then(|s| s.pages.get(page))
            .and_then(|p| p.bands.get(band))
            .and_then(|b| b.frames.first())
            .and_then(|i| snapshot.frames.get(*i))
            .map_or(WritingMode::HorizontalTb, |f| f.writing_mode);
        if let Some(metadata) = &mut snapshot.sections {
            if let Some(b) = metadata
                .pages
                .get_mut(page)
                .and_then(|p| p.bands.get_mut(band))
            {
                match mode {
                    WritingMode::HorizontalTb => b.body.height = depth,
                    WritingMode::VerticalRl => {
                        b.body.origin.x += b.body.width - depth;
                        b.body.width = depth;
                    }
                    WritingMode::VerticalLr => b.body.width = depth,
                }
            }
            for rule in metadata
                .rules
                .iter_mut()
                .filter(|r| r.page == page && r.band == band)
            {
                match mode {
                    WritingMode::HorizontalTb => rule.rect.height = depth,
                    WritingMode::VerticalRl => {
                        rule.rect.origin.x += rule.rect.width - depth;
                        rule.rect.width = depth;
                    }
                    WritingMode::VerticalLr => rule.rect.width = depth,
                }
            }
        }
    }

    /// A physical derived region is independent of a main band's trial depth.
    /// This append still goes through the sole snapshot allocator.
    pub(crate) fn append_region(snapshot: &mut LayoutSnapshot, frame: FrameLayout) -> usize {
        let index = snapshot.frames.len();
        snapshot.frames.push(frame);
        index
    }

    pub(crate) fn float_sources(
        snapshot: &mut LayoutSnapshot,
        page: usize,
        band: usize,
        full_depth: Length,
    ) {
        let Some(body) = snapshot
            .sections
            .as_ref()
            .and_then(|s| s.pages.get(page))
            .and_then(|p| p.bands.get(band))
            .cloned()
        else {
            return;
        };
        for (column, index) in body.frames.iter().enumerate() {
            let Some(mut frame) = snapshot.frame(*index).cloned() else {
                continue;
            };
            frame.name = format!("reprise.float.{}.{}", body.section, column);
            frame.role = FrameRole::Flow(frame.name.clone());
            frame.rect.height = full_depth;
            Self::append_region(snapshot, frame);
        }
    }
}
