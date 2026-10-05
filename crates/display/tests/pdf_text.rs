//! Extraction from emitted PDF objects, not the source display list. lopdf
//! (MIT) parses/decompresses the PDF. Its built-in extractor ignores ActualText
//! and its CMap parser rejects krilla's CMapType 0 header, so this reader parses
//! the emitted bfchar mappings and honours marked-content replacement text
//! (PDF 1.7, 14.9.4). It never reads the source display list during extraction.

use lopdf::{Document, Object, content::Content};
use reprise_display::{Color, DisplayList, Glyph, GlyphRun, Item, Layer, Path, pdf};
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

/// Orchestrator review: workstream 5 puts rotated and mirrored frames into
/// groups, and glyph positions are now encoded as offsets divided by the font
/// size. A ligature run inside a mirrored, quarter-turned and clipped group,
/// and a run at the smallest size with glyphs far from the origin, must still
/// render and extract exactly their source text.
#[test]
fn transformed_groups_and_tiny_sizes_keep_their_text() {
    use reprise_geom::{Matrix, Point, Rect};

    let (ligature, fonts) = run("office", InlineDirection::Ltr);
    let (mut tiny, _) = run("far away", InlineDirection::Ltr);
    tiny.size = Length(1);
    for glyph in &mut tiny.glyphs {
        glyph.x = Length(i32::MAX / 4) + glyph.x;
        glyph.y = Length(i32::MIN / 4);
    }
    let transform = Matrix::mirror_x()
        .then(&Matrix::rotate_quarter(3))
        .then(&Matrix::translate(
            Length::from_pt(200),
            Length::from_pt(100),
        ));
    let clip = Path::rect(Rect::new(
        Point::origin(),
        Length::from_pt(300),
        Length::from_pt(200),
    ));
    let list = DisplayList {
        width: Length::from_pt(420),
        height: Length::from_pt(300),
        items: vec![Item::Group {
            transform,
            clip: Some(clip),
            items: vec![
                Item::Group {
                    transform: Matrix::rotate_quarter(-7),
                    clip: None,
                    items: vec![Item::Glyphs(ligature)],
                },
                Item::Glyphs(tiny),
            ],
        }],
    };
    let bytes = pdf::render(&[list], &fonts).expect("transformed text renders");
    let (text, _) = extract(&bytes);
    assert_eq!(text, "officefar away");
}

