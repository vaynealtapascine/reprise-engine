//! Tagged PDF structure: parsed back from the emitted bytes by the checker.

mod common;

use common::checker::check;
use reprise_display::pdf;

fn ordered(fixture: &reprise_fixtures::hostile::Fixture) -> (Vec<u8>, String) {
    use reprise_layout::DisplayOptions;
    let snapshot = fixture.engine.layout(&fixture.doc);
    let lists = snapshot.to_display_lists(DisplayOptions::default());
    let order = snapshot.pdf_reading_order(&fixture.doc);
    let bytes = pdf::render_ordered(&lists, &fixture.engine.fonts, &order).unwrap();
    let expected = snapshot
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
    (bytes, expected)
}

#[test]
fn ordered_layouts_are_tagged_and_follow_reading_order() {
    for fixture in [
        reprise_fixtures::hostile::transformed_rtl().unwrap(),
        reprise_fixtures::hostile::vertical_rl().unwrap(),
        reprise_fixtures::hostile::spiral_text().unwrap(),
        reprise_fixtures::hostile::rational_rotation_extreme().unwrap(),
    ] {
        let (bytes, expected) = ordered(&fixture);
        let report = check(&bytes);
        assert_eq!(report.text, expected, "{}", fixture.name);
        // The PDF/UA-1 claim and the catalog entries it needs go together.
        let claims = report.xmp.contains("pdfuaid");
        assert_eq!(claims, report.display_doc_title, "{}", fixture.name);
        assert!(report.marked, "{}", fixture.name);
        assert_eq!(report.lang.as_deref(), Some("en"));
        assert!(report.xmp.contains("Untitled document"));
        assert_eq!(report.roles.first().map(String::as_str), Some("Document"));
        assert!(report.leaves > 0 && report.artifacts > 0);
        // No time or randomness: the same input gives the same bytes.
        assert_eq!(bytes, ordered(&fixture).0, "{}", fixture.name);
    }
}

#[test]
fn a_clean_document_claims_pdf_ua_1() {
    use reprise_doc::{BlockKind, Document};
    use reprise_layout::DisplayOptions;
    let doc = Document::new(reprise_fixtures::PEER).unwrap();
    doc.append_block(BlockKind::Paragraph, "", "A room.")
        .unwrap();
    doc.append_block(BlockKind::Paragraph, "", "Another office room.")
        .unwrap();
    doc.commit();
    let engine = reprise_fixtures::engine();
    let snapshot = engine.layout(&doc);
    let lists = snapshot.to_display_lists(DisplayOptions::default());
    let order = snapshot.pdf_reading_order(&doc);
    let bytes = pdf::render_ordered(&lists, &engine.fonts, &order).unwrap();
    let report = check(&bytes);
    assert!(report.xmp.contains("pdfuaid:part"), "{}", report.xmp);
    assert!(report.display_doc_title && report.marked);
    assert_eq!(report.text, "A room.Another office room.");
    assert_eq!(report.roles, ["Document", "P", "P"]);
    assert_eq!(report.outline_titles, Vec::<String>::new());
}

// ---- Structures built by hand over display lists ----

use pdf::ReadingRun;
use pdf::tags::{self, CellRole, Child, Content, Node, Role, Scope, Structure};
use reprise_display::{Color, DisplayList, Glyph, GlyphRun, Item, Layer};
use reprise_font::{Face, FontStore};
use reprise_geom::{InlineDirection, Length, Point, Rect};
use reprise_shape::{HarfRust, ShapeRequest, ShapingAdapter};

fn pt(v: i32) -> Length {
    Length::from_pt(v)
}

