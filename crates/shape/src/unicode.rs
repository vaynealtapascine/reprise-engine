//! Pinned ICU4X properties for UAX #9 and UAX #24.

use icu_properties::props::{
    BidiClass as IcuBidiClass, BidiMirroringGlyph, BidiPairedBracketType, Script as IcuScript,
};
use icu_properties::{CodePointMapData, PropertyNamesShort};
use reprise_diag::Note;
use unicode_bidi::data_source::{BidiDataSource, BidiMatchedOpeningBracket};
use unicode_bidi::{BidiClass, Level, ParagraphBidiInfo};

use crate::{Script, paragraph::codes};

/// Unicode properties bundled by icu_properties_data 2.3.0 (ICU 78.1rc).
/// Together with Cargo.lock, this is part of the reproducibility envelope.
pub const UNICODE_VERSION: &str = "17.0.0";

pub(crate) struct IcuBidi;

impl BidiDataSource for IcuBidi {
    fn bidi_class(&self, c: char) -> BidiClass {
        CodePointMapData::<IcuBidiClass>::new().get(c).into()
    }

    // Override the trait's default: bracket properties must come from the same
    // ICU data as classes, not unicode-bidi's separate hardcoded Unicode data.
    fn bidi_matched_opening_bracket(&self, c: char) -> Option<BidiMatchedOpeningBracket> {
        let property = CodePointMapData::<BidiMirroringGlyph>::new().get(c);
        let (opening, is_open) = match property.paired_bracket_type {
            BidiPairedBracketType::Open => (c, true),
            BidiPairedBracketType::Close => (property.mirroring_glyph?, false),
            _ => return None,
        };
        // The only canonically equivalent bracket pair in BidiBrackets.txt.
        let opening = if opening == '\u{2329}' {
            '\u{3008}'
        } else {
            opening
        };
        Some(BidiMatchedOpeningBracket { opening, is_open })
    }
}

pub(crate) fn bidi(text: &str, base: Option<Level>) -> ParagraphBidiInfo<'_> {
    ParagraphBidiInfo::new_with_data_source(&IcuBidi, text, base)
}

const MAX_SCRIPT_BRACKETS: usize = 63;