#[test]
fn ordered_layout_extracts_rotated_mirrored_vertical_and_spiral_text() {
    use reprise_layout::DisplayOptions;
    for fixture in [
        reprise_fixtures::hostile::transformed_rtl().unwrap(),
        reprise_fixtures::hostile::vertical_rl().unwrap(),
        reprise_fixtures::hostile::spiral_text().unwrap(),
        reprise_fixtures::hostile::rational_rotation_extreme().unwrap(),
    ] {
        let snapshot = fixture.engine.layout(&fixture.doc);
        let lists = snapshot.to_display_lists(DisplayOptions::default());
        let order = snapshot.pdf_reading_order(&fixture.doc);
        let bytes = pdf::render_ordered(&lists, &fixture.engine.fonts, &order).unwrap();
        let expected: String = snapshot
            .reading_order(&fixture.doc)
            .into_iter()
            .flat_map(|step| {
                let line = snapshot.line(step.line).unwrap();
                let block = snapshot.block(step.line.node).unwrap();
                let mut runs: Vec<_> = line.runs.iter().collect();
                runs.sort_by_key(|r| r.range.start);
                runs.into_iter()
                    .map(|r| block.text[r.range.clone()].to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        let (text, spans) = extract(&bytes);
        assert_eq!(text, expected, "{}", fixture.name);
        assert!(spans > 0);
    }
}

#[test]
fn ordered_layout_override_changes_extraction_without_moving_glyphs() {
    use reprise_doc::{BlockKind, Document, SchemaRegistry};
    use reprise_layout::DisplayOptions;
    let doc = Document::new(reprise_fixtures::PEER).unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "", "A room.")
        .unwrap();
    let b = doc
        .append_block(BlockKind::Paragraph, "", "B room.")
        .unwrap();
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &reprise_doc::reading::before(b, a),
    )
    .unwrap();
    doc.commit();
    let engine = reprise_fixtures::engine();
    let snapshot = engine.layout(&doc);
    let lists = snapshot.to_display_lists(DisplayOptions::default());
    let order = snapshot.pdf_reading_order(&doc);
    assert_eq!(
        extract(&pdf::render_ordered(&lists, &engine.fonts, &order).unwrap()).0,
        "B room.A room."
    );
    assert!(pdf::render_ordered(&lists, &engine.fonts, &order[..1]).is_err());
    let mut duplicate = order.clone();
    duplicate.push(order[0].clone());
    assert!(pdf::render_ordered(&lists, &engine.fonts, &duplicate).is_err());
    let mut invalid = order;
    invalid[0].path = vec![usize::MAX];
    assert!(pdf::render_ordered(&lists, &engine.fonts, &invalid).is_err());
}

#[test]
fn ordered_pdf_preserves_nested_transforms_clips_and_rejects_deep_groups() {
    use reprise_geom::{Matrix, Point, Rect};
    let (run, fonts) = run("clipped sideways", InlineDirection::Ltr);
    let mut item = Item::Glyphs(run);
    for turns in [1, 2, 3] {
        item = Item::Group {
            transform: Matrix::rotate_quarter(turns),
            clip: Some(Path::rect(Rect::new(
                Point::origin(),
                Length::from_pt(150),
                Length::from_pt(100),
            ))),
            items: vec![item],
        };
    }
    let list = DisplayList {
        width: Length::from_pt(420),
        height: Length::from_pt(300),
        items: vec![item],
    };
    let order = [pdf::ReadingRun {
        page: 0,
        path: vec![0, 0, 0, 0],
    }];
    assert!(pdf::render_ordered(&[], &fonts, &[]).is_ok());
    let bytes = pdf::render_ordered(std::slice::from_ref(&list), &fonts, &order).unwrap();
    assert_eq!(extract(&bytes).0, "clipped sideways");
    let mut item = list.items[0].clone();
    for _ in 0..257 {
        item = Item::Group {
            transform: Matrix::IDENTITY,
            clip: None,
            items: vec![item],
        };
    }
    assert!(
        pdf::render_ordered(
            &[DisplayList {
                items: vec![item],
                ..list
            }],
            &fonts,
            &[]
        )
        .is_err()
    );
}

#[test]
fn images_extract_alt_text_in_order_even_when_missing_or_zero_size() {
    use reprise_geom::{Point, Rect};
    let fonts = FontStore::default();
    let mut assets = reprise_display::AssetStore::default();
    let hash = assets
        .insert(include_bytes!("../../../fixtures/images/red-1x1.png").as_slice())
        .unwrap();
    let pt = Length::from_pt;
    let list = DisplayList {
        width: pt(100),
        height: pt(100),
        items: vec![
            Item::Image {
                asset: hash,
                rect: Rect::new(Point::new(pt(0), pt(0)), pt(10), pt(10)),
                alt: "First café image.".into(),
                layer: Layer::Content,
            },
            Item::Image {
                asset: "0".repeat(64),
                rect: Rect::new(Point::new(pt(20), pt(0)), pt(10), pt(10)),
                alt: "Second missing image.".into(),
                layer: Layer::Content,
            },
            Item::Image {
                asset: "0".repeat(64),
                rect: Rect::new(Point::new(pt(40), pt(0)), Length::ZERO, Length::ZERO),
                alt: "Third zero size.".into(),
                layer: Layer::Content,
            },
        ],
    };
    let order = [2, 0, 1].map(|i| pdf::ReadingRun {
        page: 0,
        path: vec![i],
    });
    let bytes = pdf::render_ordered_with_assets(&[list], &fonts, &assets, &order).unwrap();
    assert_eq!(
        extract(&bytes),
        (
            "Third zero size.First café image.Second missing image.".into(),
            3
        )
    );
}