/// One run per text, stacked down the page, sharing one font store.
fn runs(texts: &[&str]) -> (Vec<GlyphRun>, FontStore) {
    let face = Face::from_bytes(reprise_fixtures::SERIF).unwrap();
    let mut fonts = FontStore::default();
    let id = fonts.add(Face::from_bytes(reprise_fixtures::SERIF).unwrap());
    let size = pt(12);
    let runs = texts
        .iter()
        .enumerate()
        .map(|(row, text)| {
            let shaped = HarfRust.shape(&ShapeRequest {
                upright: false,
                combined: false,
                text,
                range: 0..text.len(),
                context: 0..text.len(),
                face: &face,
                size,
                direction: InlineDirection::Ltr,
                script: None,
                language: None,
                features: &[],
            });
            let mut ends: Vec<u32> = shaped.iter().map(|g| g.cluster).collect();
            ends.push(text.len() as u32);
            ends.sort_unstable();
            ends.dedup();
            let mut x = pt(10);
            let glyphs = shaped
                .iter()
                .map(|g| {
                    let end = ends.iter().copied().find(|&b| b > g.cluster).unwrap();
                    let glyph = Glyph {
                        id: g.id,
                        x: x + g.x_offset,
                        y: pt(20 + 20 * row as i32) - g.y_offset,
                        text: g.cluster..end,
                    };
                    x += g.advance;
                    glyph
                })
                .collect();
            GlyphRun {
                face: id.clone(),
                size,
                color: Color::BLACK,
                text: (*text).into(),
                glyphs,
                layer: Layer::Content,
            }
        })
        .collect();
    (runs, fonts)
}

fn page(items: Vec<Item>) -> DisplayList {
    DisplayList {
        width: pt(300),
        height: pt(300),
        items,
    }
}

fn leaf(page: usize, i: usize) -> Child {
    Child::Content(Content::whole(ReadingRun {
        page,
        path: vec![i],
    }))
}

fn part(page: usize, i: usize, bytes: std::ops::Range<usize>) -> Child {
    Child::Content(Content {
        run: ReadingRun {
            page,
            path: vec![i],
        },
        part: Some(bytes),
    })
}

fn node(role: Role, children: Vec<Child>) -> Child {
    Child::Node(Node::new(role, children))
}

fn glyphs(runs: Vec<GlyphRun>) -> Vec<Item> {
    runs.into_iter().map(Item::Glyphs).collect()
}

fn render(
    pages: &[DisplayList],
    fonts: &FontStore,
    assets: &reprise_display::AssetStore,
    children: Vec<Child>,
) -> Result<pdf::Tagged, reprise_display::RenderError> {
    let children = children
        .into_iter()
        .map(|c| match c {
            Child::Node(n) => n,
            Child::Content(_) => panic!("top level must be nodes"),
        })
        .collect();
    pdf::render_tagged(
        pages,
        fonts,
        assets,
        &Structure {
            title: "House of Leaves".into(),
            lang: "fr".into(),
            children,
            artifacts: Vec::new(),
        },
    )
}

fn codes(t: &pdf::Tagged) -> Vec<String> {
    t.notes.iter().map(|n| n.code.to_string()).collect()
}

