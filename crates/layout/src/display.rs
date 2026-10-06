//! From a layout snapshot to display lists, with the optional debug overlay (39).
//!
//! Each frame becomes a group whose transform is the frame's transform to its
//! page, so its lines and glyphs are drawn in frame space.

use reprise_compose::BreakReason;
use reprise_diag::Severity;
use reprise_display::{Color, DisplayList, Glyph, GlyphRun, Item, Layer, Path, Stroke};
use reprise_geom::{FrameSpace, Length, Point, Rect};

use crate::{LayoutSnapshot, LineLayout, PositionedRun, RelationStatus, Resolution, Subject};

/// Display list options.
#[derive(Clone, Copy, Debug)]
pub struct DisplayOptions {
    /// Master switch. Defaults to false; the enabled overlay is a superset
    /// of the original frames, block/line boxes, baselines and relation links.
    pub debug: bool,
    /// Frame, block and line boxes.
    pub boxes: bool,
    /// Baseline across each available line interval.
    pub baselines: bool,
    /// Available interval (cyan) and used width (blue), one point apart.
    pub intervals: bool,
    /// Purple boxes around positioned runs.
    pub run_boundaries: bool,
    /// Wrap hook, forced-break arrow, end square or overflow cross.
    pub break_reasons: bool,
    /// Magenta outlines on lines whose boundaries required reshaping.
    pub reshaped_lines: bool,
    /// Green valid, blue rebound, amber ambiguous, red missing links.
    pub relations: bool,
    /// Blue info, amber warning and red error markers over source clusters.
    pub diagnostics: bool,
}

impl Default for DisplayOptions {
    fn default() -> Self {
        Self {
            debug: false,
            boxes: true,
            baselines: true,
            intervals: true,
            run_boundaries: true,
            break_reasons: true,
            reshaped_lines: true,
            relations: true,
            diagnostics: true,
        }
    }
}

const FRAME: Color = Color(40, 110, 220, 160);
const BLOCK: Color = Color(40, 110, 220, 100);
const LINE_BOX: Color = Color(40, 110, 220, 60);
const BASELINE: Color = Color(220, 60, 60, 110);
const LINK: Color = Color(30, 160, 90, 200);
const AVAILABLE: Color = Color(30, 150, 190, 190);
const USED: Color = Color(20, 90, 160, 230);
const RUN: Color = Color(150, 70, 190, 160);
const RESHAPED: Color = Color(210, 50, 160, 220);

fn severity_color(severity: Severity) -> Color {
    match severity {
        Severity::Info => Color(30, 130, 200, 220),
        Severity::Warning => Color(210, 130, 10, 230),
        Severity::Error => Color(220, 40, 40, 230),
    }
}

fn status_color(status: RelationStatus) -> Color {
    match status {
        RelationStatus::Valid => LINK,
        RelationStatus::Rebound => Color(30, 130, 210, 220),
        RelationStatus::Ambiguous => severity_color(Severity::Warning),
        RelationStatus::Missing | RelationStatus::OwnerDeleted => severity_color(Severity::Error),
        // Out of effect by the schema's own policy: not a problem, so grey.
        RelationStatus::Deleted => Color(130, 130, 130, 200),
    }
}

fn hairline(color: Color) -> Option<Stroke> {
    Some(Stroke {
        color,
        width: Length(256),
    })
}

fn debug_path(path: Path, color: Color) -> Item {
    Item::Path {
        path,
        fill: None,
        stroke: hairline(color),
        layer: Layer::Debug,
    }
}

/// Frame-space geometry as display-list coordinates inside the frame's group.
fn p(point: Point<FrameSpace>) -> Point<reprise_geom::PageSpace> {
    Point::new(point.x, point.y)
}

fn r(rect: Rect<FrameSpace>) -> Rect<reprise_geom::PageSpace> {
    Rect::new(p(rect.origin), rect.width, rect.height)
}

/// One glyph run or image of the default display lists, addressed for the PDF.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfReadingRun {
    pub run: reprise_display::pdf::ReadingRun,
    /// The line the run sits on.
    pub line: crate::LineRef,
    /// The bytes of the block's text the run draws; None for an image.
    pub bytes: Option<std::ops::Range<usize>>,
}

