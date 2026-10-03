//! Paragraph shaping: itemisation, font fallback and reshaping (21, 22).

use std::ops::Range;

use reprise_diag::Note;
use reprise_font::FontStore;
use reprise_geom::{InlineDirection, Length};

use crate::{Feature, Item, Reshape, ShapeRequest, ShapedRun, ShapedText, ShapingAdapter};

/// Diagnostic codes reported while shaping.
pub mod codes {
    use reprise_diag::Code;

    /// A family in a fallback chain was unavailable and a later one was used.
    pub const FONT_FALLBACK: Code = Code::new("font.fallback");
    /// No family in the fallback chain was available; the text has no glyphs.
    pub const FONT_MISSING: Code = Code::new("font.missing");
    /// A style run was out of bounds, reversed, overlapping or not on
    /// character boundaries, and was ignored.
    pub const BAD_STYLE_RUN: Code = Code::new("shape.bad-style-run");
}

/// The styling of part of a paragraph, as itemisation needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleRun {
    pub range: Range<usize>,
    /// The fallback chain: families in order of preference (21).
    pub families: Vec<String>,
    pub size: Length,
    pub language: Option<String>,
    pub features: Vec<Feature>,
}

pub struct ParagraphInput<'a> {
    pub text: &'a str,
    /// In order and not overlapping. Text outside every run is not shaped.
    pub styles: &'a [StyleRun],
    /// The paragraph direction, or `None` to take it from the text (UAX #9
    /// rules P2 and P3).
    pub direction: Option<InlineDirection>,
}

/// The result of [`itemize`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Itemized {
    /// In logical order, not overlapping.
    pub items: Vec<Item>,
    /// The paragraph's base bidi level: 0 for left to right, 1 for right to left.
    pub base_level: u8,
    /// Substitutions and anything that couldn't be itemised.
    pub notes: Vec<Note>,
}

/// Splits a paragraph into items with one face, size, bidi level and script.
///
/// Every character inside a style run is in exactly one item, unless no face
/// in its fallback chain is available; then it is in none and a
/// [`codes::FONT_MISSING`] note says so. Each use of a fallback family is
/// reported with [`codes::FONT_FALLBACK`] (21).
///
/// Not yet implemented: the bidi algorithm and script itemisation. Every item
/// takes the paragraph's base level, and its script is left to the adapter.
pub fn itemize(input: &ParagraphInput<'_>, fonts: &FontStore) -> Itemized {
    let text = input.text;
    let base_level = match input.direction {
        Some(InlineDirection::Rtl) => 1,
        _ => 0,
    };
    let mut out = Itemized {
        base_level,
        ..Itemized::default()
    };
    let mut end_of_previous = 0;
    for run in input.styles {
        let r = run.range.clone();
        let valid = r.start <= r.end
            && r.start >= end_of_previous
            && text.is_char_boundary(r.start)
            && text.is_char_boundary(r.end)
            && r.end <= text.len();
        if !valid {
            out.notes.push(
                Note::warning(
                    codes::BAD_STYLE_RUN,
                    format!(
                        "style run {r:?} doesn't fit the {} bytes of text",
                        text.len()
                    ),
                )
                .at(r),
            );
            continue;
        }
        end_of_previous = r.end;
        if r.is_empty() {
            continue;
        }
        let Some((index, face)) = run
            .families
            .iter()
            .enumerate()
            .find_map(|(i, f)| fonts.by_family(f).map(|face| (i, face)))
        else {
            out.notes.push(
                Note::error(
                    codes::FONT_MISSING,
                    format!("no face for any of {:?}; text not shaped", run.families),
                )
                .at(r),
            );
            continue;
        };
        if index > 0 {
            out.notes.push(
                Note::warning(
                    codes::FONT_FALLBACK,
                    format!(
                        "{:?} unavailable; used {:?}",
                        &run.families[..index],
                        run.families[index]
                    ),
                )
                .at(r.clone()),
            );
        }
        out.items.push(Item {
            range: r,
            face: face.id().clone(),
            size: run.size,
            level: base_level,
            script: None,
            language: run.language.clone(),
            features: run.features.clone(),
        });
    }
    out
}

/// Shapes a paragraph's items with an adapter, and reshapes lines of it.
pub struct Shaper<'a> {
    pub text: &'a str,
    pub items: &'a [Item],
    pub fonts: &'a FontStore,
    pub adapter: &'a dyn ShapingAdapter,
}

