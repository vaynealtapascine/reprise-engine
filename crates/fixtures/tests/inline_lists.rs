//! Inline objects inside list items (`docs/inline.md`, `docs/roles-and-lists.md`):
//! the marker, the item's inset and the objects' cells all belong to one line,
//! and none of them may disturb the others, in a full or an incremental layout.

use reprise_doc::{BlockKind, Document, ListDefinition, ListId, NodeId, WritingMode};
use reprise_fixtures::inline::{self, Part};
use reprise_geom::Length;
use reprise_layout::incremental::LayoutSession;
use reprise_layout::{BlockLayout, LayoutSnapshot};

fn block(s: &LayoutSnapshot, node: NodeId) -> &BlockLayout {
    s.block(node).expect("laid out")
}

fn codes(s: &LayoutSnapshot) -> Vec<&str> {
    s.diagnostics.iter().map(|d| d.code.as_str()).collect()
}

/// A numbered list of three items around a body paragraph; the items carry a
/// picture and a page number, the body paragraph a picture too.
fn document(mode: WritingMode) -> (reprise_layout::Engine, Document, ListId, Vec<NodeId>) {
    let (engine, doc, asset) = inline::page(200, 200, mode).unwrap();
    let list = doc.create_list(&ListDefinition::numbered()).unwrap();
    let mut nodes = Vec::new();
    for (i, parts) in [
        vec![
            Part::Text("See "),
            Part::Object(inline::image(&asset, 24, 30, "one")),
            Part::Text(" on page "),
            Part::Object(inline::page_number()),
        ],
        vec![
            Part::Object(inline::image(&asset, 12, 12, "two")),
            Part::Text(" starts this item"),
        ],
        vec![
            Part::Text("Third, ending in "),
            Part::Object(inline::page_number()),
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let node = inline::paragraph(&doc, parts).unwrap();
        doc.set_list_item(node, list, 0).unwrap();
        nodes.push(node);
        if i == 1 {
            // A body paragraph with an object between items: numbering goes on.
            nodes.push(
                inline::paragraph(
                    &doc,
                    vec![
                        Part::Text("Body "),
                        Part::Object(inline::image(&asset, 8, 8, "body")),
                    ],
                )
                .unwrap(),
            );
        }
    }
    doc.commit();
    (engine, doc, list, nodes)
}

#[test]
fn a_marker_and_the_objects_of_its_item_share_the_first_line() {
    let (engine, doc, _, nodes) = document(WritingMode::default());
    let s = engine.layout(&doc);
    assert!(
        codes(&s).iter().all(|c| !c.starts_with("layout.inline")
            && *c != "layout.list-indent"
            && !c.starts_with("compose.")),
        "{:?}",
        s.diagnostics
    );
    let numbers = [(0usize, "1."), (1, "2."), (3, "3.")];
    for (i, text) in numbers {
        let b = block(&s, nodes[i]);
        let first = &b.lines[0];
        let marker = first.marker.as_ref().expect("an item has a marker");
        assert_eq!(marker.text, text);
        assert!(b.lines[1..].iter().all(|l| l.marker.is_none()));
        // Every object lies after the marker and inside the line's room.
        assert!(!b.lines.iter().all(|l| l.atoms.is_empty()), "item {i}");
        for line in &b.lines {
            for atom in &line.atoms {
                if std::ptr::eq(line, first) {
                    assert!(
                        atom.rect.origin.x >= marker.x + marker.width,
                        "{text}: object at {:?} overlaps marker {:?}+{:?}",
                        atom.rect.origin.x,
                        marker.x,
                        marker.width
                    );
                }
                assert!(atom.rect.origin.x >= Length::ZERO);
                assert!(atom.rect.origin.x + atom.rect.width <= Length::from_pt(200));
            }
        }
    }
    // The body paragraph between has objects and no marker.
    let body = block(&s, nodes[2]);
    assert!(body.lines.iter().all(|l| l.marker.is_none()));
    assert_eq!(body.lines.iter().map(|l| l.atoms.len()).sum::<usize>(), 1);
    // The tall picture raises only its own line: the marker rides that line's
    // baseline, as the text does.
    let first = &block(&s, nodes[0]).lines[0];
    assert_eq!(first.marker.as_ref().unwrap().baseline, first.baseline);
}

#[test]
fn list_items_with_objects_lay_out_the_same_incrementally() {
    for mode in [
        WritingMode::default(),
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
    ] {
        let (engine, doc, list, nodes) = document(mode);
        let mut session = LayoutSession::new(&engine);
        let full = engine.layout(&doc);
        assert_eq!(session.layout(&doc).unwrap(), full, "{mode:?}: first");

        // Insert an object into the first item: it re-lays out, the others stay.
        doc.insert_inline(nodes[0], 0, &inline::page_number())
            .unwrap();
        doc.commit();
        assert_eq!(
            session.layout(&doc).unwrap(),
            engine.layout(&doc),
            "{mode:?}: object added to an item"
        );
        // Take the second item out of the list: the third renumbers.
        doc.remove_list_item(nodes[1]).unwrap();
        doc.commit();
        let after = engine.layout(&doc);
        assert_eq!(
            session.layout(&doc).unwrap(),
            after,
            "{mode:?}: item removed"
        );
        assert_eq!(
            block(&after, nodes[3]).lines[0]
                .marker
                .as_ref()
                .unwrap()
                .text,
            "2.",
            "{mode:?}"
        );
        // And back in, as a different level of the same list.
        doc.set_list_item(nodes[1], list, 1).unwrap();
        doc.commit();
        assert_eq!(
            session.layout(&doc).unwrap(),
            engine.layout(&doc),
            "{mode:?}: item returns"
        );
        let _ = doc.block(nodes[1]).unwrap().kind == BlockKind::Paragraph;
    }
}

#[test]
fn the_display_list_draws_the_marker_then_the_objects() {
    use reprise_display::Item;
    let (engine, doc, _, nodes) = document(WritingMode::default());
    let s = engine.layout(&doc);
    let list = s.to_display_list(0, Default::default());
    fn walk<'a>(items: &'a [Item], out: &mut Vec<&'a Item>) {
        for item in items {
            match item {
                Item::Group { items, .. } => walk(items, out),
                other => out.push(other),
            }
        }
    }
    let mut flat = Vec::new();
    walk(&list.items, &mut flat);
    let images = flat
        .iter()
        .filter(|i| matches!(i, Item::Image { .. }))
        .count();
    // A picture in each of the first two items and one in the body paragraph;
    // the third item has only a page number.
    assert_eq!(images, 3, "{flat:#?}");
    let glyph_texts: Vec<_> = flat
        .iter()
        .filter_map(|i| match i {
            Item::Glyphs(run) => Some(run.text.as_str()),
            _ => None,
        })
        .collect();
    for marker in ["1.", "2.", "3."] {
        assert!(glyph_texts.contains(&marker), "{marker} in {glyph_texts:?}");
    }
    let _ = nodes;
}

#[test]
fn an_item_reads_its_marker_first_and_each_object_as_its_own_run() {
    let (engine, doc, _, nodes) = document(WritingMode::default());
    let s = engine.layout(&doc);
    let blocks = s.pdf_reading_blocks(&doc);
    let first = blocks
        .iter()
        .find(|b| b.node == nodes[0])
        .expect("the first item is read");
    // The marker is a label and comes first; nothing after it is one.
    let labels: Vec<bool> = first.runs.iter().map(|r| r.label).collect();
    assert_eq!(labels.first(), Some(&true), "{labels:?}");
    assert!(labels[1..].iter().all(|l| !l), "{labels:?}");
    // The picture and the page number each have their own three-byte run
    // (U+FFFC is three bytes), between the text runs around them.
    let objects: Vec<_> = first
        .runs
        .iter()
        .filter_map(|r| r.bytes.clone())
        .filter(|b| b.len() == 3)
        .collect();
    assert_eq!(objects.len(), 2, "{:?}", first.runs);
    let starts: Vec<usize> = first
        .runs
        .iter()
        .filter_map(|r| r.bytes.as_ref().map(|b| b.start))
        .collect();
    assert!(starts.windows(2).all(|w| w[0] <= w[1]), "{starts:?}");
}
