use reprise_clipboard::{
    Disposition, ExportOptions, Exporter, Feature, Html, ImportLimits, Native, Pdf, PlainText,
    copy_all, import_html, import_plain,
};
use reprise_doc::{Document, SchemaRegistry};
use reprise_edit::Editor;

fn text(import: &reprise_clipboard::Import) -> Vec<String> {
    import
        .fragment
        .fragment
        .blocks
        .iter()
        .filter(|b| b.table.is_none())
        .map(|b| b.text.clone())
        .collect()
}

#[test]
fn plain_html_unicode_breaks_styles_and_tables() {
    assert_eq!(
        text(&import_plain("one\r\nline\r\n\r\nאבג").unwrap()),
        ["one\nline", "אבג"]
    );
    let import = import_html("<p dir='rtl' style='font-size: 12.5pt; line-height: -1pt; font-family: Serif'>אבג &amp; e&#769;<br>office</p><table><tr><td>A</td><td>B</td></tr><tr><td>C</td></tr></table>", ImportLimits::default()).unwrap();
    assert_eq!(text(&import), ["אבג & e\u{301}\noffice", "A", "B", "C"]);
    assert_eq!(
        import.fragment.fragment.blocks[0].overrides.size,
        Some(reprise_doc::LengthExpr::Pt(reprise_geom::Length(12800)))
    );
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    import.fragment.paste(&mut editor, None, "target").unwrap();
    let schemas = SchemaRegistry::builtin();
    let options = ExportOptions {
        source_namespace: "target",
        schemas: &schemas,
        fonts: None,
    };
    let plain = PlainText.export(editor.document(), None, &options).unwrap();
    assert_eq!(
        String::from_utf8(plain.bytes).unwrap(),
        "אבג & e\u{301}\noffice\n\nA\tB\nC"
    );
    let rich = Html.export(editor.document(), None, &options).unwrap();
    let rich = String::from_utf8(rich.bytes).unwrap();
    assert!(rich.contains("dir=\"rtl\""));
    assert!(rich.contains("<table><tr><td><p"));
    assert!(rich.contains("font-size: 12.5pt"));
    assert_eq!(
        text(&import_html(&rich, ImportLimits::default()).unwrap()),
        ["אבג & e\u{301}\noffice", "A", "B", "C"]
    );
}

#[test]
fn imported_tables_have_inferred_columns_and_render_all_cells() {
    let imported = import_html(
        "<table><tr><td>AAA</td><td>BBB</td></tr><tr><td>CCC</td><td>DDD</td></tr></table>",
        ImportLimits::default(),
    )
    .unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    imported
        .fragment
        .paste(&mut editor, None, "target")
        .unwrap();
    let engine = reprise_fixtures::engine();
    let layout = engine.layout(editor.document());
    assert!(
        layout
            .diagnostics_with("layout.table-invalid")
            .next()
            .is_none()
    );
    for text in ["AAA", "BBB", "CCC", "DDD"] {
        assert!(
            layout
                .blocks
                .iter()
                .any(|b| b.text == text && !b.lines.is_empty())
        );
    }
    let html = format!("<table><tr>{}</tr></table>", "<td>x</td>".repeat(129));
    assert!(matches!(
        import_html(&html, ImportLimits::default()),
        Err(reprise_clipboard::ClipboardError::Limit(
            "HTML table columns"
        ))
    ));
}

#[test]
fn html_direction_matches_shaping_for_pinned_unicode_17_and_isolates() {
    let engine = reprise_fixtures::engine();
    for text in [
        "\u{10940}",
        "123 אב",
        "\u{2067}אב\u{2069} Latin",
        "\u{2068}Latin\u{2069} אב",
        "\u{202e}Latin\u{202c}",
        "\nאב",
    ] {
        let doc = Document::new(1).unwrap();
        let node = doc
            .append_block(reprise_doc::BlockKind::Paragraph, "", text)
            .unwrap();
        doc.commit();
        let snapshot = engine.layout(&doc);
        let base = snapshot
            .blocks
            .iter()
            .find(|b| b.node == node)
            .unwrap()
            .base_level;
        let options = ExportOptions {
            source_namespace: "source",
            schemas: &engine.schemas,
            fonts: None,
        };
        let html = String::from_utf8(Html.export(&doc, None, &options).unwrap().bytes).unwrap();
        assert!(
            html.contains(if base % 2 == 1 {
                "dir=\"rtl\""
            } else {
                "dir=\"ltr\""
            }),
            "direction differs for {text:?}"
        );
    }
}

