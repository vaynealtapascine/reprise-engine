//! Sections in HTML and plain text (sections §15, S10): the export writes each
//! section, its stories (once, by label), bindings with their three states,
//! breaks and the geometry mode; the import reads them back; losses are
//! reported; plain text is the body only.

use reprise_clipboard::{ExportOptions, Exporter, Html, ImportLimits, PlainText, import_html};
use reprise_doc::annotation::{AnnotationData, LinkTarget};
use reprise_doc::sections::{BreakKind, SectionId, SectionProperties};
use reprise_doc::{
    BlockKind, Document, Domain, InlineObject, LengthExpr, NewBlock, NodeId, SchemaRegistry, Style,
};
use reprise_edit::Editor;
use reprise_geom::Length;
use serde_json::{Value, json};

fn doc() -> Document {
    let doc = Document::new(1).unwrap();
    doc.define_style(
        "s",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(10))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(12))),
            ..Style::default()
        },
    )
    .unwrap();
    doc
}

fn props(entries: &[(&str, Value)]) -> SectionProperties {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn first(doc: &Document, entries: &[(&str, Value)]) {
    doc.write_section_properties(SectionId::First, "0", &props(entries), &[])
        .unwrap();
}

fn start(doc: &Document, node: NodeId, kind: BreakKind, entries: &[(&str, Value)]) {
    let life = doc.mint_lifetime(node).unwrap();
    doc.activate_start(node, &life, kind, &props(entries))
        .unwrap();
}

fn para(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "s", text).unwrap()
}

fn story(doc: &Document, texts: &[&str]) -> NodeId {
    let blocks: Vec<_> = texts
        .iter()
        .map(|t| NewBlock::new(BlockKind::Paragraph, "s", t))
        .collect();
    doc.create_story(&blocks).unwrap()
}

fn id(story: NodeId) -> Value {
    json!(story.to_string())
}

struct Exported {
    html: String,
    notes: Vec<reprise_diag::Note>,
}

fn export_html(doc: &Document, with_layout: bool) -> Exported {
    doc.commit();
    let engine = reprise_fixtures::engine();
    let layout = engine.layout(doc);
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &engine.schemas,
        fonts: Some(&engine.fonts),
    };
    let out = Html
        .export(doc, with_layout.then_some(&layout), &options)
        .unwrap();
    Exported {
        html: String::from_utf8(out.bytes).unwrap(),
        notes: out.losses.notes,
    }
}

/// A document with every kind of thing the export writes: a header with a page
/// number and a link, a footer, a background; three sections whose bindings are
/// inherited, none and a story; columns; a numbering format; an unknown key; a
/// page break; a link in the body; and a pageless geometry mode.
fn source() -> (Document, Vec<NodeId>, [NodeId; 3]) {
    let doc = doc();
    let head = story(&doc, &["Head "]);
    let head_block = doc.top(Domain::Story(head))[0];
    doc.insert_inline(head_block, 5, &InlineObject::field("page-number"))
        .unwrap();
    let link_end = doc.block(head_block).unwrap().text.len();
    let _ = link_end;
    doc.add_annotation(
        head_block,
        0..4,
        &AnnotationData::Link(LinkTarget::Url {
            url: "https://example.com/head".into(),
        }),
    )
    .unwrap();
    let foot = story(&doc, &["Foot one", "Foot two"]);
    let mark = story(&doc, &["DRAFT"]);
    first(
        &doc,
        &[
            ("region.header.default", id(head)),
            ("region.footer.default", id(foot)),
            ("unknown.first", json!("kept")),
        ],
    );
    let p: Vec<_> = (0..6)
        .map(|i| para(&doc, &format!("paragraph {i}")))
        .collect();
    doc.add_annotation(
        p[0],
        0..4,
        &AnnotationData::Link(LinkTarget::Url {
            url: "https://example.com/body".into(),
        }),
    )
    .unwrap();
    // Section 2: no footer (explicit none), columns, roman numbers, an unknown key.
    start(
        &doc,
        p[2],
        BreakKind::Section,
        &[
            ("region.footer.default", json!("")),
            ("columns.count", json!(2)),
            ("columns.gap", json!(12 * 1024)),
            ("numbering.format", json!("lower-roman")),
            ("future.key", json!("also kept")),
            ("start", json!("odd-page")),
        ],
    );
    // Section 3: a background, the first-page header off.
    start(
        &doc,
        p[4],
        BreakKind::Section,
        &[
            ("region.background.default", id(mark)),
            ("region.header.first", json!("")),
            ("region.first", json!(true)),
        ],
    );
    start(&doc, p[5], BreakKind::Page, &[]);
    doc.set_geometry_mode("pageless").unwrap();
    doc.commit();
    (doc, p, [head, foot, mark])
}