/// The runs of one block in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfReadingBlock {
    pub node: reprise_doc::NodeId,
    pub runs: Vec<PdfReadingRun>,
}

impl LayoutSnapshot {
    /// One display list per page.
    pub fn to_display_lists(&self, options: DisplayOptions) -> Vec<DisplayList> {
        (0..self.pages.len())
            .map(|page| self.to_display_list(page, options))
            .collect()
    }

    /// Run addresses for `to_display_lists(DisplayOptions::default())`, in
    /// semantic/overridden reading order. RTL runs sort by source bytes while
    /// glyphs retain visual positions. Pass these to `pdf::render_ordered`.
    pub fn pdf_reading_order(
        &self,
        doc: &reprise_doc::Document,
    ) -> Vec<reprise_display::pdf::ReadingRun> {
        self.pdf_reading_blocks(doc)
            .into_iter()
            .flat_map(|block| block.runs.into_iter().map(|r| r.run))
            .collect()
    }

    /// The same runs as [`Self::pdf_reading_order`], grouped by the block they
    /// belong to, with the source bytes each draws. A read-only view for
    /// exporters that build a structure tree over the reading order.
    pub fn pdf_reading_blocks(&self, doc: &reprise_doc::Document) -> Vec<PdfReadingBlock> {
        let mut addresses = self.pdf_addresses().0;
        let mut blocks: Vec<PdfReadingBlock> = Vec::new();
        for step in self.reading_order(doc) {
            let mut runs = addresses.remove(&step.line).unwrap_or_default();
            runs.sort_by_key(|r| r.bytes.as_ref().map_or(0, |b| b.start));
            match blocks.last_mut() {
                Some(last) if last.node == step.line.node => last.runs.extend(runs),
                _ => blocks.push(PdfReadingBlock {
                    node: step.line.node,
                    runs,
                }),
            }
        }
        blocks
    }

    /// The glyph runs and images of repeated table headers in the default display
    /// lists. They are derived copies, not content: a PDF marks them as artifacts,
    /// and no reading order names them.
    pub fn pdf_artifact_runs(&self) -> Vec<reprise_display::pdf::ReadingRun> {
        self.pdf_addresses().1
    }

