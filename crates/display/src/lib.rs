//! The backend-neutral display list (decision 32) and its backends.
//!
//! Layout emits one [`DisplayList`] per page: glyph runs, paths and groups.
//! Backends only draw it; they never make layout decisions.
//!
//! # Coordinates and transforms
//!
//! Item coordinates are in the space of the enclosing [`Item::Group`]; at the
//! top level that is page space, in points with y growing downwards. A group's
//! [`Matrix`] maps its children's space into its parent's, so a rotated or
//! mirrored frame is one group (20). Everything is fixed point: backends turn
//! it into floats only to draw.
//!
//! # Resources
//!
//! Glyph runs name their face by [`FaceId`]; the faces belong to the
//! `FontStore` the backend is given, not to the list. Images name SHA-256 hashes
//! in the host-owned [`AssetStore`].
//!
//! # Source text
//!
//! Every glyph run carries the source text it draws and, for each glyph, the
//! bytes of that text it came from, so exporters can make text selectable and
//! copyable (PDF ToUnicode and ActualText) and accessible.

use std::ops::Range;

use reprise_font::{FaceId, FontError};
use reprise_geom::{Length, Matrix, PageSpace, Point, Rect};
use serde::{Deserialize, Serialize};

pub mod assets;
mod image_pixels;
pub mod pdf;
pub use assets::AssetStore;

/// Rendering/export preflight, with the same decoding limits as PNG and PDF.
/// Layout must use `assets::image_header` instead of this pixel operation.
pub fn image_renderable(bytes: &[u8]) -> bool {
    image_pixels::decode(bytes).is_some()
}
pub mod png;
pub mod svg;

/// Why a backend could not draw a display list.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Font(#[from] FontError),
    #[error("page is {0}x{1} px, which cannot be rasterised")]
    BadSize(f32, f32),
    #[error("could not encode the PNG: {0}")]
    Encode(String),
    #[error("could not build the PDF: {0}")]
    Pdf(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub const BLACK: Color = Color(0, 0, 0, 255);

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    pub fn alpha(self) -> f32 {
        self.3 as f32 / 255.0
    }
}

/// Content is the document; debug is the overlay showing how it was laid out (39).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    Content,
    Debug,
}

/// One positioned glyph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Glyph {
    pub id: u32,
    /// Where the glyph's origin sits on the baseline.
    pub x: Length,
    pub y: Length,
    /// The bytes of the run's `text` this glyph draws. Glyphs of one cluster
    /// share a range; a ligature's range covers every character it joins.
    pub text: Range<u32>,
}

/// Glyphs of one face and size, with the text they draw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlyphRun {
    pub face: FaceId,
    pub size: Length,
    pub color: Color,
    /// The source text, in logical order.
    pub text: String,
    /// In visual order.
    pub glyphs: Vec<Glyph>,
    pub layer: Layer,
}

/// One segment of a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Segment {
    Move(Point<PageSpace>),
    Line(Point<PageSpace>),
    Quad(Point<PageSpace>, Point<PageSpace>),
    Cubic(Point<PageSpace>, Point<PageSpace>, Point<PageSpace>),
    Close,
}

/// A vector path.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Path(pub Vec<Segment>);

impl Path {
    pub fn rect(r: Rect<PageSpace>) -> Path {
        let (x0, y0, x1, y1) = (r.origin.x, r.origin.y, r.max_x(), r.max_y());
        Path(vec![
            Segment::Move(Point::new(x0, y0)),
            Segment::Line(Point::new(x1, y0)),
            Segment::Line(Point::new(x1, y1)),
            Segment::Line(Point::new(x0, y1)),
            Segment::Close,
        ])
    }