#[test]
fn headings_notes_tables_and_figures_get_their_elements() {
    let (runs, fonts) = runs(&["Title", "See note", "The note", "a", "b", "c"]);
    let mut assets = reprise_display::AssetStore::default();
    let hash = assets
        .insert(include_bytes!("../../../fixtures/images/red-1x1.png").as_slice())
        .unwrap();
    let mut items = glyphs(runs);
    let image = |asset: &str, alt: &str, x: i32| Item::Image {
        asset: asset.into(),
        rect: Rect::new(Point::new(pt(x), pt(200)), pt(10), pt(10)),
        alt: alt.into(),
        layer: Layer::Content,
    };
    items.push(image(&hash, "A red dot", 10)); // 6
    items.push(image(&hash, "  ", 30)); // 7: no alt text
    // A decorative rule is an artifact.
    items.push(Item::Path {
        path: reprise_display::Path::line(Point::new(pt(0), pt(5)), Point::new(pt(300), pt(5))),
        fill: None,
        stroke: Some(reprise_display::Stroke {
            color: Color::BLACK,
            width: Length(512),
        }),
        layer: Layer::Content,
    });
    let pages = [page(items)];
    let mut note = Node::new(
        Role::Note {
            id: Some("note-1".into()),
        },
        vec![leaf(0, 2)],
    );
    note.key = Some(7);
    note.at = Some(Point::new(pt(10), pt(60)));
    let mut heading = Node::new(
        Role::Heading {
            level: 1,
            title: "Title".into(),
        },
        vec![leaf(0, 0)],
    );
    heading.at = Some(Point::new(pt(10), pt(20)));
    let reference = Role::Reference {
        link: Some(tags::Link {
            rect: Rect::new(Point::new(pt(40), pt(30)), pt(30), pt(14)),
            target: 7,
            alt: "Go to note".into(),
        }),
    };
    let structure = vec![
        Child::Node(heading),
        node(
            Role::P,
            vec![part(0, 1, 0..4), node(reference, vec![part(0, 1, 4..8)])],
        ),
        node(
            Role::Table,
            vec![node(
                Role::Row,
                vec![
                    node(
                        Role::Cell(CellRole {
                            header: Some(Scope::Column),
                            ..CellRole::default()
                        }),
                        vec![leaf(0, 3)],
                    ),
                    node(
                        Role::Cell(CellRole {
                            row_span: 2,
                            col_span: 3,
                            ..CellRole::default()
                        }),
                        vec![leaf(0, 4)],
                    ),
                ],
            )],
        ),
        node(
            Role::Figure {
                alt: "A red dot".into(),
            },
            vec![leaf(0, 6)],
        ),
        node(Role::Figure { alt: "  ".into() }, vec![leaf(0, 7)]),
        Child::Node(note),
        node(Role::Div, vec![leaf(0, 5)]),
    ];
    let t = render(&pages, &fonts, &assets, structure).unwrap();
    assert!(t.ua, "{:?}", t.notes);
    assert_eq!(codes(&t), [tags::FIGURE_ALT_MISSING.to_string()]);
    let r = check(&t.bytes);
    assert_eq!(
        r.roles,
        [
            "Document",
            "H1",
            "P",
            "Reference",
            "Link",
            "Table",
            "TR",
            "TH",
            "TD",
            "Figure",
            "Note",
            "Div"
        ]
    );
    // krilla prefixes caller IDs with `U` so they cannot clash with its own `Note N`.
    assert_eq!(r.note_ids, ["Unote-1"]);
    assert_eq!(r.figure_alts, ["A red dot"]);
    assert_eq!(r.outline_titles, ["Title"]);
    assert_eq!((r.links, r.tagged_links), (1, 1));
    assert_eq!(r.lang.as_deref(), Some("fr"));
    assert!(r.xmp.contains("House of Leaves"));
    assert!(r.cell_attrs[0].contains("Column"), "{:?}", r.cell_attrs);
    assert!(
        r.cell_attrs[1].contains("RowSpan") && r.cell_attrs[1].contains("ColSpan"),
        "{:?}",
        r.cell_attrs
    );
    // The alt-less image is an artifact; its ActualText is gone, the rest is in order.
    assert_eq!(
        r.text,
        "TitleSee note".to_string() + "a" + "b" + "A red dot" + "The note" + "c"
    );
    assert!(r.artifacts >= 3, "background, rule and the alt-less image");
}

#[test]
fn an_unsplittable_run_stays_whole_and_is_reported() {
    let (runs, fonts) = runs(&["office"]);
    let pages = [page(glyphs(runs))];
    // The parts leave a hole, so the run cannot be split along them.
    let structure = vec![node(Role::P, vec![part(0, 0, 0..2), part(0, 0, 3..6)])];
    let t = render(&pages, &fonts, &Default::default(), structure).unwrap();
    assert_eq!(codes(&t), [tags::RUN_SPLIT.to_string()]);
    assert_eq!(check(&t.bytes).text, "office");
}

