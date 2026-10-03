//! Font faces with pinned identities (decision 21).
//!
//! A face is identified by its family name and a hash of its bytes, so a
//! document can say exactly which face it was laid out with and report any
//! substitution.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::TableProvider;
use skrifa::string::StringId;
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// A pinned face identity: family plus the first 16 bytes of the SHA-256 of the file.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FaceId {
    pub family: String,
    pub hash: String,
}

impl fmt::Debug for FaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.family, &self.hash[..8])
    }
}

/// Vertical metrics in font design units. Read from integer tables, so exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaceMetrics {
    pub units_per_em: u16,
    pub ascent: i32,
    /// Positive distance below the baseline.
    pub descent: i32,
    pub line_gap: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum FontError {
    #[error("not a font file the engine can read: {0}")]
    Unreadable(String),
    #[error("no face {0:?} in the store")]
    Missing(FaceId),
}

pub struct Face {
    id: FaceId,
    data: Arc<[u8]>,
    metrics: FaceMetrics,
}

impl Face {
    pub fn from_bytes(data: impl Into<Arc<[u8]>>) -> Result<Face, FontError> {
        let data: Arc<[u8]> = data.into();
        let font = FontRef::new(&data).map_err(|e| FontError::Unreadable(e.to_string()))?;
        let family = font
            .localized_strings(StringId::FAMILY_NAME)
            .english_or_first()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unnamed".into());
        let unreadable = |e: skrifa::raw::ReadError| FontError::Unreadable(e.to_string());
        let head = font.head().map_err(unreadable)?;
        let hhea = font.hhea().map_err(unreadable)?;
        let metrics = FaceMetrics {
            units_per_em: head.units_per_em(),
            ascent: hhea.ascender().to_i16() as i32,
            descent: -(hhea.descender().to_i16() as i32),
            line_gap: hhea.line_gap().to_i16() as i32,
        };
        let digest = Sha256::digest(&data);
        let hash = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
        Ok(Face {
            id: FaceId { family, hash },
            data,
            metrics,
        })
    }

    pub fn id(&self) -> &FaceId {
        &self.id
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn metrics(&self) -> FaceMetrics {
        self.metrics
    }

    pub fn font_ref(&self) -> FontRef<'_> {
        FontRef::new(&self.data).expect("validated in from_bytes")
    }

    /// The glyph's outline in font design units, y up. For rendering only.
    pub fn outline(&self, glyph: u32) -> Vec<PathCmd> {
        let font = self.font_ref();
        let mut pen = Recorder(Vec::new());
        if let Some(outline) = font.outline_glyphs().get(GlyphId::new(glyph)) {
            let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
            let _ = outline.draw(settings, &mut pen);
        }
        pen.0
    }
}

/// A path command in font design units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCmd {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

struct Recorder(Vec<PathCmd>);

impl OutlinePen for Recorder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(PathCmd::Move(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(PathCmd::Line(x, y));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.push(PathCmd::Quad(cx0, cy0, x, y));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.push(PathCmd::Cubic(cx0, cy0, cx1, cy1, x, y));
    }
    fn close(&mut self) {
        self.0.push(PathCmd::Close);
    }
}

/// The faces available to a layout. Documents can bundle their own (34).
#[derive(Default)]
pub struct FontStore {
    faces: BTreeMap<FaceId, Arc<Face>>,
}

impl FontStore {
    pub fn add(&mut self, face: Face) -> FaceId {
        let id = face.id.clone();
        self.faces.insert(id.clone(), Arc::new(face));
        id
    }

    pub fn get(&self, id: &FaceId) -> Result<&Arc<Face>, FontError> {
        self.faces
            .get(id)
            .ok_or_else(|| FontError::Missing(id.clone()))
    }

    /// The first face of a family. Real fallback chains (21) come later.
    pub fn by_family(&self, family: &str) -> Option<&Arc<Face>> {
        self.faces.values().find(|f| f.id.family == family)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");

    #[test]
    fn identity_and_metrics_are_pinned() {
        let face = Face::from_bytes(SERIF).unwrap();
        assert_eq!(face.id().family, "Source Serif Pro");
        assert_eq!(face.metrics().units_per_em, 1000);
        assert!(face.metrics().ascent > 0 && face.metrics().descent > 0);
    }

    #[test]
    fn outlines_are_readable() {
        let face = Face::from_bytes(SERIF).unwrap();
        let glyph = face.font_ref().charmap().map('a').unwrap().to_u32();
        assert!(!face.outline(glyph).is_empty());
    }
}
