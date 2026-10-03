//! The backend-neutral display list (decision 32) and its backends.
//!
//! Layout emits a [`DisplayList`]: glyph runs, rectangles and lines in page
//! space. Backends only draw it; they never make layout decisions.

use reprise_font::{FaceId, FontError};
use reprise_geom::{Length, PageSpace, Point, Rect};
use serde::{Deserialize, Serialize};

pub mod pdf;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Glyph {
    pub id: u32,
    /// Where the glyph's origin sits on the baseline, in page space.
    pub x: Length,
    pub y: Length,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Item {
    Glyphs {
        face: FaceId,
        size: Length,
        color: Color,
        glyphs: Vec<Glyph>,
    },
    Rect {
        rect: Rect<PageSpace>,
        fill: Option<Color>,
        stroke: Option<Color>,
        layer: Layer,
    },
    Line {
        from: Point<PageSpace>,
        to: Point<PageSpace>,
        color: Color,
        layer: Layer,
    },
}

impl Item {
    pub fn layer(&self) -> Layer {
        match self {
            Item::Glyphs { .. } => Layer::Content,
            Item::Rect { layer, .. } | Item::Line { layer, .. } => *layer,
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
                .filter(|i| i.layer() == Layer::Content)
                .cloned()
                .collect(),
        }
    }
}