#[test]
fn a_missing_glyph_drops_the_claim_but_stays_tagged() {
    let (mut runs, fonts) = runs(&["x"]);
    runs[0].glyphs[0].id = 0; // .notdef
    let pages = [page(glyphs(runs))];
    let t = render(
        &pages,
        &fonts,
        &Default::default(),
        vec![node(Role::P, vec![leaf(0, 0)])],
    )
    .unwrap();
    assert!(!t.ua);
    assert_eq!(codes(&t), [tags::UA_NOT_MET.to_string()]);
    assert!(t.notes[0].message.contains("missing from its font"));
    let r = check(&t.bytes);
    assert!(!r.xmp.contains("pdfuaid") && r.marked);
    assert_eq!(r.text, "x");
}

#[test]
fn reading_order_may_name_a_later_page_first() {
    let (mut runs, fonts) = runs(&["one", "two"]);
    let second = runs.pop().unwrap();
    let pages = [page(glyphs(runs)), page(glyphs(vec![second]))];
    let structure = vec![
        node(Role::P, vec![leaf(1, 0)]),
        node(Role::P, vec![leaf(0, 0)]),
    ];
    let t = render(&pages, &fonts, &Default::default(), structure).unwrap();
    assert!(t.ua);
    assert_eq!(check(&t.bytes).text, "twoone");
}

#[test]
fn bad_leaf_sets_are_errors_not_panics() {
    let (runs, fonts) = runs(&["a", "b"]);
    let pages = [page(glyphs(runs))];
    let go = |children: Vec<Child>| {
        render(
            &pages,
            &fonts,
            &Default::default(),
            vec![node(Role::P, children)],
        )
        .is_err()
    };
    assert!(go(vec![leaf(0, 0)]), "omits a run");
    assert!(go(vec![leaf(0, 0), leaf(0, 0), leaf(0, 1)]), "duplicate");
    assert!(go(vec![leaf(0, 0), leaf(0, 1), leaf(0, 5)]), "missing item");
    assert!(go(vec![leaf(0, 0), leaf(0, 1), leaf(9, 0)]), "missing page");
    assert!(
        go(vec![leaf(0, 0), part(0, 0, 0..1), leaf(0, 1)]),
        "whole and part of one run"
    );
    assert!(!go(vec![leaf(0, 0), leaf(0, 1)]));
}

#[test]
fn deep_structure_is_flattened_with_a_report_and_keeps_every_run_once() {
    let (runs, fonts) = runs(&["deep"]);
    let pages = [page(glyphs(runs))];
    let mut inner = node(Role::P, vec![leaf(0, 0)]);
    for _ in 0..2000 {
        inner = node(Role::Div, vec![inner]);
    }
    let t = render(&pages, &fonts, &Default::default(), vec![inner]).unwrap();
    assert!(codes(&t).contains(&tags::STRUCTURE_DEPTH.to_string()));
    let r = check(&t.bytes);
    assert_eq!(r.text, "deep");
    assert!(r.roles.len() <= tags::MAX_DEPTH + 3, "{}", r.roles.len());
}

#[test]
fn empty_structures_and_pages_are_still_valid_pdfs() {
    let (_, fonts) = runs(&["x"]);
    for pages in [vec![], vec![page(vec![])]] {
        let t = render(&pages, &fonts, &Default::default(), vec![]).unwrap();
        assert!(t.bytes.starts_with(b"%PDF-"));
        check(&t.bytes);
    }
}

