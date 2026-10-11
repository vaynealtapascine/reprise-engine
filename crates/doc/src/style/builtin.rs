//! Built-in named styles: engine data, never written to the document.
//!
//! A built-in applies to a name only while the document has no nested
//! definition of it (`styles` root), so documents that defined their own
//! `Heading 1` before built-ins existed lay out exactly as they did. Flat
//! `styles1` patches overlay a built-in property by property, unless the
//! document *defined* the name, which replaces it (see `store`).
//!
//! Sizes and spacing are in `em`, so they follow whatever base size the
//! document has. `Normal`'s parent is the document base (the empty name);
//! every other built-in's parent is `Normal`.

use crate::{LengthExpr, Property, Style, TextSlant};

/// The built-in names, in menu order.
pub const BUILTIN_STYLES: [&str; 9] = [
    "Normal",
    "Title",
    "Subtitle",
    "Heading 1",
    "Heading 2",
    "Heading 3",
    "Heading 4",
    "Heading 5",
    "Heading 6",
];

/// `(size, space before, space after)` in thousandths of an em, and whether
/// the style is bold and italic.
fn metrics(name: &str) -> Option<(i32, i32, i32, bool, bool)> {
    Some(match name {
        "Title" => (2600, 0, 300, false, false),
        "Subtitle" => (1500, 0, 800, false, false),
        "Heading 1" => (2000, 1000, 300, true, false),
        "Heading 2" => (1600, 900, 300, true, false),
        "Heading 3" => (1300, 800, 250, true, false),
        "Heading 4" => (1150, 700, 200, true, false),
        "Heading 5" => (1050, 600, 200, true, false),
        "Heading 6" => (1000, 600, 200, true, true),
        _ => return None,
    })
}

/// The built-in definition of `name`, if it has one.
pub fn builtin_style(name: &str) -> Option<Style> {
    if name == "Normal" {
        return Some(Style {
            parent: Some(String::new()),
            ..Style::default()
        });
    }
    let (size, before, after, bold, italic) = metrics(name)?;
    let mut style = Style {
        parent: Some("Normal".into()),
        size: Some(LengthExpr::Em(size)),
        keep_with_next: Some(true),
        keep_together: Some(name.starts_with("Heading")).filter(|k| *k),
        weight: bold.then_some(700),
        slant: italic.then_some(TextSlant::Italic),
        ..Style::default()
    };
    if name == "Subtitle" {
        style.color = Some([0x5f, 0x63, 0x68, 0xff]);
    }
    for (p, permille) in [
        (Property::SpaceBefore, before),
        (Property::SpaceAfter, after),
    ] {
        if permille != 0 {
            style.set(p, crate::Authored::Legacy(LengthExpr::Em(permille)));
        }
    }
    Some(style)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_is_defined_and_chains_to_the_base() {
        for name in BUILTIN_STYLES {
            let style = builtin_style(name).unwrap();
            let parent = style.parent.as_deref().unwrap();
            assert!(parent.is_empty() || parent == "Normal", "{name}");
        }
        assert!(builtin_style("Heading 7").is_none());
        assert!(builtin_style("").is_none());
        assert!(builtin_style("heading 1").is_none(), "names are exact");
    }
}
