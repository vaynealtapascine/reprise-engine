//! Extraction from emitted PDF objects, not the source display list. lopdf
//! (MIT) parses/decompresses the PDF. Its built-in extractor ignores ActualText
//! and its CMap parser rejects krilla's CMapType 0 header, so this reader parses
//! the emitted bfchar mappings and honours marked-content replacement text
//! (PDF 1.7, 14.9.4). It never reads the source display list during extraction.

use lopdf::{Document, Object, content::Content};
use reprise_display::{Color, DisplayList, Glyph, GlyphRun, Item, Layer, pdf};
use reprise_font::{Face, FontStore};
use reprise_geom::{InlineDirection, Length};
use reprise_shape::{HarfRust, ShapeRequest, ShapingAdapter};

fn hex_bytes(hex: &str) -> Vec<u8> {
    let hex = hex.strip_prefix('<').unwrap().strip_suffix('>').unwrap();
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn utf16(bytes: &[u8]) -> String {
    assert_eq!(bytes.len() % 2, 0);
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).unwrap()
}

fn cmap(bytes: &[u8]) -> std::collections::BTreeMap<u16, String> {
    let mut mappings = std::collections::BTreeMap::new();
    let source = std::str::from_utf8(bytes).unwrap();
    assert!(
        !source.contains("beginbfrange"),
        "extend this reader if krilla starts emitting ranges"
    );
    let mut remaining = 0;
    for line in source.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.last() == Some(&"beginbfchar") {
            remaining = fields[0].parse::<usize>().unwrap();
        } else if line == "endbfchar" {
            assert_eq!(remaining, 0, "bfchar count matches actual mappings");
        } else if remaining > 0 {
            assert_eq!(fields.len(), 2);
            let code = hex_bytes(fields[0]);
            assert_eq!(code.len(), 2, "krilla's embedded CID font uses two bytes");
            mappings.insert(
                u16::from_be_bytes([code[0], code[1]]),
                utf16(&hex_bytes(fields[1])),
            );
            remaining -= 1;
        }
    }
    assert_eq!(remaining, 0);
    mappings
}