#[test]
fn a_document_without_sections_is_exported_as_it_always_was() {
    let doc = doc();
    para(&doc, "plain");
    let out = export_html(&doc, true);
    assert!(!out.html.contains("<section"));
    assert!(!out.html.contains("data-reprise-section"));
    assert!(!out.html.contains("data-reprise-geometry"));
    assert!(out.html.starts_with("<!doctype html><html><body><p "));
    assert!(!out.notes.iter().any(|n| n.code == "export.sections"));
}

#[test]
fn the_export_writes_sections_stories_once_bindings_breaks_and_the_geometry() {
    let (doc, _, [head, foot, mark]) = source();
    let out = export_html(&doc, true);
    let html = &out.html;
    // Three sections, the first holding `first`'s stored form.
    assert_eq!(
        html.matches("<section data-reprise-section=").count(),
        3,
        "{html}"
    );
    assert!(html.contains("&quot;first&quot;:true"), "{html}");
    // The header and footer are visible, once, in the first section that binds
    // them as the default variant; the background (and any other variant) is inert.
    assert_eq!(
        html.matches("<header data-reprise-story=\"s0\">").count(),
        1
    );
    assert_eq!(
        html.matches("<footer data-reprise-story=\"s1\">").count(),
        1
    );
    assert_eq!(
        html.matches("<template data-reprise-story=\"s2\">").count(),
        1
    );
    assert_eq!(
        html.matches("Foot one").count(),
        1,
        "a story is written once"
    );
    assert_eq!(html.matches("DRAFT").count(), 1);
    // Bindings keep their three states: a label, "" for none, absent for same
    // as previous.
    assert!(
        html.contains("&quot;region.footer.default&quot;:&quot;&quot;"),
        "{html}"
    );
    assert!(html.contains("&quot;region.header.default&quot;:&quot;s0&quot;"));
    assert!(html.contains("&quot;region.background.default&quot;:&quot;s2&quot;"));
    // Unknown keys travel; a page break is CSS on its block; the geometry mode
    // is on the root element.
    assert!(html.contains("future.key"));
    assert!(html.contains("break-before: page"));
    assert!(html.contains("<body data-reprise-geometry="), "{html}");
    assert!(html.contains("pageless"));
    // CSS columns for viewers.
    assert!(html.contains("columns: 2; column-gap: 12pt"), "{html}");
    // A page field in a story shows the page it is first placed on.
    assert!(html.contains("data-field=\"page-number\""));
    assert!(html.contains(">1</span>"), "{html}");
    let _ = (head, foot, mark);
    // The losses are said.
    let sections: Vec<_> = out
        .notes
        .iter()
        .filter(|n| n.code == "export.sections")
        .collect();
    assert!(
        sections
            .iter()
            .any(|n| n.message.contains("pages are not in HTML")),
        "{:?}",
        out.notes
    );
    // Without a layout a page field is a question mark.
    let bare = export_html(&doc, false);
    assert!(bare.html.contains(">?</span>"), "{}", bare.html);
    assert!(bare.notes.iter().any(|n| n.message.contains("no layout")));
}

/// A document, read back through its HTML and pasted into an empty one.
fn round_trip(doc: &Document) -> (Document, reprise_clipboard::Import) {
    let out = export_html(doc, true);
    let imported = import_html(&out.html, ImportLimits::default()).unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    imported
        .fragment
        .paste(&mut editor, None, "html-roundtrip")
        .unwrap();
    (editor.into_document(), imported)
}

fn story_texts(doc: &Document, story: NodeId) -> Vec<String> {
    doc.top(Domain::Story(story))
        .into_iter()
        .map(|n| {
            doc.block(n)
                .unwrap()
                .text
                .to_string()
                .replace('\u{fffc}', "")
        })
        .collect()
}

