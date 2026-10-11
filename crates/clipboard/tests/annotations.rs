//! Links and comment anchors through the clipboard formats: HTML export and
//! import (with the same URL validation as authoring), plain text, native
//! copy and paste, and the tagged PDF (`docs/annotations.md`).

#[path = "../../display/tests/common/checker.rs"]
mod checker;

use checker::check;
use reprise_clipboard::{
    ExportOptions, Exporter, Html, ImportLimits, Native, NativeFragment, PdfWithAssets, PlainText,
    import_html,
};
use reprise_doc::annotation::{
    AnnotationData, AnnotationState, CommentAnchor, Kind, LINK_LENGTH, LINK_SCHEME, LinkTarget,
};
use reprise_doc::formatting::TextStyle;
use reprise_doc::text::RangePolicy;
use reprise_doc::{BlockKind, Document, NodeId, SchemaRegistry};
use reprise_edit::Editor;
use reprise_fixtures::spike::define_styles;
use reprise_fixtures::{engine, hostile};

fn url(u: &str) -> AnnotationData {
    AnnotationData::Link(LinkTarget::Url { url: u.into() })
}

fn comment(t: &str) -> AnnotationData {
    AnnotationData::Comment(CommentAnchor { thread: t.into() })
}

fn document(texts: &[&str]) -> (Document, Vec<NodeId>) {
    let doc = Document::new(1).unwrap();
    define_styles(&doc).unwrap();
    let ids = texts
        .iter()
        .map(|t| doc.append_block(BlockKind::Paragraph, "body", t).unwrap())
        .collect();
    doc.commit();
    (doc, ids)
}

fn options(schemas: &SchemaRegistry) -> ExportOptions<'_> {
    ExportOptions {
        source_namespace: "source",
        schemas,
        fonts: None,
    }
}

/// The `export.annotations` notes of a loss report, as (severity, message).
fn annotation_losses(
    losses: &reprise_clipboard::LossReport,
) -> Vec<(reprise_diag::Severity, String)> {
    losses
        .notes
        .iter()
        .filter(|n| n.code.as_str() == "export.annotations")
        .map(|n| (n.severity, n.message.clone()))
        .collect()
}

fn html_of(doc: &Document) -> (String, reprise_clipboard::LossReport) {
    let schemas = SchemaRegistry::builtin();
    let out = Html.export(doc, None, &options(&schemas)).unwrap();
    (String::from_utf8(out.bytes).unwrap(), out.losses)
}

/// Pastes imported HTML into an empty document and returns it.
fn pasted(html: &str) -> (Editor, reprise_clipboard::Import) {
    let import = import_html(html, ImportLimits::default()).unwrap();
    let mut editor = Editor::new(Document::new(1).unwrap(), SchemaRegistry::builtin());
    import
        .fragment
        .clone()
        .paste(&mut editor, None, "destination")
        .unwrap();
    (editor, import)
}

