//! PNG backend. Glyphs are filled from their outlines, like the SVG backend,
//! so a render depends on nothing but the display list and the font bytes.

use reprise_font::{FontStore, PathCmd};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::{Color, DisplayList, Item, RenderError};

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.0, c.1, c.2, c.3);
    p.anti_alias = true;
    p
}

/// Outline in font design units, y up, as a tiny-skia path.
fn outline_path(cmds: &[PathCmd]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for c in cmds {
        match *c {
            PathCmd::Move(x, y) => pb.move_to(x, y),
            PathCmd::Line(x, y) => pb.line_to(x, y),
            PathCmd::Quad(a, b, x, y) => pb.quad_to(a, b, x, y),
            PathCmd::Cubic(a, b, c, d, x, y) => pb.cubic_to(a, b, c, d, x, y),
            PathCmd::Close => pb.close(),
        }
    }
    pb.finish()
}

/// Renders the page at `pixels_per_pt` on a white background.
pub fn render(
    list: &DisplayList,
    fonts: &FontStore,
    pixels_per_pt: f32,
) -> Result<Vec<u8>, RenderError> {
    let w = (list.width.to_pt_f32() * pixels_per_pt).ceil();
    let h = (list.height.to_pt_f32() * pixels_per_pt).ceil();
    let bad = || RenderError::BadSize(w, h);
    if !(w >= 1.0 && h >= 1.0) {
        return Err(bad());
    }
    let mut pixmap = Pixmap::new(w as u32, h as u32).ok_or_else(bad)?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let page = Transform::from_scale(pixels_per_pt, pixels_per_pt);

    for item in &list.items {
        match item {
            Item::Glyphs {
                face,
                size,
                color,
                glyphs,
            } => {
                let font = fonts.get(face)?;
                let scale = size.to_pt_f32() / font.metrics().units_per_em as f32;
                let paint = paint(*color);
                for g in glyphs {
                    let Some(path) = outline_path(&font.outline(g.id)) else {
                        continue;
                    };
                    let at = page
                        .pre_translate(g.x.to_pt_f32(), g.y.to_pt_f32())
                        .pre_scale(scale, -scale);
                    pixmap.fill_path(&path, &paint, FillRule::Winding, at, None);
                }
            }
            Item::Rect {
                rect, fill, stroke, ..
            } => {
                let Some(r) = Rect::from_xywh(
                    rect.origin.x.to_pt_f32(),
                    rect.origin.y.to_pt_f32(),
                    rect.width.to_pt_f32(),
                    rect.height.to_pt_f32(),
                ) else {
                    continue;
                };
                let path = PathBuilder::from_rect(r);
                if let Some(c) = fill {
                    pixmap.fill_path(&path, &paint(*c), FillRule::Winding, page, None);
                }
                if let Some(c) = stroke {
                    let s = Stroke {
                        width: 0.25,
                        ..Stroke::default()
                    };
                    pixmap.stroke_path(&path, &paint(*c), &s, page, None);
                }
            }
            Item::Line {
                from, to, color, ..
            } => {
                let mut pb = PathBuilder::new();
                pb.move_to(from.x.to_pt_f32(), from.y.to_pt_f32());
                pb.line_to(to.x.to_pt_f32(), to.y.to_pt_f32());
                if let Some(path) = pb.finish() {
                    let s = Stroke {
                        width: 0.5,
                        ..Stroke::default()
                    };
                    pixmap.stroke_path(&path, &paint(*color), &s, page, None);
                }
            }
        }
    }

    pixmap
        .encode_png()
        .map_err(|e| RenderError::Encode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use reprise_font::Face;
    use reprise_geom::{Length, Point, Rect};
    use skrifa::MetadataProvider;

    use super::*;
    use crate::{Glyph, Layer};

    fn sample() -> (DisplayList, FontStore) {
        let face = Face::from_bytes(
            include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
        )
        .expect("fixture font loads");
        let id_a = face
            .font_ref()
            .charmap()
            .map('a')
            .expect("a is mapped")
            .to_u32();
        let mut fonts = FontStore::default();
        let face_id = fonts.add(face);
        let list = DisplayList {
            width: Length::from_pt(100),
            height: Length::from_pt(50),
            items: vec![
                Item::Glyphs {
                    face: face_id,
                    size: Length::from_pt(40),
                    color: Color::BLACK,
                    glyphs: vec![Glyph {
                        id: id_a,
                        x: Length::from_pt(10),
                        y: Length::from_pt(40),
                    }],
                },
                Item::Rect {
                    rect: Rect::new(
                        Point::new(Length::from_pt(60), Length::from_pt(5)),
                        Length::from_pt(30),
                        Length::from_pt(10),
                    ),
                    fill: Some(Color(255, 0, 0, 255)),
                    stroke: Some(Color::BLACK),
                    layer: Layer::Content,
                },
                Item::Line {
                    from: Point::new(Length::from_pt(0), Length::from_pt(48)),
                    to: Point::new(Length::from_pt(100), Length::from_pt(48)),
                    color: Color::BLACK,
                    layer: Layer::Content,
                },
            ],
        };
        (list, fonts)
    }

    #[test]
    fn renders_a_decodable_png_with_ink() {
        let (list, fonts) = sample();
        let bytes = render(&list, &fonts, 2.0).expect("renders");
        let img = Pixmap::decode_png(&bytes).expect("decodes");
        assert_eq!((img.width(), img.height()), (200, 100));
        let dark = |x: u32, y: u32| img.pixel(x, y).is_some_and(|p| p.red() < 128);
        let glyph_ink = (20..60).any(|x| (40..80).any(|y| dark(x, y)));
        assert!(glyph_ink, "glyph area has dark pixels");
        let red = img.pixel(150, 20).expect("in bounds");
        assert_eq!((red.red(), red.green(), red.blue()), (255, 0, 0));
        assert!(dark(100, 96), "line is drawn");
        let corner = img.pixel(0, 0).expect("in bounds");
        assert_eq!(
            (corner.red(), corner.alpha()),
            (255, 255),
            "background is white"
        );
    }

    #[test]
    fn rendering_is_deterministic() {
        let (list, fonts) = sample();
        assert_eq!(
            render(&list, &fonts, 2.0).expect("renders"),
            render(&list, &fonts, 2.0).expect("renders")
        );
    }

    #[test]
    fn empty_pages_are_rejected() {
        let (mut list, fonts) = sample();
        list.width = Length::from_pt(0);
        assert!(matches!(
            render(&list, &fonts, 1.0),
            Err(RenderError::BadSize(..))
        ));
    }
}
