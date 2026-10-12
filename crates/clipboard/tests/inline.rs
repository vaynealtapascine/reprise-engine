//! Inline objects at the clipboard boundary (`docs/inline.md`): HTML that
//! imports again, plain text that reads alt text and field text, tagged PDF
//! with figures, and HTML from other tools.

#[path = "../../display/tests/common/checker.rs"]
mod checker;

use checker::check;
use reprise_clipboard::{
    ExportOptions, Exporter, Html, ImportLimits, PdfWithAssets, PlainText, import_html,
};
use reprise_doc::{Document, InlineAlign, InlineObject, NodeId, SchemaRegistry};
use reprise_edit::Editor;
use reprise_fixtures::inline::{self, Part};
use reprise_layout::{Engine, LayoutSnapshot};

struct Setup {
    engine: Engine,
    doc: Document,
    asset: String,
    node: NodeId,
    layout: LayoutSnapshot,
}

fn equation() -> InlineObject {
    InlineObject {
        kind: "equation".into(),
        ..Default::default()
    }
    .with("src", "x^2")
}

fn setup() -> Setup {
    let (engine, doc, asset) = inline::page(200, 200, Default::default()).unwrap();
    let node = inline::paragraph(
        &doc,
        vec![
            Part::Text("Look "),
            Part::Object(inline::image(&asset, 24, 12, "a \"cat\" & <dog>")),
            Part::Text(" on page "),
            Part::Object(inline::page_number()),
            Part::Text(" or "),
            Part::Object(InlineObject::field("text").with("text", "literal <b>")),
            Part::Text(" and "),
            Part::Object(equation()),
            Part::Text(" end"),
        ],
    )
    .unwrap();
    doc.commit();
    let layout = engine.layout(&doc);
    Setup {
        engine,
        doc,
        asset,
        node,
        layout,
    }
}

fn export<E: Exporter>(exporter: E, s: &Setup) -> Vec<u8> {
    let schemas = SchemaRegistry::builtin();
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &schemas,
        fonts: Some(&s.engine.fonts),
    };
    exporter
        .export(&s.doc, Some(&s.layout), &options)
        .unwrap()
        .bytes
}

#[test]
fn html_writes_images_and_resolved_fields_and_imports_them_again() {
    let s = setup();
    let html = String::from_utf8(export(Html, &s)).unwrap();
    assert!(html.contains("<img "), "{html}");
    assert!(html.contains(&format!("data-reprise-asset=\"{}\"", s.asset)));
    assert!(
        html.contains("alt=\"a &quot;cat&quot; &amp; &lt;dog&gt;\""),
        "{html}"
    );
    // The field is a span with its resolved text; the text field its text.
    assert!(html.contains("data-field=\"page-number\""));
    assert!(html.contains(">1</span>"), "{html}");
    assert!(html.contains(">literal &lt;b&gt;</span>"), "{html}");

    let imported = import_html(&html, ImportLimits::default()).unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    let pasted = imported
        .fragment
        .paste(&mut editor, None, "target")
        .unwrap();
    let node = pasted.ids.nodes.values().copied().next().unwrap();
    let before: Vec<_> = s
        .doc
        .inline_objects(s.node)
        .unwrap()
        .into_iter()
        .map(|e| (e.at, e.object))
        .collect();
    let after: Vec<_> = editor
        .document()
        .inline_objects(node)
        .unwrap()
        .into_iter()
        .map(|e| (e.at, e.object))
        .collect();
    assert_eq!(after, before, "objects round-trip through HTML");
    assert_eq!(
        editor.document().block(node).unwrap().text.to_string(),
        s.doc.block(s.node).unwrap().text.to_string()
    );
    // And the pasted objects are new objects.
    let originals = s.doc.inline_objects(s.node).unwrap();
    for entry in editor.document().inline_objects(node).unwrap() {
        assert!(originals.iter().all(|o| o.id != entry.id));
    }
}

#[test]
fn plain_text_reads_alt_text_and_field_text() {
    let s = setup();
    let plain = String::from_utf8(export(PlainText, &s)).unwrap();
    assert_eq!(
        plain,
        "Look a \"cat\" & <dog> on page 1 or literal <b> and  end"
    );
    // Without a layout a field that needs pagination reads as "?", and a
    // text field as its own text.
    let schemas = SchemaRegistry::builtin();
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &schemas,
        fonts: None,
    };
    let unplaced = PlainText.export(&s.doc, None, &options).unwrap();
    assert_eq!(
        String::from_utf8(unplaced.bytes).unwrap(),
        "Look a \"cat\" & <dog> on page ? or literal <b> and  end"
    );
}

