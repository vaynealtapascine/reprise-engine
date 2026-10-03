//! Paragraph shaping: itemisation, font fallback and reshaping (21, 22).

use std::ops::Range;

use reprise_diag::Note;
use reprise_font::FontStore;
use reprise_geom::{InlineDirection, Length};

use crate::{Feature, Item, Reshape, ShapeRequest, ShapedRun, ShapedText, ShapingAdapter, unicode};

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
    /// Script bracket matching exceeded its bounded stack; contextual script
    /// resolution continues but pairing stops for the rest of the paragraph.
    pub const SCRIPT_DEPTH: Code = Code::new("shape.script-depth");
    /// Invalid input to line reordering was rejected without changing output.
    pub const BAD_LINE: Code = Code::new("shape.bad-line");
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
    /// Resolved UAX #9 levels before L1, one per UTF-8 byte, including text
    /// without a styled/available face. Retain this for line reordering and
    /// caret/navigation mappings; all bytes of a scalar have the same level.
    pub levels: Vec<u8>,
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
/// Bidi resolves UAX #9 through I2 with ICU4X properties. Line owners apply
/// L1/L2 with [`crate::reorder_line`] after composition. Scripts follow UAX #24
/// contextual runs; all-Common/Inherited text uses ISO 15924 `Zyyy`.
pub fn itemize(input: &ParagraphInput<'_>, fonts: &FontStore) -> Itemized {
    let text = input.text;
    let bidi = unicode::bidi(
        text,
        input.direction.map(|direction| match direction {
            InlineDirection::Ltr => unicode_bidi::Level::ltr(),
            InlineDirection::Rtl => unicode_bidi::Level::rtl(),
        }),
    );
    let base_level = bidi.paragraph_level.number();
    let mut out = Itemized {
        base_level,
        levels: bidi.levels.iter().map(|level| level.number()).collect(),
        ..Itemized::default()
    };
    let scripts = unicode::scripts(text, &mut out.notes);
    let resolved: Vec<_> = text
        .char_indices()
        .zip(scripts)
        .map(|((byte, c), script)| {
            (
                byte..byte.saturating_add(c.len_utf8()),
                bidi.levels
                    .get(byte)
                    .map_or(base_level, |level| level.number()),
                script,
            )
        })
        .collect();
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
        let first = resolved.partition_point(|(range, _, _)| range.start < r.start);
        for (range, level, script) in resolved
            .iter()
            .skip(first)
            .take_while(|(range, _, _)| range.start < r.end)
        {
            if let Some(last) = out.items.last_mut().filter(|last| {
                last.range.start >= r.start
                    && last.range.end == range.start
                    && last.level == *level
                    && last.script == Some(*script)
            }) {
                last.range.end = range.end;
                continue;
            }
            out.items.push(Item {
                range: range.clone(),
                face: face.id().clone(),
                size: run.size,
                level: *level,
                script: Some(*script),
                language: run.language.clone(),
                features: run.features.clone(),
            });
        }
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

    #[test]
    fn items_expose_resolved_bidi_levels_and_scripts() {
        let fonts = fonts();
        let text = "a \u{202e}office\u{202c} α\u{301};Б";
        let styles = [run(0..text.len(), &["Source Serif Pro"])];
        let out = itemize(
            &ParagraphInput {
                text,
                styles: &styles,
                direction: None,
            },
            &fonts,
        );
        assert_eq!(out.base_level, 0);
        let office = text.find("office").unwrap();
        assert!(out.items.iter().any(|i| i.range.contains(&office)
            && i.level == 1
            && i.script == Some(crate::Script(*b"Latn"))));
        let greek = text.find('α').unwrap();
        let item = out.items.iter().find(|i| i.range.contains(&greek)).unwrap();
        assert_eq!(item.script, Some(crate::Script(*b"Grek")));
        assert!(item.range.end >= greek + "α\u{301};".len());
        assert!(out.items.iter().all(|i| i.script.is_some()));
        for pair in out.items.windows(2) {
            assert_eq!(pair[0].range.end, pair[1].range.start);
        }
        assert_eq!(out.items.last().unwrap().range.end, text.len());
    }

    #[test]
    fn automatic_base_direction_ignores_isolated_strong_text() {
        let fonts = fonts();
        for (text, direction, expected_base, expected_latin) in [
            ("abc", Some(InlineDirection::Rtl), 1, 2),
            ("abc", Some(InlineDirection::Ltr), 0, 0),
            ("אabc", None, 1, 2),
            ("\u{2067}א\u{2069}abc", None, 0, 0),
            ("123", None, 0, 0),
            ("", None, 0, 0),
        ] {
            let styles = [run(0..text.len(), &["Source Serif Pro"])];
            let out = itemize(
                &ParagraphInput {
                    text,
                    styles: &styles,
                    direction,
                },
                &fonts,
            );
            assert_eq!(out.base_level, expected_base);
            if let Some(byte) = text.find('a') {
                assert_eq!(
                    out.items
                        .iter()
                        .find(|i| i.range.contains(&byte))
                        .unwrap()
                        .level,
                    expected_latin
                );
            }
        }
    }

    #[test]
    fn font_size_and_style_boundaries_remain_item_boundaries() {
        let fonts = fonts();
        let styles = [
            run(0..2, &["Source Serif Pro"]),
            StyleRun {
                size: Length::MAX,
                ..run(2..4, &["Source Serif Pro"])
            },
            run(4..6, &["Source Serif Pro"]),
        ];
        let out = itemize(
            &ParagraphInput {
                text: "abcdef",
                styles: &styles,
                direction: None,
            },
            &fonts,
        );
        assert_eq!(
            out.items
                .iter()
                .map(|i| i.range.clone())
                .collect::<Vec<_>>(),
            [0..2, 2..4, 4..6]
        );
        assert_eq!(out.items[1].size, Length::MAX);
    }

    /// Orchestrator review: every line split of real mixed-direction text,
    /// reordered with `reorder_line`, keeps each glyph exactly once, stays
    /// inside the line, never overlaps, and keeps odd-level runs right to left.
    #[test]
    fn reordering_every_line_split_conserves_glyphs() {
        let fonts = fonts();
        for text in [
            "a \u{202e}office  ab\u{202c} c ",
            "\u{2067}x (y) 12\u{2069}  z\u{2066}",
            "\u{202b}\u{202b}a b\u{202c} c\u{202c}\t\u{2029}d",
            " e\u{301}  \u{202e}fi\u{202c} ",
        ] {
            for direction in [None, Some(InlineDirection::Rtl)] {
                let styles = [run(0..text.len(), &["Source Serif Pro"])];
                let itemized = itemize(
                    &ParagraphInput {
                        text,
                        styles: &styles,
                        direction,
                    },
                    &fonts,
                );
                let shaped = Shaper {
                    text,
                    items: &itemized.items,
                    fonts: &fonts,
                    adapter: &HarfRust,
                }
                .shape();
                let bounds: Vec<usize> = text
                    .char_indices()
                    .map(|(i, _)| i)
                    .chain([text.len()])
                    .collect();
                for &start in &bounds {
                    for &end in bounds.iter().filter(|&&e| e >= start) {
                        let line = start..end;
                        let runs = shaped.slice(line.clone());
                        let out = crate::reorder_line(
                            text,
                            &runs,
                            &itemized.levels,
                            line.clone(),
                            itemized.base_level,
                        )
                        .unwrap_or_else(|n| panic!("{text:?} {line:?}: {}", n.message));
                        let mut want: Vec<u32> = runs
                            .iter()
                            .flat_map(|r| &r.glyphs)
                            .map(|g| g.cluster)
                            .collect();
                        let mut got: Vec<u32> = out
                            .iter()
                            .flat_map(|r| &r.glyphs)
                            .map(|g| g.cluster)
                            .collect();
                        want.sort_unstable();
                        got.sort_unstable();
                        assert_eq!(got, want, "{text:?} {line:?}");
                        let mut ranges: Vec<_> = out.iter().map(|r| r.range.clone()).collect();
                        ranges.sort_by_key(|r| r.start);
                        for pair in ranges.windows(2) {
                            assert!(pair[0].end <= pair[1].start, "{text:?} {line:?}");
                        }
                        for r in &out {
                            assert!(line.start <= r.range.start && r.range.end <= line.end);
                            let clusters: Vec<u32> = r.glyphs.iter().map(|g| g.cluster).collect();
                            let ordered = if r.level % 2 == 1 {
                                clusters.windows(2).all(|w| w[0] >= w[1])
                            } else {
                                clusters.windows(2).all(|w| w[0] <= w[1])
                            };
                            assert!(ordered, "{text:?} {line:?} level {}", r.level);
                        }
                    }
                }
            }
        }
    }
}
