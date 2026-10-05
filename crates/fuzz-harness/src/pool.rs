//! Fixed pools the decoder draws from. Everything a scenario can say is an
//! index into one of these, so a scenario stays small and reproducible.

use reprise_display::AssetStore;
use reprise_doc::{Property, Style};
use reprise_font::{Descriptors, FontDeclaration};

/// Text that has hurt layout engines before: combining marks, ZWJ sequences,
/// right-to-left runs, stray bidi controls, ligatures, long unbreakable words,
/// line and paragraph separators, and nothing at all.
pub const TEXTS: [&str; 24] = [
    "",
    "x",
    "The quick brown fox jumps over the lazy dog. ",
    "office affinity fficult ",
    "e\u{301}\u{302}\u{303} combining ",
    "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} family ",
    "\u{5d0}\u{5d1}\u{5d2} \u{5d3}\u{5d4} ",
    "\u{627}\u{644}\u{639}\u{631}\u{628}\u{64a}\u{629} ",
    "mixed \u{5d0}\u{5d1} and 123 \u{5d2} text ",
    "\u{202e}override\u{202c} \u{2067}isolate\u{2069} ",
    "tab\there\u{a0}nbsp\u{2028}ls\u{2029}ps",
    "line\nbreak\r\nand\rreturns",
    "Supercalifragilisticexpialidocious_Supercalifragilisticexpialidocious_Supercalifragilisticexpialidocious_Supercalifragilisticexpialidocious",
    "a b c d e f g h i j k l m n o p q r s t u v w x y z ",
    "\u{200b}\u{200c}\u{200d}\u{feff}\u{2060}",
    "CJK \u{4e2d}\u{6587}\u{3042}\u{3044} text ",
    "<b>&amp; \"quoted\" 'tags' &#65;</b>",
    "  leading and trailing  ",
    "\u{e9}\u{e8}\u{ea}\u{eb} \u{fb01}\u{fb02} ",
    "0123456789 ",
    "ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ZZZ ",
    "\u{1f1e8}\u{1f1f3} flags \u{1f3f3}\u{fe0f}\u{200d}\u{1f308} ",
    "\u{0}\u{1}\u{7f} controls",
    "\u{10ffff}\u{fffd}\u{e000} edge scalars",
];

pub fn text(index: usize) -> &'static str {
    TEXTS[index % TEXTS.len()]
}

pub const STYLE_NAMES: [&str; 6] = ["", "body", "note", "alpha", "beta", "gamma"];

pub fn style_name(index: usize) -> &'static str {
    STYLE_NAMES[index % STYLE_NAMES.len()]
}

/// Font families a style may ask for: bundled generics, registered faces and
/// one that is never available.
pub const FAMILIES: [&str; 7] = [
    "serif",
    "sans-serif",
    "monospace",
    "cursive",
    "Collection",
    "Hostile Serif",
    "Nobody Has This Font",
];

/// Direct style overrides, from a few bytes. Several are extreme on purpose:
/// huge and negative lengths, expressions that divide by zero or depend on
/// themselves, a parent that may not exist or may form a cycle, and a stored
/// value this engine can't read.
pub fn style(variant: usize, family: usize, parent: usize) -> Style {
    use reprise_doc::expr::Expr;
    use reprise_doc::{Authored, LengthExpr};
    use reprise_geom::Length;
    let mut style = Style::default();
    match family % 9 {
        0..=2 => {}
        3..=6 => style.family = Some(FAMILIES[family % FAMILIES.len()].into()),
        7 => {
            style.families = Some(vec![
                FAMILIES[family % FAMILIES.len()].into(),
                FAMILIES[(family / 3) % FAMILIES.len()].into(),
            ])
        }
        _ => style.unparsed_families = Some("families9:not-json".into()),
    }
    if parent % 4 == 1 {
        style.parent = Some(style_name(parent / 4 + 1).into());
    }
    let size = match variant % 13 {
        0 => None,
        1 => Some(Authored::Legacy(LengthExpr::Pt(Length::from_pt(9)))),
        2 => Some(Authored::Legacy(LengthExpr::Pt(Length::from_pt(14)))),
        3 => Some(Authored::Legacy(LengthExpr::Em(1500))),
        4 => Some(Authored::Legacy(LengthExpr::Em(250))),
        5 => Some(Authored::Legacy(LengthExpr::Pt(Length::MAX))),
        6 => Some(Authored::Legacy(LengthExpr::Pt(Length::MIN))),
        7 => Some(Authored::Legacy(LengthExpr::Pt(Length::ZERO))),
        8 => Expr::parse("frame-width / 20").ok().map(Authored::Expr),
        9 => Expr::parse("1em / 0pt").ok().map(Authored::Expr),
        10 => Expr::parse("clamp(6pt, page-height / 30, 40pt)")
            .ok()
            .map(Authored::Expr),
        11 => Expr::parse("lh + 1pt").ok().map(Authored::Expr),
        _ => Some(Authored::Unparsed("future:size".into())),
    };
    if let Some(size) = size {
        style.set(Property::Size, size);
    }
    let line = match (variant / 13) % 5 {
        0 => None,
        1 => Some(Authored::Legacy(LengthExpr::Em(1200))),
        2 => Some(Authored::Legacy(LengthExpr::Pt(Length::ZERO))),
        3 => Some(Authored::Legacy(LengthExpr::Pt(Length::MIN))),
        _ => Expr::parse("block-width / 30").ok().map(Authored::Expr),
    };
    if let Some(line) = line {
        style.set(Property::LineHeight, line);
    }
    style
}

