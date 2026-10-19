//! A PDF export of a document with sections (sections §15, S10): each page has
//! its own `MediaBox`, `/PageLabels` say what the pages are labelled, and the
//! running regions are `/Artifact` marked content with the right `Subtype`,
//! while the body's tagging is what it was.

#[path = "../../display/tests/common/checker.rs"]
mod checker;

use checker::{Report, check};
use reprise_clipboard::{ExportOptions, Exporter, LossReport, PdfWithAssets};
use reprise_doc::sections::{BreakKind, SectionId, SectionProperties};
use reprise_doc::{BlockKind, Document, Domain, LengthExpr, NewBlock, NodeId, Style};
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
    doc.set_page_setup_patch(reprise_doc::page_setup::PageSetupPatch {
        width: Some(Length::from_pt(300)),
        height: Some(Length::from_pt(200)),
        top: Some(Length::from_pt(30)),
        right: Some(Length::from_pt(20)),
        bottom: Some(Length::from_pt(30)),
        left: Some(Length::from_pt(20)),
    })
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

fn paragraphs(doc: &Document, count: usize) -> Vec<NodeId> {
    (0..count)
        .map(|i| {
            doc.append_block(BlockKind::Paragraph, "s", &format!("body page {i}"))
                .unwrap()
        })
        .collect()
}

fn story(doc: &Document, text: &str) -> NodeId {
    doc.create_story(&[NewBlock::new(BlockKind::Paragraph, "s", text)])
        .unwrap()
}

fn export(doc: &Document) -> (Report, LossReport, reprise_layout::LayoutSnapshot) {
    doc.commit();
    let engine = reprise_fixtures::engine();
    let layout = engine.layout(doc);
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &engine.schemas,
        fonts: Some(&engine.fonts),
    };
    let pdf = PdfWithAssets {
        assets: &engine.assets,
    }
    .export(doc, Some(&layout), &options)
    .unwrap();
    (check(&pdf.bytes), pdf.losses, layout)
}

fn pt(length: Length) -> f32 {
    length.to_pt_f32()
}

#[test]
fn every_page_has_the_media_box_of_its_own_size() {
    // Portrait, then a landscape section, then portrait again.
    let doc = doc();
    let p = paragraphs(&doc, 3);
    start(
        &doc,
        p[1],
        BreakKind::Section,
        &[("width", json!(400 * 1024)), ("height", json!(150 * 1024))],
    );
    start(&doc, p[2], BreakKind::Section, &[]);
    let (report, _, layout) = export(&doc);
    assert_eq!(layout.pages.len(), 3);
    let sizes: Vec<_> = layout
        .pages
        .iter()
        .map(|p| (pt(p.width), pt(p.height)))
        .collect();
    assert_eq!(sizes, [(300.0, 200.0), (400.0, 150.0), (300.0, 200.0)]);
    let boxes: Vec<_> = report
        .media_boxes
        .iter()
        .map(|b| (b[2] - b[0], b[3] - b[1]))
        .collect();
    assert_eq!(boxes, sizes);
}

#[test]
fn a_roman_front_matter_and_a_decimal_body_are_page_labels() {
    let doc = doc();
    first(&doc, &[("numbering.format", json!("lower-roman"))]);
    let p = paragraphs(&doc, 4);
    start(&doc, p[1], BreakKind::Page, &[]);
    start(
        &doc,
        p[2],
        BreakKind::Section,
        &[
            ("numbering.format", json!("decimal")),
            ("numbering.restart", json!(1)),
        ],
    );
    start(&doc, p[3], BreakKind::Page, &[]);
    let (report, losses, layout) = export(&doc);
    let labels: Vec<_> = layout
        .sections
        .as_ref()
        .unwrap()
        .pages
        .iter()
        .map(|p| p.label.clone())
        .collect();
    assert_eq!(labels, ["i", "ii", "1", "2"]);
    // Each page has its entry: i and ii in /r from 1 and 2, then 1 and 2 in /D.
    let entry =
        |page: i64, style: &str, number: i64| (page, style.to_owned(), String::new(), number);
    assert_eq!(
        report.page_labels,
        [
            entry(0, "r", 1),
            entry(1, "r", 2),
            entry(2, "D", 1),
            entry(3, "D", 2)
        ]
    );
    assert!(
        !losses
            .notes
            .iter()
            .any(|n| n.code == "export.sections" && n.message.contains("page labels")),
        "{:?}",
        losses.notes
    );
}

