//! PDF backend. Glyph runs are real PDF text, positioned glyph by glyph at the
//! coordinates layout chose, so the PDF never re-shapes or re-advances them.

use std::collections::BTreeMap;

use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::text::{Font, GlyphId, KrillaGlyph};
use reprise_font::{FaceId, FontStore};

use crate::{Color, DisplayList, Item, RenderError};

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

/// Renders a one-page PDF whose page size in points matches the list.
pub fn render(list: &DisplayList, fonts: &FontStore) -> Result<Vec<u8>, RenderError> {
    let (w, h) = (list.width.to_pt_f32(), list.height.to_pt_f32());
    let settings = PageSettings::from_wh(w, h).ok_or(RenderError::BadSize(w, h))?;
    let mut doc = Document::new();
    let mut page = doc.start_page_with(settings);
    let mut surface = page.surface();

    surface.set_fill(Some(fill(Color(255, 255, 255, 255))));
    let background = Rect::from_xywh(0.0, 0.0, w, h).map(|r| {
        let mut pb = PathBuilder::new();
        pb.push_rect(r);
        pb.finish()
    });
    if let Some(Some(path)) = background {
        surface.draw_path(&path);
    }

    let mut krilla_fonts: BTreeMap<FaceId, Font> = BTreeMap::new();
    for item in &list.items {
        match item {
            Item::Glyphs {
                face,
                size,
                color,
                glyphs,
            } => {
                let font = match krilla_fonts.get(face) {
                    Some(f) => f.clone(),
                    None => {
                        let font = Font::new(fonts.get(face)?.data().to_vec().into(), 0)
                            .ok_or_else(|| {
                                RenderError::Pdf("krilla could not read the font".into())
                            })?;
                        krilla_fonts.insert(face.clone(), font.clone());
                        font
                    }
                };
                surface.set_fill(Some(fill(*color)));
                surface.set_stroke(None);
                for g in glyphs {
                    // One glyph per call, so each sits exactly where layout put it and
                    // the advance is never consulted. The display list carries no source
                    // text, so there is no ToUnicode mapping to offer yet.
                    let glyph =
                        KrillaGlyph::new(GlyphId::new(g.id), 0.0, 0.0, 0.0, 0.0, 0..0, None);
                    surface.draw_glyphs(
                        Point::from_xy(g.x.to_pt_f32(), g.y.to_pt_f32()),
                        &[glyph],
                        font.clone(),
                        "",
                        size.to_pt_f32(),
                        false,
                    );
                }
            }
            Item::Rect {
                rect,
                fill: fill_color,
                stroke: stroke_color,
                ..
            } => {
                let Some(r) = Rect::from_xywh(
                    rect.origin.x.to_pt_f32(),
                    rect.origin.y.to_pt_f32(),
                    rect.width.to_pt_f32(),
                    rect.height.to_pt_f32(),
                ) else {
                    continue;
                };
                let mut pb = PathBuilder::new();
                pb.push_rect(r);
                let Some(path) = pb.finish() else { continue };
                surface.set_fill(fill_color.map(fill));
                surface.set_stroke(stroke_color.map(|c| stroke(c, 0.25)));
                surface.draw_path(&path);
            }
            Item::Line {
                from, to, color, ..
            } => {
                let mut pb = PathBuilder::new();
                pb.move_to(from.x.to_pt_f32(), from.y.to_pt_f32());
                pb.line_to(to.x.to_pt_f32(), to.y.to_pt_f32());
                let Some(path) = pb.finish() else { continue };
                surface.set_fill(None);
                surface.set_stroke(Some(stroke(*color, 0.5)));
                surface.draw_path(&path);
            }
        }
    }

    surface.finish();
    page.finish();
    doc.finish().map_err(|e| RenderError::Pdf(format!("{e:?}")))
}

#[cfg(test)]
mod tests {
    use reprise_font::Face;
    use reprise_geom::{Length, Point, Rect};
    use skrifa::MetadataProvider;

    use super::*;
    use crate::{Glyph, Layer};

    #[test]
    fn renders_a_one_page_pdf() {
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
        let bytes = render(&list, &fonts).expect("renders");
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
    }
}