    /// Addresses of the authored runs by line, and of the repeated header copies.
    fn pdf_addresses(
        &self,
    ) -> (
        std::collections::BTreeMap<crate::LineRef, Vec<PdfReadingRun>>,
        Vec<reprise_display::pdf::ReadingRun>,
    ) {
        use std::collections::BTreeMap;
        let mut addresses = BTreeMap::<crate::LineRef, Vec<PdfReadingRun>>::new();
        let mut copies = Vec::new();
        let mut groups = vec![0usize; self.pages.len()];
        for (frame_index, frame) in self.frames.iter().enumerate() {
            let Some(group) = groups.get_mut(frame.page) else {
                continue;
            };
            let mut child = 0usize;
            for block in &self.blocks {
                for (line_index, line) in block
                    .lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.frame == frame_index)
                {
                    let at = crate::LineRef {
                        node: block.node,
                        line: line_index,
                    };
                    if block.image.is_some() {
                        addresses.entry(at).or_default().push(PdfReadingRun {
                            run: reprise_display::pdf::ReadingRun {
                                page: frame.page,
                                path: vec![*group, child],
                            },
                            line: at,
                            bytes: None,
                        });
                        child = child.saturating_add(1);
                    }
                    for run in &line.runs {
                        addresses.entry(at).or_default().push(PdfReadingRun {
                            run: reprise_display::pdf::ReadingRun {
                                page: frame.page,
                                path: vec![*group, child],
                            },
                            line: at,
                            bytes: Some(run.range.clone()),
                        });
                        child = child.saturating_add(1);
                    }
                }
            }
            // Repeated header copies follow the authored items of the group.
            for block in self
                .repeated_headers
                .iter()
                .filter(|h| h.frame == frame_index)
                .flat_map(|h| h.blocks.iter())
            {
                for line in block.lines.iter().filter(|l| l.frame == frame_index) {
                    let items = usize::from(block.image.is_some()) + line.runs.len();
                    for _ in 0..items {
                        copies.push(reprise_display::pdf::ReadingRun {
                            page: frame.page,
                            path: vec![*group, child],
                        });
                        child = child.saturating_add(1);
                    }
                }
            }
            *group = group.saturating_add(1);
        }
        (addresses, copies)
    }
    /// The display list of one page; empty if there is no such page.
    pub fn to_display_list(&self, page: usize, options: DisplayOptions) -> DisplayList {
        let Some(size) = self.pages.get(page) else {
            return DisplayList {
                width: Length::ZERO,
                height: Length::ZERO,
                items: Vec::new(),
            };
        };
        let mut items = Vec::new();
        for (index, frame) in self.frames.iter().enumerate() {
            if frame.page != page {
                continue;
            }
            let mut children = Vec::new();
            if options.debug && options.boxes {
                children.push(debug_path(Path::rect(r(frame.rect)), FRAME));
            }
            // Repeated table headers are drawn after the authored blocks, so the
            // indices of authored items in this group do not move.
            let copies = self
                .repeated_headers
                .iter()
                .filter(|h| h.frame == index)
                .flat_map(|h| h.blocks.iter());
            for block in self.blocks.iter().chain(copies) {
                let lines: Vec<&LineLayout> =
                    block.lines.iter().filter(|l| l.frame == index).collect();
                if options.debug
                    && options.boxes
                    && let Some(bounds) = lines.iter().map(|l| l.rect).reduce(|a, b| a.union(&b))
                {
                    children.push(debug_path(Path::rect(r(bounds)), BLOCK));
                }
                for line in lines {
                    if let Some(image) = &block.image {
                        children.push(Item::Image {
                            asset: image.asset.clone(),
                            rect: r(line.rect),
                            alt: image.alt.clone(),
                            layer: Layer::Content,
                        });
                    }
                    if options.debug {
                        if options.boxes {
                            children.push(debug_path(Path::rect(r(line.rect)), LINE_BOX));
                        }
                        if options.baselines {
                            children.push(debug_path(
                                Path::line(
                                    Point::new(line.rect.origin.x, line.baseline),
                                    Point::new(line.rect.max_x(), line.baseline),
                                ),
                                BASELINE,
                            ));
                        }
                        line_overlays(&mut children, line, options);
                        if options.diagnostics {
                            for diagnostic in &self.diagnostics {
                                if diagnostic.subject == Subject::Node(block.node) {
                                    diagnostic_overlay(
                                        &mut children,
                                        line,
                                        &block.text,
                                        diagnostic.bytes.as_ref(),
                                        severity_color(diagnostic.severity),
                                    );
                                }
                            }
                        }
                    }
                    for run in &line.runs {
                        children.push(Item::Glyphs(glyph_run(run, line, &block.text)));
                    }
                }
            }
            items.push(Item::Group {
                transform: *frame.to_page.matrix(),
                clip: None,
                items: children,
            });
        }
        if options.debug && options.relations {
            items.extend(self.relation_links(page));
        }
        DisplayList {
            width: size.width,
            height: size.height,
            items,
        }
    }

    /// Links for every resolved target, even for unapplied relations. Missing
    /// targets get a cross at their placed owner. Unplaced owners have no
    /// geometry in the snapshot and cannot be marked at an invented position.
    fn relation_links(&self, page: usize) -> Vec<Item> {
        let mut out = Vec::new();
        for rel in &self.relations {
            let Some(owner) = rel.owner.and_then(|o| self.block(o)) else {
                continue;
            };
            let Some(first) = owner.lines.first() else {
                continue;
            };
            let Some(to_frame) = self.frame(first.frame) else {
                continue;
            };
            if to_frame.page != page {
                continue;
            }
            let to = to_frame
                .to_page
                .apply(Point::new(first.rect.origin.x, first.baseline));
            if rel.targets.is_empty() {
                out.push(debug_path(cross(to), status_color(rel.status)));
            }
            for target in &rel.targets {
                let color = status_color(target.status);
                let ends = self.link_ends(target.resolved.as_ref());
                let mut drawn = false;
                for (on, from) in ends {
                    if on == page {
                        out.push(debug_path(Path::line(from, to), color));
                        drawn = true;
                    }
                }
                if !drawn {
                    // No geometry on this page: a missing target, a page
                    // target, or one whose endpoints are on other pages.
                    out.push(debug_path(cross(to), color));
                }
            }
        }
        out
    }

    /// Where links to a resolved target start: the end of each line it
    /// names, or a frame's start corner, with the page each is on.
    fn link_ends(
        &self,
        resolved: Option<&Resolution>,
    ) -> Vec<(usize, Point<reprise_geom::PageSpace>)> {
        let line_end = |line: &LineLayout| {
            self.frame(line.frame).map(|frame| {
                (
                    frame.page,
                    frame
                        .to_page
                        .apply(Point::new(line.rect.origin.x + line.width, line.baseline)),
                )
            })
        };
        let first_line = |node| self.block(node).and_then(|b| b.lines.first());
        match resolved {
            Some(Resolution::Line(at)) => self.line(*at).and_then(line_end).into_iter().collect(),
            Some(Resolution::Node(node)) => {
                first_line(*node).and_then(line_end).into_iter().collect()
            }
            Some(Resolution::Range { node, bytes }) => self
                .line_containing(*node, bytes.start)
                .and_then(|at| self.line(at))
                .and_then(line_end)
                .into_iter()
                .collect(),
            Some(Resolution::Nodes(nodes)) => nodes
                .iter()
                .filter_map(|&n| first_line(n).and_then(line_end))
                .collect(),
            Some(Resolution::Lines(lines)) => lines
                .iter()
                .filter_map(|&at| self.line(at).and_then(line_end))
                .collect(),
            Some(Resolution::Frame(index)) => self
                .frame(*index)
                .map(|f| (f.page, f.to_page.apply(f.rect.origin)))
                .into_iter()
                .collect(),
            Some(Resolution::Snapshot(content)) if content.exists_now => first_line(content.node)
                .and_then(line_end)
                .into_iter()
                .collect(),
            Some(Resolution::Snapshot(_) | Resolution::Page(_)) | None => Vec::new(),
        }
    }
}