/// Common/Inherited adopt the preceding script, or the following script at
/// the start; an all-Common/Inherited paragraph uses Zyyy. Matching closing
/// brackets use their opening's resolved script, including leading brackets.
/// This is the contextual script-run convention, not language detection.
pub(crate) fn scripts(text: &str, notes: &mut Vec<Note>) -> Vec<Script> {
    let properties = CodePointMapData::<IcuScript>::new();
    let chars: Vec<_> = text.char_indices().collect();
    let mut resolved = Vec::with_capacity(chars.len());
    let mut preceding = None;
    for &(_, c) in &chars {
        let script = properties.get(c);
        if script != IcuScript::Common && script != IcuScript::Inherited {
            preceding = Some(script);
        }
        resolved.push(preceding);
    }
    let mut following = IcuScript::Common;
    for script in resolved.iter_mut().rev() {
        if let Some(value) = script {
            following = *value;
        } else {
            *script = Some(following);
        }
    }
    let mut brackets: Vec<(char, IcuScript)> = Vec::new();
    let mut pairing = true;
    let mut current = IcuScript::Common;
    for ((byte, c), script) in chars.iter().copied().zip(&mut resolved) {
        let raw = properties.get(c);
        // A matched close also becomes the preceding script for punctuation
        // after it. Strong characters always start their own script run.
        if (raw == IcuScript::Common || raw == IcuScript::Inherited) && current != IcuScript::Common
        {
            *script = Some(current);
        }
        if let Some(bracket) = IcuBidi.bidi_matched_opening_bracket(c).filter(|_| pairing) {
            if bracket.is_open {
                if brackets.len() == MAX_SCRIPT_BRACKETS {
                    // Disable pairing for the rest of this paragraph rather
                    // than accidentally matching an ignored opening's close.
                    pairing = false;
                    brackets.clear();
                    notes.push(Note::warning(codes::SCRIPT_DEPTH,
                        "script bracket nesting exceeds 63; remaining brackets use surrounding scripts")
                        .at(byte..byte.saturating_add(c.len_utf8())));
                } else {
                    brackets.push((bracket.opening, script.unwrap_or(IcuScript::Common)));
                }
            } else if let Some(index) = brackets
                .iter()
                .rposition(|(open, _)| *open == bracket.opening)
            {
                if let Some((_, opening_script)) = brackets.get(index) {
                    *script = Some(*opening_script);
                }
                brackets.truncate(index);
            }
        }
        current = script.unwrap_or(IcuScript::Common);
    }
    let names = PropertyNamesShort::<IcuScript>::new();
    resolved
        .into_iter()
        .map(|script| {
            let tag = names
                .get(script.unwrap_or(IcuScript::Common))
                .and_then(|name| name.as_bytes().try_into().ok())
                .unwrap_or(*b"Zzzz");
            Script(tag)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visual_order;

    #[test]
    fn unicode_17_bidi_character_test_subset() {
        let mut count = 0;
        for row in include_str!("bidi-character-subset.txt")
            .lines()
            .filter(|s| !s.starts_with('#') && !s.is_empty())
        {
            let fields: Vec<_> = row.split(';').collect();
            let text: String = fields[0]
                .split_whitespace()
                .map(|hex| char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap())
                .collect();
            let base = match fields[1] {
                "0" => Some(Level::ltr()),
                "1" => Some(Level::rtl()),
                "2" => None,
                _ => panic!("bad test row"),
            };
            let info = bidi(&text, base);
            assert_eq!(
                info.paragraph_level.number(),
                fields[2].parse::<u8>().unwrap(),
                "{row}"
            );
            let adjusted = info.reordered_levels(0..text.len());
            let expected: Vec<_> = fields[3].split_whitespace().collect();
            let mut active_levels = Vec::new();
            let mut active_indices = Vec::new();
            for (((byte, _), expected), index) in text.char_indices().zip(expected).zip(0..) {
                if expected == "x" {
                    continue;
                }
                let level = expected.parse::<u8>().unwrap();
                assert_eq!(adjusted[byte].number(), level, "character {index}: {row}");
                active_levels.push(level);
                active_indices.push(index);
            }
            let actual: Vec<_> = visual_order(&active_levels)
                .into_iter()
                .map(|i| active_indices[i])
                .collect();
            let expected: Vec<usize> = fields[4]
                .split_whitespace()
                .map(|s| s.parse().unwrap())
                .collect();
            assert_eq!(actual, expected, "{row}");
            // Exercise the public helper as well, with X9 characters omitted
            // from shaped output but retained in the paragraph-level data.
            let bytes: Vec<_> = text.char_indices().map(|(byte, _)| byte).collect();
            let runs: Vec<_> = text
                .char_indices()
                .enumerate()
                .filter(|(index, _)| active_indices.contains(index))
                .map(|(_, (byte, c))| crate::ShapedRun {
                    upright: false,
                    combined: false,
                    horizontal_scale: reprise_geom::Fixed::ONE,
                    range: byte..byte + c.len_utf8(),
                    face: reprise_font::FaceId {
                        family: "conformance".into(),
                        hash: "0123456789".into(),
                    },
                    size: reprise_geom::Length(1),
                    level: info.levels[byte].number(),
                    glyphs: vec![crate::ShapedGlyph {
                        id: 1,
                        cluster: byte as u32,
                        advance: reprise_geom::Length(1),
                        x_offset: reprise_geom::Length::ZERO,
                        y_offset: reprise_geom::Length::ZERO,
                        unsafe_to_break: false,
                        unsafe_to_concat: false,
                    }],
                })
                .collect();
            let paragraph_levels: Vec<_> = info.levels.iter().map(|l| l.number()).collect();
            let visual = crate::reorder_line(
                &text,
                &runs,
                &paragraph_levels,
                0..text.len(),
                info.paragraph_level.number(),
            )
            .unwrap();
            let actual: Vec<_> = visual
                .iter()
                .flat_map(|r| &r.glyphs)
                .map(|g| bytes.binary_search(&(g.cluster as usize)).unwrap())
                .collect();
            assert_eq!(actual, expected, "line helper: {row}");
            count += 1;
        }
        assert_eq!(count, 38);
    }

    #[test]
    fn controls_overflow_and_unbalanced_input_are_bounded() {
        for (open, character, expected) in [
            ("\u{202b}", 'a', 126),
            ("\u{202e}", 'a', 125),
            ("\u{2067}", 'a', 126),
        ] {
            let text = format!(
                "{}{}{}z",
                open.repeat(2000),
                character,
                "\u{202c}".repeat(2000)
            );
            let info = bidi(&text, Some(Level::ltr()));
            let at = open.len() * 2000;
            assert_eq!(info.levels[at].number(), expected);
            assert!(info.levels.iter().all(|l| l.number() <= 126));
        }
        let text = "\u{202c}\u{2069}abc \u{2067}unterminated";
        let info = bidi(text, None);
        assert_eq!(info.paragraph_level.number(), 0);
        assert_eq!(info.levels.last().unwrap().number(), 2);
        assert_eq!(
            bidi("\u{2067}א\u{2069}abc", None).paragraph_level.number(),
            0
        );
        assert_eq!(bidi("123 ()", None).paragraph_level.number(), 0);
        assert_eq!(bidi("א abc", None).paragraph_level.number(), 1);
        assert_eq!(bidi("", None).paragraph_level.number(), 0);
    }

    fn tags(text: &str) -> Vec<Script> {
        scripts(text, &mut Vec::new())
    }

    #[test]
    fn contextual_scripts_and_brackets() {
        let latin = Script(*b"Latn");
        let greek = Script(*b"Grek");
        let cyrillic = Script(*b"Cyrl");
        assert_eq!(
            tags(". a,α;Б"),
            [latin, latin, latin, latin, greek, greek, cyrillic]
        );
        assert_eq!(
            tags("\u{301}α\u{301}a\u{301}"),
            [greek, greek, greek, latin, latin]
        );
        assert_eq!(
            tags("a(α[Б])!α"),
            [
                latin, latin, greek, greek, cyrillic, greek, latin, latin, greek
            ]
        );
        assert_eq!(tags("(α)a"), [greek, greek, greek, latin]);
        assert_eq!(tags("123 ,\u{301}"), vec![Script(*b"Zyyy"); 6]);
        assert!(tags("").is_empty());
        assert_eq!(tags("a\u{2329}α\u{3009}"), [latin, latin, greek, latin]);
    }

    #[test]
    fn script_pair_stack_has_an_explicit_reported_bound() {
        let mut notes = Vec::new();
        let text = format!("a{}α{}", "(".repeat(10000), ")".repeat(10000));
        let scripts = scripts(&text, &mut notes);
        assert_eq!(scripts.len(), text.chars().count());
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].code, codes::SCRIPT_DEPTH);
        assert_eq!(notes[0].severity, reprise_diag::Severity::Warning);
    }
}
