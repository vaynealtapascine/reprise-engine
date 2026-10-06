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