fn cross(at: Point<reprise_geom::PageSpace>) -> Path {
    let d = Length::from_pt(2);
    Path(vec![
        reprise_display::Segment::Move(Point::new(at.x - d, at.y - d)),
        reprise_display::Segment::Line(Point::new(at.x + d, at.y + d)),
        reprise_display::Segment::Move(Point::new(at.x - d, at.y + d)),
        reprise_display::Segment::Line(Point::new(at.x + d, at.y - d)),
    ])
}

fn line_overlays(out: &mut Vec<Item>, line: &LineLayout, options: DisplayOptions) {
    let x = line.rect.origin.x;
    let end = x + line.width;
    let top = line.rect.origin.y;
    if options.intervals {
        out.push(debug_path(
            Path::line(Point::new(x, top), Point::new(line.rect.max_x(), top)),
            AVAILABLE,
        ));
        let y = top + Length::from_pt(1);
        out.push(debug_path(
            Path::line(Point::new(x, y), Point::new(end, y)),
            USED,
        ));
    }
    if options.run_boundaries {
        for run in &line.runs {
            out.push(debug_path(
                Path::rect(Rect::new(
                    Point::new(run.x, top),
                    run.width,
                    line.rect.height,
                )),
                RUN,
            ));
        }
    }
    if options.reshaped_lines && line.explanation.reshaped {
        out.push(debug_path(Path::rect(r(line.rect)), RESHAPED));
    }
    if options.break_reasons {
        // Symbols sit just beyond used text: wrap=hook, forced=down arrow,
        // end=square, overflow=cross; unknown future reasons=diamond.
        use reprise_display::Segment::{Close, Line, Move};
        let at = Point::new(end + Length::from_pt(4), line.baseline);
        let d = Length::from_pt(2);
        let path = match line.explanation.reason {
            BreakReason::Opportunity => Path(vec![
                Move(Point::new(at.x - d, at.y - d)),
                Line(Point::new(at.x + d, at.y - d)),
                Line(Point::new(at.x + d, at.y + d)),
                Line(Point::new(at.x, at.y + d)),
            ]),
            BreakReason::Forced => Path(vec![
                Move(Point::new(at.x, at.y - d)),
                Line(Point::new(at.x, at.y + d)),
                Move(Point::new(at.x - d, at.y)),
                Line(Point::new(at.x, at.y + d)),
                Line(Point::new(at.x + d, at.y)),
            ]),
            BreakReason::End => Path::rect(Rect::new(Point::new(at.x - d, at.y - d), d + d, d + d)),
            BreakReason::Overflow => cross(at),
            _ => Path(vec![
                Move(Point::new(at.x, at.y - d)),
                Line(Point::new(at.x + d, at.y)),
                Line(Point::new(at.x, at.y + d)),
                Line(Point::new(at.x - d, at.y)),
                Close,
            ]),
        };
        out.push(debug_path(path, USED));
    }
}

