//! The `/PageLabels` of a PDF export of a document with sections (sections
//! §15). A page's label in the layout is its counter in its section's number
//! format. A PDF label is a numbering style (`/D`, `/r`, `/R`, `/a`, `/A`) and
//! a start value, so a page is written only when its label is exactly what one
//! of those styles makes of its counter. When any page isn't, no labels are
//! written at all, rather than labels that read differently in a viewer than in
//! the document, and the export says why.

use reprise_display::pdf::tags::{LabelStyle, PageLabel};
use reprise_layout::LayoutSnapshot;

/// The styles tried, in order: the first that writes the page's label wins.
const STYLES: [LabelStyle; 5] = [
    LabelStyle::Decimal,
    LabelStyle::LowerRoman,
    LabelStyle::UpperRoman,
    LabelStyle::LowerAlpha,
    LabelStyle::UpperAlpha,
];

/// One label per page, `Ok(None)` when the layout has no sections (a plain
/// document writes no labels), or why the labels can't be written.
pub(crate) fn labels(layout: &LayoutSnapshot) -> Result<Option<Vec<PageLabel>>, String> {
    let Some(sections) = layout.sections.as_ref() else {
        return Ok(None);
    };
    if sections.pages.len() != layout.pages.len() {
        return Err("the layout's page descriptors don't match its pages".into());
    }
    let mut out = Vec::with_capacity(sections.pages.len());
    for (index, page) in sections.pages.iter().enumerate() {
        let number = u32::try_from(page.counter)
            .ok()
            .filter(|n| *n >= 1)
            .ok_or_else(|| {
                format!(
                    "page {} has the number {}, and a PDF page label starts at 1 or more",
                    index + 1,
                    page.counter
                )
            })?;
        let style = STYLES
            .into_iter()
            .find(|style| written(*style, number).as_deref() == Some(page.label.as_str()))
            .ok_or_else(|| {
                format!(
                    "page {} is labelled {:?}, which no PDF numbering style writes for {number}",
                    index + 1,
                    page.label
                )
            })?;
        out.push(PageLabel { style, number });
    }
    Ok(Some(out))
}

/// What a viewer shows for page number `n` in `style` (ISO 32000-1, 12.4.2).
/// `None` where a viewer's reading is not worth relying on (a roman numeral
/// past 3,999).
fn written(style: LabelStyle, n: u32) -> Option<String> {
    match style {
        LabelStyle::Decimal => Some(n.to_string()),
        LabelStyle::LowerRoman => roman(n).map(|s| s.to_lowercase()),
        LabelStyle::UpperRoman => roman(n),
        LabelStyle::LowerAlpha => Some(letters(n, false)),
        LabelStyle::UpperAlpha => Some(letters(n, true)),
    }
}

fn roman(n: u32) -> Option<String> {
    if !(1..=3999).contains(&n) {
        return None;
    }
    const TABLE: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut rest = n;
    let mut out = String::new();
    for (value, text) in TABLE {
        while rest >= value {
            out.push_str(text);
            rest -= value;
        }
    }
    Some(out)
}

/// `a`..`z`, then `aa`..`zz`, then `aaa`..: the letter repeats, it does not
/// carry (PDF's scheme, not a base-26 one).
fn letters(n: u32, upper: bool) -> String {
    let index = n.saturating_sub(1);
    let letter = char::from(b'a' + u8::try_from(index % 26).unwrap_or(0));
    let letter = if upper {
        letter.to_ascii_uppercase()
    } else {
        letter
    };
    (0..index / 26 + 1).map(|_| letter).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_write_what_a_viewer_shows() {
        assert_eq!(written(LabelStyle::Decimal, 12).unwrap(), "12");
        assert_eq!(written(LabelStyle::LowerRoman, 4).unwrap(), "iv");
        assert_eq!(written(LabelStyle::UpperRoman, 1994).unwrap(), "MCMXCIV");
        assert_eq!(written(LabelStyle::LowerAlpha, 1).unwrap(), "a");
        assert_eq!(written(LabelStyle::LowerAlpha, 26).unwrap(), "z");
        // The letter repeats; it does not carry.
        assert_eq!(written(LabelStyle::LowerAlpha, 27).unwrap(), "aa");
        assert_eq!(written(LabelStyle::UpperAlpha, 54).unwrap(), "BBB");
        assert_eq!(written(LabelStyle::UpperRoman, 4000), None);
        assert_eq!(written(LabelStyle::UpperRoman, 0), None);
    }
}
