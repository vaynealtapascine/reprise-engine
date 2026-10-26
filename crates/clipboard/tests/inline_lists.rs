//! List items that hold inline objects at the clipboard boundary
//! (`docs/inline.md`, `docs/roles-and-lists.md`): HTML that imports again as
//! the same items with the same objects, a native copy that pastes both with
//! fresh identity, and plain text that writes the marker then the item.

use reprise_clipboard::{
    ExportOptions, Exporter, Html, ImportLimits, NativeFragment, PlainText, copy_all, import_html,
};
use reprise_doc::{Document, InlineObject, ListDefinition, NodeId, SchemaRegistry};
use reprise_edit::Editor;
use reprise_fixtures::inline::{self, Part};
use reprise_layout::{Engine, LayoutSnapshot};

struct Setup {
    engine: Engine,
    doc: Document,
    nodes: Vec<NodeId>,
    layout: LayoutSnapshot,
}

/// Two numbered items and a body paragraph, each with objects.
fn setup() -> Setup {
    let (engine, doc, asset) = inline::page(300, 300, Default::default()).unwrap();
    let list = doc.create_list(&ListDefinition::numbered()).unwrap();
    let a = inline::paragraph(
        &doc,
        vec![
            Part::Text("See "),
            Part::Object(inline::image(&asset, 24, 12, "a cat")),
            Part::Text(" here"),
        ],
    )
    .unwrap();
    let b = inline::paragraph(
        &doc,
        vec![
            Part::Text("Page "),
            Part::Object(inline::page_number()),
            Part::Text(" of "),
            Part::Object(InlineObject::field("text").with("text", "many")),
        ],
    )
    .unwrap();
    let c = inline::paragraph(
        &doc,
        vec![
            Part::Text("Body "),
            Part::Object(inline::image(&asset, 8, 8, "dot")),
        ],
    )
    .unwrap();
    doc.set_list_item(a, list, 0).unwrap();
    doc.set_list_item(b, list, 0).unwrap();
    doc.commit();
    let layout = engine.layout(&doc);
    Setup {
        engine,
        doc,
        nodes: vec![a, b, c],
        layout,
    }
}

fn options<'a>(s: &'a Setup, schemas: &'a SchemaRegistry) -> ExportOptions<'a> {
    ExportOptions {
        source_namespace: "source",
        schemas,
        fonts: Some(&s.engine.fonts),
    }
}

/// An object as a reader sees it: its kind and its sorted parameters.
type SeenObject = (String, Vec<(String, String)>);

/// A block as a reader sees it: its marker, its text (with a U+FFFC for each
/// object) and its objects.
type SeenBlock = (Option<String>, String, Vec<SeenObject>);

/// What a reader sees of each block of `doc`, in order.
fn seen(doc: &Document) -> Vec<SeenBlock> {
    doc.order(reprise_doc::Domain::Body)
        .into_iter()
        .filter(|&n| doc.block(n).is_ok())
        .map(|n| {
            let text = doc.block(n).unwrap().text.to_string();
            let objects = doc
                .inline_objects(n)
                .unwrap()
                .into_iter()
                .map(|e| {
                    let mut params: Vec<_> = e
                        .object
                        .params
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    params.sort();
                    (e.object.kind.clone(), params)
                })
                .collect();
            (doc.list_item(n).map(|i| i.marker), text, objects)
        })
        .collect()
}

#[test]
fn html_writes_items_with_their_objects_and_reads_them_back() {
    let s = setup();
    let schemas = SchemaRegistry::builtin();
    let html = String::from_utf8(
        Html.export(&s.doc, Some(&s.layout), &options(&s, &schemas))
            .unwrap()
            .bytes,
    )
    .unwrap();
    assert!(html.contains("<ol"), "{html}");
    assert!(html.contains("<li"), "{html}");
    // The picture and the resolved page number are inside the first two items.
    let first = html.find("<li").unwrap();
    let list_end = html.find("</ol>").unwrap();
    let items = &html[first..list_end];
    assert!(items.contains("<img "), "{items}");
    assert!(items.contains("data-field=\"page-number\""), "{items}");
    assert!(items.contains(">1</span>"), "{items}");
    // The body paragraph's picture is outside the list.
    assert!(html[list_end..].contains("<img "), "{html}");

    let imported = import_html(&html, ImportLimits::default()).unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    imported
        .fragment
        .paste(&mut editor, None, "target")
        .unwrap();
    let before = seen(&s.doc);
    let after = seen(editor.document());
    assert_eq!(after.len(), before.len(), "{after:?}");
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(a.0, b.0, "marker");
        assert_eq!(a.1, b.1, "text with its object characters");
        // Kinds and order of the objects survive; the page-number field keeps
        // its function and the images their alt text.
        assert_eq!(
            a.2.iter().map(|o| o.0.as_str()).collect::<Vec<_>>(),
            b.2.iter().map(|o| o.0.as_str()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn a_native_copy_pastes_items_and_objects_with_fresh_identity() {
    let s = setup();
    let schemas = SchemaRegistry::builtin();
    let bytes = copy_all(&s.doc, "source", &schemas, None, None)
        .unwrap()
        .encode()
        .unwrap();
    let fragment = NativeFragment::decode(&bytes).unwrap();
    let originals: Vec<_> = s
        .nodes
        .iter()
        .flat_map(|&n| s.doc.inline_objects(n).unwrap())
        .map(|e| e.id)
        .collect();
    let list_before = s.doc.list_item(s.nodes[0]).unwrap().list;
    let mut editor = Editor::new(s.doc, SchemaRegistry::builtin());
    fragment.paste(&mut editor, None, "target").unwrap();

    let doc = editor.document();
    let all = seen(doc);
    let (kept, copied) = all.split_at(all.len() / 2);
    assert_eq!(kept, copied, "the paste repeats what was copied");
    // Fresh identities: no object of the copy shares an id with the source.
    let order = doc.order(reprise_doc::Domain::Body);
    let pasted_ids: Vec<_> = order[order.len() / 2..]
        .iter()
        .flat_map(|&n| doc.inline_objects(n).unwrap())
        .map(|e| e.id)
        .collect();
    assert_eq!(pasted_ids.len(), originals.len());
    assert!(pasted_ids.iter().all(|id| !originals.contains(id)));
    // And a fresh list definition, as for any pasted items.
    let pasted_list = doc.list_item(order[order.len() / 2]).unwrap().list;
    assert_ne!(pasted_list, list_before);
}

#[test]
fn plain_text_writes_the_marker_then_the_item_with_its_objects_as_text() {
    let s = setup();
    let schemas = SchemaRegistry::builtin();
    let text = String::from_utf8(
        PlainText
            .export(&s.doc, Some(&s.layout), &options(&s, &schemas))
            .unwrap()
            .bytes,
    )
    .unwrap();
    // Paragraphs are separated by a blank line.
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    assert!(lines[0].starts_with("1. See "), "{lines:?}");
    assert!(lines[0].contains("a cat"), "{lines:?}");
    assert!(lines[1].starts_with("2. Page 1 of many"), "{lines:?}");
    assert!(lines[2].starts_with("Body "), "{lines:?}");
    assert!(lines[2].contains("dot"), "{lines:?}");
}
