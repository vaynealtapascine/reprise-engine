//! From a layout snapshot to display lists, with the optional debug overlay (39).
//!
//! Each frame becomes a group whose transform is the frame's transform to its
//! page, so its lines and glyphs are drawn in frame space.

use reprise_display::{Color, DisplayList, Glyph, GlyphRun, Item, Layer, Path, Stroke};
use reprise_geom::{FrameSpace, Length, Point, Rect};

use crate::{LayoutSnapshot, LineLayout, PositionedRun};

/// Display list options.
#[derive(Clone, Copy, Debug, Default)]
pub struct DisplayOptions {
    /// Draw frames, line boxes, baselines and relation links (39).
    pub debug: bool,
}

const FRAME: Color = Color(40, 110, 220, 160);
const BLOCK: Color = Color(40, 110, 220, 100);
const LINE_BOX: Color = Color(40, 110, 220, 60);
const BASELINE: Color = Color(220, 60, 60, 110);
const LINK: Color = Color(30, 160, 90, 200);

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

impl LayoutSnapshot {
    /// One display list per page.
    pub fn to_display_lists(&self, options: DisplayOptions) -> Vec<DisplayList> {
        (0..self.pages.len())
            .map(|page| self.to_display_list(page, options))
            .collect()
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
            if options.debug {
                children.push(debug_path(Path::rect(r(frame.rect)), FRAME));
            }
            for block in &self.blocks {
                let lines: Vec<&LineLayout> =
                    block.lines.iter().filter(|l| l.frame == index).collect();
                if options.debug
                    && let Some(bounds) = lines.iter().map(|l| l.rect).reduce(|a, b| a.union(&b))
                {
                    children.push(debug_path(Path::rect(r(bounds)), BLOCK));
                }
                for line in lines {
                    if options.debug {
                        children.push(debug_path(Path::rect(r(line.rect)), LINE_BOX));
                        children.push(debug_path(
                            Path::line(
                                Point::new(line.rect.origin.x, line.baseline),
                                Point::new(line.rect.max_x(), line.baseline),
                            ),
                            BASELINE,
                        ));
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
        if options.debug {
            items.extend(self.relation_links(page));
        }
        DisplayList {
            width: size.width,
            height: size.height,
            items,
        }
    }

    /// Lines from each followed line's end to the block it placed, on the page.
    fn relation_links(&self, page: usize) -> Vec<Item> {
        let mut out = Vec::new();
        for rel in self.relations.iter().filter(|r| r.applied) {
            let Some(owner) = rel.owner.and_then(|o| self.block(o)) else {
                continue;
            };
            let target = rel.targets.iter().find_map(|t| match t.resolved {
                Some(crate::Resolution::Line(line)) => Some(line),
                _ => None,
            });
            let (Some(target), Some(first)) = (target, owner.lines.first()) else {
                continue;
            };
            let (Some(from_line), Some(to_frame)) = (self.line(target), self.frame(first.frame))
            else {
                continue;
            };
            let Some(from_frame) = self.frame(from_line.frame) else {
                continue;
            };
            if from_frame.page != page || to_frame.page != page {
                continue;
            }
            let from = from_frame.to_page.apply(Point::new(
                from_line.rect.origin.x + from_line.width,
                from_line.baseline,
            ));
            let to = to_frame
                .to_page
                .apply(Point::new(first.rect.origin.x, first.baseline));
            out.push(debug_path(Path::line(from, to), LINK));
        }
        out
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
