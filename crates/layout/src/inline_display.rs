//! Inline paint uses existing image and glyph display operations (32, 33).
use std::borrow::Cow;

use reprise_display::{Color, Glyph, GlyphRun, Item, Layer};
use reprise_doc::WritingMode;
use reprise_geom::{Point, Rect, Transform};

use crate::{AtomLayout, AtomPaint, LineLayout, PositionedRun};

/// Split advance-only atom cells out of text paint without changing navigation
/// runs. Linear in the glyph count apart from the sorted cluster boundaries.
pub(crate) fn text_runs(line: &LineLayout) -> Cow<'_, [PositionedRun]> {
    if line.atoms.is_empty() {
        return Cow::Borrowed(&line.runs);
    }
    let mut out = Vec::new();
    for run in &line.runs {
        let mut boundaries: Vec<_> = run.glyphs.iter().map(|g| g.cluster as usize).collect();
        boundaries.push(run.range.end);
        boundaries.sort_unstable();
        boundaries.dedup();
        let mut pen = run.x;
        let mut part = Vec::new();
        let mut start = pen;
        let flush = |part: &mut Vec<reprise_shape::ShapedGlyph>,
                     start,
                     pen,
                     out: &mut Vec<PositionedRun>| {
            let Some(first) = part.iter().map(|g| g.cluster as usize).min() else {
                return;
            };
            let last = part
                .iter()
                .map(|g| g.cluster as usize)
                .max()
                .unwrap_or(first);
            let end = boundaries
                .get(boundaries.partition_point(|b| *b <= last))
                .copied()
                .unwrap_or(run.range.end);
            out.push(PositionedRun {
                range: first..end,
                x: start,
                width: pen - start,
                glyphs: std::mem::take(part),
                ..run.clone()
            });
        };
        let mut atom_cluster = None;
        for glyph in &run.glyphs {
            let leftover =
                glyph.id == reprise_compose::INVISIBLE_GLYPH && atom_cluster == Some(glyph.cluster);
            if glyph.id == reprise_compose::ATOM_GLYPH {
                flush(&mut part, start, pen, &mut out);
                atom_cluster = Some(glyph.cluster);
                pen += glyph.advance;
                start = pen;
            } else if leftover {
                // Another glyph the font made for the object's character.
                pen += glyph.advance;
                start = pen;
            } else {
                part.push(*glyph);
                pen += glyph.advance;
            }
        }
        flush(&mut part, start, pen, &mut out);
    }
    Cow::Owned(out)
}

pub(crate) fn atom_item(atom: &AtomLayout, mode: WritingMode) -> Option<Item> {
    let rect = Rect::new(
        Point::new(atom.rect.origin.x, atom.rect.origin.y),
        atom.rect.width,
        atom.rect.height,
    );
    // Cancel the frame's writing-mode turn/reflection for upright images.
    let upright = super::display::glyph_transform(mode, true);
    match &atom.paint {
        AtomPaint::None => None,
        AtomPaint::Image { asset, alt, .. } => {
            let image = |rect| Item::Image {
                asset: asset.clone(),
                rect,
                alt: alt.clone(),
                layer: Layer::Content,
            };
            Some(
                match upright.and_then(|m| m.inverse().map(|inverse| (m, inverse))) {
                    Some((transform, inverse)) => Item::Group {
                        transform,
                        clip: None,
                        items: vec![image(Transform::new(inverse).bounds(&rect))],
                    },
                    None => image(rect),
                },
            )
        }
        AtomPaint::Text {
            text,
            face,
            size,
            glyphs,
            ..
        } => {
            let mut boundaries: Vec<_> = glyphs.iter().map(|g| g.cluster).collect();
            boundaries.push(u32::try_from(text.len()).unwrap_or(u32::MAX));
            boundaries.sort_unstable();
            boundaries.dedup();
            let mut pen = atom.rect.origin.x;
            let glyphs = glyphs
                .iter()
                .map(|g| {
                    let end = boundaries
                        .get(boundaries.partition_point(|b| *b <= g.cluster))
                        .copied()
                        .unwrap_or(g.cluster);
                    let glyph = Glyph {
                        id: g.id,
                        x: pen + g.x_offset,
                        y: atom.baseline - g.y_offset,
                        text: g.cluster..end,
                    };
                    pen += g.advance;
                    glyph
                })
                .collect();
            let mut run = GlyphRun {
                synthetic_oblique: false,
                face: face.clone(),
                size: *size,
                color: Color(atom.color[0], atom.color[1], atom.color[2], atom.color[3]),
                text: text.clone(),
                glyphs,
                layer: Layer::Content,
            };
            // Text is drawn as the surrounding text is: sideways text in a
            // vertical frame takes the frame's turn (right to left) or its
            // reflection (left to right) with it, and ordinary runs undo
            // what their glyph positions already include.
            match super::display::glyph_transform(mode, false)
                .and_then(|m| m.inverse().map(|inverse| (m, inverse)))
            {
                Some((transform, inverse)) => {
                    for glyph in &mut run.glyphs {
                        let (x, y) = inverse.apply(glyph.x, glyph.y);
                        glyph.x = x;
                        glyph.y = y;
                    }
                    Some(Item::Group {
                        transform,
                        clip: None,
                        items: vec![Item::Glyphs(run)],
                    })
                }
                None => Some(Item::Glyphs(run)),
            }
        }
    }
}
