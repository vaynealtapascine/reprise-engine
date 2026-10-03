//! From a layout snapshot to a display list, with the optional debug overlay (39).

use reprise_display::{Color, DisplayList, Glyph, Item, Layer};
use reprise_geom::Point;

use crate::LayoutSnapshot;

/// Display list options.
#[derive(Clone, Copy, Debug, Default)]
pub struct DisplayOptions {
    /// Draw frames, line boxes, baselines and relation links (39).
    pub debug: bool,
}

const FRAME: Color = Color(40, 110, 220, 160);
const LINE_BOX: Color = Color(40, 110, 220, 60);
const BASELINE: Color = Color(220, 60, 60, 110);
const LINK: Color = Color(30, 160, 90, 200);

impl LayoutSnapshot {
    pub fn to_display_list(&self, options: DisplayOptions) -> DisplayList {
        let mut items = Vec::new();
        for block in &self.blocks {
            if options.debug {
                items.push(Item::Rect {
                    rect: block.frame,
                    fill: None,
                    stroke: Some(FRAME),
                    layer: Layer::Debug,
                });
            }
            for line in &block.lines {
                if options.debug {
                    items.push(Item::Rect {
                        rect: line.rect,
                        fill: None,
                        stroke: Some(LINE_BOX),
                        layer: Layer::Debug,
                    });
                    items.push(Item::Line {
                        from: Point::new(line.rect.origin.x, line.baseline),
                        to: Point::new(line.rect.origin.x + line.rect.width, line.baseline),
                        color: BASELINE,
                        layer: Layer::Debug,
                    });
                }
                let mut x = line.rect.origin.x;
                let glyphs = line
                    .glyphs
                    .iter()
                    .map(|g| {
                        let glyph = Glyph {
                            id: g.id,
                            x: x + g.x_offset,
                            y: line.baseline - g.y_offset,
                        };
                        x += g.advance;
                        glyph
                    })
                    .collect();
                items.push(Item::Glyphs {
                    face: block.face.clone(),
                    size: block.style.size,
                    color: Color::BLACK,
                    glyphs,
                });
            }
        }
        if options.debug {
            for rel in &self.relations {
                let (Some((node, line)), Some(source)) = (rel.target_line, self.block(rel.source))
                else {
                    continue;
                };
                let Some(target) = self.block(node).map(|b| &b.lines[line]) else {
                    continue;
                };
                items.push(Item::Line {
                    from: Point::new(target.rect.origin.x + target.width, target.baseline),
                    to: Point::new(
                        source.frame.origin.x,
                        source
                            .lines
                            .first()
                            .map_or(source.frame.origin.y, |l| l.baseline),
                    ),
                    color: LINK,
                    layer: Layer::Debug,
                });
            }
        }
        DisplayList {
            width: self.page.width,
            height: self.page.height,
            items,
        }
    }
}
