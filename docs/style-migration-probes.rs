//! Standalone expected-failing acceptance probes for pending findings 8/10.
//! They are outside the passing workspace gate: no migration is implemented.
//! Compile with rustc --test and the current reprise_doc rlib.
use reprise_doc::{BlockKind, Document, PersistenceMode, Style, StylePatch};

#[test]
fn rename_preserves_a_concurrent_property_patch() {
    for reverse in [false, true] {
        let a = Document::new(1).unwrap();
        a.define_named_style(
            "Quote",
            &Style {
                weight: Some(400),
                ..Default::default()
            },
        )
        .unwrap();
        let node = a
            .append_block(BlockKind::Paragraph, "Quote", "quote")
            .unwrap();
        a.commit();
        let b = a.fork(2).unwrap();
        a.rename_named_style("Quote", "Quotation").unwrap();
        b.patch_named_style(
            "Quote",
            &StylePatch {
                set: Style {
                    color: Some([0, 0, 255, 255]),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        a.commit();
        b.commit();
        if reverse {
            b.merge(&a).unwrap();
            a.merge(&b).unwrap();
        } else {
            a.merge(&b).unwrap();
            b.merge(&a).unwrap();
        }
        for doc in [&a, &b] {
            assert_eq!(
                doc.computed_style(node).unwrap().color,
                Some([0, 0, 255, 255])
            );
        }
    }
}

#[test]
fn authoring_an_untouched_builtin_declares_paragraph_semantics() {
    let doc = Document::new(1).unwrap();
    doc.append_block(BlockKind::Paragraph, "Heading 1", "title")
        .unwrap();
    doc.commit();
    assert!(
        doc.has_paragraph_styles(),
        "no required semantic signal for built-in use"
    );
}

#[test]
fn a_legacy_unmarked_undefined_heading_keeps_base_appearance() {
    let doc = Document::new(1).unwrap();
    let heading = doc
        .append_block(BlockKind::Paragraph, "Heading 1", "title")
        .unwrap();
    let base = doc.append_block(BlockKind::Paragraph, "", "base").unwrap();
    doc.commit();
    assert!(!doc.has_paragraph_styles());
    let old = Document::import(&doc.try_export(PersistenceMode::History).unwrap(), 2).unwrap();
    assert_eq!(
        old.computed_style(heading).unwrap().size,
        old.computed_style(base).unwrap().size
    );
}
