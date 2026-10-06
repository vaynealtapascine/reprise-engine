//! Tagged PDF export over real layouts: every hostile fixture exports a PDF whose
//! structure the checker accepts, and the structure says what the document says.

#[path = "../../display/tests/common/checker.rs"]
mod checker;

use checker::{Report, check};
use reprise_clipboard::{Disposition, ExportOptions, Exporter, Feature, PdfWithAssets, PlainText};
use reprise_fixtures::hostile::{self, Fixture};

fn export(f: &Fixture) -> (Vec<u8>, Report, reprise_clipboard::LossReport, String) {
    let layout = f.engine.layout(&f.doc);
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &f.engine.schemas,
        fonts: Some(&f.engine.fonts),
    };
    let pdf = PdfWithAssets {
        assets: &f.engine.assets,
    }
    .export(&f.doc, Some(&layout), &options)
    .unwrap_or_else(|e| panic!("{}: {e}", f.name));
    let plain = PlainText.export(&f.doc, Some(&layout), &options).unwrap();
    let report = check(&pdf.bytes);
    // Text that is never drawn (a run with no glyphs, or a size of zero or less)
    // has nothing to tag, so it cannot appear in the PDF's text.
    let undrawn = layout
        .blocks
        .iter()
        .flat_map(|b| b.lines.iter())
        .flat_map(|l| l.runs.iter())
        .any(|r| r.glyphs.is_empty() || r.size <= reprise_geom::Length::ZERO);
    let unplaced =
        undrawn
            || plain.losses.features.iter().any(|l| {
                l.feature == Feature::ReadingOrder && l.disposition != Disposition::Preserved
            });
    let plain = String::from_utf8(plain.bytes).unwrap();
    (
        pdf.bytes,
        report,
        pdf.losses,
        if unplaced { String::new() } else { plain },
    )
}

fn squeeze(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn every_hostile_fixture_exports_a_checked_tagged_pdf() {
    let mut exported = 0;
    for f in hostile::all().unwrap() {
        let (bytes, report, _, plain) = export(&f);
        assert_eq!(
            report.roles.first().map(String::as_str),
            Some("Document"),
            "{}",
            f.name
        );
        assert!(report.marked, "{}", f.name);
        assert_eq!(
            report.xmp.contains("pdfuaid"),
            report.display_doc_title,
            "{}",
            f.name
        );
        // Extracted text, in structure order, is the plain-text export.
        if !plain.is_empty() {
            assert_eq!(squeeze(&report.text), squeeze(&plain), "{}", f.name);
        }
        assert_eq!(
            (report.links, report.tagged_links),
            (report.links, report.links),
            "{}",
            f.name
        );
        let ids: std::collections::BTreeSet<_> = report.note_ids.iter().collect();
        assert_eq!(
            ids.len(),
            report.note_ids.len(),
            "{}: note IDs are unique",
            f.name
        );
        // The same document gives the same bytes.
        assert_eq!(bytes, export(&f).0, "{}", f.name);
        exported += 1;
    }
    assert!(exported > 50);
}

fn fixture_report(name: &str) -> Report {
    export(
        &hostile::all()
            .unwrap()
            .into_iter()
            .find(|f| f.name == name)
            .unwrap(),
    )
    .1
}

#[test]
fn notes_are_note_elements_with_ids_and_linked_references() {
    for (name, notes) in [
        ("note_taller_than_page", 1),
        ("note_on_last_line", 1),
        ("notes_take_over_page", 1),
        ("notes_nested_three_deep", 3),
        ("image_note", 1),
    ] {
        let r = fixture_report(name);
        let count = r.roles.iter().filter(|x| *x == "Note").count();
        assert_eq!(count, notes, "{name}: {:?}", r.roles);
        assert_eq!(r.note_ids.len(), notes, "{name}");
        assert_eq!(
            r.roles.iter().filter(|x| *x == "Reference").count(),
            notes,
            "{name}"
        );
        assert_eq!((r.links, r.tagged_links), (notes, notes), "{name}: linked");
    }
}

#[test]
fn floats_tables_and_images_get_their_elements() {
    let r = fixture_report("float_moves_its_anchor");
    assert!(r.roles.iter().any(|x| x == "Div"), "{:?}", r.roles);
    for name in ["table_conflicting_widths", "table_row_taller_than_page"] {
        let r = fixture_report(name);
        let tables: Vec<&str> = r
            .roles
            .iter()
            .map(String::as_str)
            .filter(|x| matches!(*x, "Table" | "TR" | "TD" | "TH"))
            .collect();
        assert_eq!(tables.first(), Some(&"Table"), "{name}");
        assert_eq!(tables.get(1), Some(&"TR"), "{name}");
        assert_eq!(tables.get(2), Some(&"TD"), "{name}");
        assert!(
            !tables.contains(&"TH"),
            "{name}: header cells await the tables workstream"
        );
    }
    let r = fixture_report("image_float");
    assert_eq!(r.roles.iter().filter(|x| *x == "Figure").count(), 1);
    assert_eq!(r.figure_alts.len(), 1);
}

fn simple(blocks: &[(&str, &str)]) -> (reprise_doc::Document, reprise_layout::Engine) {
    let doc = reprise_doc::Document::new(reprise_fixtures::PEER).unwrap();
    for (style, text) in blocks {
        doc.append_block(reprise_doc::BlockKind::Paragraph, style, text)
            .unwrap();
    }
    doc.commit();
    (doc, reprise_fixtures::engine())
}

#[test]
fn heading_styles_make_headings_titles_and_an_outline() {
    let (doc, engine) = simple(&[
        ("h1", "The House"),
        ("", "A room."),
        ("Heading-2", "The Hall"),
        ("", "Another room."),
        ("H3", "The Door"),
    ]);
    let layout = engine.layout(&doc);
    let pdf = reprise_clipboard::export_pdf(
        &doc,
        &layout,
        &engine.fonts,
        &engine.assets,
        &Default::default(),
    )
    .unwrap();
    let r = check(&pdf.bytes);
    assert_eq!(r.roles, ["Document", "H1", "P", "H2", "P", "H3"]);
    assert_eq!(r.outline_titles, ["The House", "The Hall", "The Door"]);
    assert!(
        r.xmp.contains("The House"),
        "the first heading titles the PDF"
    );
    assert!(r.xmp.contains("pdfuaid:part"));
    // The caller can name the title and language; the document records neither.
    let meta = reprise_clipboard::PdfMetadata {
        title: Some("Navidson Record".into()),
        lang: Some("de".into()),
    };
    let pdf =
        reprise_clipboard::export_pdf(&doc, &layout, &engine.fonts, &engine.assets, &meta).unwrap();
    let r = check(&pdf.bytes);
    assert!(r.xmp.contains("Navidson Record"));
    assert_eq!(r.lang.as_deref(), Some("de"));
    // Without a heading the title is the start of the first block.
    let (doc, engine) = simple(&[("", "Only text here.")]);
    let layout = engine.layout(&doc);
    let pdf = reprise_clipboard::export_pdf(
        &doc,
        &layout,
        &engine.fonts,
        &engine.assets,
        &Default::default(),
    )
    .unwrap();
    assert!(check(&pdf.bytes).xmp.contains("Only text here."));
}

#[test]
fn an_authored_reading_override_orders_the_structure_not_the_paint() {
    use reprise_doc::{BlockKind, Document, SchemaRegistry};
    let doc = Document::new(reprise_fixtures::PEER).unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "", "A room.")
        .unwrap();
    let b = doc
        .append_block(BlockKind::Paragraph, "", "B room.")
        .unwrap();
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &reprise_doc::reading::before(b, a),
    )
    .unwrap();
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
    assert_eq!(check(&pdf.bytes).text, "B room.A room.");
    let reading = pdf
        .losses
        .features
        .iter()
        .find(|l| l.feature == Feature::ReadingOrder)
        .unwrap();
    assert_eq!(reading.disposition, Disposition::Preserved);
    assert!(reading.detail.contains("PDF/UA-1"), "{}", reading.detail);
}