#[test]
fn huge_deep_token_heavy_unbalanced_and_active_html_are_bounded() {
    for (html, limits) in [
        (
            "a".repeat(1025),
            ImportLimits {
                bytes: 1024,
                ..ImportLimits::default()
            },
        ),
        (
            format!("{}x{}", "<span>".repeat(65), "</span>".repeat(65)),
            ImportLimits::default(),
        ),
        (
            "<br>".repeat(12),
            ImportLimits {
                tokens: 10,
                ..ImportLimits::default()
            },
        ),
        (
            "<p>x</p>".repeat(3),
            ImportLimits {
                blocks: 2,
                ..ImportLimits::default()
            },
        ),
    ] {
        let error = match import_html(&html, limits) {
            Err(error) => error,
            Ok(_) => panic!("hostile import accepted"),
        };
        assert_eq!(error.note().code, "clipboard.limit");
    }
    for html in [
        "<p><div>bad</p></div>",
        "<td>stray</td>",
        "<p a='unterminated",
        "<!--",
        "<p>é🙂<>",
        "</p></table>",
    ] {
        assert!(import_html(html, ImportLimits::default()).is_ok(), "{html}");
    }
    let active = import_html(
        "<script>alert('x')</script><p>safe</p><iframe>secret</iframe><p>&lt;script&gt;</p>",
        ImportLimits::default(),
    )
    .unwrap();
    assert_eq!(text(&active), ["safe", "<script>"]);
    assert!(
        active
            .notes
            .iter()
            .any(|n| n.code == "clipboard.html-dropped"
                && n.severity == reprise_diag::Severity::Error)
    );
    assert!(
        import_html(
            "<table><tr><td><table>x</table></td></tr></table>",
            ImportLimits::default()
        )
        .is_err()
    );
    let mut seed = 7u64;
    // Deterministic malformed UTF-8-safe tag soup, including quoted '>' and numeric entities.
    let pieces = [
        "<",
        ">",
        "</p>",
        "<p>",
        "<x a='>'>",
        "\"",
        "é",
        "אב",
        "&amp;",
        "&#x110000;",
        "<!--",
        "-->",
        "<br/>",
    ];
    for _ in 0..200 {
        let mut html = String::new();
        for _ in 0..100 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            html.push_str(pieces[(seed % pieces.len() as u64) as usize]);
        }
        let _ = import_html(&html, ImportLimits::default());
    }
}

#[test]
fn resources_are_hash_checked_and_fonts_transfer() {
    let fixture = reprise_fixtures::hostile::transformed_rtl().unwrap();
    let layout = fixture.engine.layout(&fixture.doc);
    let mut fragment = copy_all(
        &fixture.doc,
        "source",
        &fixture.engine.schemas,
        Some(&layout),
        Some(&fixture.engine.fonts),
    )
    .unwrap();
    assert!(!fragment.resources.is_empty());
    let hash = fragment.attach_asset(b"image payload".to_vec()).unwrap();
    let bytes = fragment.encode().unwrap();
    let decoded = reprise_clipboard::NativeFragment::decode(&bytes).unwrap();
    let mut fonts = reprise_font::FontStore::default();
    decoded.install_fonts(&mut fonts).unwrap();
    for resource in decoded.resources.values() {
        if let reprise_clipboard::ResourceKind::Font { pin, .. } = &resource.kind {
            assert!(fonts.get(pin).is_ok());
        }
    }
    fragment.resources.get_mut(&hash).unwrap().bytes.push(0);
    assert!(fragment.encode().is_err());
    let mut future = decoded.fragment.clone();
    future.version = 2;
    let malformed = reprise_clipboard::NativeFragment {
        fragment: future,
        resources: Default::default(),
        notes: Vec::new(),
    };
    assert_eq!(
        malformed.encode().unwrap_err().note().code,
        "clipboard.version"
    );
    let raw = serde_json::to_vec(&malformed).unwrap();
    assert!(reprise_clipboard::NativeFragment::decode(&raw).is_err());
    assert!(reprise_clipboard::NativeFragment::decode(&[b'['; 200]).is_err());
}

#[test]
fn every_exporter_reports_all_features_on_house_of_leaves_documents() {
    for fixture in [
        reprise_fixtures::hostile::spiral_text().unwrap(),
        reprise_fixtures::hostile::notes_nested_three_deep().unwrap(),
        reprise_fixtures::hostile::reading_cycle().unwrap(),
        reprise_fixtures::hostile::transformed_rtl().unwrap(),
    ] {
        let snapshot = fixture.engine.layout(&fixture.doc);
        let options = ExportOptions {
            source_namespace: "source",
            schemas: &fixture.engine.schemas,
            fonts: Some(&fixture.engine.fonts),
        };
        for exporter in [&PlainText as &dyn Exporter, &Html, &Native, &Pdf] {
            let result = exporter
                .export(&fixture.doc, Some(&snapshot), &options)
                .unwrap();
            assert!(!result.bytes.is_empty());
            assert_eq!(result.losses.features.len(), Feature::ALL.len());
            for feature in Feature::ALL {
                let loss = result
                    .losses
                    .features
                    .iter()
                    .find(|l| l.feature == feature)
                    .unwrap();
                assert_eq!(loss.code, feature.code());
                assert!(!loss.detail.is_empty());
            }
        }
        assert_eq!(
            PlainText
                .export(&fixture.doc, Some(&snapshot), &options)
                .unwrap()
                .losses
                .features
                .iter()
                .find(|l| l.feature == Feature::Transforms)
                .unwrap()
                .disposition,
            Disposition::Dropped
        );
        let pdf = Pdf.export(&fixture.doc, Some(&snapshot), &options).unwrap();
        assert!(pdf.bytes.starts_with(b"%PDF"));
        assert_eq!(
            pdf.losses
                .features
                .iter()
                .find(|l| l.feature == Feature::EditingStructure)
                .unwrap()
                .disposition,
            Disposition::Dropped
        );
        let native = Native
            .export(&fixture.doc, Some(&snapshot), &options)
            .unwrap();
        assert_eq!(
            native
                .losses
                .features
                .iter()
                .find(|l| l.feature == Feature::Transforms)
                .unwrap()
                .disposition,
            Disposition::Preserved
        );
    }
}

