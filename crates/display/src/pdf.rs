//! PDF backend. Glyph runs are real PDF text, positioned glyph by glyph at the
//! coordinates layout chose, so the PDF never re-shapes or re-advances them.
//! Each display list becomes one page.
//!
//! Text extraction follows item order, and uses logical source order inside
//! each run, including RTL runs. Viewers must honour PDF ActualText. The list
//! has no separate reading-order metadata for ordering spatial blocks or runs.

use std::collections::BTreeMap;

use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, Stroke};
use krilla::surface::Surface;
use krilla::tagging::{ContentTag, SpanTag};
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
    render_with_assets(pages, fonts, &crate::AssetStore::default())
}

pub fn render_with_assets(
    pages: &[DisplayList],
    fonts: &FontStore,
    assets: &crate::AssetStore,
) -> Result<Vec<u8>, RenderError> {
    render_impl(pages, fonts, assets, false)
}

fn render_impl(
    pages: &[DisplayList],
    fonts: &FontStore,
    assets: &crate::AssetStore,
    ordered: bool,
) -> Result<Vec<u8>, RenderError> {
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
            assets,
            krilla_fonts: &mut krilla_fonts,
            ordered,
        };
        pdf.items(&mut surface, &list.items)?;
        surface.finish();
        page.finish();
    }
    doc.finish().map_err(|e| RenderError::Pdf(format!("{e:?}")))
}

/// A glyph-run address in an original display list, indexing Group children.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReadingRun {
    pub page: usize,
    pub path: Vec<usize>,
}

/// Emit text in explicit reading order, with a whole-run ActualText span.
/// The order must address every glyph item exactly once. Nested transforms
/// and clips are preserved. Non-text leaves paint first in original order.
/// Pages stay in physical order; cross-page reversals return an error.
/// This supplies extraction spans, not a PDF/UA semantic structure tree.
pub fn render_ordered(
    pages: &[DisplayList],
    fonts: &FontStore,
    order: &[ReadingRun],
) -> Result<Vec<u8>, RenderError> {
    render_ordered_with_assets(pages, fonts, &crate::AssetStore::default(), order)
}

pub fn render_ordered_with_assets(
    pages: &[DisplayList],
    fonts: &FontStore,
    assets: &crate::AssetStore,
    order: &[ReadingRun],
) -> Result<Vec<u8>, RenderError> {
    let error = |s: &str| RenderError::Pdf(s.into());
    let mut text = BTreeMap::new();
    let mut output: Vec<DisplayList> = pages
        .iter()
        .map(|p| DisplayList {
            width: p.width,
            height: p.height,
            items: Vec::new(),
        })
        .collect();
    for (page, list) in pages.iter().enumerate() {
        let mut stack = Vec::new();
        for (i, item) in list.items.iter().enumerate().rev() {
            stack.push((item, vec![i], Vec::<(Matrix, Option<Path>)>::new()));
        }
        while let Some((item, path, parents)) = stack.pop() {
            if path.len() > 256 {
                return Err(error("ordered PDF group nesting exceeds 256"));
            }
            if let Item::Group {
                transform,
                clip,
                items,
            } = item
            {
                let mut parents = parents;
                parents.push((*transform, clip.clone()));
                for (i, item) in items.iter().enumerate().rev() {
                    let mut child = path.clone();
                    child.push(i);
                    stack.push((item, child, parents.clone()));
                }
            } else {
                let mut wrapped = item.clone();
                for (transform, clip) in parents.into_iter().rev() {
                    wrapped = Item::Group {
                        transform,
                        clip,
                        items: vec![wrapped],
                    };
                }
                if matches!(item, Item::Glyphs(_) | Item::Image { .. }) {
                    text.insert(ReadingRun { page, path }, wrapped);
                } else {
                    if let Some(page) = output.get_mut(page) {
                        page.items.push(wrapped);
                    }
                }
            }
        }
    }
    let mut previous = 0;
    for key in order {
        if key.page < previous {
            return Err(error("ordered PDF cannot reverse physical page order"));
        }
        previous = key.page;
        let Some(item) = text.remove(key) else {
            return Err(error("ordered PDF has a duplicate or missing run address"));
        };
        let Some(page) = output.get_mut(key.page) else {
            return Err(error("ordered PDF page does not exist"));
        };
        page.items.push(item);
    }
    if !text.is_empty() {
        return Err(error("ordered PDF order omits glyph runs"));
    }
    render_impl(&output, fonts, assets, true)
}

struct Pdf<'a> {
    fonts: &'a FontStore,
    assets: &'a crate::AssetStore,
    ordered: bool,
    krilla_fonts: &'a mut BTreeMap<FaceId, Font>,
}

