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

#[test]
fn mixed_script_marks_do_not_split_graphemes_into_adapter_runs() {
    let fonts = FontStore::default();
    // Hebrew and Devanagari marks have their own script, rather than Inherited.
    // A Latin base plus either is still one extended grapheme cluster.
    for text in ["a\u{05b0}", "a\u{093f}", "a\u{05b0}\u{093f}"] {
        let out = item(text, &["serif"], &fonts);
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].range, 0..text.len());
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
                .flat_map(|r| &r.glyphs)
                .all(|g| g.cluster == 0)
        );
    }
}

#[test]
fn legacy_entrypoint_keeps_its_full_family_search_contract() {
    let mut fonts = FontStore::default();
    fonts.add(
        reprise_font::Face::from_bytes(
            include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
        )
        .unwrap(),
    );
    let mut style = run("abc", &[]);
    style.families = vec!["unavailable".into(); 100];
    style.families.push("Source Serif Pro".into());
    let out = itemize(
        &ParagraphInput {
            text: "abc",
            styles: &[style],
            direction: None,
        },
        &fonts,
    );
    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].face.family, "Source Serif Pro");
    assert_eq!(out.notes.len(), 1);
    assert_eq!(out.notes[0].code, codes::FONT_FALLBACK);
    let explicit = item("abc", &["SANS-SERIF"], &fonts);
    assert_eq!(
        explicit.items[0].face,
        *fonts.generic(GenericFamily::SansSerif).id()
    );
    assert!(explicit.notes.is_empty());
}
fn host_fonts() -> (FontStore, reprise_font::FaceId, reprise_font::FaceId) {
    let mut fonts = FontStore::default();
    let declare = |family: &str| FontDeclaration {
        family: family.into(),
        descriptors: Descriptors::default(),
        face_index: 0,
    };
    let mono = fonts
        .register(
            include_bytes!("../../../fixtures/fonts/PaperMono-TestSubset.ttf").as_slice(),
            declare("Paper Mono"),
        )
        .unwrap();
    let universal = fonts
        .register(
            include_bytes!("../../../fixtures/fonts/AtkinsonHyperlegibleNext-Regular.ttf")
                .as_slice(),
            declare("Universal"),
        )
        .unwrap();
    fonts
        .set_settings(reprise_font::FontSettings {
            generics: [(GenericFamily::Monospace, "Paper Mono".into())].into(),
            fallbacks: vec!["Absent".into(), "Universal".into()],
            synthetic_oblique: true,
        })
        .unwrap();
    (fonts, mono, universal)
}

#[test]
fn host_fallback_covers_mid_word_and_keeps_rtl_clusters() {
    let (fonts, mono, universal) = host_fonts();
    for direction in [None, Some(reprise_geom::InlineDirection::Rtl)] {
        let text = "aéb α אב";
        let out = itemize(
            &ParagraphInput {
                text,
                styles: &[run(text, &["monospace"])],
                direction,
            },
            &fonts,
        );
        assert_eq!(out.items[0].face, mono);
        assert_eq!(
            out.items
                .iter()
                .find(|i| i.range.contains(&1))
                .unwrap()
                .face,
            universal
        );
        assert_eq!(
            out.items
                .iter()
                .find(|i| i.range.contains(&3))
                .unwrap()
                .face,
            mono
        );
        assert!(
            out.notes
                .iter()
                .any(|n| n.code == codes::FONT_FALLBACK && n.bytes == Some(1..3))
        );
        assert!(out.notes.iter().any(|n| n.code == codes::FONT_MISSING));
        if direction.is_some() {
            assert_eq!(out.base_level, 1);
        }
    }
    let out = item("abc", &["Lost"], &fonts);
    assert_eq!(out.items[0].face, universal);
    assert!(out.notes.iter().any(|n| n.code == codes::FONT_FALLBACK));
    let out = item("abc", &["Lost", "Paper Mono"], &fonts);
    assert_eq!(out.items[0].face, mono);
    assert_eq!(
        out.notes
            .iter()
            .filter(|n| n.code == codes::FONT_FALLBACK)
            .count(),
        1
    );
}

#[test]
fn configured_missing_generic_and_synthetic_reshape_remain_bounded() {
    let (mut fonts, _, universal) = host_fonts();
    fonts
        .set_settings(reprise_font::FontSettings {
            generics: [(GenericFamily::Serif, "Missing".into())].into(),
            ..fonts.settings().clone()
        })
        .unwrap();
    let out = item("aé", &["serif"], &fonts);
    assert_eq!(out.items[0].face, universal);
    assert!(out.notes.iter().any(|n| n.code == codes::FONT_FALLBACK));
    for size in [Length::MIN, Length::ZERO, Length::MAX] {
        let text = "ab";
        let mut style = run(text, &["Paper Mono"]);
        style.size = size;
        let out = itemize_emphasis(
            &ParagraphInput {
                text,
                styles: &[style],
                direction: None,
            },
            &fonts,
            false,
            &[Descriptors {
                style: reprise_font::FontStyle::Oblique,
                ..Default::default()
            }],
        );
        assert!(out.items.iter().all(|i| i.synthetic_oblique));
        let shaper = Shaper {
            text,
            items: &out.items,
            fonts: &fonts,
            adapter: &HarfRust,
        };
        assert!(shaper.shape().runs.iter().all(|r| r.synthetic_oblique));
        assert!(shaper.reshape(1..2).iter().all(|r| r.synthetic_oblique));
    }
    let empty = item("", &["monospace"], &fonts);
    assert!(empty.items.is_empty());
    let long = "aé".repeat(8192);
    let out = item(&long, &["monospace"], &fonts);
    assert_eq!(out.items.last().unwrap().range.end, long.len());
    let out = item("a", &vec!["missing"; 4096], &fonts);
    assert!(out.notes.iter().any(|n| n.code == codes::FONT_CHAIN_LIMIT));
}