impl Shaper<'_> {
    /// Shapes every item, with the whole paragraph as context.
    pub fn shape(&self) -> ShapedText {
        ShapedText {
            runs: self.shape_within(0..self.text.len(), 0..self.text.len()),
        }
    }

    /// Shapes the part of each item inside `range`, letting the shaper see
    /// `context` around it.
    fn shape_within(&self, range: Range<usize>, context: Range<usize>) -> Vec<ShapedRun> {
        self.items
            .iter()
            .filter_map(|item| {
                let part = item.range.start.max(range.start)..item.range.end.min(range.end);
                if part.is_empty() {
                    return None;
                }
                let face = self.fonts.get(&item.face).ok()?;
                let glyphs = self.adapter.shape(&ShapeRequest {
                    text: self.text,
                    range: part.clone(),
                    context: context.clone(),
                    face,
                    size: item.size,
                    direction: item.direction(),
                    script: item.script,
                    language: item.language.as_deref(),
                    features: &item.features,
                });
                Some(ShapedRun {
                    range: part,
                    face: item.face.clone(),
                    size: item.size,
                    level: item.level,
                    glyphs,
                })
            })
            .collect()
    }
}

impl Reshape for Shaper<'_> {
    fn reshape(&self, range: Range<usize>) -> Vec<ShapedRun> {
        self.shape_within(range.clone(), range)
    }
}

#[cfg(test)]
mod tests {
    use reprise_font::Face;

    use super::*;
    use crate::HarfRust;

    const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");

    fn fonts() -> FontStore {
        let mut fonts = FontStore::default();
        fonts.add(Face::from_bytes(SERIF).unwrap());
        fonts
    }

    fn run(range: Range<usize>, families: &[&str]) -> StyleRun {
        StyleRun {
            range,
            families: families.iter().map(|f| f.to_string()).collect(),
            size: Length::from_pt(10),
            language: None,
            features: Vec::new(),
        }
    }

    fn codes_of(notes: &[Note]) -> Vec<&str> {
        notes.iter().map(|n| n.code.as_str()).collect()
    }

    #[test]
    fn fallback_chains_pick_the_first_available_family_and_report_it() {
        let fonts = fonts();
        let text = "plain";
        let styles = [run(0..5, &["Nonexistent Sans", "Source Serif Pro"])];
        let out = itemize(
            &ParagraphInput {
                text,
                styles: &styles,
                direction: None,
            },
            &fonts,
        );
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].face.family, "Source Serif Pro");
        assert_eq!(codes_of(&out.notes), ["font.fallback"]);
    }

    #[test]
    fn text_with_no_available_face_is_left_out_and_reported() {
        let fonts = fonts();
        let text = "ab";
        let styles = [
            run(0..1, &["Nonexistent"]),
            run(1..2, &["Source Serif Pro"]),
        ];
        let out = itemize(
            &ParagraphInput {
                text,
                styles: &styles,
                direction: None,
            },
            &fonts,
        );
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].range, 1..2);
        assert_eq!(codes_of(&out.notes), ["font.missing"]);
        assert_eq!(out.notes[0].bytes, Some(0..1));
    }

    #[test]
    fn bad_style_runs_are_ignored_not_fatal() {
        let fonts = fonts();
        let text = "é!";
        let styles = [
            run(0..1, &["Source Serif Pro"]), // inside the é
            run(Range { start: 2, end: 1 }, &["Source Serif Pro"]), // reversed
            run(0..99, &["Source Serif Pro"]), // past the end
            run(2..3, &["Source Serif Pro"]), // fine
            run(0..2, &["Source Serif Pro"]), // overlaps the previous one
        ];
        let out = itemize(
            &ParagraphInput {
                text,
                styles: &styles,
                direction: None,
            },
            &fonts,
        );
        assert_eq!(out.items.len(), 1);
        assert_eq!(codes_of(&out.notes), ["shape.bad-style-run"; 4]);
    }

    #[test]
    fn reshaping_a_line_matches_shaping_it_alone() {
        let fonts = fonts();
        let text = "one two three";
        let styles = [run(0..text.len(), &["Source Serif Pro"])];
        let items = itemize(
            &ParagraphInput {
                text,
                styles: &styles,
                direction: None,
            },
            &fonts,
        )
        .items;
        let shaper = Shaper {
            text,
            items: &items,
            fonts: &fonts,
            adapter: &HarfRust,
        };
        let line = shaper.reshape(4..7);
        assert_eq!(line.len(), 1);
        assert_eq!(line[0].range, 4..7);
        let alone = Shaper {
            text: "two",
            items: &[Item {
                range: 0..3,
                ..items[0].clone()
            }],
            fonts: &fonts,
            adapter: &HarfRust,
        }
        .shape();
        let ids = |runs: &[ShapedRun]| -> Vec<u32> {
            runs.iter()
                .flat_map(|r| r.glyphs.iter().map(|g| g.id))
                .collect()
        };
        assert_eq!(ids(&line), ids(&alone.runs));
        assert_eq!(shaper.shape().width(4..7), line[0].width());
    }
}