fn extract(bytes: &[u8]) -> (String, usize) {
    let doc = Document::load_mem(bytes).unwrap();
    let mut text = String::new();
    let mut replacements = 0;
    for page in doc.get_pages().values() {
        let fonts = doc.get_page_fonts(*page).unwrap();
        let encodings: std::collections::BTreeMap<_, _> = fonts
            .iter()
            .map(|(name, font)| {
                (
                    name.clone(),
                    cmap(
                        &font
                            .get_deref(b"ToUnicode", &doc)
                            .unwrap()
                            .as_stream()
                            .unwrap()
                            .get_plain_content()
                            .unwrap(),
                    ),
                )
            })
            .collect();
        let content = Content::decode(&doc.get_page_content(*page).unwrap()).unwrap();
        let mut font = Vec::new();
        let mut spans = Vec::new();
        for op in content.operations {
            match op.operator.as_str() {
                "Tf" => font = op.operands[0].as_name().unwrap().to_vec(),
                "BDC" | "BMC" => {
                    let actual = op
                        .operands
                        .get(1)
                        .and_then(|o| o.as_dict().ok())
                        .and_then(|d| d.get(b"ActualText").ok());
                    if let Some(actual) = actual
                        && !spans.contains(&true)
                    {
                        text.push_str(&lopdf::decode_text_string(actual).unwrap());
                        replacements += 1;
                    }
                    spans.push(actual.is_some());
                }
                "EMC" => {
                    spans.pop().expect("balanced marked content");
                }
                "Tj" | "TJ" if !spans.contains(&true) => {
                    let strings = match &op.operands[0] {
                        Object::Array(array) => array.as_slice(),
                        object => std::slice::from_ref(object),
                    };
                    for string in strings {
                        if let Object::String(bytes, _) = string {
                            assert_eq!(bytes.len() % 2, 0);
                            for code in bytes.as_chunks::<2>().0 {
                                if let Some(mapped) =
                                    encodings[&font].get(&u16::from_be_bytes([code[0], code[1]]))
                                {
                                    text.push_str(mapped);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        assert!(spans.is_empty());
    }
    (text, replacements)
}

fn run(text: &str, direction: InlineDirection) -> (GlyphRun, FontStore) {
    let face = Face::from_bytes(reprise_fixtures::SERIF).unwrap();
    let size = Length::from_pt(12);
    let shaped = HarfRust.shape(&ShapeRequest {
        text,
        range: 0..text.len(),
        context: 0..text.len(),
        face: &face,
        size,
        direction,
        script: None,
        language: None,
        features: &[],
    });
    let mut boundaries: Vec<_> = shaped.iter().map(|g| g.cluster).collect();
    boundaries.push(text.len() as u32);
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut x = Length::from_pt(10);
    let glyphs = shaped
        .iter()
        .map(|g| {
            let end = boundaries.iter().copied().find(|&b| b > g.cluster).unwrap();
            let glyph = Glyph {
                id: g.id,
                x: x + g.x_offset,
                y: Length::from_pt(25) - g.y_offset,
                text: g.cluster..end,
            };
            x += g.advance;
            glyph
        })
        .collect();
    let mut fonts = FontStore::default();
    let face = fonts.add(face);
    (
        GlyphRun {
            face,
            size,
            color: Color::BLACK,
            text: text.into(),
            glyphs,
            layer: Layer::Content,
        },
        fonts,
    )
}

fn bytes(run: GlyphRun, fonts: &FontStore) -> Vec<u8> {
    pdf::render(
        &[DisplayList {
            width: Length::from_pt(420),
            height: Length::from_pt(300),
            items: vec![Item::Glyphs(run)],
        }],
        fonts,
    )
    .unwrap()
}

#[test]
fn ligature_uses_multi_character_tounicode() {
    let (run, fonts) = run("office", InlineDirection::Ltr);
    assert!(
        run.glyphs
            .iter()
            .any(|g| run.text.get(g.text.start as usize..g.text.end as usize) == Some("ffi"))
    );
    let (text, actual) = extract(&bytes(run, &fonts));
    assert_eq!(text, "office");
    assert_eq!(actual, 0, "the ffi text is decoded directly from ToUnicode");
}

#[test]
fn combining_cluster_is_extracted_once() {
    let source = "Z\u{335}\u{322}\u{31b}\u{332} a\u{30a}\u{30a}";
    let (run, fonts) = run(source, InlineDirection::Ltr);
    assert!(
        run.glyphs
            .windows(2)
            .any(|pair| pair[0].text == pair[1].text)
    );
    let (text, actual) = extract(&bytes(run, &fonts));
    assert_eq!(text, source);
    assert!(actual > 0);
}

#[test]
fn rtl_visual_glyphs_extract_in_logical_source_order() {
    // Latin under an RTL override gives real glyphs in decreasing byte order.
    let source = "office abc";
    let (run, fonts) = run(source, InlineDirection::Rtl);
    assert!(run.glyphs.first().unwrap().text.start > run.glyphs.last().unwrap().text.start);
    let (text, actual) = extract(&bytes(run, &fonts));
    assert_eq!(text, source);
    assert!(actual > 0);
}

#[test]
fn empty_invalid_reversed_and_non_boundary_ranges_use_whole_run_fallback() {
    for range in [
        0..0,
        0..u32::MAX,
        1..2,
        std::ops::Range { start: 5, end: 2 },
    ] {
        let (mut run, fonts) = run("é office", InlineDirection::Ltr);
        run.glyphs[0].text = range;
        let (text, actual) = extract(&bytes(run, &fonts));
        assert_eq!(text, "é office");
        assert!(actual > 0);
    }
    let (mut run, fonts) = run("a", InlineDirection::Ltr);
    run.glyphs[0].text = 99..100;
    assert_eq!(extract(&bytes(run, &fonts)).0, "a");
}

#[test]
fn notdef_emoji_zwj_preserves_supplementary_characters() {
    let source = "👩‍👩‍👧‍👦 🧑‍🤝‍🧑";
    let (run, fonts) = run(source, InlineDirection::Ltr);
    assert!(run.glyphs.iter().any(|g| g.id == 0));
    assert_eq!(extract(&bytes(run, &fonts)).0, source);
}

#[test]
fn empty_run_is_empty_text() {
    let (run, fonts) = run("", InlineDirection::Ltr);
    assert_eq!(extract(&bytes(run, &fonts)).0, "");
}

#[test]
fn mappings_survive_conflicting_glyphs_across_runs_and_pages() {
    let (first, fonts) = run("a", InlineDirection::Ltr);
    let mut second = first.clone();
    second.text = "ffi".into();
    second.glyphs[0].text = 0..3;
    let page = |glyphs| DisplayList {
        width: Length::from_pt(100),
        height: Length::from_pt(50),
        items: vec![Item::Glyphs(glyphs)],
    };
    let (text, actual) = extract(&pdf::render(&[page(first), page(second)], &fonts).unwrap());
    assert_eq!(text, "affi");
    assert!(actual > 0);
}

#[test]
fn hostile_paragraph_pdf_extracts_all_original_bytes() {
    let fixture = reprise_fixtures::hostile::display_text_clusters().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    let pages = snapshot.to_display_lists(Default::default());
    let source: String = snapshot.blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(
        extract(&pdf::render(&pages, &fixture.engine.fonts).unwrap()).0,
        source
    );
}

#[test]
fn very_long_text_and_nonpositive_sizes_are_bounded() {
    let source = "office ".repeat(4096);
    let (long_run, fonts) = run(&source, InlineDirection::Ltr);
    assert_eq!(extract(&bytes(long_run, &fonts)).0, source);
    for size in [Length::ZERO, Length(i32::MIN)] {
        let (mut run, fonts) = run("office", InlineDirection::Ltr);
        run.size = size;
        assert_eq!(extract(&bytes(run, &fonts)).0, "");
    }
}