/// Mark whole affected clusters, since source bytes inside a ligature or
/// combining cluster have no independent glyph geometry. A point range marks
/// its containing cluster; malformed/outside ranges are ignored.
fn diagnostic_overlay(
    out: &mut Vec<Item>,
    line: &LineLayout,
    text: &str,
    bytes: Option<&std::ops::Range<usize>>,
    color: Color,
) {
    let Some(bytes) = bytes else {
        if line.text.start == 0 {
            out.push(debug_path(
                cross(Point::new(line.rect.origin.x, line.baseline)),
                color,
            ));
        }
        return;
    };
    if bytes.start > bytes.end || text.get(bytes.clone()).is_none() {
        return;
    }
    for run in &line.runs {
        let display = glyph_run(run, line, text);
        for (glyph, shaped) in display.glyphs.iter().zip(&run.glyphs) {
            let start = run.range.start.saturating_add(glyph.text.start as usize);
            let end = run.range.start.saturating_add(glyph.text.end as usize);
            let overlaps = if bytes.is_empty() {
                start <= bytes.start && bytes.start < end
            } else {
                start < bytes.end && bytes.start < end
            };
            if overlaps {
                let width = shaped.advance.max(Length::from_pt(1));
                out.push(debug_path(
                    Path::rect(Rect::new(
                        Point::new(glyph.x, line.rect.origin.y),
                        width,
                        line.rect.height,
                    )),
                    color,
                ));
            }
        }
    }
    if bytes.is_empty() && bytes.start == line.text.end && bytes.start == text.len() {
        out.push(debug_path(
            cross(Point::new(line.rect.origin.x + line.width, line.baseline)),
            color,
        ));
    }
}

