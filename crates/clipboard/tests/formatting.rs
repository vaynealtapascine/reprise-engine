//! Character formatting through HTML and native clipboard formats.
use reprise_clipboard::{ExportOptions, Exporter, Html, ImportLimits, copy_all, import_html};
use reprise_doc::{
    BlockKind, Document, SchemaRegistry,
    formatting::{FormatRun, TextFeature, TextStyle},
    text::RangePolicy,
};
use reprise_geom::Length;

fn html(doc: &Document) -> String {
    let schemas = SchemaRegistry::builtin();
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &schemas,
        fonts: None,
    };
    String::from_utf8(Html.export(doc, None, &options).unwrap().bytes).unwrap()
}

/// The non-default runs of each imported paragraph.
fn imported(html: &str) -> Vec<(String, Vec<FormatRun>)> {
    import_html(html, ImportLimits::default())
        .unwrap()
        .fragment
        .fragment
        .blocks
        .into_iter()
        .filter(|b| b.table.is_none())
        .map(|b| {
            let runs = b
                .formatting
                .into_iter()
                .filter(|r| r.style != TextStyle::default())
                .collect();
            (b.text, runs)
        })
        .collect()
}

fn style(families: &[&str], pt: i32, language: &str) -> TextStyle {
    TextStyle {
        families: Some(families.iter().map(|s| s.to_string()).collect()),
        size: Some(Length::from_pt(pt)),
        language: Some(language.into()),
        features: Some(vec![TextFeature {
            tag: *b"smcp",
            value: 1,
        }]),
        reset: false,
    }
}

#[test]
fn html_export_and_import_roundtrip_every_supported_property() {
    let doc = Document::new(1).unwrap();
    let node = doc
        .append_block(BlockKind::Paragraph, "", "plain fancy plain")
        .unwrap();
    // A family name that needs CSS quoting and HTML escaping both.
    let fancy = style(&["Odd \"Name\", <b>", "Serif"], 18, "fr");
    doc.format_text(node, 6..11, &fancy, RangePolicy::EXPANDING)
        .unwrap();
    doc.commit();
    let out = html(&doc);
    assert!(!out.contains("<b>"), "family name must be escaped: {out}");
    let paragraphs = imported(&out);
    assert_eq!(paragraphs.len(), 1);
    assert_eq!(paragraphs[0].0, "plain fancy plain");
    assert_eq!(
        paragraphs[0].1,
        [FormatRun {
            bytes: 6..11,
            style: fancy
        }]
    );
}

#[test]
fn nested_and_unbalanced_spans_inherit_and_close_deterministically() {
    let html = "<p>a<span style='font-size: 20pt'>b<span lang='de'>c</span>d</span>e\
                <span style='font-size: 30pt'>f</p><p>g</p>";
    let paragraphs = imported(html);
    let size = |pt| Some(Length::from_pt(pt));
    assert_eq!(paragraphs[0].0, "abcdef");
    let runs = &paragraphs[0].1;
    // b and d are 20pt; c is 20pt German; f is 30pt.
    assert_eq!(runs[0].bytes, 1..2);
    assert_eq!(runs[0].style.size, size(20));
    assert_eq!(runs[1].bytes, 2..3);
    assert_eq!(runs[1].style.language.as_deref(), Some("de"));
    assert_eq!(runs[1].style.size, size(20));
    assert_eq!(runs[2].bytes, 3..4);
    assert_eq!(runs.last().unwrap().bytes, 5..6);
    assert_eq!(runs.last().unwrap().style.size, size(30));
    // The unclosed span does not leak into the next paragraph.
    assert_eq!(paragraphs[1], ("g".into(), Vec::new()));
}

#[test]
fn hostile_inline_css_is_reported_not_applied() {
    for css in [
        "font-size: 0pt",
        "font-size: -4pt",
        "font-size: 1e9pt",
        "font-family: ",
        "font-family: 'unterminated",
        "font-feature-settings: 'toolong' 1",
        "font-feature-settings: 'liga' 1, 'liga' 0",
        "color: red",
        "font-size: calc(1pt + 2pt)",
    ] {
        let html = format!("<p>x<span style=\"{css}\">y</span></p>");
        let import = import_html(&html, ImportLimits::default()).unwrap();
        let block = &import.fragment.fragment.blocks[0];
        assert_eq!(block.text, "xy", "{css}");
        for run in &block.formatting {
            run.style.validate().unwrap();
            assert_ne!(run.style.size, Some(Length::ZERO), "{css}");
        }
        assert!(
            import
                .notes
                .iter()
                .any(|n| n.code == "clipboard.html-approximated"),
            "{css}: {:?}",
            import.notes
        );
    }
}

#[test]
fn many_spans_are_bounded() {
    let mut html = String::from("<p>");
    for i in 0..5000 {
        html.push_str(&format!(
            "<span style='font-size: {}pt'>x</span>",
            8 + i % 2
        ));
    }
    html.push_str("</p>");
    assert!(import_html(&html, ImportLimits::default()).is_err());
}

#[test]
fn native_copy_carries_effective_formatting_across_paragraphs() {
    let engine = reprise_fixtures::engine();
    let doc = Document::new(1).unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "", "first para")
        .unwrap();
    let b = doc
        .append_block(BlockKind::Paragraph, "", "second para")
        .unwrap();
    let big = TextStyle {
        size: Some(Length::from_pt(22)),
        ..TextStyle::default()
    };
    doc.format_text(a, 6..10, &big, RangePolicy::EXPANDING)
        .unwrap();
    doc.format_text(b, 0..6, &big, RangePolicy::EXPANDING)
        .unwrap();
    doc.commit();
    let fragment = copy_all(&doc, "source", &engine.schemas, None, None).unwrap();
    let blocks = &fragment.fragment.blocks;
    let runs = |i: usize| -> Vec<_> {
        blocks[i]
            .formatting
            .iter()
            .filter(|r| r.style.size.is_some())
            .map(|r| r.bytes.clone())
            .collect()
    };
    assert_eq!(runs(0), vec![6..10]);
    assert_eq!(runs(1), vec![0..6]);

    // Pasting into an unformatted document reproduces the same runs.
    let target = Document::new(2).unwrap();
    let mut editor = reprise_edit::Editor::new(target, SchemaRegistry::builtin());
    editor
        .apply_command(reprise_edit::Command::Paste {
            fragment: Box::new(fragment.fragment.clone()),
            at: None,
            target_namespace: "target".into(),
        })
        .unwrap();
    let doc = editor.document();
    let pasted = doc.blocks();
    for (node, expected) in pasted.iter().zip([6..10, 0..6]) {
        let sized: Vec<_> = doc
            .text_formats(*node)
            .unwrap()
            .runs
            .into_iter()
            .filter(|r| r.style.size == big.size)
            .map(|r| r.bytes)
            .collect();
        assert_eq!(sized, vec![expected]);
    }
}