/// Font bytes a scenario may register, and the declaration to register each
/// under. Index 3 and 4 are damaged on purpose.
pub fn font(index: usize) -> (Vec<u8>, FontDeclaration) {
    use reprise_fixtures::{SERIF, fonts};
    match index % 5 {
        0 => (fonts::collection(), fonts::declaration("Collection", 0)),
        1 => (fonts::collection(), fonts::declaration("Collection", 1)),
        2 => (
            SERIF.to_vec(),
            FontDeclaration {
                family: "Hostile Serif".into(),
                descriptors: Descriptors::default(),
                face_index: 0,
            },
        ),
        3 => (
            SERIF[..SERIF.len() / 3].to_vec(),
            fonts::declaration("Hostile Serif", 0),
        ),
        _ => {
            let mut flipped = SERIF.to_vec();
            for at in (64..flipped.len()).step_by(997) {
                flipped[at] ^= 0x5a;
            }
            (flipped, fonts::declaration("Hostile Serif", 0))
        }
    }
}

const RED_PNG: &[u8] = include_bytes!("../../../fixtures/images/red-1x1.png");
const RED_JPG: &[u8] = include_bytes!("../../../fixtures/images/red-2x1.jpg");
const DENSITY_PNG: &[u8] = include_bytes!("../../../fixtures/images/density.png");
const HUGE_HEADER: &[u8] = include_bytes!("../../../fixtures/images/header-65535.png");

/// Image bytes a scenario may register and reference. Index 4 is truncated
/// and 5 is not an image at all.
pub fn image(index: usize) -> Vec<u8> {
    match index % 6 {
        0 => RED_PNG.to_vec(),
        1 => RED_JPG.to_vec(),
        2 => DENSITY_PNG.to_vec(),
        3 => HUGE_HEADER.to_vec(),
        4 => RED_PNG[..RED_PNG.len().saturating_sub(20).max(8)].to_vec(),
        _ => b"definitely not an image".to_vec(),
    }
}

/// The content hash of [`image`], whether or not it is registered.
pub fn image_hash(index: usize) -> String {
    AssetStore::default()
        .insert(image(index))
        .unwrap_or_else(|_| "0".repeat(64))
}

/// HTML the clipboard reader may be handed: well-formed, tag soup, active
/// content, deep nesting and nested tables.
pub fn html(index: usize) -> String {
    match index % 10 {
        0 => "<p>one</p><p>two &amp; three</p>".into(),
        1 => "<div>a<br>b<br/>c</div><pre>  keep   spaces </pre>".into(),
        2 => "<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>".into(),
        3 => "<p>unclosed <b>bold <i>italic</p><p>next".into(),
        4 => "<script>alert(1)</script><p>after</p><style>p{}</style>".into(),
        5 => "<p style=\"font-size: 14pt; font-family: serif\">styled</p>".into(),
        6 => "<table><tr><td><table><tr><td>nested</td></tr></table></td></tr></table>".into(),
        7 => format!("{}deep{}", "<div>".repeat(80), "</div>".repeat(80)),
        8 => "<p dir=\"rtl\">\u{5d0}\u{5d1}</p><p dir=\"ltr\">\u{5d0}</p>&#x1F600;&#0;&nbsp;".into(),
        _ => "<<<>>>&&&;;; <p <b> </ ".into(),
    }
}