fn links_of(doc: &Document) -> Vec<(String, AnnotationData)> {
    let mut out = Vec::new();
    for a in doc.annotations_of_kind(Kind::Link).annotations {
        let AnnotationState::Active { pieces, .. } = &a.state else {
            continue;
        };
        let text: String = pieces
            .iter()
            .map(|p| {
                doc.block(p.node)
                    .unwrap()
                    .text
                    .to_string()
                    .get(p.bytes.clone())
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("|");
        out.push((text, a.data.clone()));
    }
    out
}

#[test]
fn html_export_wraps_linked_text_in_anchors_and_round_trips() {
    let (doc, ids) = document(&["read the docs now"]);
    let p = ids[0];
    doc.add_annotation(p, 9..13, &url("https://docs.example/a?x=1&y=2"))
        .unwrap();
    // Formatting that straddles the link's start: the anchor still nests.
    doc.format_text(
        p,
        5..11,
        &TextStyle {
            weight: Some(700),
            ..TextStyle::default()
        },
        RangePolicy::EXPANDING,
    )
    .unwrap();
    doc.commit();
    let (html, losses) = html_of(&doc);
    assert!(
        html.contains("<a href=\"https://docs.example/a?x=1&amp;y=2\">"),
        "{html}"
    );
    assert_eq!(html.matches("<a ").count(), html.matches("</a>").count());
    // Nothing was lost, so nothing is reported.
    assert!(annotation_losses(&losses).is_empty(), "{:?}", losses.notes);

    let (editor, import) = pasted(&html);
    assert!(
        !import
            .notes
            .iter()
            .any(|n| n.code.as_str().starts_with("annotation.")),
        "{:?}",
        import.notes
    );
    assert_eq!(
        links_of(editor.document()),
        vec![("docs".to_string(), url("https://docs.example/a?x=1&y=2"))]
    );
}

#[test]
fn internal_links_get_ids_and_survive_a_round_trip_inside_the_html() {
    let (doc, ids) = document(&["Introduction", "see the introduction above"]);
    doc.add_annotation(
        ids[1],
        8..20,
        &AnnotationData::Link(LinkTarget::Heading { node: ids[0] }),
    )
    .unwrap();
    doc.commit();
    let (html, _) = html_of(&doc);
    let id = format!("r{}", ids[0]).replace('@', "-");
    assert!(html.contains(&format!("id=\"{id}\"")), "{html}");
    assert!(html.contains(&format!("<a href=\"#{id}\"")), "{html}");
    // Only the target paragraph gets an id.
    assert_eq!(html.matches(" id=\"").count(), 1);

    let (editor, _) = pasted(&html);
    let links = links_of(editor.document());
    assert_eq!(links.len(), 1);
    let AnnotationData::Link(LinkTarget::Node { node }) = &links[0].1 else {
        panic!("{links:?}");
    };
    assert_eq!(
        editor.document().block(*node).unwrap().text.to_string(),
        "Introduction"
    );
    // A link to an id that is not in the pasted HTML is text, with a note.
    let (editor, import) = pasted("<p>go <a href=\"#nowhere\">there</a></p>");
    assert!(links_of(editor.document()).is_empty());
    assert!(
        import
            .notes
            .iter()
            .any(|n| n.code.as_str() == "clipboard.html-approximated")
    );
}

#[test]
fn unsafe_and_malformed_addresses_are_refused_but_the_text_stays() {
    let huge = format!("https://a.example/{}", "x".repeat(1 << 20));
    let hostile_hrefs = [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        "&#106;avascript:alert(1)",
        "java&#9;script:alert(1)",
        " javascript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "vbscript:msgbox(1)",
        "file:///etc/passwd",
        "//evil.example/",
        "/relative/path",
        "https://",
        "http:\\\\evil.example",
    ];
    let mut html = String::new();
    for (i, href) in hostile_hrefs.iter().enumerate() {
        html.push_str(&format!("<p>item{i} <a href=\"{href}\">bad{i}</a></p>"));
    }
    html.push_str(&format!("<p><a href=\"{huge}\">huge</a></p>"));
    html.push_str("<p><a href=\"https://good.example/\">good</a></p>");
    let (editor, import) = pasted(&html);
    // Only the good link was made; all the text is there.
    assert_eq!(
        links_of(editor.document()),
        vec![("good".to_string(), url("https://good.example/"))]
    );
    let text: String = editor
        .document()
        .blocks()
        .into_iter()
        .map(|b| editor.document().block(b).unwrap().text.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    for i in 0..hostile_hrefs.len() {
        assert!(text.contains(&format!("bad{i}")), "{text}");
    }
    assert!(text.contains("huge") && text.contains("good"));
    let codes: Vec<&str> = import.notes.iter().map(|n| n.code.as_str()).collect();
    assert!(codes.contains(&LINK_SCHEME.as_str()), "{codes:?}");
    assert!(codes.contains(&LINK_LENGTH.as_str()), "{codes:?}");
    assert!(codes.contains(&"annotation.link-invalid"), "{codes:?}");
    assert!(
        import
            .notes
            .iter()
            .filter(|n| n.code.as_str().starts_with("annotation."))
            .all(|n| n.severity == reprise_diag::Severity::Warning)
    );
}

#[test]
fn anchors_nest_and_close_deterministically() {
    // An anchor inside an anchor closes the first.
    let (editor, _) = pasted(
        "<p><a href=\"https://a.example\">one <a href=\"https://b.example\">two</a> three</a></p>",
    );
    assert_eq!(
        links_of(editor.document()),
        vec![
            ("one ".to_string(), url("https://a.example")),
            ("two".to_string(), url("https://b.example")),
        ]
    );
    // An anchor around several paragraphs is a link in each.
    let (editor, _) =
        pasted("<a href=\"https://c.example\"><p>first</p><p>second</p></a><p>third</p>");
    assert_eq!(
        links_of(editor.document()),
        vec![
            ("first".to_string(), url("https://c.example")),
            ("second".to_string(), url("https://c.example")),
        ]
    );
    // An anchor closed by its parent, and one never closed.
    let (editor, _) = pasted("<p><a href=\"https://d.example\">inside</p><p>outside</p>");
    assert_eq!(
        links_of(editor.document()),
        vec![("inside".to_string(), url("https://d.example"))]
    );
    let (editor, _) = pasted("<p>start <a href=\"https://e.example\">open end");
    assert_eq!(
        links_of(editor.document()),
        vec![("open end".to_string(), url("https://e.example"))]
    );
    // An anchor with no text, and one with no href, make no link.
    let (editor, _) = pasted("<p>a<a href=\"https://f.example\"></a>b <a name=\"x\">c</a></p>");
    assert!(links_of(editor.document()).is_empty());
}

#[test]
fn a_page_of_links_is_bounded_and_keeps_the_text() {
    let mut html = String::from("<p>");
    for i in 0..6000 {
        html.push_str(&format!("<a href=\"https://l.example/{i}\">w{i}</a> "));
    }
    html.push_str("</p>");
    let (editor, import) = pasted(&html);
    let links = links_of(editor.document());
    assert_eq!(links.len(), 4096);
    let doc = editor.document();
    let text = doc.block(doc.blocks()[0]).unwrap().text.to_string();
    assert!(text.contains("w5999") && text.contains("w0 "));
    assert!(
        import
            .notes
            .iter()
            .any(|n| n.code.as_str() == "clipboard.html-approximated")
    );
}

#[test]
fn overlapping_links_export_without_nested_anchors_and_say_so() {
    let (doc, ids) = document(&["alpha beta gamma delta"]);
    let p = ids[0];
    doc.add_annotation(p, 0..11, &url("https://one.example"))
        .unwrap();
    doc.add_annotation(p, 6..17, &url("https://two.example"))
        .unwrap();
    doc.add_annotation(p, 0..5, &comment("c1")).unwrap();
    doc.commit();
    let (html, losses) = html_of(&doc);
    assert!(
        !html.contains("<a href=\"https://one.example\"><a"),
        "{html}"
    );
    // Depth never exceeds one anchor.
    let mut depth = 0i32;
    for part in html.split('<') {
        if part.starts_with("a ") {
            depth += 1;
            assert!(depth <= 1, "{html}");
        } else if part.starts_with("/a>") {
            depth -= 1;
        }
    }
    // The first keeps the shared text, the second covers the rest.
    let (editor, _) = pasted(&html);
    assert_eq!(
        links_of(editor.document()),
        vec![
            ("alpha beta ".to_string(), url("https://one.example")),
            ("gamma ".to_string(), url("https://two.example")),
        ]
    );
    let reported = annotation_losses(&losses);
    assert!(
        reported
            .iter()
            .any(|(sev, m)| *sev == reprise_diag::Severity::Warning && m.contains("overlapping")),
        "{reported:?}"
    );
    assert!(
        reported
            .iter()
            .any(|(sev, m)| *sev == reprise_diag::Severity::Info && m.contains("comment anchors")),
        "{reported:?}"
    );
}

#[test]
fn plain_text_keeps_link_text_and_reports_the_loss() {
    let (doc, ids) = document(&["see the docs"]);
    doc.add_annotation(ids[0], 8..12, &url("https://docs.example"))
        .unwrap();
    doc.commit();
    let schemas = SchemaRegistry::builtin();
    let out = PlainText.export(&doc, None, &options(&schemas)).unwrap();
    assert_eq!(String::from_utf8(out.bytes).unwrap(), "see the docs");
    let reported = annotation_losses(&out.losses);
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!(reported[0].0, reprise_diag::Severity::Info);
    assert!(reported[0].1.contains("link addresses"));
    // A document with none loses nothing.
    let (plain, _) = document(&["nothing here"]);
    let out = PlainText.export(&plain, None, &options(&schemas)).unwrap();
    assert!(annotation_losses(&out.losses).is_empty());
}

#[test]
fn native_copy_carries_links_with_new_ids_and_leaves_comments_behind() {
    let (doc, ids) = document(&["one two three"]);
    let link = doc
        .add_annotation(ids[0], 4..7, &url("https://n.example"))
        .unwrap();
    let note = doc.add_annotation(ids[0], 0..3, &comment("t1")).unwrap();
    doc.commit();
    let schemas = SchemaRegistry::builtin();
    let out = Native.export(&doc, None, &options(&schemas)).unwrap();
    // The comment anchor stays behind, and the copy says so.
    assert!(
        out.losses
            .notes
            .iter()
            .any(|n| n.code == reprise_doc::fragment::ANNOTATION_DROPPED
                && n.severity == reprise_diag::Severity::Info)
    );
    let decoded = NativeFragment::decode(&out.bytes).unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    let pasted = decoded.paste(&mut editor, None, "elsewhere").unwrap();
    let pasted_link = pasted.ids.ranges[&link];
    assert_ne!(pasted_link, link);
    assert_eq!(
        editor.document().annotation(pasted_link).unwrap().data,
        url("https://n.example")
    );
    assert!(!pasted.ids.ranges.contains_key(&note));
    assert!(
        editor
            .document()
            .annotations_of_kind(Kind::Comment)
            .annotations
            .is_empty()
    );
}

fn pdf_of(doc: &Document) -> (checker::Report, reprise_clipboard::LossReport) {
    let engine = engine();
    let layout = engine.layout(doc);
    let out = PdfWithAssets {
        assets: &engine.assets,
    }
    .export(
        doc,
        Some(&layout),
        &ExportOptions {
            source_namespace: "source",
            schemas: &engine.schemas,
            fonts: Some(&engine.fonts),
        },
    )
    .unwrap();
    (check(&out.bytes), out.losses)
}

#[test]
fn the_pdf_has_link_elements_with_annotations_and_alt_text() {
    let (doc, ids) = document(&[
        "Introduction",
        "Read the manual and the introduction, then write to us.",
    ]);
    let body = ids[1];
    doc.add_annotation(body, 9..15, &url("https://manual.example/start?a=1&b=2"))
        .unwrap();
    doc.add_annotation(
        body,
        24..36,
        &AnnotationData::Link(LinkTarget::Heading { node: ids[0] }),
    )
    .unwrap();
    doc.add_annotation(body, 43..55, &url("mailto:team@example.com"))
        .unwrap();
    doc.add_annotation(body, 0..4, &comment("not-exported"))
        .unwrap();
    doc.commit();
    let (report, losses) = pdf_of(&doc);
    assert_eq!(report.links, 3, "{report:?}");
    assert_eq!(report.tagged_links, 3);
    assert_eq!(report.roles.iter().filter(|r| *r == "Link").count(), 3);
    assert_eq!(
        report.link_uris,
        vec![
            "https://manual.example/start?a=1&b=2".to_string(),
            "mailto:team@example.com".to_string()
        ]
    );
    assert_eq!(report.link_destinations, 1);
    // /Contents says what each is, for assistive technology.
    assert!(report.link_contents.iter().all(|c| !c.trim().is_empty()));
    assert!(
        report
            .link_contents
            .iter()
            .any(|c| c.contains("manual") && c.contains("https://manual.example"))
    );
    assert!(
        report
            .link_contents
            .iter()
            .any(|c| c.contains("Introduction"))
    );
    // The text is still tagged once and the file still claims PDF/UA.
    assert!(report.text.contains("Read the manual"));
    assert!(report.xmp.contains("pdfuaid"), "{}", report.xmp);
    let reported = annotation_losses(&losses);
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert!(reported[0].1.contains("comment anchors"), "the comment is omitted");
}

#[test]
fn a_link_over_several_lines_has_an_annotation_for_each_and_one_tag_per_run() {
    let long = "word ".repeat(120);
    let (doc, ids) = document(&[&long]);
    doc.add_annotation(ids[0], 3..400, &url("https://long.example"))
        .unwrap();
    doc.commit();
    let layout = engine().layout(&doc);
    let lines = layout.block(ids[0]).unwrap().lines.len();
    assert!(lines > 3);
    let (report, _) = pdf_of(&doc);
    assert!(report.links >= 3, "{report:?}");
    assert_eq!(report.links, report.tagged_links);
    assert_eq!(report.links, report.link_uris.len());
    assert!(report.link_uris.iter().all(|u| u == "https://long.example"));
}

#[test]
fn links_in_right_to_left_and_rotated_text_still_export_a_checked_pdf() {
    for fixture in [
        hostile::rtl_mixed().unwrap(),
        hostile::transformed_rtl().unwrap(),
        hostile::vertical_rl().unwrap(),
    ] {
        let snapshot = fixture.engine.layout(&fixture.doc);
        let node = snapshot.blocks[0].node;
        let len = snapshot.block(node).unwrap().text.len();
        let text = snapshot.block(node).unwrap().text.clone();
        let mut end = (len / 2).min(len);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut start = 2usize.min(end);
        while !text.is_char_boundary(start) {
            start -= 1;
        }
        fixture
            .doc
            .add_annotation(node, start..end, &url("https://bidi.example"))
            .unwrap();
        fixture.doc.commit();
        let layout = fixture.engine.layout(&fixture.doc);
        let out = PdfWithAssets {
            assets: &fixture.engine.assets,
        }
        .export(
            &fixture.doc,
            Some(&layout),
            &ExportOptions {
                source_namespace: "source",
                schemas: &fixture.engine.schemas,
                fonts: Some(&fixture.engine.fonts),
            },
        )
        .unwrap();
        let report = check(&out.bytes);
        assert!(report.links >= 1, "{}: {report:?}", fixture.name);
        assert_eq!(report.links, report.tagged_links, "{}", fixture.name);
        assert_eq!(report.links, report.link_uris.len(), "{}", fixture.name);
    }
}

#[test]
fn overlapping_and_off_page_links_are_reported_not_broken() {
    let (doc, ids) = document(&["alpha beta gamma delta", "other"]);
    let p = ids[0];
    doc.add_annotation(p, 0..11, &url("https://one.example"))
        .unwrap();
    doc.add_annotation(p, 6..17, &url("https://two.example"))
        .unwrap();
    doc.add_annotation(
        p,
        0..5,
        &AnnotationData::Link(LinkTarget::Node { node: ids[1] }),
    )
    .unwrap();
    doc.commit();
    // The target block disappears: the link is text, with a warning.
    doc.delete_block(ids[1]).unwrap();
    doc.commit();
    let engine = engine();
    let layout = engine.layout(&doc);
    let out = PdfWithAssets {
        assets: &engine.assets,
    }
    .export(
        &doc,
        Some(&layout),
        &ExportOptions {
            source_namespace: "source",
            schemas: &engine.schemas,
            fonts: Some(&engine.fonts),
        },
    )
    .unwrap();
    let report = check(&out.bytes);
    assert_eq!(report.links, report.tagged_links);
    assert!(report.link_uris.len() >= 2);
    assert!(
        out.losses
            .notes
            .iter()
            .any(|n| n.code.as_str() == "pdf.link-unresolved"
                && n.severity == reprise_diag::Severity::Warning),
        "{:?}",
        out.losses.notes
    );
    assert!(report.text.contains("alpha beta gamma delta"));
}

#[test]
fn a_hostile_native_fragment_is_refused_whole() {
    let (doc, ids) = document(&["one two three four"]);
    doc.add_annotation(ids[0], 4..7, &url("https://fine.example"))
        .unwrap();
    doc.commit();
    let schemas = SchemaRegistry::builtin();
    let honest = reprise_clipboard::copy_all(&doc, "source", &schemas, None, None).unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    let attacks: Vec<(&str, Box<dyn Fn(&mut NativeFragment)>)> = vec![
        (
            "a forbidden scheme",
            Box::new(|f| {
                for r in &mut f.fragment.ranges {
                    r.annotation = Some(url("javascript:alert(1)"));
                }
            }),
        ),
        (
            "an oversized thread id",
            Box::new(|f| {
                for r in &mut f.fragment.ranges {
                    r.annotation = Some(comment(&"t".repeat(5000)));
                }
            }),
        ),
        (
            "an empty annotated span",
            Box::new(|f| {
                for r in &mut f.fragment.ranges {
                    r.annotation = Some(url("https://a.example"));
                    r.bytes = 2..2;
                }
            }),
        ),
        (
            "more annotations than a document holds",
            Box::new(|f| {
                let template = f.fragment.ranges[0].clone();
                for i in 0..70_000u32 {
                    let mut r = template.clone();
                    r.id = reprise_doc::RangeId::parse(&format!("{i}@77")).unwrap();
                    f.fragment.ranges.push(r);
                }
            }),
        ),
    ];
    for (name, attack) in attacks {
        let mut hostile = honest.clone();
        attack(&mut hostile);
        let refused = hostile.fragment.validate().is_err()
            || hostile
                .encode()
                .ok()
                .and_then(|bytes| NativeFragment::decode(&bytes).ok())
                .is_none_or(|decoded| decoded.paste(&mut editor, None, "elsewhere").is_err());
        assert!(refused, "{name}");
        assert!(
            editor.document().blocks().is_empty() && !editor.document().has_annotations(),
            "{name}: nothing was written"
        );
    }
}
