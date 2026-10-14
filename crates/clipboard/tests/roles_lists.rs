//! Roles and lists end to end (`docs/roles-and-lists.md`): every role through HTML
//! and back, nested mixed lists to depth nine, numbering across body paragraphs,
//! restarts and starts, plain-text markers, a native copy, and the PDF structure,
//! checked by the same checker `pdf_ua.rs` uses.

#[path = "../../display/tests/common/checker.rs"]
mod checker;

use checker::check;
use lopdf::{Document as PdfDocument, Object};
use reprise_clipboard::{
    ExportOptions, Exporter, Html, ImportLimits, NativeFragment, PlainText, copy_all, copy_blocks,
    import_html,
};
use reprise_doc::fragment::CopyBlock;
use reprise_doc::{
    BlockKind, Document, LevelDef, ListDefinition, ListId, NodeId, NumberFormat, Role,
    SchemaRegistry,
};
use reprise_edit::Editor;
use reprise_fixtures::PEER;

fn blank() -> Document {
    Document::new(PEER).unwrap()
}

fn text_block(doc: &Document, text: &str, role: Role) -> NodeId {
    let node = doc.append_block(BlockKind::Paragraph, "", text).unwrap();
    doc.set_role(node, role).unwrap();
    node
}

fn body(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "", text).unwrap()
}

fn item(doc: &Document, text: &str, list: ListId, level: u8) -> NodeId {
    let node = doc.append_block(BlockKind::Paragraph, "", text).unwrap();
    doc.set_list_item(node, list, level).unwrap();
    node
}

/// A definition whose level `n` shows `formats[n]`, with default templates.
fn list_of(formats: &[NumberFormat]) -> ListDefinition {
    ListDefinition {
        base: None,
        levels: formats
            .iter()
            .enumerate()
            .map(|(n, f)| {
                (
                    n as u8,
                    LevelDef {
                        format: Some(f.clone()),
                        ..LevelDef::default()
                    },
                )
            })
            .collect(),
    }
}

/// What a reader sees of one block, without the list's identity.
#[derive(Debug, PartialEq)]
struct Seen {
    text: String,
    role: Role,
    marker: Option<String>,
    level: Option<u8>,
}

fn seen(doc: &Document) -> Vec<Seen> {
    doc.document_order()
        .into_iter()
        .map(|node| {
            let item = doc.list_item(node);
            Seen {
                text: doc.block(node).unwrap().text.to_string(),
                role: doc.role(node).unwrap(),
                marker: item.as_ref().map(|i| i.marker.clone()),
                level: item.as_ref().map(|i| i.level),
            }
        })
        .collect()
}

fn markers(doc: &Document) -> Vec<String> {
    doc.document_order()
        .into_iter()
        .filter_map(|node| doc.list_item(node).map(|i| i.marker))
        .collect()
}

fn options(schemas: &SchemaRegistry) -> ExportOptions<'_> {
    ExportOptions {
        source_namespace: "source",
        schemas,
        fonts: None,
    }
}

fn html_of(doc: &Document) -> String {
    let schemas = SchemaRegistry::builtin();
    String::from_utf8(Html.export(doc, None, &options(&schemas)).unwrap().bytes).unwrap()
}

fn plain_of(doc: &Document) -> String {
    let schemas = SchemaRegistry::builtin();
    String::from_utf8(
        PlainText
            .export(doc, None, &options(&schemas))
            .unwrap()
            .bytes,
    )
    .unwrap()
}

/// Whether `html` has an element `tag` holding exactly `text`, with `class` on
/// its opening tag when one is given. Other attributes (direction, style) are
/// ignored: the exporter writes them on every element.
fn has_element(html: &str, tag: &str, class: Option<&str>, text: &str) -> bool {
    let close = format!(">{text}</{tag}>");
    html.match_indices(&close).any(|(end, _)| {
        let Some(start) = html[..end].rfind(&format!("<{tag}")) else {
            return false;
        };
        let open = &html[start..end];
        let after_name = open[1 + tag.len()..].chars().next();
        after_name.is_some_and(|c| c == ' ' || c == '>')
            && class.is_none_or(|c| open.contains(&format!("class=\"{c}\"")))
    })
}