impl Pdf<'_> {
    fn items(&mut self, surface: &mut Surface<'_>, items: &[Item]) -> Result<(), RenderError> {
        for item in items {
            match item {
                Item::Glyphs(run) => self.glyphs(surface, run)?,
                Item::Image {
                    asset, rect, alt, ..
                } => {
                    surface.start_tagged(ContentTag::Span(SpanTag {
                        actual_text: Some(alt),
                        ..SpanTag::empty()
                    }));
                    if rect.width <= Length::ZERO || rect.height <= Length::ZERO {
                        // Keep semantic text for an authored zero-size image.
                        let marker = reprise_geom::Rect::new(rect.origin, Length(1), Length(1));
                        if let Some(path) = krilla_path(&Path::rect(marker)) {
                            surface.set_fill(Some(fill(Color(0, 0, 0, 0))));
                            surface.set_stroke(None);
                            surface.draw_path(&path);
                        }
                        surface.end_tagged();
                        continue;
                    }
                    if let Some((w, h, rgba)) =
                        self.assets.get(asset).and_then(crate::image_pixels::decode)
                    {
                        if let Some(size) =
                            krilla::geom::Size::from_wh(pt(rect.width), pt(rect.height))
                        {
                            surface.push_transform(&krilla::geom::Transform::from_translate(
                                pt(rect.origin.x),
                                pt(rect.origin.y),
                            ));
                            surface.draw_image(krilla::image::Image::from_rgba8(rgba, w, h), size);
                            surface.pop();
                        }
                    } else if let Some(path) = krilla_path(&Path::rect(*rect)) {
                        surface.set_fill(Some(fill(Color(221, 221, 221, 255))));
                        surface.set_stroke(None);
                        surface.draw_path(&path);
                    }
                    surface.end_tagged();
                }
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
        if run.glyphs.is_empty() || run.size <= Length::ZERO {
            return Ok(());
        }
        let font = match self.krilla_fonts.get(&run.face) {
            Some(f) => f.clone(),
            None => {
                // A face from a collection (TTC/OTC) is one of several in the
                // same bytes; krilla must open the same one shaping used.
                let face = self.fonts.get(&run.face)?;
                let font = Font::new(face.data().to_vec().into(), face.declaration().face_index)
                    .ok_or_else(|| RenderError::Pdf("krilla could not read the font".into()))?;
                self.krilla_fonts.insert(run.face.clone(), font.clone());
                font
            }
        };
        surface.set_fill(Some(fill(run.color)));
        surface.set_stroke(None);
        // krilla must see all glyphs in a cluster together to emit ActualText
        // rather than duplicate the source once per glyph. Zero advances and
        // offsets from the origin keep the layout's absolute glyph positions.
        let size = pt(run.size);
        let fallback = needs_run_text(run);
        let glyphs: Vec<_> = run
            .glyphs
            .iter()
            .map(|g| {
                let range = if fallback {
                    0..run.text.len()
                } else {
                    g.text.start as usize..g.text.end as usize
                };
                KrillaGlyph::new(
                    GlyphId::new(g.id),
                    0.0,
                    pt(g.x) / size,
                    -pt(g.y) / size,
                    0.0,
                    range,
                    None,
                )
            })
            .collect();
        if self.ordered {
            surface.start_tagged(ContentTag::Span(SpanTag {
                actual_text: Some(&run.text),
                ..SpanTag::empty()
            }));
        }
        surface.draw_glyphs(
            Point::from_xy(0.0, 0.0),
            &glyphs,
            font,
            &run.text,
            size,
            false,
        );
        if self.ordered {
            surface.end_tagged();
        }
        Ok(())
    }
}

/// A valid LTR mapping partitions all source bytes, with repeated ranges
/// allowed for a cluster. Everything else (RTL, holes, overlaps, empty or
/// invalid UTF-8 ranges) falls back to one source span for the entire run.
/// A single glyph can express that span through ToUnicode; multiple glyphs
/// make krilla emit ActualText. Neither case guesses missing characters.
fn needs_run_text(run: &GlyphRun) -> bool {
    let mut end = 0;
    let mut previous = None;
    for glyph in &run.glyphs {
        let range = glyph.text.start as usize..glyph.text.end as usize;
        if range.is_empty() || run.text.get(range.clone()).is_none() {
            return true;
        }
        if previous.as_ref() != Some(&range) {
            if range.start != end {
                return true;
            }
            end = range.end;
            previous = Some(range);
        }
    }
    end != run.text.len()
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
