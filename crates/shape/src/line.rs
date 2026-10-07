//! Line-boundary bidi rules. Composition chooses boundaries; this module
//! changes only derived run levels and visual order, never authored text.

use std::ops::Range;

use reprise_diag::Note;
use unicode_bidi::data_source::BidiDataSource;
use unicode_bidi::{Level, ParagraphBidiInfo};

use crate::{ShapedRun, paragraph::codes, unicode::IcuBidi, visual_order};

/// Apply UAX #9 L1 and L2 to one composed line. `runs` must be in logical
/// order, with paragraph byte ranges and resolved levels from itemisation.
/// Pass the full paragraph text, `Itemized.levels`, the line's byte range,
/// and `Itemized.base_level`. Levels include characters without shaped runs,
/// so missing fonts cannot accidentally join separate bidi reversal groups.
/// Runs may cover more than the line and may have gaps (missing fonts).
/// The result contains clipped/split runs in visual order, leftmost first.
/// Glyphs stay in their run's visual order; when L1 changes parity their
/// order is reversed. L1 affects whitespace/controls only, not shaping.
///
/// Layout should call this *after* selecting/reshaping each line, replacing
/// its current `visual_order` call. Do not apply L1 before line breaking.
/// Empty lines succeed. Bad byte boundaries, overlapping runs, invalid levels
/// or clusters produce a `shape.bad-line` Warning, never a panic.
pub fn reorder_line(
    text: &str,
    runs: &[ShapedRun],
    paragraph_levels: &[u8],
    line: Range<usize>,
    paragraph_level: u8,
) -> Result<Vec<ShapedRun>, Note> {
    let invalid = || {
        Note::warning(
            codes::BAD_LINE,
            "invalid byte ranges, levels or glyph clusters for line reordering",
        )
        .at(line.clone())
    };
    let Some(line_text) = text.get(line.clone()) else {
        return Err(invalid());
    };
    let base = match paragraph_level {
        0 => Level::ltr(),
        1 => Level::rtl(),
        _ => return Err(invalid()),
    };
    if paragraph_levels.len() != text.len() {
        return Err(invalid());
    }
    // Validate scalar uniformity before passing byte-indexed data downstream.
    for (byte, c) in text.char_indices() {
        let Some(values) = paragraph_levels.get(byte..byte.saturating_add(c.len_utf8())) else {
            return Err(invalid());
        };
        if values
            .first()
            .is_some_and(|first| *first > 126 || values.iter().any(|l| l != first))
        {
            return Err(invalid());
        }
    }
    let Some(line_levels) = paragraph_levels.get(line.clone()) else {
        return Err(invalid());
    };
    let levels = line_levels
        .iter()
        .map(|level| Level::new(*level).map_err(|_| invalid()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut end = 0;
    for run in runs {
        if run.range.start < end
            || text.get(run.range.clone()).is_none()
            || run.level > 126
            || run.glyphs.iter().any(|g| {
                !run.range.contains(&(g.cluster as usize))
                    || !text.is_char_boundary(g.cluster as usize)
            })
        {
            return Err(invalid());
        }
        end = run.range.end;
        let Some(values) = paragraph_levels.get(run.range.clone()) else {
            return Err(invalid());
        };
        if values.iter().any(|level| *level != run.level) {
            return Err(invalid());
        }
    }
    let mut original_classes = Vec::with_capacity(line_text.len());
    for c in line_text.chars() {
        original_classes.extend(std::iter::repeat_n(IcuBidi.bidi_class(c), c.len_utf8()));
    }
    // Construct from supplied levels: no paragraph resolution is rerun here.
    // All lengths/ranges are validated before the dependency's L1 method.
    let info = ParagraphBidiInfo {
        text: line_text,
        original_classes,
        levels,
        paragraph_level: base,
        is_pure_ltr: false,
    };
    let adjusted = info.reordered_levels(0..line_text.len());
    let mut split: Vec<ShapedRun> = Vec::new();
    for run in runs {
        let range = run.range.start.max(line.start)..run.range.end.min(line.end);
        let Some(part) = text.get(range.clone()).filter(|_| !range.is_empty()) else {
            continue;
        };
        let mut pieces: Vec<(Range<usize>, u8)> = Vec::new();
        for (offset, c) in part.char_indices() {
            let byte = range.start.saturating_add(offset);
            let level = adjusted
                .get(byte.saturating_sub(line.start))
                .map_or(paragraph_level, |l| l.number());
            let end = byte.saturating_add(c.len_utf8());
            if let Some((last, _)) = pieces.last_mut().filter(|(_, previous)| *previous == level) {
                last.end = end;
            } else {
                pieces.push((byte..end, level));
            }
        }
        let mut parts: Vec<_> = pieces
            .into_iter()
            .map(|(range, level)| ShapedRun {
                upright: run.upright,
                combined: run.combined,
                horizontal_scale: run.horizontal_scale,
                range,
                level,
                glyphs: Vec::new(),
                face: run.face.clone(),
                size: run.size,
            })
            .collect();
        for glyph in &run.glyphs {
            let byte = glyph.cluster as usize;
            let index = parts.partition_point(|part| part.range.end <= byte);
            if let Some(part) = parts
                .get_mut(index)
                .filter(|part| part.range.contains(&byte))
            {
                part.glyphs.push(*glyph);
            }
        }
        for mut part in parts {
            if (part.level % 2) != (run.level % 2) && !part.upright {
                part.glyphs.reverse();
            }
            split.push(part);
        }
    }
    let mut groups: Vec<(Range<usize>, u8)> = Vec::new();
    for ((offset, c), level) in line_text.char_indices().map(|pair| {
        let level = adjusted.get(pair.0).map_or(paragraph_level, |l| l.number());
        (pair, level)
    }) {
        let start = line.start.saturating_add(offset);
        let end = start.saturating_add(c.len_utf8());
        if let Some((range, _)) = groups.last_mut().filter(|(_, last)| *last == level) {
            range.end = end;
        } else {
            groups.push((start..end, level));
        }
    }
    let order = visual_order(&groups.iter().map(|(_, level)| *level).collect::<Vec<_>>());
    let mut output = Vec::with_capacity(split.len());
    for index in order {
        let Some((range, level)) = groups.get(index) else {
            continue;
        };
        let first = split.partition_point(|r| r.range.end <= range.start);
        let last = split.partition_point(|r| r.range.start < range.end);
        let Some(parts) = split.get(first..last) else {
            return Err(invalid());
        };
        if level % 2 == 1 {
            output.extend(parts.iter().rev().cloned());
        } else {
            output.extend(parts.iter().cloned());
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShapedGlyph;
    use reprise_font::FaceId;
    use reprise_geom::Length;

    fn run(text: &str, range: Range<usize>, level: u8) -> ShapedRun {
        let mut glyphs: Vec<_> = text
            .get(range.clone())
            .unwrap()
            .char_indices()
            .map(|(i, _)| ShapedGlyph {
                id: 1,
                cluster: (range.start + i) as u32,
                advance: Length(1),
                x_offset: Length::ZERO,
                y_offset: Length::ZERO,
                unsafe_to_break: false,
                unsafe_to_concat: false,
            })
            .collect();
        if level % 2 == 1 {
            glyphs.reverse();
        }
        ShapedRun {
            upright: false,
            combined: false,
            horizontal_scale: reprise_geom::Fixed::ONE,
            range,
            level,
            glyphs,
            face: FaceId {
                family: "test".into(),
                hash: "0123456789".into(),
            },
            size: Length::from_pt(12),
        }
    }

    fn reorder_line(
        text: &str,
        runs: &[ShapedRun],
        line: Range<usize>,
        base: u8,
    ) -> Result<Vec<ShapedRun>, Note> {
        let mut levels = vec![base; text.len()];
        for run in runs {
            if let Some(values) = levels.get_mut(run.range.clone()) {
                values.fill(run.level);
            }
        }
        super::reorder_line(text, runs, &levels, line, base)
    }

    #[test]
    fn l1_splits_trailing_whitespace_and_reverses_its_glyphs() {
        let text = "abc  ";
        let runs = [run(text, 0..5, 1)];
        let result = reorder_line(text, &runs, 0..5, 0).unwrap();
        assert_eq!(
            result
                .iter()
                .map(|r| (r.range.clone(), r.level))
                .collect::<Vec<_>>(),
            [(0..3, 1), (3..5, 0)]
        );
        assert_eq!(
            result
                .iter()
                .flat_map(|r| r.glyphs.iter().map(|g| g.cluster))
                .collect::<Vec<_>>(),
            [2, 1, 0, 3, 4]
        );
        assert_eq!(runs[0].level, 1); // Pure: input has not been modified.
    }

    #[test]
    fn l1_resets_segment_separators_preceding_spaces_and_line_end() {
        let text = "abc \tdef  ";
        let result = reorder_line(text, &[run(text, 0..text.len(), 2)], 0..text.len(), 1).unwrap();
        assert_eq!(
            result
                .iter()
                .map(|r| (r.range.clone(), r.level))
                .collect::<Vec<_>>(),
            [(8..10, 1), (5..8, 2), (3..5, 1), (0..3, 2)]
        );
        assert_eq!(
            result
                .iter()
                .flat_map(|r| r.glyphs.iter().map(|g| g.cluster))
                .collect::<Vec<_>>(),
            [9, 8, 5, 6, 7, 4, 3, 0, 1, 2]
        );
        let text = "abc \u{2029}def";
        let result = reorder_line(text, &[run(text, 0..text.len(), 2)], 0..text.len(), 1).unwrap();
        assert!(result.iter().any(|r| r.range == (3..7) && r.level == 1));
    }

    #[test]
    fn l1_handles_retained_controls_isolates_and_nonzero_line_offsets() {
        let text = "prefix abc \u{2067}\u{2069}\u{202c}";
        let result = reorder_line(text, &[run(text, 0..text.len(), 1)], 7..text.len(), 0).unwrap();
        assert_eq!(
            result
                .iter()
                .map(|r| (r.range.clone(), r.level))
                .collect::<Vec<_>>(),
            [(7..10, 1), (10..text.len(), 0)]
        );
        // A trailing space on the first line resets even though more text follows.
        let result = reorder_line("abc def", &[run("abc def", 0..7, 1)], 0..4, 0).unwrap();
        assert_eq!(
            result
                .iter()
                .map(|r| (r.range.clone(), r.level))
                .collect::<Vec<_>>(),
            [(0..3, 1), (3..4, 0)]
        );
    }

    #[test]
    fn l2_keeps_run_glyphs_visual_and_does_not_split_utf8() {
        let text = "aאב12גz";
        let runs = [
            run(text, 0..1, 0),
            run(text, 1..5, 1),
            run(text, 5..7, 2),
            run(text, 7..9, 1),
            run(text, 9..10, 0),
        ];
        let result = reorder_line(text, &runs, 0..text.len(), 0).unwrap();
        assert_eq!(
            result.iter().map(|r| r.range.clone()).collect::<Vec<_>>(),
            [0..1, 7..9, 5..7, 1..5, 9..10]
        );
        assert_eq!(
            result
                .iter()
                .flat_map(|r| r.glyphs.iter().map(|g| g.cluster))
                .collect::<Vec<_>>(),
            [0, 7, 5, 6, 3, 1, 9]
        );
    }

    #[test]
    fn empty_gaps_and_malformed_requests_do_not_panic() {
        assert!(reorder_line("", &[], 0..0, 0).unwrap().is_empty());
        assert!(reorder_line("abc", &[], 0..3, 0).unwrap().is_empty());
        let text = "é";
        for range in [0..1, 0..99, Range { start: 2, end: 1 }] {
            assert_eq!(
                reorder_line(text, &[], range, 0).unwrap_err().code,
                codes::BAD_LINE
            );
        }
        assert!(reorder_line(text, &[], 0..2, 2).is_err());
        assert!(reorder_line(text, &[run(text, 0..2, 127)], 0..2, 0).is_err());
        assert!(reorder_line(text, &[run(text, 0..2, 1), run(text, 0..2, 1)], 0..2, 0).is_err());
        let mut bad_cluster = run(text, 0..2, 1);
        bad_cluster.glyphs[0].cluster = 1;
        assert!(reorder_line(text, &[bad_cluster], 0..2, 0).is_err());
    }

    #[test]
    fn missing_font_gaps_still_separate_l2_reversal_groups() {
        let text = "abc";
        let runs = [run(text, 0..1, 2), run(text, 2..3, 2)];
        let out = super::reorder_line(text, &runs, &[2, 1, 2], 0..3, 0).unwrap();
        assert_eq!(
            out.iter().map(|r| r.range.clone()).collect::<Vec<_>>(),
            [2..3, 0..1]
        );
        assert!(super::reorder_line(text, &runs, &[2, 1], 0..3, 0).is_err());
        assert!(super::reorder_line("é", &[], &[0, 1], 0..2, 0).is_err());
    }
}
