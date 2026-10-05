//! Tagged PDF structure: parsed back from the emitted bytes by the checker.

mod common;

use common::checker::check;
use reprise_display::pdf;

fn ordered(fixture: &reprise_fixtures::hostile::Fixture) -> (Vec<u8>, String) {
    use reprise_layout::DisplayOptions;
    let snapshot = fixture.engine.layout(&fixture.doc);
    let lists = snapshot.to_display_lists(DisplayOptions::default());
    let order = snapshot.pdf_reading_order(&fixture.doc);
    let bytes = pdf::render_ordered(&lists, &fixture.engine.fonts, &order).unwrap();
    let expected = snapshot
        .reading_order(&fixture.doc)
        .into_iter()
        .flat_map(|step| {
            let line = snapshot.line(step.line).unwrap();
            let block = snapshot.block(step.line.node).unwrap();
            let mut runs: Vec<_> = line.runs.iter().collect();
            runs.sort_by_key(|r| r.range.start);
            runs.into_iter()
                .map(|r| block.text[r.range.clone()].to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    (bytes, expected)
}

#[test]
fn ordered_layouts_are_tagged_and_follow_reading_order() {
    for fixture in [
        reprise_fixtures::hostile::transformed_rtl().unwrap(),
        reprise_fixtures::hostile::vertical_rl().unwrap(),
        reprise_fixtures::hostile::spiral_text().unwrap(),
        reprise_fixtures::hostile::rational_rotation_extreme().unwrap(),
    ] {
        let (bytes, expected) = ordered(&fixture);
        let report = check(&bytes);
        assert_eq!(report.text, expected, "{}", fixture.name);
        // The PDF/UA-1 claim and the catalog entries it needs go together.
        let claims = report.xmp.contains("pdfuaid");
        assert_eq!(claims, report.display_doc_title, "{}", fixture.name);
        assert!(report.marked, "{}", fixture.name);
        assert_eq!(report.lang.as_deref(), Some("en"));
        assert!(report.xmp.contains("Untitled document"));
        assert_eq!(report.roles.first().map(String::as_str), Some("Document"));
        assert!(report.leaves > 0 && report.artifacts > 0);
        // No time or randomness: the same input gives the same bytes.
        assert_eq!(bytes, ordered(&fixture).0, "{}", fixture.name);
    }
}

#[test]
fn a_clean_document_claims_pdf_ua_1() {
    use reprise_doc::{BlockKind, Document};
    use reprise_layout::DisplayOptions;
    let doc = Document::new(reprise_fixtures::PEER).unwrap();
    doc.append_block(BlockKind::Paragraph, "", "A room.")
        .unwrap();
    doc.append_block(BlockKind::Paragraph, "", "Another office room.")
        .unwrap();
    doc.commit();
    let engine = reprise_fixtures::engine();
    let snapshot = engine.layout(&doc);
    let lists = snapshot.to_display_lists(DisplayOptions::default());
    let order = snapshot.pdf_reading_order(&doc);
    let bytes = pdf::render_ordered(&lists, &engine.fonts, &order).unwrap();
    let report = check(&bytes);
    assert!(report.xmp.contains("pdfuaid:part"), "{}", report.xmp);
    assert!(report.display_doc_title && report.marked);
    assert_eq!(report.text, "A room.Another office room.");
    assert_eq!(report.roles, ["Document", "P", "P"]);
    assert_eq!(report.outline_titles, Vec::<String>::new());
}