#[test]
fn an_unmet_requirement_is_reported_in_the_loss_report() {
    // The note fixtures use text the bundled font lacks glyphs for.
    let f = hostile::all()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "transformed_rtl")
        .unwrap();
    let (_, report, losses, _) = export(&f);
    assert!(!report.xmp.contains("pdfuaid"));
    assert!(
        losses
            .notes
            .iter()
            .any(|n| n.code == "pdf.ua-not-met" && n.severity == reprise_diag::Severity::Warning)
    );
    let reading = losses
        .features
        .iter()
        .find(|l| l.feature == Feature::ReadingOrder)
        .unwrap();
    assert_eq!(reading.disposition, Disposition::Approximated);
}

#[test]
fn the_structure_storm_keeps_every_run_once_and_reports_what_it_cannot_link() {
    let f = hostile::pdf_structure_storm().unwrap();
    let (_, r, losses, plain) = export(&f);
    assert_eq!(&r.roles[..2], ["Document", "H1"]);
    for role in ["Div", "Table", "TR", "TD", "Note", "Reference"] {
        assert!(r.roles.iter().any(|x| x == role), "{role}: {:?}", r.roles);
    }
    // The empty note has no glyphs and so no element; the others keep their IDs.
    assert_eq!(r.note_ids.len(), 6);
    // Anchors that cut a ligature or overlap another are reported, not linked.
    let codes: Vec<String> = losses.notes.iter().map(|n| n.code.to_string()).collect();
    for code in ["pdf.run-split", "pdf.reference-unlinked"] {
        assert!(codes.iter().any(|c| c == code), "{code}: {codes:?}");
    }
    let references = r.roles.iter().filter(|x| *x == "Reference").count();
    assert!(r.links >= 1 && r.links <= references);
    assert_eq!((r.links, r.tagged_links), (r.links, r.links));
    // Every run is drawn once, in reading order: the text is the plain text.
    assert_eq!(squeeze(&r.text), squeeze(&plain));
}
