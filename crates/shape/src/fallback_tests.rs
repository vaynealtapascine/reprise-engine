use super::*;
use reprise_font::{Descriptors, FontDeclaration, FontStore, GenericFamily};
use reprise_geom::Length;
fn run(text: &str, families: &[&str]) -> StyleRun {
    StyleRun {
        range: 0..text.len(),
        families: families.iter().map(|f| f.to_string()).collect(),
        size: Length::from_pt(10),
        language: None,
        features: vec![],
    }
}
fn item(text: &str, families: &[&str], fonts: &FontStore) -> Itemized {
    itemize_families(
        &ParagraphInput {
            text,
            styles: &[run(text, families)],
            direction: None,
        },
        fonts,
    )
}
#[test]
fn all_generics_resolve_and_missing_families_append_serif() {
    let fonts = FontStore::default();
    for g in [
        GenericFamily::Serif,
        GenericFamily::SansSerif,
        GenericFamily::Monospace,
        GenericFamily::Script,
    ] {
        let out = item("hello", &[g.name()], &fonts);
        assert_eq!(out.items[0].face, *fonts.generic(g).id());
        assert!(out.notes.is_empty());
    }
    let out = item("abc", &["missing"], &fonts);
    assert_eq!(out.items[0].face, *fonts.generic(GenericFamily::Serif).id());
    assert_eq!(out.notes.len(), 1);
    assert_eq!(out.notes[0].code, codes::FONT_FALLBACK);
    assert_eq!(out.notes[0].bytes, Some(0..3));
}
#[test]
fn per_character_chain_selects_three_faces() {
    let mut fonts = FontStore::default();
    let declare = |family: &str| FontDeclaration {
        family: family.into(),
        descriptors: Descriptors::default(),
        face_index: 0,
    };
    let script = fonts.generic(GenericFamily::Script).clone();
    let serif = fonts.generic(GenericFamily::Serif).clone();
    let sans = fonts.generic(GenericFamily::SansSerif).clone();
    fonts.register(script.data(), declare("One")).unwrap();
    fonts.register(serif.data(), declare("Two")).unwrap();
    let middle = (32..0x3000)
        .filter_map(char::from_u32)
        .find(|c| c.is_alphabetic() && !script.covers(*c) && serif.covers(*c))
        .unwrap();
    let last = (32..0x3000)
        .filter_map(char::from_u32)
        .find(|c| c.is_alphabetic() && !script.covers(*c) && !serif.covers(*c) && sans.covers(*c))
        .unwrap();
    let text = format!("a{middle}{last}");
    let out = item(&text, &["One", "Two", "sans-serif"], &fonts);
    let faces: std::collections::BTreeSet<_> =
        out.items.iter().map(|i| i.face.family.as_str()).collect();
    assert_eq!(
        faces,
        std::collections::BTreeSet::from(["One", "Two", sans.id().family.as_str()])
    );
    assert_eq!(
        out.notes
            .iter()
            .filter(|n| n.code == codes::FONT_FALLBACK)
            .count(),
        2
    );
    assert!(!out.notes.iter().any(|n| n.code == codes::FONT_MISSING));
}
#[test]
fn uncovered_graphemes_keep_notdef_and_combining_clusters_stay_whole() {
    let fonts = FontStore::default();
    let text = "a\u{301} 👩\u{200d}🚀 \u{10ffff}";
    let out = item(text, &["script", "serif"], &fonts);
    assert!(out.notes.iter().any(|n| n.code == codes::FONT_MISSING));
    let boundaries: Vec<_> = icu_segmenter::GraphemeClusterSegmenter::new()
        .segment_str(text)
        .collect();
    for i in &out.items {
        assert!(boundaries.contains(&i.range.start));
        assert!(boundaries.contains(&i.range.end));
    }
    let shaped = Shaper {
        text,
        items: &out.items,
        fonts: &fonts,
        adapter: &HarfRust,
    }
    .shape();
    assert!(
        shaped
            .runs
            .iter()
            .any(|r| r.glyphs.iter().any(|g| g.id == 0))
    );
    for run in &shaped.runs {
        for glyph in &run.glyphs {
            assert!(boundaries.contains(&(glyph.cluster as usize)));
        }
    }
}
#[test]
fn limits_extremes_empty_and_malformed_style_ranges_are_total() {
    let fonts = FontStore::default();
    assert!(item("", &[], &fonts).items.is_empty());
    for size in [Length::MIN, Length::ZERO, Length(-1), Length::MAX] {
        let text = "a";
        let mut style = run(text, &["missing", "monospace"]);
        style.size = size;
        let out = itemize_families(
            &ParagraphInput {
                text,
                styles: &[style],
                direction: None,
            },
            &fonts,
        );
        let _ = Shaper {
            text,
            items: &out.items,
            fonts: &fonts,
            adapter: &HarfRust,
        }
        .shape();
    }
    let mut style = run("a", &[]);
    style.families = vec!["missing".into(); 100_000];
    style.families.push("monospace".into());
    let out = itemize_families(
        &ParagraphInput {
            text: "a",
            styles: &[style],
            direction: None,
        },
        &fonts,
    );
    assert_eq!(
        out.items[0].face,
        *fonts.generic(GenericFamily::Monospace).id()
    );
    assert!(out.notes.iter().any(|n| n.code == codes::FONT_CHAIN_LIMIT));
    let out = itemize_families(
        &ParagraphInput {
            text: "a\u{301}",
            styles: &[run("a", &["serif"])],
            direction: None,
        },
        &fonts,
    );
    assert_eq!(out.notes[0].code, codes::BAD_STYLE_RUN);
    let text = "a".repeat(32_768);
    assert_eq!(item(&text, &["serif"], &fonts).items.len(), 1);
}
