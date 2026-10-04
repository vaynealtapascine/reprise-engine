//! The shaping adapter contract and the default adapter (decision 22).
//!
//! An adapter turns one run of text into positioned glyphs and keeps the
//! mapping from each glyph back to the source bytes it came from. The adapter
//! and its version are part of the engine configuration, so they are inputs to
//! the determinism guarantee (38).
//!
//! Shaping a paragraph has three steps, each behind a stable interface:
//!
//! 1.  [`itemize`] splits the paragraph into [`Item`]s: runs with one face,
//!     size, bidi level and script. Font fallback happens here, and every
//!     substitution is reported (21).
//! 2.  [`Shaper::shape`] shapes each item with the configured
//!     [`ShapingAdapter`], giving a [`ShapedText`].
//! 3.  When a line break lands where the shaping depended on text across the
//!     break, the composer calls [`Reshape::reshape`] to shape the line again
//!     without context across its edges.
//!
//! Every position is a UTF-8 byte offset into the paragraph's text.

use std::ops::Range;

use reprise_font::{Face, FaceId};
use reprise_geom::{InlineDirection, Length};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod fallback_tests;
mod line;
mod paragraph;
mod unicode;

pub use line::reorder_line;
pub use paragraph::{Itemized, ParagraphInput, Shaper, StyleRun, codes, itemize, itemize_families};
pub use unicode::UNICODE_VERSION;

/// Who shaped a run, recorded with the output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterInfo {
    pub name: String,
    pub version: String,
    /// True when output depends only on declared inputs, so it is identical on
    /// every platform. Adapters that wrap an OS or browser shaper are false.
    pub platform_independent: bool,
}

/// An ISO 15924 script tag, such as `Latn` or `Arab`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Script(pub [u8; 4]);

/// An OpenType feature setting, such as `liga` off (value 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Feature {
    pub tag: [u8; 4],
    pub value: u32,
}

/// One run to shape.
pub struct ShapeRequest<'a> {
    /// The whole paragraph. Clusters in the result are byte offsets into it.
    pub text: &'a str,
    /// The run to shape.
    pub range: Range<usize>,
    /// The text the shaper may look at around the run, for joining and
    /// contextual forms; it contains `range`. Equal to `range` for no context,
    /// as at a line edge.
    pub context: Range<usize>,
    pub face: &'a Face,
    pub size: Length,
    pub direction: InlineDirection,
    /// `None` lets the adapter guess from the text.
    pub script: Option<Script>,
    /// A BCP 47 language tag.
    pub language: Option<&'a str>,
    pub features: &'a [Feature],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapedGlyph {
    pub id: u32,
    /// Byte offset of the start of this glyph's cluster in the paragraph.
    pub cluster: u32,
    pub advance: Length,
    pub x_offset: Length,
    pub y_offset: Length,
    /// Breaking the line before this glyph's cluster needs the text reshaped.
    pub unsafe_to_break: bool,
    /// Shaping the text either side of this glyph's cluster separately and
    /// joining the results may differ from shaping it whole.
    pub unsafe_to_concat: bool,
}

/// The contract every shaping adapter fulfils (22).
///
/// -   `shape` is a pure function of the request: no time, randomness, I/O or
///     hidden state that changes output. Caches are allowed if they don't.
/// -   Glyphs come back in visual order for the run's direction: logical order
///     for left-to-right runs, reversed for right-to-left ones.
/// -   Every glyph's cluster is a byte offset inside `request.range`, on a
///     character boundary. Clusters never split a grapheme cluster.
/// -   Shaping never panics on any text; unknown characters give the face's
///     `.notdef` glyph (ID 0).
pub trait ShapingAdapter: Send + Sync {
    fn info(&self) -> AdapterInfo;
    fn shape(&self, request: &ShapeRequest<'_>) -> Vec<ShapedGlyph>;
}

/// An itemised run of a paragraph: one face, size, bidi level and script (21, 22).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub range: Range<usize>,
    pub face: FaceId,
    pub size: Length,
    /// The Unicode bidi embedding level. Even is left to right, odd right to left.
    pub level: u8,
    pub script: Option<Script>,
    pub language: Option<String>,
    pub features: Vec<Feature>,
}

