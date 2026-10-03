//! PDF backend. Glyph runs are real PDF text, positioned glyph by glyph at the
//! coordinates layout chose, so the PDF never re-shapes or re-advances them.
//! Each display list becomes one page.

use std::collections::BTreeMap;

use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, Stroke};
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph};
use reprise_font::{FaceId, FontStore};
use reprise_geom::{Length, Matrix};

use crate::{Color, DisplayList, GlyphRun, Item, Path, RenderError, Segment};

fn pt(l: Length) -> f32 {
    l.to_pt_f32()
}

fn rgb_of(c: Color) -> rgb::Color {
    rgb::Color::new(c.0, c.1, c.2)
}

fn opacity(c: Color) -> NormalizedF32 {
    NormalizedF32::new(c.alpha()).unwrap_or(NormalizedF32::ONE)
}

fn fill(c: Color) -> Fill {
    Fill {
        paint: rgb_of(c).into(),
        opacity: opacity(c),
        rule: Default::default(),
    }
}

fn stroke(c: Color, width: f32) -> Stroke {
    Stroke {
        paint: rgb_of(c).into(),
        width,
        opacity: opacity(c),
        ..Default::default()
    }
}

fn krilla_path(path: &Path) -> Option<krilla::geom::Path> {
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

fn krilla_matrix(m: &Matrix) -> krilla::geom::Transform {
    krilla::geom::Transform::from_row(
        m.xx.to_f32(),
        m.yx.to_f32(),
        m.xy.to_f32(),
        m.yy.to_f32(),
        pt(m.tx),
        pt(m.ty),
    )
}

/// Renders one page per display list. Each page's size in points matches its list.
pub fn render(pages: &[DisplayList], fonts: &FontStore) -> Result<Vec<u8>, RenderError> {
    let mut doc = Document::new();
    let mut krilla_fonts = BTreeMap::new();
    for list in pages {
        let (w, h) = (pt(list.width), pt(list.height));
        let settings = PageSettings::from_wh(w, h).ok_or(RenderError::BadSize(w, h))?;
        let mut page = doc.start_page_with(settings);
        let mut surface = page.surface();
        surface.set_fill(Some(fill(Color(255, 255, 255, 255))));
        let background = krilla::geom::Rect::from_xywh(0.0, 0.0, w, h).and_then(|r| {
            let mut pb = PathBuilder::new();
            pb.push_rect(r);
            pb.finish()
        });
        if let Some(path) = background {
            surface.draw_path(&path);
        }
        let mut pdf = Pdf {
            fonts,
            krilla_fonts: &mut krilla_fonts,
        };
        pdf.items(&mut surface, &list.items)?;
        surface.finish();
        page.finish();
    }
    doc.finish().map_err(|e| RenderError::Pdf(format!("{e:?}")))
}

struct Pdf<'a> {
    fonts: &'a FontStore,
    krilla_fonts: &'a mut BTreeMap<FaceId, Font>,
}

impl Pdf<'_> {
    fn items(&mut self, surface: &mut Surface<'_>, items: &[Item]) -> Result<(), RenderError> {
        for item in items {
            match item {
                Item::Glyphs(run) => self.glyphs(surface, run)?,
                Item::Path {
                    path,
                    fill: fill_color,
                    stroke: stroke_style,
                    ..
                } => {
                    let Some(path) = krilla_path(path) else {
                        continue;
                    };
                    surface.set_fill(fill_color.map(fill));
                    surface.set_stroke(stroke_style.map(|s| stroke(s.color, pt(s.width))));
                    surface.draw_path(&path);
                }
                Item::Group {
                    transform,
                    clip,
                    items,
                } => {
                    surface.push_transform(&krilla_matrix(transform));
                    let clip = clip.as_ref().and_then(krilla_path);
                    if let Some(path) = &clip {
                        surface.push_clip_path(path, &FillRule::NonZero);
                    }
                    self.items(surface, items)?;
                    if clip.is_some() {
                        surface.pop();
                    }
                    surface.pop();
                }
            }
        }
        Ok(())
    }

    fn glyphs(&mut self, surface: &mut Surface<'_>, run: &GlyphRun) -> Result<(), RenderError> {
        let font = match self.krilla_fonts.get(&run.face) {
            Some(f) => f.clone(),
            None => {
                let font = Font::new(self.fonts.get(&run.face)?.data().to_vec().into(), 0)
                    .ok_or_else(|| RenderError::Pdf("krilla could not read the font".into()))?;
                self.krilla_fonts.insert(run.face.clone(), font.clone());
                font
            }
        };
        surface.set_fill(Some(fill(run.color)));
        surface.set_stroke(None);
        for g in &run.glyphs {
            // One glyph per call, so each sits exactly where layout put it and
            // the advance is never consulted. ToUnicode text from `run.text`
            // is not passed on yet.
            let glyph = KrillaGlyph::new(GlyphId::new(g.id), 0.0, 0.0, 0.0, 0.0, 0..0, None);
            surface.draw_glyphs(
                Point::from_xy(pt(g.x), pt(g.y)),
                &[glyph],
                font.clone(),
                "",
                pt(run.size),
                false,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample;

    #[test]
    fn renders_one_page_per_list() {
        let (list, fonts) = sample::list();
        let bytes = render(&[list.clone(), list], &fonts).expect("renders");
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(
            bytes.len() > 1000,
            "embeds the font subset: {} bytes",
            bytes.len()
        );
        assert!(
            bytes.windows(5).any(|w| w == b"/Font"),
            "text is drawn with a font, not outlines"
        );
        let pages = bytes
            .windows(10)
            .filter(|w| w == b"/Type /Pag" || w == b"/Type/Page")
            .count();
        assert!(pages >= 2, "two pages: {pages}");
    }

    #[test]
    fn no_pages_is_still_a_pdf() {
        let (_, fonts) = sample::list();
        let bytes = render(&[], &fonts).expect("renders");
        assert!(bytes.starts_with(b"%PDF-"));
    }
}