#[test]
fn sections_survive_html_with_their_bindings_none_variants_links_and_geometry() {
    let (doc, _, _) = source();
    let (back, imported) = round_trip(&doc);
    // Sections: first, and the two section starts, with their own keys.
    let before = doc.section_table();
    let after = back.section_table();
    assert_eq!(before.len(), after.len());
    for (a, b) in before.iter().zip(&after) {
        let plain = |view: &reprise_doc::sections::SectionView| -> SectionProperties {
            view.own
                .iter()
                .filter(|(k, _)| !reprise_doc::sections::binding_key(k) && k.as_str() != "break")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        assert_eq!(plain(a), plain(b), "{:?}", a.id);
        // Bindings: the same state in every slot and variant, and the stories
        // they resolve to say the same.
        for (x, y) in a.bindings.iter().zip(&b.bindings) {
            assert_eq!(
                (&x.slot, &x.variant, &x.authored),
                (&y.slot, &y.variant, &y.authored)
            );
            assert_eq!(
                x.resolved.map(|s| story_texts(&doc, s)),
                y.resolved.map(|s| story_texts(&back, s)),
                "{} {}",
                x.slot,
                x.variant
            );
        }
    }
    assert!(
        after[0].own.contains_key("unknown.first") && after[1].own.contains_key("future.key"),
        "unknown keys are kept"
    );
    // The page break is on the same block, by position.
    let breaks = |d: &Document| -> Vec<usize> {
        let body = d.top(Domain::Body);
        d.effective_starts()
            .into_iter()
            .filter(|r| r.kind == Some(BreakKind::Page))
            .filter_map(|r| match r.id {
                SectionId::Block(n) => body.iter().position(|b| *b == n),
                SectionId::First => None,
            })
            .collect()
    };
    assert_eq!(breaks(&doc), breaks(&back));
    assert_eq!(breaks(&back), [5]);
    // The geometry mode.
    assert_eq!(back.geometry_mode_record(), doc.geometry_mode_record());
    // The links, in the body and in the header story.
    assert_eq!(back.annotation_count(), doc.annotation_count());
    // The body text.
    let text = |d: &Document| -> Vec<String> {
        d.top(Domain::Body)
            .into_iter()
            .map(|n| d.block(n).unwrap().text.to_string())
            .collect()
    };
    assert_eq!(text(&doc), text(&back));
    assert!(
        !imported
            .notes
            .iter()
            .any(|n| n.severity == reprise_diag::Severity::Error),
        "{:?}",
        imported.notes
    );
}

#[test]
fn losses_are_reported_not_silent() {
    let doc = doc();
    let used = story(&doc, &["Used"]);
    story(&doc, &["Bound to nothing"]);
    first(&doc, &[("region.header.default", id(used))]);
    let p = para(&doc, "a");
    let q = para(&doc, "b");
    let _ = p;
    start(
        &doc,
        q,
        BreakKind::Section,
        &[
            ("columns.count", json!(2)),
            ("columns.widths", json!("1,3")),
            // A story that is not there: kept as written.
            ("region.footer.default", json!("999@9")),
        ],
    );
    let out = export_html(&doc, true);
    let notes: Vec<_> = out
        .notes
        .iter()
        .filter(|n| n.code == "export.sections")
        .collect();
    // An orphan is written, not lost, so there is nothing to report of it.
    assert!(
        !notes
            .iter()
            .any(|n| n.message.contains("bound to no section")),
        "{notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.severity == reprise_diag::Severity::Warning
                && n.message.contains("unequal widths")),
        "{notes:?}"
    );
    assert!(notes.iter().any(|n| n.message.contains("no longer exists")));
    // The orphan is in the HTML, in an inert template after the last section;
    // the dangling binding is, as written.
    let tail = out.html.rsplit("</section>").next().unwrap();
    assert!(
        tail.starts_with("<template data-reprise-story=\"s1\">")
            && tail.contains("Bound to nothing")
            && tail.ends_with("</template></body></html>"),
        "{tail}"
    );
    assert!(out.html.contains("999@9"));
    let (back, _) = round_trip(&doc);
    let texts: Vec<_> = back
        .stories()
        .into_iter()
        .map(|s| story_texts(&back, s))
        .collect();
    assert_eq!(
        texts,
        [["Used"], ["Bound to nothing"]],
        "both stories came back"
    );
    let view = &back.section_table()[1];
    let footer = view
        .bindings
        .iter()
        .find(|b| b.slot == "footer" && b.variant == "default")
        .unwrap();
    assert_eq!(footer.authored, "story");
    assert_eq!(footer.resolved, None, "it still names no live story");
}