/// A run's glyphs at their pen positions, with the source text they draw.
fn glyph_run(run: &PositionedRun, line: &LineLayout, text: &str) -> GlyphRun {
    let source = text.get(run.range.clone()).unwrap_or_default();
    let start = run.range.start as u32;
    // Each glyph draws from its cluster to the next cluster boundary.
    let mut clusters: Vec<u32> = run.glyphs.iter().map(|g| g.cluster).collect();
    clusters.sort_unstable();
    clusters.dedup();
    let end_of = |c: u32| {
        clusters
            .iter()
            .copied()
            .find(|&n| n > c)
            .unwrap_or(run.range.end as u32)
    };
    let mut x = run.x;
    let glyphs = run
        .glyphs
        .iter()
        .map(|g| {
            let glyph = Glyph {
                id: g.id,
                x: x + g.x_offset,
                y: line.baseline - g.y_offset,
                text: g.cluster.saturating_sub(start)..end_of(g.cluster).saturating_sub(start),
            };
            x += g.advance;
            glyph
        })
        .collect();
    GlyphRun {
        face: run.face.clone(),
        size: run.size,
        color: Color::BLACK,
        text: source.to_string(),
        glyphs,
        layer: Layer::Content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> LayoutSnapshot {
        let mut fonts = reprise_font::FontStore::default();
        fonts.add(
            reprise_font::Face::from_bytes(
                include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
            )
            .unwrap(),
        );
        let doc = reprise_doc::Document::new(1).unwrap();
        doc.append_block(
            reprise_doc::BlockKind::Paragraph,
            "",
            "office Z\u{335}\u{322} abc",
        )
        .unwrap();
        crate::Engine::new(fonts).layout(&doc)
    }

    fn only() -> DisplayOptions {
        DisplayOptions {
            debug: true,
            boxes: false,
            baselines: false,
            intervals: false,
            run_boundaries: false,
            break_reasons: false,
            reshaped_lines: false,
            relations: false,
            diagnostics: false,
        }
    }

    #[test]
    fn every_break_reason_has_a_distinct_path_and_reshaping_is_optional() {
        let mut snapshot = snapshot();
        let line = &mut snapshot.blocks[0].lines[0];
        let mut paths = Vec::new();
        for reason in [
            BreakReason::Opportunity,
            BreakReason::Forced,
            BreakReason::End,
            BreakReason::Overflow,
        ] {
            line.explanation.reason = reason;
            let mut items = Vec::new();
            line_overlays(
                &mut items,
                line,
                DisplayOptions {
                    break_reasons: true,
                    ..only()
                },
            );
            assert_eq!(items.len(), 1);
            assert!(!paths.contains(&items[0]));
            paths.push(items[0].clone());
        }
        line.explanation.reshaped = true;
        let mut items = Vec::new();
        line_overlays(&mut items, line, only());
        assert!(items.is_empty());
        line_overlays(
            &mut items,
            line,
            DisplayOptions {
                reshaped_lines: true,
                ..only()
            },
        );
        assert_eq!(items, vec![debug_path(Path::rect(r(line.rect)), RESHAPED)]);
    }

    #[test]
    fn intervals_show_available_and_used_extents_and_run_bounds_toggle() {
        let snapshot = snapshot();
        let line = &snapshot.blocks[0].lines[0];
        let mut items = Vec::new();
        line_overlays(
            &mut items,
            line,
            DisplayOptions {
                intervals: true,
                ..only()
            },
        );
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            debug_path(
                Path::line(
                    p(line.rect.origin),
                    Point::new(line.rect.max_x(), line.rect.origin.y)
                ),
                AVAILABLE
            )
        );
        let y = line.rect.origin.y + Length::from_pt(1);
        assert_eq!(
            items[1],
            debug_path(
                Path::line(
                    Point::new(line.rect.origin.x, y),
                    Point::new(line.rect.origin.x + line.width, y)
                ),
                USED
            )
        );
        items.clear();
        line_overlays(
            &mut items,
            line,
            DisplayOptions {
                run_boundaries: true,
                ..only()
            },
        );
        assert_eq!(items.len(), line.runs.len());
    }

    #[test]
    fn diagnostic_ranges_mark_only_affected_clusters() {
        let snapshot = snapshot();
        let block = &snapshot.blocks[0];
        let line = &block.lines[0];
        let mut items = Vec::new();
        diagnostic_overlay(&mut items, line, &block.text, Some(&(2..3)), Color::BLACK);
        assert_eq!(
            items.len(),
            1,
            "a byte inside the ffi ligature marks its whole glyph"
        );
        diagnostic_overlay(
            &mut items,
            line,
            &block.text,
            Some(&(usize::MAX..usize::MAX)),
            Color::BLACK,
        );
        diagnostic_overlay(
            &mut items,
            line,
            &block.text,
            Some(&std::ops::Range { start: 10, end: 2 }),
            Color::BLACK,
        );
        assert_eq!(items.len(), 1, "malformed ranges are ignored");
        items.clear();
        diagnostic_overlay(&mut items, line, &block.text, Some(&(0..0)), Color::BLACK);
        assert_eq!(items.len(), 1, "point marks the cluster at the caret");
        items.clear();
        diagnostic_overlay(
            &mut items,
            line,
            &block.text,
            Some(&(block.text.len()..block.text.len())),
            Color::BLACK,
        );
        assert_eq!(items.len(), 1, "end caret is a cross");
    }

    #[test]
    fn extreme_negative_and_zero_overlay_geometry_saturates() {
        let mut snapshot = snapshot();
        let line = &mut snapshot.blocks[0].lines[0];
        for extent in [Length(i32::MIN), Length::ZERO, Length(i32::MAX)] {
            line.rect.origin = Point::new(extent, extent);
            line.rect.width = extent;
            line.rect.height = extent;
            line.width = extent;
            line.baseline = extent;
            for run in &mut line.runs {
                run.x = extent;
                run.width = extent;
            }
            let mut items = Vec::new();
            line_overlays(
                &mut items,
                line,
                DisplayOptions {
                    debug: true,
                    ..Default::default()
                },
            );
            diagnostic_overlay(&mut items, line, "", Some(&(0..0)), Color::BLACK);
            assert!(!items.is_empty());
        }
        assert!(
            snapshot
                .to_display_list(usize::MAX, only())
                .items
                .is_empty()
        );
    }
}