#[test]
fn heading_levels_clamp_and_titles_default() {
    let (runs, fonts) = runs(&["h"]);
    let pages = [page(glyphs(runs))];
    let heading = Role::Heading {
        level: 9,
        title: "Deep".into(),
    };
    let t = pdf::render_tagged(
        &pages,
        &fonts,
        &Default::default(),
        &Structure {
            children: vec![Node::new(heading, vec![leaf(0, 0)])],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(codes(&t), [tags::HEADING_LEVEL.to_string()]);
    let r = check(&t.bytes);
    assert_eq!(r.roles, ["Document", "H6"]);
    assert!(r.xmp.contains(tags::DEFAULT_TITLE));
    assert_eq!(r.lang.as_deref(), Some(tags::DEFAULT_LANG));
}

#[test]
fn extreme_geometry_never_panics_the_tagged_renderer() {
    let (runs, fonts) = runs(&["edge", "far"]);
    let mut items = glyphs(runs);
    for (i, extent) in [Length::MIN, Length::MAX, Length::ZERO]
        .into_iter()
        .enumerate()
    {
        if let Item::Glyphs(run) = &mut items[i % 2] {
            for glyph in &mut run.glyphs {
                glyph.x = extent;
                glyph.y = extent;
            }
            run.size = if i == 2 { Length::ZERO } else { run.size };
        }
        items.push(Item::Image {
            asset: "0".repeat(64),
            rect: Rect::new(Point::new(extent, extent), extent, extent),
            alt: "extreme".into(),
            layer: Layer::Content,
        });
    }
    let pages = [page(items)];
    let mut heading = Node::new(
        Role::Heading {
            level: 0,
            title: "H".into(),
        },
        vec![leaf(0, 0)],
    );
    heading.at = Some(Point::new(Length::MAX, Length::MIN));
    heading.key = Some(u32::MAX);
    let reference = Role::Reference {
        link: Some(tags::Link {
            rect: Rect::new(
                Point::new(Length::MIN, Length::MAX),
                Length::MAX,
                Length::MIN,
            ),
            target: u32::MAX,
            alt: String::new(),
        }),
    };
    let structure = vec![
        Child::Node(heading),
        node(Role::P, vec![leaf(0, 1)]),
        node(reference, vec![leaf(0, 2)]),
        node(
            Role::Cell(CellRole {
                header: Some(Scope::Both),
                row_span: 0,
                col_span: u32::MAX,
            }),
            vec![leaf(0, 3), leaf(0, 4)],
        ),
    ];
    let t = render(&pages, &fonts, &Default::default(), structure).unwrap();
    check(&t.bytes);
}

#[test]
fn named_artifact_runs_paint_as_artifacts_and_must_not_also_be_leaves() {
    let (runs, fonts) = runs(&["body", "copy"]);
    let pages = [page(glyphs(runs))];
    let go = |structure_children: Vec<Child>, artifacts: Vec<ReadingRun>| {
        let children = structure_children
            .into_iter()
            .map(|c| match c {
                Child::Node(n) => n,
                Child::Content(_) => unreachable!(),
            })
            .collect();
        pdf::render_tagged(
            &pages,
            &fonts,
            &Default::default(),
            &Structure {
                children,
                artifacts,
                ..Default::default()
            },
        )
    };
    let copy = ReadingRun {
        page: 0,
        path: vec![1],
    };
    let t = go(vec![node(Role::P, vec![leaf(0, 0)])], vec![copy.clone()]).unwrap();
    let r = check(&t.bytes);
    assert_eq!(r.text, "body", "the copy is not content");
    assert_eq!(r.artifacts, 2, "the background and the copy");
    // Named twice, named and a leaf, or not there: errors, not panics.
    assert!(
        go(
            vec![node(Role::P, vec![leaf(0, 0)])],
            vec![copy.clone(), copy.clone()]
        )
        .is_err()
    );
    assert!(
        go(
            vec![node(Role::P, vec![leaf(0, 0), leaf(0, 1)])],
            vec![copy]
        )
        .is_err()
    );
    let missing = ReadingRun {
        page: 0,
        path: vec![9],
    };
    assert!(
        go(
            vec![node(Role::P, vec![leaf(0, 0), leaf(0, 1)])],
            vec![missing]
        )
        .is_err()
    );
}