#[test]
fn plain_text_is_the_body_only_and_says_what_it_leaves_out() {
    let (doc, _, _) = source();
    doc.commit();
    let engine = reprise_fixtures::engine();
    let layout = engine.layout(&doc);
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &engine.schemas,
        fonts: Some(&engine.fonts),
    };
    for layout in [Some(&layout), None] {
        let out = PlainText.export(&doc, layout, &options).unwrap();
        let text = String::from_utf8(out.bytes).unwrap();
        for hidden in ["Head", "Foot one", "Foot two", "DRAFT"] {
            assert!(!text.contains(hidden), "{hidden} is a story: {text}");
        }
        assert!(
            text.contains("paragraph 0") && text.contains("paragraph 5"),
            "{text}"
        );
        let notes: Vec<_> = out
            .losses
            .notes
            .iter()
            .filter(|n| n.code == "export.sections")
            .collect();
        assert!(
            notes.iter().any(|n| n.message.contains("3 stories")),
            "{:?}",
            out.losses.notes
        );
        assert!(
            notes.iter().any(|n| n.message.contains("breaks")),
            "{:?}",
            out.losses.notes
        );
    }
    // A document with neither reports nothing.
    let plain = self::doc();
    para(&plain, "just text");
    plain.commit();
    let out = PlainText.export(&plain, None, &options).unwrap();
    assert!(!out.losses.notes.iter().any(|n| n.code == "export.sections"));
}

#[test]
fn hostile_markup_never_makes_a_section_it_cannot_hold() {
    let cases: [(&str, &str); 7] = [
        (
            "not json",
            "<section data-reprise-section=\"{\"><p>x</p></section>",
        ),
        (
            "a future version",
            "<section data-reprise-section=\"{&quot;v&quot;:2}\"><p>x</p></section>",
        ),
        (
            "a binding to a story the HTML lacks",
            "<section data-reprise-section=\"{&quot;v&quot;:1,&quot;props&quot;:{},&quot;bindings&quot;:{&quot;region.header.default&quot;:&quot;nope&quot;}}\"><p>x</p></section>",
        ),
        (
            "a key that is not a property",
            "<section data-reprise-section=\"{&quot;v&quot;:1,&quot;props&quot;:{&quot;a/b&quot;:1,&quot;active&quot;:true},&quot;bindings&quot;:{}}\"><p>x</p></section>",
        ),
        (
            "a story inside a story",
            "<header data-reprise-story=\"a\"><p>one</p><footer data-reprise-story=\"b\"><p>two</p></footer></header><p>x</p>",
        ),
        (
            "a duplicate label",
            "<header data-reprise-story=\"a\"><p>one</p></header><header data-reprise-story=\"a\"><p>two</p></header><p>x</p>",
        ),
        (
            "a table in a story",
            "<template data-reprise-story=\"a\"><table><tr><td>cell</td></tr></table></template><p>x</p>",
        ),
    ];
    for (what, html) in cases {
        let imported =
            import_html(html, ImportLimits::default()).unwrap_or_else(|e| panic!("{what}: {e}"));
        imported
            .fragment
            .validate()
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
        imported
            .fragment
            .paste(&mut editor, None, "hostile")
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        let d = editor.document();
        assert!(
            d.top(Domain::Body)
                .iter()
                .any(|n| d.block(*n).unwrap().text.to_string().contains('x')),
            "{what}: the body text is kept"
        );
    }
    // An empty last section has no block to start at: dropped, and said.
    let imported = import_html(
        "<p>a</p><section data-reprise-section=\"{&quot;v&quot;:1,&quot;props&quot;:{},&quot;bindings&quot;:{}}\"></section>",
        ImportLimits::default(),
    )
    .unwrap();
    assert!(
        imported
            .notes
            .iter()
            .any(|n| n.message.contains("no content")),
        "{:?}",
        imported.notes
    );
}