impl Item {
    pub fn direction(&self) -> InlineDirection {
        direction_of(self.level)
    }
}

pub fn direction_of(level: u8) -> InlineDirection {
    if level.is_multiple_of(2) {
        InlineDirection::Ltr
    } else {
        InlineDirection::Rtl
    }
}

/// The visual order of a line's runs from their bidi levels, by rule L2 of
/// UAX #9: from the highest level down to the lowest odd level, reverse every
/// maximal sequence of runs at that level or higher. Returns indices into
/// `levels`, leftmost first.
pub fn visual_order(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let Some(&highest) = levels.iter().max() else {
        return order;
    };
    let lowest_odd = levels.iter().copied().filter(|l| l % 2 == 1).min();
    let Some(lowest_odd) = lowest_odd else {
        return order;
    };
    for level in (lowest_odd..=highest).rev() {
        let mut i = 0;
        while i < order.len() {
            if levels[order[i]] < level {
                i += 1;
                continue;
            }
            let start = i;
            while i < order.len() && levels[order[i]] >= level {
                i += 1;
            }
            order[start..i].reverse();
        }
    }
    order
}

/// A shaped run: one item, or the part of one that fell on a line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapedRun {
    pub range: Range<usize>,
    pub face: FaceId,
    pub size: Length,
    pub level: u8,
    /// In visual order for the run's direction.
    pub glyphs: Vec<ShapedGlyph>,
}

impl ShapedRun {
    pub fn direction(&self) -> InlineDirection {
        direction_of(self.level)
    }

    pub fn width(&self) -> Length {
        self.glyphs.iter().map(|g| g.advance).sum()
    }
}

/// A shaped paragraph: its runs in logical order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapedText {
    pub runs: Vec<ShapedRun>,
}

impl ShapedText {
    fn glyphs(&self) -> impl Iterator<Item = &ShapedGlyph> {
        self.runs.iter().flat_map(|r| &r.glyphs)
    }

    /// The advance width of the glyphs whose clusters start inside `bytes`.
    pub fn width(&self, bytes: Range<usize>) -> Length {
        self.glyphs()
            .filter(|g| bytes.contains(&(g.cluster as usize)))
            .map(|g| g.advance)
            .sum()
    }

    /// The runs cut down to the glyphs whose clusters start inside `bytes`.
    /// Runs left with no glyphs are dropped.
    pub fn slice(&self, bytes: Range<usize>) -> Vec<ShapedRun> {
        self.runs
            .iter()
            .filter_map(|run| {
                let range = run.range.start.max(bytes.start)..run.range.end.min(bytes.end);
                if range.is_empty() {
                    return None;
                }
                let glyphs: Vec<_> = run
                    .glyphs
                    .iter()
                    .filter(|g| range.contains(&(g.cluster as usize)))
                    .copied()
                    .collect();
                (!glyphs.is_empty()).then(|| ShapedRun {
                    range,
                    glyphs,
                    ..run.clone()
                })
            })
            .collect()
    }

    /// True when the line may break at byte `at` without reshaping: either no
    /// glyph's cluster starts there (it is between runs or outside the text),
    /// or the glyphs that start there are not marked unsafe to break.
    pub fn is_safe_to_break(&self, at: usize) -> bool {
        !self
            .glyphs()
            .any(|g| g.cluster as usize == at && g.unsafe_to_break)
    }
}

/// Shapes part of a paragraph again, as a line on its own (22).
pub trait Reshape {
    /// Shapes `range` of the paragraph with no shaping context across
    /// `range.start` or `range.end`. Context between items inside the range
    /// is kept. Returns runs in logical order, like [`ShapedText::runs`].
    fn reshape(&self, range: Range<usize>) -> Vec<ShapedRun>;
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

    fn shape(&self, request: &ShapeRequest<'_>) -> Vec<ShapedGlyph> {
        let local;
        let data = match request
            .face
            .adapter_data(|| harfrust::ShaperData::new(&request.face.font_ref()))
        {
            Some(data) => data,
            None => {
                local = harfrust::ShaperData::new(&request.face.font_ref());
                &local
            }
        };
        shape_with_data(request, data)
    }
}