#[test]
fn a_restart_in_the_middle_labels_the_page_with_its_start() {
    let doc = doc();
    let p = paragraphs(&doc, 3);
    start(&doc, p[1], BreakKind::Page, &[]);
    start(
        &doc,
        p[2],
        BreakKind::Section,
        &[
            ("numbering.format", json!("upper-alpha")),
            ("numbering.restart", json!(3)),
        ],
    );
    let (report, _, layout) = export(&doc);
    let labels: Vec<_> = layout
        .sections
        .as_ref()
        .unwrap()
        .pages
        .iter()
        .map(|p| p.label.clone())
        .collect();
    assert_eq!(labels, ["1", "2", "C"]);
    let entry =
        |page: i64, style: &str, number: i64| (page, style.to_owned(), String::new(), number);
    assert_eq!(
        report.page_labels,
        [entry(0, "D", 1), entry(1, "D", 2), entry(2, "A", 3)]
    );
}

#[test]
fn labels_a_pdf_cannot_write_are_left_out_with_a_note_not_written_wrongly() {
    for (what, entries) in [
        (
            "a format PDF has no style for",
            vec![("numbering.format", json!("decimal-leading-zero"))],
        ),
        (
            "a counter below one",
            vec![("numbering.restart", json!(-2))],
        ),
    ] {
        let doc = doc();
        first(&doc, &entries);
        let p = paragraphs(&doc, 2);
        start(&doc, p[1], BreakKind::Page, &[]);
        let (report, losses, _) = export(&doc);
        assert!(report.page_labels.is_empty(), "{what}");
        let note = losses
            .notes
            .iter()
            .find(|n| n.code == "export.sections")
            .unwrap_or_else(|| panic!("{what}: {:?}", losses.notes));
        assert_eq!(note.severity, reprise_diag::Severity::Warning, "{what}");
        assert!(note.message.contains("no page labels"), "{what}: {note:?}");
    }
}

#[test]
fn a_document_without_sections_writes_no_page_labels() {
    let doc = doc();
    paragraphs(&doc, 2);
    let (report, losses, layout) = export(&doc);
    assert!(layout.sections.is_none());
    assert!(report.page_labels.is_empty());
    assert!(!losses.notes.iter().any(|n| n.code == "export.sections"));
}

#[test]
fn running_regions_are_artifacts_with_their_subtype_and_the_body_is_tagged_as_before() {
    let build = |regions: bool| {
        let doc = doc();
        if regions {
            let header = story(&doc, "Running head");
            let footer = story(&doc, "Running foot");
            let mark = story(&doc, "DRAFT");
            first(
                &doc,
                &[
                    ("region.header.default", json!(header.to_string())),
                    ("region.footer.default", json!(footer.to_string())),
                    ("region.background.default", json!(mark.to_string())),
                ],
            );
        }
        let p = paragraphs(&doc, 3);
        start(&doc, p[1], BreakKind::Page, &[]);
        start(&doc, p[2], BreakKind::Page, &[]);
        doc
    };
    let (plain, _, plain_layout) = export(&build(false));
    let (with, losses, layout) = export(&build(true));
    // The file still claims PDF/UA-1: the validator accepted the subtypes.
    assert!(
        losses.features.iter().any(|l| {
            l.feature == reprise_clipboard::Feature::ReadingOrder
                && l.disposition == reprise_clipboard::Disposition::Preserved
        }),
        "{:?}",
        losses.notes
    );
    let pages = layout.pages.len();
    assert_eq!(pages, 3);
    assert_eq!(plain_layout.pages.len(), pages);
    // Each page's header, footer and watermark carries its own subtype.
    assert_eq!(with.artifact_subtypes.get("Header"), Some(&pages));
    assert_eq!(with.artifact_subtypes.get("Footer"), Some(&pages));
    assert_eq!(with.artifact_subtypes.get("Watermark"), Some(&pages));
    assert!(plain.artifact_subtypes.is_empty());
    // The body is tagged exactly as it was without them: the same structure,
    // the same text in the same order, the same leaves. Running text is not in
    // the structure, and not in the extracted text.
    assert_eq!(with.roles, plain.roles);
    assert_eq!(with.text, plain.text);
    assert_eq!(with.leaves, plain.leaves);
    assert!(!with.text.contains("Running"));
    // Only artifacts were added: three runs a page.
    assert!(with.artifacts >= plain.artifacts + 3 * pages);
}

#[test]
fn a_header_story_may_use_the_same_story_in_two_slots() {
    let doc = doc();
    let shared = story(&doc, "Shared");
    first(
        &doc,
        &[
            ("region.header.default", json!(shared.to_string())),
            ("region.footer.default", json!(shared.to_string())),
        ],
    );
    paragraphs(&doc, 1);
    let (report, _, _) = export(&doc);
    assert_eq!(report.artifact_subtypes.get("Header"), Some(&1));
    assert_eq!(report.artifact_subtypes.get("Footer"), Some(&1));
    let _ = Domain::Body;
}