#[test]
fn reading_order_is_respected_and_html_text_is_escaped() {
    let doc = Document::new(1).unwrap();
    let a = doc
        .append_block(reprise_doc::BlockKind::Paragraph, "", "<A> & \"x\"")
        .unwrap();
    let b = doc
        .append_block(reprise_doc::BlockKind::Paragraph, "", "אבג")
        .unwrap();
    let engine = reprise_fixtures::engine();
    doc.add_relation(&engine.schemas, &reprise_doc::reading::before(b, a))
        .unwrap();
    doc.commit();
    let snapshot = engine.layout(&doc);
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &engine.schemas,
        fonts: None,
    };
    assert_eq!(
        String::from_utf8(
            PlainText
                .export(&doc, Some(&snapshot), &options)
                .unwrap()
                .bytes
        )
        .unwrap(),
        "אבג\n\n<A> & \"x\""
    );
    let html =
        String::from_utf8(Html.export(&doc, Some(&snapshot), &options).unwrap().bytes).unwrap();
    assert!(html.find("אבג").unwrap() < html.find("&lt;A&gt;").unwrap());
    assert!(html.contains("&amp; &quot;x&quot;"));
    assert!(Pdf.export(&doc, None, &options).is_err());
    doc.block(a).unwrap().text.insert(0, "new").unwrap();
    doc.commit();
    assert!(PlainText.export(&doc, Some(&snapshot), &options).is_err());
}

#[test]
fn whitespace_direction_attribute_and_css_limits_are_explicit() {
    let imported = import_plain("  one   two\tthree\n four  ").unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    imported
        .fragment
        .paste(&mut editor, None, "target")
        .unwrap();
    let schemas = SchemaRegistry::builtin();
    let options = ExportOptions {
        source_namespace: "target",
        schemas: &schemas,
        fonts: None,
    };
    let html = String::from_utf8(
        Html.export(editor.document(), None, &options)
            .unwrap()
            .bytes,
    )
    .unwrap();
    assert_eq!(
        text(&import_html(&html, ImportLimits::default()).unwrap()),
        text(&imported)
    );
    let explicit = import_html("<p dir='rtl'>123 Latin</p>", ImportLimits::default()).unwrap();
    assert!(
        explicit
            .notes
            .iter()
            .any(|n| n.code == "clipboard.html-approximated")
    );
    for html in [
        format!("<p {}>x</p>", "x='y' ".repeat(129)),
        format!("<p style='{}'>x</p>", "font-size:1pt;".repeat(129)),
    ] {
        let error = match import_html(&html, ImportLimits::default()) {
            Err(error) => error,
            Ok(_) => panic!("silent truncation"),
        };
        assert_eq!(error.note().code, "clipboard.limit");
    }
}

#[test]
fn plain_export_retains_the_unplaced_tail_and_resource_dedup_keeps_font_pins() {
    let fixture = reprise_fixtures::hostile::page_limit().unwrap();
    let layout = fixture.engine.layout(&fixture.doc);
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &fixture.engine.schemas,
        fonts: Some(&fixture.engine.fonts),
    };
    let plain = String::from_utf8(
        PlainText
            .export(&fixture.doc, Some(&layout), &options)
            .unwrap()
            .bytes,
    )
    .unwrap();
    for node in fixture.doc.blocks() {
        let text = fixture.doc.block(node).unwrap().text.to_string();
        assert!(plain.contains(&text), "unplaced tail lost");
    }
    let mut native = copy_all(
        &fixture.doc,
        "source",
        &fixture.engine.schemas,
        Some(&layout),
        Some(&fixture.engine.fonts),
    )
    .unwrap();
    let font = native
        .resources
        .values()
        .find(|r| matches!(r.kind, reprise_clipboard::ResourceKind::Font { .. }))
        .unwrap()
        .bytes
        .clone();
    let hash = native.attach_asset(font).unwrap();
    assert!(matches!(
        native.resources[&hash].kind,
        reprise_clipboard::ResourceKind::Font { .. }
    ));
}