#[test]
fn tagged_pdf_has_a_figure_with_alt_and_fields_as_text() {
    let s = setup();
    let pdf = PdfWithAssets {
        assets: &s.engine.assets,
    }
    .export(
        &s.doc,
        Some(&s.layout),
        &ExportOptions {
            source_namespace: "source",
            schemas: &SchemaRegistry::builtin(),
            fonts: Some(&s.engine.fonts),
        },
    )
    .unwrap();
    let report = check(&pdf.bytes);
    assert_eq!(report.roles.iter().filter(|r| *r == "Figure").count(), 1);
    assert_eq!(report.figure_alts, ["a \"cat\" & <dog>"]);
    // The field's resolved text is in the paragraph's text; the picture's
    // alt text is not repeated as text.
    let text: String = report.text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(text.contains("onpage1or"), "{}", report.text);
    assert!(text.contains("literal"), "{}", report.text);
}

#[test]
fn html_from_other_tools_is_read_as_far_as_it_goes() {
    let (engine, _, asset) = inline::page(200, 200, Default::default()).unwrap();
    let _ = engine;
    let html = format!(
        "<p>See <img data-reprise-asset=\"{asset}\" alt=\"x\" style=\"width: 30pt; height: 12pt; \
         vertical-align: middle\"> and <span data-field=\"page-number\">7</span>, \
         <img src=\"http://example.com/a.png\" alt=\"remote\"> a&#xFFFC;b\u{FFFC}c</p>"
    );
    let imported = import_html(&html, ImportLimits::default()).unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    let pasted = imported
        .fragment
        .paste(&mut editor, None, "target")
        .unwrap();
    let node = pasted.ids.nodes.values().copied().next().unwrap();
    let objects = editor.document().inline_objects(node).unwrap();
    assert_eq!(objects.len(), 2);
    let image = objects[0].object.as_image().unwrap();
    assert_eq!(image.asset, asset);
    assert_eq!(image.alt, "x");
    assert_eq!(objects[0].object.align, InlineAlign::Middle);
    assert_eq!(objects[0].object.param("width"), Some(&inline::pt(30)[..]));
    assert_eq!(objects[1].object, InlineObject::field("page-number"));
    // The remote image is left out with a note, and a literal object
    // character is never an object.
    let text = editor.document().block(node).unwrap().text.to_string();
    assert_eq!(text.matches('\u{FFFC}').count(), 2, "{text:?}");
    assert!(
        text.ends_with(", a b c") || text.ends_with(", abc"),
        "{text:?}"
    );
    assert!(
        imported
            .fragment
            .notes
            .iter()
            .any(|n| n.code.as_str() == "clipboard.html-dropped"),
        "{:?}",
        imported.fragment.notes
    );
}

#[test]
fn hostile_html_never_makes_an_object_it_cannot_validate() {
    let huge = "x".repeat(40_000);
    for html in [
        // Stored form with a bad kind, a huge value, an unknown field in the
        // JSON, and plain nonsense.
        "<p>a<span data-reprise-inline='{\"kind\":\"BAD KIND\"}'></span>b</p>".to_string(),
        format!(
            "<p>a<span data-reprise-inline='{{\"kind\":\"field\",\"params\":{{\"field\":\"text\",\"text\":\"{huge}\"}}}}'></span>b</p>"
        ),
        "<p>a<span data-reprise-inline='{\"kind\":\"field\",\"nope\":1}'></span>b</p>".to_string(),
        "<p>a<span data-reprise-inline='not json' data-field=\"Bad Name\"></span>b</p>".to_string(),
        "<p>a<img data-reprise-asset=\"not-a-hash\" alt=\"z\">b</p>".to_string(),
    ] {
        let imported = import_html(&html, ImportLimits::default()).unwrap();
        let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
        let pasted = imported
            .fragment
            .paste(&mut editor, None, "target")
            .unwrap();
        let node = pasted.ids.nodes.values().copied().next().unwrap();
        assert!(
            editor.document().inline_objects(node).unwrap().is_empty(),
            "{}",
            &html[..html.len().min(80)]
        );
        assert_eq!(
            editor.document().block(node).unwrap().text.to_string(),
            "ab"
        );
    }
}
