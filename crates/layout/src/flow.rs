//! Pass 1: flow. Paragraphs stack down the main column; annotations are laid
//! out but left for the relation pass to place.

use reprise_compose::{ComposeRequest, Measure};
use reprise_doc::{BlockKind, Document, NodeId};
use reprise_geom::{FrameSpace, Length, PageSpace, Point, Rect, Transform};
use reprise_shape::ShapeRequest;

use crate::{BlockLayout, Diagnostic, Engine, LayoutSnapshot, LineLayout};

pub(crate) struct Laid {
    pub block: BlockLayout,
    pub height: Length,
}

/// Lays out every block, places the paragraphs and returns the annotations.
pub(crate) fn run(engine: &Engine, doc: &Document, snapshot: &mut LayoutSnapshot) -> Vec<Laid> {
    let page = engine.page;
    let mut y = page.margin_top;
    let mut annotations = Vec::new();
    for node in doc.blocks() {
        let Some(kind) = doc.block(node).ok().map(|b| b.kind) else {
            continue;
        };
        let width = match kind {
            BlockKind::Paragraph => page.column_width,
            BlockKind::Annotation => page.margin_column_width,
        };
        let Some(laid) = lay_block(engine, doc, node, width, &mut snapshot.diagnostics) else {
            continue;
        };
        match kind {
            BlockKind::Paragraph => {
                let origin = Point::new(page.margin_left, y);
                y += laid.height + page.paragraph_spacing;
                snapshot.blocks.push(place(laid, origin));
            }
            BlockKind::Annotation => annotations.push(laid),
        }
    }
    annotations
}

/// Shapes and composes one block at the frame origin.
fn lay_block(
    engine: &Engine,
    doc: &Document,
    node: NodeId,
    width: Length,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Laid> {
    let block = doc.block(node).ok()?;
    let style = match doc.computed_style(node) {
        Ok(s) => s,
        Err(e) => {
            diagnostics.push(Diagnostic {
                node: Some(node),
                message: e.to_string(),
            });
            return None;
        }
    };
    let Some(face) = engine.fonts.by_family(&style.family) else {
        diagnostics.push(Diagnostic {
            node: Some(node),
            message: format!("no face for family {:?}; block not laid out", style.family),
        });
        return None;
    };
    let text = block.text.to_string();
    let shaped = engine.shaper.shape(&ShapeRequest {
        text: &text,
        face,
        size: style.size,
        direction: Default::default(),
    });
    let composition = engine.composer.compose(&ComposeRequest {
        text: &text,
        shaped: &shaped,
        line_height: style.line_height,
        geometry: &Measure(width),
    });
    diagnostics.extend(
        composition
            .diagnostics
            .into_iter()
            .map(|message| Diagnostic {
                node: Some(node),
                message,
            }),
    );

    // Half-leading: the glyphs' ascent and descent are centred in the line box.
    let m = face.metrics();
    let ascent = Length::from_font_units(m.ascent, style.size, m.units_per_em);
    let descent = Length::from_font_units(m.descent, style.size, m.units_per_em);
    let half_leading = (style.line_height - ascent - descent).mul_ratio(1, 2);

    let lines: Vec<LineLayout> = composition
        .lines
        .iter()
        .map(|l| {
            let top = l.block_offset;
            LineLayout {
                text: l.text.clone(),
                preview: text[l.text.clone()].trim_end().to_string(),
                rect: Rect::new(
                    Point::new(l.available.start, top),
                    l.available.width(),
                    style.line_height,
                ),
                baseline: top + half_leading + ascent,
                width: l.width,
                break_reason: l.break_reason,
                glyphs: shaped.glyphs[l.glyphs.clone()].to_vec(),
            }
        })
        .collect();
    let height = style.line_height.mul_ratio(lines.len() as i32, 1);
    Some(Laid {
        block: BlockLayout {
            node,
            kind: block.kind,
            face: face.id().clone(),
            style,
            frame: Rect::new(Point::origin(), width, height),
            lines,
        },
        height,
    })
}

/// Moves a block laid out at the frame origin to `origin` on the page.
pub(crate) fn place(laid: Laid, origin: Point<PageSpace>) -> BlockLayout {
    let to_page: Transform<FrameSpace, PageSpace> = Transform::translate(origin.x, origin.y);
    let mut block = laid.block;
    let frame_point = |p: Point<PageSpace>| to_page.apply(Point::<FrameSpace>::new(p.x, p.y));
    block.frame.origin = origin;
    for line in &mut block.lines {
        line.rect.origin = frame_point(line.rect.origin);
        line.baseline += origin.y;
    }
    block
}
