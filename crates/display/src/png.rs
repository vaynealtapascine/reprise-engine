//! PNG backend. Glyphs are filled from their outlines, like the SVG backend,
//! so a render depends on nothing but the display list and the font bytes.

use reprise_font::{FontStore, PathCmd};
use reprise_geom::{Length, Matrix};
use tiny_skia::{FillRule, Mask, Paint, PathBuilder, Pixmap, Transform};

use crate::{Color, DisplayList, GlyphRun, Item, Path, RenderError, Segment};

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.0, c.1, c.2, c.3);
    p.anti_alias = true;
    p
}

/// The most pixels one rendered page may have (64 MiB of RGBA), the same cap
/// the backends apply when decoding images.
const MAX_PIXELS: f64 = 16_777_216.0;

fn pt(l: Length) -> f32 {
    l.to_pt_f32()
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

/// A display-list path in points as a tiny-skia path.
fn skia_path(path: &Path) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for s in &path.0 {
        match *s {
            Segment::Move(p) => pb.move_to(pt(p.x), pt(p.y)),
            Segment::Line(p) => pb.line_to(pt(p.x), pt(p.y)),
            Segment::Quad(c, p) => pb.quad_to(pt(c.x), pt(c.y), pt(p.x), pt(p.y)),
            Segment::Cubic(a, b, p) => {
                pb.cubic_to(pt(a.x), pt(a.y), pt(b.x), pt(b.y), pt(p.x), pt(p.y))
            }
            Segment::Close => pb.close(),
        }
    }
    pb.finish()
}

fn skia_matrix(m: &Matrix) -> Transform {
    Transform::from_row(
        m.xx.to_f32(),
        m.yx.to_f32(),
        m.xy.to_f32(),
        m.yy.to_f32(),
        pt(m.tx),
        pt(m.ty),
    )
}

struct Raster<'a> {
    pixmap: Pixmap,
    fonts: &'a FontStore,
    assets: &'a crate::AssetStore,
}

/// Renders the page at `pixels_per_pt` on a white background.
pub fn render(
    list: &DisplayList,
    fonts: &FontStore,
    pixels_per_pt: f32,
) -> Result<Vec<u8>, RenderError> {
    render_with_assets(list, fonts, &crate::AssetStore::default(), pixels_per_pt)
}

pub fn render_with_assets(
    list: &DisplayList,
    fonts: &FontStore,
    assets: &crate::AssetStore,
    pixels_per_pt: f32,
) -> Result<Vec<u8>, RenderError> {
    let w = (pt(list.width) * pixels_per_pt).ceil();
    let h = (pt(list.height) * pixels_per_pt).ceil();
    let bad = || RenderError::BadSize(w, h);
    // A page the size of the medium can be enormous (a saturated medium is
    // about two million points square). The pixmap is allocated up front, so
    // an unbounded size would abort the process instead of failing the render.
    if !(w >= 1.0 && h >= 1.0 && f64::from(w) * f64::from(h) <= MAX_PIXELS) {
        return Err(bad());
    }
    let mut pixmap = Pixmap::new(w as u32, h as u32).ok_or_else(bad)?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let mut raster = Raster {
        pixmap,
        fonts,
        assets,
    };
    let page = Transform::from_scale(pixels_per_pt, pixels_per_pt);
    raster.items(&list.items, page, None)?;
    raster
        .pixmap
        .encode_png()
        .map_err(|e| RenderError::Encode(e.to_string()))
}

