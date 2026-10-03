//! The shaping adapter contract and the default adapter (decision 22).
//!
//! An adapter turns text into positioned glyphs and keeps the mapping from each
//! glyph back to the source bytes it came from. The adapter and its version are
//! part of the engine configuration, so they are inputs to the determinism
//! guarantee (38).

use reprise_font::Face;
use reprise_geom::{InlineDirection, Length};
use serde::{Deserialize, Serialize};

/// Who shaped a run, recorded with the output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterInfo {
    pub name: String,
    pub version: String,
    /// True when output depends only on declared inputs, so it is identical on
    /// every platform. Adapters that wrap an OS or browser shaper are false.
    pub platform_independent: bool,
}

pub struct ShapeRequest<'a> {
    pub text: &'a str,
    pub face: &'a Face,
    pub size: Length,
    pub direction: InlineDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapedGlyph {
    pub id: u32,
    /// Byte offset of the start of this glyph's cluster in the shaped text.
    pub cluster: u32,
    pub advance: Length,
    pub x_offset: Length,
    pub y_offset: Length,
    /// Breaking the line before this glyph would need the text reshaped.
    pub unsafe_to_break: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapedText {
    pub size: Length,
    pub direction: InlineDirection,
    /// In logical order for left-to-right text.
    pub glyphs: Vec<ShapedGlyph>,
}

impl ShapedText {
    /// Glyphs whose clusters start inside `bytes`.
    pub fn glyphs_in(&self, bytes: std::ops::Range<usize>) -> std::ops::Range<usize> {
        let start = self
            .glyphs
            .partition_point(|g| (g.cluster as usize) < bytes.start);
        let end = self
            .glyphs
            .partition_point(|g| (g.cluster as usize) < bytes.end);
        start..end
    }

    pub fn width(&self, glyphs: std::ops::Range<usize>) -> Length {
        self.glyphs[glyphs].iter().map(|g| g.advance).sum()
    }
}

pub trait ShapingAdapter: Send + Sync {
    fn info(&self) -> AdapterInfo;
    fn shape(&self, request: &ShapeRequest<'_>) -> ShapedText;
}

/// The default adapter: HarfRust, the pure-Rust port of HarfBuzz.
#[derive(Default)]
pub struct HarfRust;

/// Bump together with the harfrust dependency; a test checks they agree.
pub const HARFRUST_VERSION: &str = "0.13.3";

impl ShapingAdapter for HarfRust {
    fn info(&self) -> AdapterInfo {
        AdapterInfo {
            name: "harfrust".into(),
            version: HARFRUST_VERSION.into(),
            platform_independent: true,
        }
    }

    fn shape(&self, request: &ShapeRequest<'_>) -> ShapedText {
        let font = request.face.font_ref();
        let data = harfrust::ShaperData::new(&font);
        let shaper = data.shaper(&font).build();
        let mut buffer = harfrust::UnicodeBuffer::new();
        buffer.push_str(request.text);
        buffer.set_direction(match request.direction {
            InlineDirection::Ltr => harfrust::Direction::LeftToRight,
            InlineDirection::Rtl => harfrust::Direction::RightToLeft,
        });
        buffer.guess_segment_properties();
        // No scale is set, so positions come back in font design units. They are
        // integers, and scaling them with integer arithmetic keeps layout exact.
        let out = shaper.shape(buffer, harfrust::ShapeOptions::new());
        let upem = request.face.metrics().units_per_em;
        let scale = |v: i32| Length::from_font_units(v, request.size, upem);
        let glyphs = out
            .glyph_infos()
            .iter()
            .zip(out.glyph_positions())
            .map(|(info, pos)| ShapedGlyph {
                id: info.glyph_id,
                cluster: info.cluster,
                advance: scale(pos.x_advance),
                x_offset: scale(pos.x_offset),
                y_offset: scale(pos.y_offset),
                unsafe_to_break: info.unsafe_to_break(),
            })
            .collect();
        ShapedText {
            size: request.size,
            direction: request.direction,
            glyphs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");

    fn shape(text: &str) -> ShapedText {
        let face = Face::from_bytes(SERIF).unwrap();
        let request = ShapeRequest {
            text,
            face: &face,
            size: Length::from_pt(12),
            direction: InlineDirection::Ltr,
        };
        HarfRust.shape(&request)
    }

    #[test]
    fn clusters_are_byte_offsets() {
        let shaped = shape("aé b");
        let clusters: Vec<u32> = shaped.glyphs.iter().map(|g| g.cluster).collect();
        assert_eq!(clusters, [0, 1, 3, 4]);
    }

    #[test]
    fn shaping_is_repeatable() {
        assert_eq!(shape("Reprise, again."), shape("Reprise, again."));
    }

    #[test]
    fn adapter_version_matches_dependency() {
        let lock = include_str!("../../../Cargo.lock");
        let entry = format!("name = \"harfrust\"\nversion = \"{HARFRUST_VERSION}\"");
        assert!(
            lock.replace("\r\n", "\n").contains(&entry),
            "update HARFRUST_VERSION"
        );
    }
}