/// The deepest `ol`/`ul` nesting in an HTML string.
fn list_depth(html: &str) -> usize {
    let (mut depth, mut max, mut rest) = (0usize, 0usize, html);
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        if rest.starts_with("<ol") || rest.starts_with("<ul") {
            depth += 1;
            max = max.max(depth);
        } else if rest.starts_with("</ol>") || rest.starts_with("</ul>") {
            depth = depth.saturating_sub(1);
        }
        rest = &rest[1..];
    }
    max
}

fn squeeze(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The structure tree as nested role names, such as `L(LI(Lbl,LBody(P)))`.
/// Marked-content leaves are dropped; only elements are named.
fn tree(pdf: &PdfDocument, k: &Object) -> Vec<String> {
    let object = match k {
        Object::Reference(id) => match pdf.get_object(*id) {
            Ok(o) => o.clone(),
            Err(_) => return Vec::new(),
        },
        o => o.clone(),
    };
    match object {
        Object::Array(items) => items.iter().flat_map(|i| tree(pdf, i)).collect(),
        Object::Dictionary(d) => {
            if matches!(d.get(b"Type").ok().and_then(|t| t.as_name().ok()), Some(t) if t == b"MCR" || t == b"OBJR")
            {
                return Vec::new();
            }
            let name = String::from_utf8(d.get(b"S").unwrap().as_name().unwrap().to_vec()).unwrap();
            let kids = d.get(b"K").map(|k| tree(pdf, k)).unwrap_or_default();
            if kids.is_empty() {
                vec![name]
            } else {
                vec![format!("{name}({})", kids.join(","))]
            }
        }
        _ => Vec::new(),
    }
}

fn structure(bytes: &[u8]) -> String {
    let pdf = PdfDocument::load_mem(bytes).unwrap();
    let root = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let catalog = pdf.get_dictionary(root).unwrap();
    let struct_root = catalog
        .get(b"StructTreeRoot")
        .unwrap()
        .as_reference()
        .unwrap();
    let k = pdf.get_dictionary(struct_root).unwrap().get(b"K").unwrap();
    tree(&pdf, k).join(",")
}

#[test]
fn every_role_survives_html_and_the_outline_reads_the_headings() {
    let roles = [
        ("Body", Role::Body),
        ("Title", Role::Title),
        ("Subtitle", Role::Subtitle),
        ("One", Role::Heading(1)),
        ("Two", Role::Heading(2)),
        ("Three", Role::Heading(3)),
        ("Four", Role::Heading(4)),
        ("Five", Role::Heading(5)),
        ("Six", Role::Heading(6)),
        ("Quote", Role::Quote),
        ("Code", Role::Code),
        ("Verse", Role::Verse),
    ];
    let doc = blank();
    for (text, role) in roles {
        text_block(&doc, text, role);
    }
    doc.commit();
    let html = html_of(&doc);
    for (tag, class, text) in [
        ("h1", Some("reprise-title"), "Title"),
        ("p", Some("reprise-subtitle"), "Subtitle"),
        ("h1", None, "One"),
        ("h2", None, "Two"),
        ("h3", None, "Three"),
        ("h4", None, "Four"),
        ("h5", None, "Five"),
        ("h6", None, "Six"),
        ("blockquote", None, "Quote"),
        ("pre", None, "Code"),
        ("p", Some("reprise-verse"), "Verse"),
    ] {
        assert!(
            has_element(&html, tag, class, text),
            "<{tag}> {class:?} {text:?} is missing from {html}"
        );
    }

    let imported = import_html(&html, ImportLimits::default()).unwrap();
    let read: Vec<(String, Role)> = imported
        .fragment
        .fragment
        .blocks
        .iter()
        .map(|b| (b.text.clone(), b.role.unwrap_or(Role::Body)))
        .collect();
    let expected: Vec<(String, Role)> = roles.iter().map(|(t, r)| (t.to_string(), *r)).collect();
    assert_eq!(read, expected);

    let mut editor = Editor::new(blank(), SchemaRegistry::builtin());
    imported
        .fragment
        .paste(&mut editor, None, "target")
        .unwrap();
    let pasted = editor.document();
    let outline: Vec<(u8, String)> = pasted
        .outline()
        .into_iter()
        .map(|e| (e.level, e.text))
        .collect();
    let headings: Vec<(u8, String)> = [
        (1, "One"),
        (2, "Two"),
        (3, "Three"),
        (4, "Four"),
        (5, "Five"),
        (6, "Six"),
    ]
    .iter()
    .map(|(l, t)| (*l, t.to_string()))
    .collect();
    assert_eq!(
        outline, headings,
        "Title, Subtitle and Quote are not in the outline"
    );
    let pasted_roles: Vec<Role> = seen(pasted).into_iter().map(|s| s.role).collect();
    assert_eq!(
        pasted_roles,
        expected.iter().map(|(_, r)| *r).collect::<Vec<_>>()
    );
}

#[test]
fn mixed_lists_nine_levels_deep_round_trip_through_html() {
    use NumberFormat as F;
    let doc = blank();
    let list = doc
        .create_list(&list_of(&[
            F::Decimal,
            F::Bullet,
            F::LowerAlpha,
            F::UpperRoman,
            F::Bullet,
            F::DecimalLeadingZero,
            F::LowerRoman,
            F::UpperAlpha,
            F::Decimal,
        ]))
        .unwrap();
    // Down to level 8 and back out, with siblings at several depths.
    let levels = [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 8, 7, 3, 2, 0, 1];
    for (i, level) in levels.into_iter().enumerate() {
        item(&doc, &format!("item {i}"), list, level);
    }
    doc.commit();

    // The numbering the definition asks for, worked out by hand: each level
    // counts its own items, and a shallower item clears the deeper counters.
    assert_eq!(
        markers(&doc),
        [
            "1.", "◦", "a.", "I.", "◦", "01.", "i.", "A.", "1.", "2.", "B.", "II.", "b.", "2.", "◦"
        ]
    );

    let html = html_of(&doc);
    assert_eq!(list_depth(&html), 9, "nine nested lists");
    assert_eq!(html.matches("<li").count(), levels.len(), "one li per item");

    let imported = import_html(&html, ImportLimits::default()).unwrap();
    let mut editor = Editor::new(blank(), SchemaRegistry::builtin());
    imported
        .fragment
        .paste(&mut editor, None, "target")
        .unwrap();
    assert_eq!(seen(editor.document()), seen(&doc));
}

#[test]
fn numbering_continues_across_body_text_and_restarts_and_starts_where_told() {
    let doc = blank();
    let mut def = list_of(&[NumberFormat::Decimal, NumberFormat::LowerAlpha]);
    def.levels.get_mut(&0).unwrap().start = Some(5);
    let list = doc.create_list(&def).unwrap();
    item(&doc, "A", list, 0);
    item(&doc, "B", list, 0);
    let between = body(&doc, "Body between items.");
    let c = item(&doc, "C", list, 0);
    item(&doc, "D", list, 1);
    item(&doc, "E", list, 1);
    let f = item(&doc, "F", list, 0);
    doc.set_list_restart(f, Some(10)).unwrap();
    item(&doc, "G", list, 0);
    doc.commit();

    // C continues the list across the body paragraph; D and E count at level 1
    // from their own start; F restarts at 10 and resets the deeper counters.
    assert_eq!(markers(&doc), ["5.", "6.", "7.", "a.", "b.", "10.", "11."]);
    assert!(doc.list_item(between).is_none());
    assert_eq!(doc.list_item(c).unwrap().value, 7);
    assert!(doc.list_item(f).unwrap().restart);

    let html = html_of(&doc);
    assert!(html.contains("start=\"5\""), "{html}");
    assert!(html.contains("value=\"10\""), "{html}");
    // The body paragraph closes the list, so C opens a new one at its value.
    assert!(html.contains("start=\"7\""), "{html}");

    let imported = import_html(&html, ImportLimits::default()).unwrap();
    let mut editor = Editor::new(blank(), SchemaRegistry::builtin());
    imported
        .fragment
        .paste(&mut editor, None, "target")
        .unwrap();
    assert_eq!(markers(editor.document()), markers(&doc));
    assert_eq!(seen(editor.document()), seen(&doc));
}

#[test]
fn plain_text_writes_each_marker_then_a_space_and_four_spaces_per_level() {
    let doc = blank();
    let list = doc
        .create_list(&list_of(&[
            NumberFormat::Decimal,
            NumberFormat::Bullet,
            NumberFormat::LowerAlpha,
        ]))
        .unwrap();
    item(&doc, "one", list, 0);
    item(&doc, "nested", list, 1);
    item(&doc, "deep", list, 2);
    body(&doc, "plain");
    item(&doc, "two", list, 0);
    let three = item(&doc, "three", list, 0);
    doc.set_list_restart(three, Some(7)).unwrap();
    doc.commit();
    assert_eq!(
        plain_of(&doc),
        "1. one\n\n    ◦ nested\n\n        a. deep\n\nplain\n\n2. two\n\n7. three"
    );
}

#[test]
fn a_native_copy_keeps_roles_and_lists_and_pastes_fresh_definitions() {
    let doc = blank();
    let list = doc
        .create_list(&list_of(&[NumberFormat::Decimal, NumberFormat::Bullet]))
        .unwrap();
    text_block(&doc, "Heading", Role::Heading(2));
    item(&doc, "one", list, 0);
    item(&doc, "nested", list, 1);
    text_block(&doc, "Quoted.", Role::Quote);
    doc.commit();
    let original = seen(&doc);
    let original_list = doc.list_item(doc.document_order()[1]).unwrap().list;

    let schemas = SchemaRegistry::builtin();
    let bytes = copy_all(&doc, "source", &schemas, None, None)
        .unwrap()
        .encode()
        .unwrap();
    let fragment = NativeFragment::decode(&bytes).unwrap();
    let mut editor = Editor::new(doc, SchemaRegistry::builtin());
    fragment.paste(&mut editor, None, "target").unwrap();

    let all = seen(editor.document());
    assert_eq!(all.len(), original.len() * 2);
    let (kept, copied) = all.split_at(original.len());
    assert_eq!(kept, original.as_slice());
    assert_eq!(copied, original.as_slice());
    let pasted = editor
        .document()
        .document_order()
        .into_iter()
        .filter_map(|n| editor.document().list_item(n))
        .last()
        .unwrap();
    assert_ne!(
        pasted.list, original_list,
        "pasted items use a fresh definition"
    );
}

/// Marker and list of every item of `editor`'s document, in order.
fn listed(editor: &Editor) -> Vec<(String, ListId)> {
    let doc = editor.document();
    doc.document_order()
        .into_iter()
        .filter_map(|n| doc.list_item(n).map(|i| (i.marker, i.list)))
        .collect()
}

#[test]
fn pasting_items_into_their_own_document_continues_the_list() {
    let doc = blank();
    let list = doc.create_list(&list_of(&[NumberFormat::Decimal])).unwrap();
    let ids: Vec<NodeId> = ["a", "b", "c", "d"]
        .iter()
        .map(|t| item(&doc, t, list, 0))
        .collect();
    doc.commit();
    let schemas = SchemaRegistry::builtin();
    let selection = |nodes: &[NodeId]| -> Vec<CopyBlock> {
        nodes
            .iter()
            .map(|&node| CopyBlock { node, bytes: None })
            .collect()
    };
    let fragment = copy_blocks(&doc, "doc", &selection(&ids[1..3]), &schemas, None, None).unwrap();
    let mut editor = Editor::new(doc, SchemaRegistry::builtin());
    let before = editor.document().revision();
    let pasted = fragment.paste(&mut editor, None, "doc").unwrap();
    // The pasted b and c join the list after d: no new definition.
    assert!(pasted.applied.lists.is_empty(), "no fresh definition");
    let items = listed(&editor);
    assert_eq!(
        items.iter().map(|i| i.0.as_str()).collect::<Vec<_>>(),
        ["1.", "2.", "3.", "4.", "5.", "6."]
    );
    assert!(items.iter().all(|i| i.1 == list), "one list throughout");
    assert!(editor.undo().unwrap());
    assert_eq!(listed(&editor).len(), 4);
    assert_ne!(
        before,
        editor.document().revision(),
        "undo is a new revision"
    );

    // Another document gets its own definition.
    let mut other = Editor::new(blank(), SchemaRegistry::builtin());
    let foreign = fragment.paste(&mut other, None, "elsewhere").unwrap();
    assert_eq!(foreign.applied.lists.len(), 1);
    assert_eq!(
        listed(&other)
            .iter()
            .map(|i| i.0.as_str())
            .collect::<Vec<_>>(),
        ["1.", "2."]
    );
    assert_ne!(listed(&other)[0].1, list);
}

#[test]
fn pasting_after_the_list_was_deleted_brings_a_fresh_definition_back() {
    let doc = blank();
    let list = doc
        .create_list(&list_of(&[NumberFormat::UpperAlpha]))
        .unwrap();
    let a = item(&doc, "a", list, 0);
    doc.commit();
    let schemas = SchemaRegistry::builtin();
    let fragment = copy_blocks(
        &doc,
        "doc",
        &[CopyBlock {
            node: a,
            bytes: None,
        }],
        &schemas,
        None,
        None,
    )
    .unwrap();
    doc.delete_list(list).unwrap();
    doc.commit();
    let mut editor = Editor::new(doc, SchemaRegistry::builtin());
    let pasted = fragment.paste(&mut editor, None, "doc").unwrap();
    assert_eq!(pasted.applied.lists.len(), 1, "the definition is recreated");
    let items = listed(&editor);
    assert_eq!(
        items.len(),
        1,
        "the deleted list's own item is a plain paragraph"
    );
    assert_eq!(items[0].0, "A.");
    assert_ne!(items[0].1, list);
}

#[test]
fn pdf_tags_name_every_role_and_nest_lists_as_l_li_lbl_lbody() {
    let doc = blank();
    text_block(&doc, "Navidson Record", Role::Title);
    text_block(&doc, "The House", Role::Heading(1));
    body(&doc, "A room.");
    for n in 2..=6 {
        text_block(&doc, &format!("Level {n}"), Role::Heading(n));
    }
    text_block(&doc, "Quoted.", Role::Quote);
    text_block(&doc, "fn main() {}", Role::Code);
    let list = doc
        .create_list(&list_of(&[
            NumberFormat::Decimal,
            NumberFormat::Bullet,
            NumberFormat::LowerAlpha,
        ]))
        .unwrap();
    item(&doc, "one", list, 0);
    item(&doc, "nested", list, 1);
    item(&doc, "deep", list, 2);
    item(&doc, "two", list, 0);
    doc.commit();

    let engine = reprise_fixtures::engine();
    let layout = engine.layout(&doc);
    let pdf = reprise_clipboard::export_pdf(
        &doc,
        &layout,
        &engine.fonts,
        &engine.assets,
        &Default::default(),
    )
    .unwrap();
    let report = check(&pdf.bytes);
    assert_eq!(report.roles.first().map(String::as_str), Some("Document"));
    assert!(report.marked);
    assert!(
        report.xmp.contains("Navidson Record"),
        "the title role titles the PDF"
    );

    // Each item's label comes before its body, and a nested list is inside the
    // parent item's body.
    let deep = "LI(Lbl,LBody(P))";
    let nested = format!("LI(Lbl,LBody(P,L({deep})))");
    let one = format!("LI(Lbl,LBody(P,L({nested})))");
    let two = "LI(Lbl,LBody(P))";
    let expected = format!("Document(Title,H1,P,H2,H3,H4,H5,H6,BlockQuote,Code,L({one},{two}))");
    assert_eq!(structure(&pdf.bytes), expected);

    // The structure reads the labels with the text, in the plain-text export's order.
    assert_eq!(squeeze(&report.text), squeeze(&plain_of(&doc)));
}

#[test]
fn skipped_levels_and_empty_roles_survive_and_pre_whitespace_is_literal() {
    let doc = blank();
    let list = doc
        .create_list(&list_of(&[
            NumberFormat::Decimal,
            NumberFormat::LowerAlpha,
            NumberFormat::UpperRoman,
        ]))
        .unwrap();
    item(&doc, "first deep", list, 2);
    item(&doc, "root", list, 0);
    item(&doc, "skip", list, 2);
    text_block(&doc, "", Role::Heading(4));
    doc.commit();
    let html = html_of(&doc);
    let imported = import_html(&html, ImportLimits::default()).unwrap();
    let mut ed = Editor::new(blank(), SchemaRegistry::builtin());
    ed.paste(&imported.fragment.fragment, None, "test").unwrap();
    assert_eq!(seen(&doc), seen(ed.document()));
    let pre = import_html("<pre>  first\n    second  </pre>", ImportLimits::default()).unwrap();
    assert_eq!(
        pre.fragment.fragment.blocks[0].text,
        "  first\n    second  "
    );
}