fn shape_with_data(request: &ShapeRequest<'_>, data: &harfrust::ShaperData) -> Vec<ShapedGlyph> {
    let text = request.text;
    let (range, context) = (request.range.clone(), &request.context);
    let (Some(run), Some(pre), Some(post)) = (
        text.get(range.clone()),
        text.get(context.start.min(range.start)..range.start),
        text.get(range.end..context.end.max(range.end)),
    ) else {
        return Vec::new(); // Not on character boundaries; nothing to shape.
    };
    let font = request.face.font_ref();
    let shaper = data.shaper(&font).build();
    let mut buffer = harfrust::UnicodeBuffer::new();
    buffer.set_pre_context(pre);
    buffer.push_str(run);
    buffer.set_post_context(post);
    buffer.set_direction(match request.direction {
        InlineDirection::Ltr => harfrust::Direction::LeftToRight,
        InlineDirection::Rtl => harfrust::Direction::RightToLeft,
    });
    if let Some(script) = request
        .script
        .and_then(|s| harfrust::Script::from_iso15924_tag(harfrust::Tag::new(&s.0)))
    {
        buffer.set_script(script);
    }
    if let Some(language) = request.language.and_then(|l| l.parse().ok()) {
        buffer.set_language(language);
    }
    buffer.guess_segment_properties();
    let features: Vec<harfrust::Feature> = request
        .features
        .iter()
        .map(|f| harfrust::Feature::new(harfrust::Tag::new(&f.tag), f.value, ..))
        .collect();
    // No scale is set, so positions come back in font design units. They are
    // integers, and scaling them with integer arithmetic keeps layout exact.
    let out = shaper.shape(buffer, harfrust::ShapeOptions::new().features(&features));
    let upem = request.face.metrics().units_per_em;
    let scale = |v: i32| Length::from_font_units(v, request.size, upem);
    out.glyph_infos()
        .iter()
        .zip(out.glyph_positions())
        .map(|(info, pos)| ShapedGlyph {
            id: info.glyph_id,
            cluster: info.cluster.saturating_add(range.start as u32),
            advance: scale(pos.x_advance),
            x_offset: scale(pos.x_offset),
            y_offset: scale(pos.y_offset),
            unsafe_to_break: info.unsafe_to_break(),
            unsafe_to_concat: info.unsafe_to_concat(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");

    fn request<'a>(text: &'a str, face: &'a Face, range: Range<usize>) -> ShapeRequest<'a> {
        ShapeRequest {
            text,
            range: range.clone(),
            context: range,
            face,
            size: Length::from_pt(12),
            direction: InlineDirection::Ltr,
            script: None,
            language: None,
            features: &[],
        }
    }

    fn shape(text: &str) -> Vec<ShapedGlyph> {
        let face = Face::from_bytes(SERIF).unwrap();
        HarfRust.shape(&request(text, &face, 0..text.len()))
    }

    #[test]
    fn clusters_are_byte_offsets() {
        let clusters: Vec<u32> = shape("aé b").iter().map(|g| g.cluster).collect();
        assert_eq!(clusters, [0, 1, 3, 4]);
    }

    #[test]
    fn clusters_are_offsets_into_the_paragraph_not_the_run() {
        let face = Face::from_bytes(SERIF).unwrap();
        let text = "one two";
        let glyphs = HarfRust.shape(&request(text, &face, 4..7));
        let clusters: Vec<u32> = glyphs.iter().map(|g| g.cluster).collect();
        assert_eq!(clusters, [4, 5, 6]);
    }

    #[test]
    fn features_reach_the_shaper() {
        let face = Face::from_bytes(SERIF).unwrap();
        let text = "office";
        let with = HarfRust.shape(&request(text, &face, 0..text.len()));
        let off = [Feature {
            tag: *b"liga",
            value: 0,
        }];
        let without = HarfRust.shape(&ShapeRequest {
            features: &off,
            ..request(text, &face, 0..text.len())
        });
        assert!(with.len() < without.len(), "the ffi ligature is one glyph");
        assert_eq!(without.len(), text.len());
    }

    #[test]
    fn rtl_latin_keeps_ligatures_and_visual_clusters() {
        let face = Face::from_bytes(SERIF).unwrap();
        let features = [Feature {
            tag: *b"liga",
            value: 0,
        }];
        let request = ShapeRequest {
            direction: InlineDirection::Rtl,
            script: Some(Script(*b"Latn")),
            ..request("office", &face, 0..6)
        };
        let with = HarfRust.shape(&request);
        let without = HarfRust.shape(&ShapeRequest {
            features: &features,
            ..request
        });
        assert!(with.len() < without.len());
        assert!(
            with.windows(2)
                .all(|pair| pair[0].cluster >= pair[1].cluster)
        );
        assert!(with.iter().all(|g| g.id != 0));
    }

    #[test]
    fn misaligned_requests_shape_nothing_instead_of_panicking() {
        let face = Face::from_bytes(SERIF).unwrap();
        let text = "é";
        assert!(HarfRust.shape(&request(text, &face, 0..1)).is_empty());
        assert!(HarfRust.shape(&request(text, &face, 0..9)).is_empty());
    }

    #[test]
    fn visual_order_follows_rule_l2() {
        assert_eq!(visual_order(&[]), Vec::<usize>::new());
        assert_eq!(visual_order(&[0, 0, 0]), [0, 1, 2]);
        assert_eq!(visual_order(&[1, 1, 1]), [2, 1, 0]);
        // English with an embedded Hebrew phrase holding a number.
        assert_eq!(visual_order(&[0, 1, 2, 1, 0]), [0, 3, 2, 1, 4]);
        // A right-to-left paragraph with embedded English.
        assert_eq!(visual_order(&[1, 2, 2, 1]), [3, 1, 2, 0]);
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

    #[test]
    fn cached_and_uncached_shaping_are_identical_on_a_varied_corpus() {
        let face = Face::from_bytes(SERIF).unwrap();
        let features = [
            Feature {
                tag: *b"liga",
                value: 0,
            },
            Feature {
                tag: *b"kern",
                value: 0,
            },
        ];
        for text in [
            "",
            "office affinity AV To",
            "é e\u{301} Z\u{335}\u{322}",
            "Ελληνικά Кириллица",
            "עברית العربية 123 (abc)",
            "\u{202e}office\u{202c}",
            "👩\u{200d}👩\u{200d}👧",
            "one\ttwo\nthree",
            "a\u{0}b",
        ] {
            for direction in [InlineDirection::Ltr, InlineDirection::Rtl] {
                for size in [
                    Length::MIN,
                    Length::MAX,
                    Length::ZERO,
                    Length::from_pt(-12),
                    Length::from_pt(12),
                ] {
                    for settings in [&[][..], &features[..]] {
                        for script in [None, Some(Script(*b"Latn")), Some(Script(*b"Zyyy"))] {
                            let request = ShapeRequest {
                                direction,
                                size,
                                features: settings,
                                script,
                                language: Some("en"),
                                ..request(text, &face, 0..text.len())
                            };
                            let fresh = harfrust::ShaperData::new(&face.font_ref());
                            let expected = shape_with_data(&request, &fresh);
                            assert_eq!(HarfRust.shape(&request), expected);
                            assert_eq!(HarfRust.shape(&request), expected);
                        }
                    }
                }
            }
        }
        let text = "AV office after";
        let request = ShapeRequest {
            context: 0..text.len(),
            ..request(text, &face, 3..9)
        };
        assert_eq!(
            HarfRust.shape(&request),
            shape_with_data(&request, &harfrust::ShaperData::new(&face.font_ref()))
        );
    }

    #[test]
    fn a_different_adapter_owning_the_slot_cannot_change_output() {
        let face = Face::from_bytes(SERIF).unwrap();
        assert_eq!(face.adapter_data(|| 42_u32), Some(&42));
        let request = request("office", &face, 0..6);
        assert_eq!(
            HarfRust.shape(&request),
            shape_with_data(&request, &harfrust::ShaperData::new(&face.font_ref()))
        );
    }

    #[test]
    fn concurrent_cache_initialization_cannot_change_output() {
        let face = Face::from_bytes(SERIF).unwrap();
        let expected = shape_with_data(
            &request("office", &face, 0..6),
            &harfrust::ShaperData::new(&face.font_ref()),
        );
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| HarfRust.shape(&request("office", &face, 0..6))))
                .collect();
            for handle in handles {
                assert_eq!(handle.join().unwrap(), expected);
            }
        });
    }
}