    pub fn line(from: Point<PageSpace>, to: Point<PageSpace>) -> Path {
        Path(vec![Segment::Move(from), Segment::Line(to)])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stroke {
    pub color: Color,
    pub width: Length,
}

/// A drawing operation.
///
/// The item set is closed to backends but open to additions: image items
/// come with document assets (34). Adding a variant means drawing it in every
/// backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Item {
    Glyphs(GlyphRun),
    /// Host-owned bitmap; destination is in the enclosing group's space.
    Image {
        asset: String,
        rect: Rect<PageSpace>,
        alt: String,
        layer: Layer,
    },
    Path {
        path: Path,
        fill: Option<Color>,
        stroke: Option<Stroke>,
        layer: Layer,
    },
    /// Children drawn through `transform`, clipped to `clip` (in the
    /// children's space) when it is set.
    Group {
        transform: Matrix,
        clip: Option<Path>,
        items: Vec<Item>,
    },
}

impl Item {
    /// The same item with only the parts on `layer`, or `None` if nothing is left.
    fn on_layer(&self, layer: Layer) -> Option<Item> {
        match self {
            Item::Glyphs(run) => (run.layer == layer).then(|| self.clone()),
            Item::Path { layer: l, .. } => (*l == layer).then(|| self.clone()),
            Item::Image { layer: l, .. } => (*l == layer).then(|| self.clone()),
            Item::Group {
                transform,
                clip,
                items,
            } => {
                let items: Vec<Item> = items.iter().filter_map(|i| i.on_layer(layer)).collect();
                (!items.is_empty()).then(|| Item::Group {
                    transform: *transform,
                    clip: clip.clone(),
                    items,
                })
            }
        }
    }
}

/// One page of drawing operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayList {
    pub width: Length,
    pub height: Length,
    pub items: Vec<Item>,
}

impl DisplayList {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("display lists serialize")
    }

    /// The same list without the debug overlay.
    pub fn content_only(&self) -> DisplayList {
        DisplayList {
            width: self.width,
            height: self.height,
            items: self
                .items
                .iter()
                .filter_map(|i| i.on_layer(Layer::Content))
                .collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod sample {
    use reprise_font::{Face, FontStore};
    use reprise_geom::{Fixed, Length, Matrix, Point, Rect};
    use skrifa::MetadataProvider;

    use super::*;

    /// A page with a glyph, a filled and stroked rectangle, a line, and a
    /// rotated, clipped group holding a debug rectangle.
    pub fn list() -> (DisplayList, FontStore) {
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
        let pt = Length::from_pt;
        let list = DisplayList {
            width: pt(100),
            height: pt(50),
            items: vec![
                Item::Glyphs(GlyphRun {
                    face: face_id,
                    size: pt(40),
                    color: Color::BLACK,
                    text: "a".into(),
                    glyphs: vec![Glyph {
                        id: id_a,
                        x: pt(10),
                        y: pt(40),
                        text: 0..1,
                    }],
                    layer: Layer::Content,
                }),
                Item::Path {
                    path: Path::rect(Rect::new(Point::new(pt(60), pt(5)), pt(30), pt(10))),
                    fill: Some(Color(255, 0, 0, 255)),
                    stroke: Some(Stroke {
                        color: Color::BLACK,
                        width: Length(256),
                    }),
                    layer: Layer::Content,
                },
                Item::Path {
                    path: Path::line(Point::new(pt(0), pt(48)), Point::new(pt(100), pt(48))),
                    fill: None,
                    stroke: Some(Stroke {
                        color: Color::BLACK,
                        width: Length(512),
                    }),
                    layer: Layer::Content,
                },
                Item::Group {
                    transform: Matrix::rotate_quarter(1).then(&Matrix::translate(pt(95), pt(20))),
                    clip: Some(Path::rect(Rect::new(Point::origin(), pt(20), pt(4)))),
                    items: vec![Item::Path {
                        // Twice as long as the clip: only the clipped half shows.
                        path: Path::rect(Rect::new(Point::origin(), pt(40), pt(4))),
                        fill: Some(Color(0, 0, 255, 255)),
                        stroke: None,
                        layer: Layer::Debug,
                    }],
                },
                Item::Group {
                    transform: Matrix::scale(Fixed::ONE, Fixed::ONE),
                    clip: None,
                    items: Vec::new(),
                },
            ],
        };
        (list, fonts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_only_drops_debug_items_and_emptied_groups() {
        let (list, _) = sample::list();
        let content = list.content_only();
        assert_eq!(content.items.len(), 3, "glyphs, rectangle, line");
        assert!(
            content
                .items
                .iter()
                .all(|i| !matches!(i, Item::Group { .. }))
        );
    }
}