impl Raster<'_> {
    fn items(
        &mut self,
        items: &[Item],
        at: Transform,
        mask: Option<&Mask>,
    ) -> Result<(), RenderError> {
        for item in items {
            match item {
                Item::Glyphs(run) => self.glyphs(run, at, mask)?,
                Item::Image { asset, rect, .. } => {
                    if rect.width <= Length::ZERO || rect.height <= Length::ZERO {
                        continue;
                    }
                    if let Some((w, h, mut rgba)) =
                        self.assets.get(asset).and_then(crate::image_pixels::decode)
                    {
                        for pixel in rgba.as_chunks_mut::<4>().0 {
                            let [r, g, b, a] = pixel;
                            let alpha = u16::from(*a);
                            *r = ((u16::from(*r) * alpha + 127) / 255) as u8;
                            *g = ((u16::from(*g) * alpha + 127) / 255) as u8;
                            *b = ((u16::from(*b) * alpha + 127) / 255) as u8;
                        }
                        if let Some(size) = tiny_skia::IntSize::from_wh(w, h)
                            && let Some(image) = Pixmap::from_vec(rgba, size)
                        {
                            let transform = at
                                .pre_translate(pt(rect.origin.x), pt(rect.origin.y))
                                .pre_scale(pt(rect.width) / w as f32, pt(rect.height) / h as f32);
                            self.pixmap.draw_pixmap(
                                0,
                                0,
                                image.as_ref(),
                                &tiny_skia::PixmapPaint::default(),
                                transform,
                                mask,
                            );
                        }
                    } else if let Some(path) = skia_path(&Path::rect(*rect)) {
                        self.pixmap.fill_path(
                            &path,
                            &paint(Color(221, 221, 221, 255)),
                            FillRule::Winding,
                            at,
                            mask,
                        );
                    }
                }
                Item::Path {
                    path, fill, stroke, ..
                } => {
                    let Some(path) = skia_path(path) else {
                        continue;
                    };
                    if let Some(c) = fill {
                        self.pixmap
                            .fill_path(&path, &paint(*c), FillRule::Winding, at, mask);
                    }
                    if let Some(s) = stroke {
                        let style = tiny_skia::Stroke {
                            width: pt(s.width),
                            ..tiny_skia::Stroke::default()
                        };
                        self.pixmap
                            .stroke_path(&path, &paint(s.color), &style, at, mask);
                    }
                }
                Item::Group {
                    transform,
                    clip,
                    items,
                } => {
                    let inner = at.pre_concat(skia_matrix(transform));
                    // A clip narrows the parent's clip, if there is one.
                    let clipped = match clip.as_ref().and_then(skia_path) {
                        Some(path) => Some(match mask {
                            Some(parent) => {
                                let mut m = parent.clone();
                                m.intersect_path(&path, FillRule::Winding, true, inner);
                                m
                            }
                            None => {
                                let (w, h) = (self.pixmap.width(), self.pixmap.height());
                                let mut m = Mask::new(w, h)
                                    .ok_or(RenderError::BadSize(w as f32, h as f32))?;
                                m.fill_path(&path, FillRule::Winding, true, inner);
                                m
                            }
                        }),
                        None => None,
                    };
                    self.items(items, inner, clipped.as_ref().or(mask))?;
                }
            }
        }
        Ok(())
    }

    fn glyphs(
        &mut self,
        run: &GlyphRun,
        at: Transform,
        mask: Option<&Mask>,
    ) -> Result<(), RenderError> {
        let font = self.fonts.get(&run.face)?;
        let scale = pt(run.size) / font.metrics().units_per_em as f32;
        let paint = paint(run.color);
        for g in &run.glyphs {
            let Some(path) = outline_path(&font.outline(g.id)) else {
                continue;
            };
            let glyph_at = at.pre_translate(pt(g.x), pt(g.y)).pre_scale(scale, -scale);
            self.pixmap
                .fill_path(&path, &paint, FillRule::Winding, glyph_at, mask);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use reprise_geom::Length;

    use super::*;
    use crate::sample;

    #[test]
    fn renders_a_decodable_png_with_ink() {
        let (list, fonts) = sample::list();
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
    fn groups_are_transformed_and_clipped() {
        let (list, fonts) = sample::list();
        let img = Pixmap::decode_png(&render(&list, &fonts, 2.0).unwrap()).unwrap();
        let blue = |x: u32, y: u32| {
            img.pixel(x, y)
                .is_some_and(|p| p.blue() > 200 && p.red() < 60)
        };
        // Rotated a quarter turn about (95, 20): the 4pt-tall bar runs
        // downwards from y = 20 at x = 91..95.
        assert!(blue(186, 50), "inside the clip");
        assert!(!blue(186, 90), "past the clip, though the bar continues");
    }

    #[test]
    fn rendering_is_deterministic() {
        let (list, fonts) = sample::list();
        assert_eq!(
            render(&list, &fonts, 2.0).expect("renders"),
            render(&list, &fonts, 2.0).expect("renders")
        );
    }

    /// Found by the cross-crate fuzzer: a page the size of a saturated medium
    /// asked tiny-skia for a terabyte and aborted the process.
    #[test]
    fn enormous_pages_are_refused_not_allocated() {
        let (mut list, fonts) = sample::list();
        list.width = Length::MAX;
        list.height = Length::MAX;
        assert!(matches!(
            render(&list, &fonts, 0.25),
            Err(RenderError::BadSize(..))
        ));
        // Just over the cap: 4097 x 4096 pixels at one pixel per point.
        list.width = Length::from_pt(4097);
        list.height = Length::from_pt(4096);
        assert!(matches!(
            render(&list, &fonts, 1.0),
            Err(RenderError::BadSize(..))
        ));
        list.width = Length::from_pt(4096);
        assert!(render(&list, &fonts, 1.0).is_ok());
    }

    #[test]
    fn empty_pages_are_rejected() {
        let (mut list, fonts) = sample::list();
        list.width = Length::from_pt(0);
        assert!(matches!(
            render(&list, &fonts, 1.0),
            Err(RenderError::BadSize(..))
        ));
    }
}
