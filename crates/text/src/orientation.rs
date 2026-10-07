//! UAX #50 `Vertical_Orientation` (decisions 20 and 38): how a character sits
//! in a vertical line by default.
//!
//! The data is a pinned table generated from the Unicode Character Database
//! by `generate_orientation.py`, for [`UNICODE_VERSION`], the version ICU4X
//! pins for the rest of the engine. A test compares every code point with
//! ICU4X's compiled data, so the two can't drift apart.

use serde::{Deserialize, Serialize};

#[path = "orientation_table.rs"]
mod table;

/// The Unicode version of the orientation data. Part of the reproducibility
/// envelope, like the shaping crate's bidi and script data.
pub const UNICODE_VERSION: &str = table::UNICODE_VERSION;

/// A character's `Vertical_Orientation` value (UAX #50).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerticalOrientation {
    /// `U`: upright, as in the code charts.
    Upright,
    /// `R`: rotated 90° clockwise.
    Rotated,
    /// `Tu`: a vertical alternate glyph if the font has one, else upright.
    TransformedUpright,
    /// `Tr`: a vertical alternate glyph if the font has one, else rotated.
    TransformedRotated,
}

impl VerticalOrientation {
    /// The property's short value alias, as in the UCD file.
    pub fn alias(self) -> &'static str {
        match self {
            VerticalOrientation::Upright => "U",
            VerticalOrientation::Rotated => "R",
            VerticalOrientation::TransformedUpright => "Tu",
            VerticalOrientation::TransformedRotated => "Tr",
        }
    }
}

/// The `Vertical_Orientation` of `c`. Unlisted code points are `R`, the
/// file's `@missing` default.
pub fn vertical_orientation(c: char) -> VerticalOrientation {
    let cp = u32::from(c);
    let at = table::RANGES.partition_point(|&(_, hi, _)| hi < cp);
    match table::RANGES.get(at) {
        Some(&(lo, _, value)) if lo <= cp => value,
        _ => VerticalOrientation::Rotated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use icu_properties::CodePointMapData;
    use icu_properties::props::VerticalOrientation as Icu;

    #[test]
    fn the_table_is_sorted_disjoint_and_merged() {
        for pair in table::RANGES.windows(2) {
            let ((_, hi, a), (lo, _, b)) = (pair[0], pair[1]);
            assert!(hi < lo, "{hi:X} {lo:X}");
            assert!(hi + 1 < lo || a != b, "unmerged at {hi:X}");
        }
        assert!(
            table::RANGES
                .iter()
                .all(|&(lo, hi, v)| lo <= hi && v != VerticalOrientation::Rotated)
        );
    }

    #[test]
    fn every_code_point_matches_the_pinned_icu4x_data() {
        assert_eq!(UNICODE_VERSION, "17.0.0");
        let icu = CodePointMapData::<Icu>::new();
        for cp in 0..=0x10FFFF_u32 {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let expected = match icu.get(c) {
                Icu::Upright => VerticalOrientation::Upright,
                Icu::TransformedUpright => VerticalOrientation::TransformedUpright,
                Icu::TransformedRotated => VerticalOrientation::TransformedRotated,
                _ => VerticalOrientation::Rotated,
            };
            assert_eq!(vertical_orientation(c), expected, "U+{cp:04X}");
        }
    }

    #[test]
    fn familiar_characters() {
        use VerticalOrientation::*;
        for (c, v) in [
            ('A', Rotated),
            ('1', Rotated),
            ('א', Rotated),
            ('紙', Upright),
            ('あ', Upright),
            ('ア', Upright),
            ('、', TransformedUpright),
            ('。', TransformedUpright),
            ('ー', TransformedRotated),
            ('「', TransformedRotated),
            ('（', TransformedRotated),
            ('Ａ', Upright),
            ('\u{E000}', Upright),
            ('\u{10FFFF}', Rotated),
            ('\0', Rotated),
        ] {
            assert_eq!(vertical_orientation(c), v, "{c:?}");
            assert!(matches!(v.alias(), "U" | "R" | "Tu" | "Tr"));
        }
    }
}
