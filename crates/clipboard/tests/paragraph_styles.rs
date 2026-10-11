//! Paragraph properties and named styles through the native and HTML
//! clipboard formats (`docs/paragraph-styles.md`).
use reprise_clipboard::{
    ExportOptions, Exporter, Html, ImportLimits, NativeFragment, copy_all, import_html,
};
use reprise_doc::marks::Alignment;
use reprise_doc::{Authored, BlockKind, Document, Expr, Property, SchemaRegistry, Style};
use reprise_edit::Editor;
use reprise_geom::Length;

fn source() -> (Document, reprise_doc::NodeId) {
    let doc = Document::new(1).unwrap();
    let mut quote = Style {
        alignment: Some(Alignment::Justify),
        keep_with_next: Some(true),
        widows: Some(3),
        background: Some([250, 240, 200, 255]),
        ..Style::default()
    };
    quote.set(
        Property::IndentStart,
        Authored::Expr(Expr::parse("18pt").unwrap()),
    );
    doc.define_named_style("Quote", &quote).unwrap();
    let node = doc
        .append_block(BlockKind::Paragraph, "Quote", "quoted words")
        .unwrap();
    let mut direct = Style {
        keep_together: Some(true),
        ..Style::default()
    };
    for (p, text) in [
        (Property::IndentFirst, "-6pt"),
        (Property::SpaceBefore, "4pt"),
        (Property::SpaceAfter, "8pt"),
    ] {
        direct.set(p, Authored::Expr(Expr::parse(text).unwrap()));
    }
    doc.set_overrides(node, &direct).unwrap();
    doc.commit();
    (doc, node)
}

#[test]
fn native_copy_and_paste_keep_named_styles_and_direct_properties() {
    let (doc, node) = source();
    let expected = doc.computed_style(node).unwrap();
    let fragment = copy_all(&doc, "source", &SchemaRegistry::builtin(), None, None).unwrap();
    let decoded = NativeFragment::decode(&fragment.encode().unwrap()).unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    let pasted = editor.paste(&decoded.fragment, None, "target").unwrap();
    let target = pasted.ids.nodes[&node];
    let got = editor.document().computed_style(target).unwrap();
    assert_eq!(got.indent_start, Length::from_pt(18));
    assert_eq!(got.indent_first, Length::from_pt(-6));
    assert_eq!(got.alignment, Some(Alignment::Justify));
    assert!(got.keep_with_next && got.keep_together);
    assert_eq!(got.widows, Some(3));
    assert_eq!(got.background, Some([250, 240, 200, 255]));
    assert_eq!(
        (got.space_before, got.space_after),
        (expected.space_before, expected.space_after)
    );
}

#[test]
fn html_export_and_import_keep_paragraph_properties() {
    let (doc, _) = source();
    let schemas = SchemaRegistry::builtin();
    let html = String::from_utf8(
        Html.export(
            &doc,
            None,
            &ExportOptions {
                source_namespace: "source",
                schemas: &schemas,
                fonts: None,
            },
        )
        .unwrap()
        .bytes,
    )
    .unwrap();
    for css in [
        "text-align: justify",
        "margin-inline-start: 18pt",
        "text-indent: -6pt",
        "margin-top: 4pt",
        "margin-bottom: 8pt",
        "break-after: avoid",
        "break-inside: avoid",
        "widows: 3",
        "background-color: #faf0c8ff",
    ] {
        assert!(html.contains(css), "{css} in {html}");
    }
    let imported = import_html(&html, ImportLimits::default()).unwrap();
    let block = imported
        .fragment
        .fragment
        .blocks
        .into_iter()
        .find(|b| b.text == "quoted words")
        .unwrap();
    let o = &block.overrides;
    assert_eq!(o.alignment, Some(Alignment::Justify));
    assert_eq!(o.keep_with_next, Some(true));
    assert_eq!(o.keep_together, Some(true));
    assert_eq!(o.widows, Some(3));
    assert_eq!(o.background, Some([250, 240, 200, 255]));
    for (p, pt) in [
        (Property::IndentStart, 18),
        (Property::IndentFirst, -6),
        (Property::SpaceBefore, 4),
        (Property::SpaceAfter, 8),
    ] {
        assert_eq!(
            o.get(p),
            Some(Authored::Legacy(reprise_doc::LengthExpr::Pt(
                Length::from_pt(pt)
            ))),
            "{p:?}"
        );
    }
    // Documents without paragraph properties export exactly as before.
    let plain = Document::new(1).unwrap();
    plain
        .append_block(BlockKind::Paragraph, "", "plain")
        .unwrap();
    let out = String::from_utf8(
        Html.export(
            &plain,
            None,
            &ExportOptions {
                source_namespace: "source",
                schemas: &schemas,
                fonts: None,
            },
        )
        .unwrap()
        .bytes,
    )
    .unwrap();
    assert!(!out.contains("margin") && !out.contains("break-") && !out.contains("indent"));
}
