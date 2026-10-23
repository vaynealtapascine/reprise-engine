//! B3 fixes (the Astra B3 review), finding 5: HTML carries every authored
//! story. A story no section binds is written once as an inert
//! `<template data-reprise-story>` after the last section, and an import keeps
//! it without binding it (sections §6, §15).
use reprise_clipboard::{ExportOptions, Exporter, Html, ImportLimits, import_html};
use reprise_doc::sections::{SectionId, SectionProperties};
use reprise_doc::{BlockKind, Document, Domain, NewBlock, NodeId, SchemaRegistry};
use reprise_edit::Editor;
use serde_json::json;

fn story(doc: &Document, texts: &[&str]) -> NodeId {
    let blocks: Vec<_> = texts
        .iter()
        .map(|t| NewBlock::new(BlockKind::Paragraph, "", t))
        .collect();
    doc.create_story(&blocks).unwrap()
}

fn export(doc: &Document) -> (String, Vec<reprise_diag::Note>) {
    doc.commit();
    let schemas = SchemaRegistry::builtin();
    let out = Html
        .export(
            doc,
            None,
            &ExportOptions {
                source_namespace: "src",
                schemas: &schemas,
                fonts: None,
            },
        )
        .unwrap();
    (String::from_utf8(out.bytes).unwrap(), out.losses.notes)
}

fn paste_into(destination: Document, html: &str) -> Document {
    let imported = import_html(html, ImportLimits::default()).unwrap();
    let mut editor = Editor::new(destination, SchemaRegistry::builtin());
    imported.fragment.paste(&mut editor, None, "dst").unwrap();
    editor.into_document()
}

fn texts(doc: &Document, story: NodeId) -> Vec<String> {
    doc.top(Domain::Story(story))
        .into_iter()
        .map(|n| doc.block(n).unwrap().text.to_string())
        .collect()
}

/// Every story the document's sections bind, whatever the slot.
fn bound(doc: &Document) -> Vec<NodeId> {
    doc.section_table()
        .iter()
        .flat_map(|v| v.bindings.iter().filter_map(|b| b.resolved))
        .collect()
}

#[test]
fn orphans_are_written_once_inert_after_the_last_section_and_come_back_unbound() {
    let src = Document::new(1).unwrap();
    let header = story(&src, &["Running head"]);
    let first = SectionProperties::from_iter([(
        "region.header.default".to_owned(),
        json!(header.to_string()),
    )]);
    src.write_section_properties(SectionId::First, "0", &first, &[])
        .unwrap();
    src.append_block(BlockKind::Paragraph, "", "body").unwrap();
    story(&src, &["one <b> & two", "second paragraph"]);
    story(&src, &[]);
    story(&src, &["last orphan"]);
    let (html, notes) = export(&src);

    // Once each, under labels after the bound story's, in the order the
    // document holds them, inert and after the last section.
    for text in ["one &lt;b&gt; &amp; two", "second paragraph", "last orphan"] {
        assert_eq!(html.matches(text).count(), 1, "{text}: {html}");
    }
    let tail = html.rsplit("</section>").next().unwrap();
    assert!(
        tail.starts_with("<template data-reprise-story=\"s1\">"),
        "{tail}"
    );
    assert!(tail.contains("<template data-reprise-story=\"s2\"></template>"));
    assert!(tail.contains("<template data-reprise-story=\"s3\">"));
    assert!(tail.ends_with("</body></html>"));
    // Nothing is left out, so nothing is reported as left out.
    assert!(
        !notes
            .iter()
            .any(|n| n.message.contains("bound to no section")),
        "{notes:?}"
    );

    // The import has all four, and the three orphans are bound by nothing.
    let back = paste_into(Document::new(2).unwrap(), &html);
    let stories = back.stories();
    assert_eq!(stories.len(), 4);
    let listed: Vec<_> = stories.iter().map(|s| texts(&back, *s)).collect();
    assert_eq!(listed[0], ["Running head"]);
    assert_eq!(listed[1], ["one <b> & two", "second paragraph"]);
    assert!(listed[2].is_empty());
    assert_eq!(listed[3], ["last orphan"]);
    assert_eq!(bound(&back), [stories[0]], "only the header is bound");

    // Writing the import again says the same: the labels and order are stable.
    let (again, _) = export(&back);
    assert_eq!(again, html);
}

#[test]
fn a_document_of_nothing_but_orphans_keeps_them() {
    let src = Document::new(1).unwrap();
    story(&src, &["alone"]);
    story(&src, &["together"]);
    let (html, _) = export(&src);
    let back = paste_into(Document::new(2).unwrap(), &html);
    let listed: Vec<_> = back.stories().iter().map(|s| texts(&back, *s)).collect();
    assert_eq!(listed, [["alone"], ["together"]]);
    assert!(bound(&back).is_empty());
}

#[test]
fn a_paste_into_a_document_with_content_takes_no_orphans() {
    // The same rule as a native paste (sections §14): stories nothing the
    // paste writes refers to are not copied into a document that has content.
    let src = Document::new(1).unwrap();
    src.append_block(BlockKind::Paragraph, "", "body").unwrap();
    story(&src, &["orphan"]);
    let (html, _) = export(&src);
    let destination = Document::new(2).unwrap();
    destination
        .append_block(BlockKind::Paragraph, "", "existing")
        .unwrap();
    destination.commit();
    let back = paste_into(destination, &html);
    assert!(back.stories().is_empty());
}
