use reprise_doc::{
    BlockKind, Document, PersistenceMode, Style, formatting::TextStyle, text::RangePolicy,
};
use reprise_format::{
    DocumentId, Limits, MigrationRegistry, Package,
    features::{REQUIRED_TEXT_EMPHASIS, REQUIRED_TEXT_FORMATTING, table_features},
};
#[test]
fn emphasis_requires_a_separate_bit_even_in_named_styles_and_tombstones() {
    let doc = Document::new(1).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "", "text").unwrap();
    doc.commit();
    assert_eq!(table_features(&doc).required, 0);
    doc.format_text(
        node,
        0..4,
        &TextStyle {
            language: Some("en".into()),
            ..Default::default()
        },
        RangePolicy::FIXED,
    )
    .unwrap();
    doc.commit();
    assert_eq!(table_features(&doc).required, REQUIRED_TEXT_FORMATTING);
    let id = doc
        .format_text(
            node,
            0..4,
            &TextStyle {
                weight: Some(700),
                ..Default::default()
            },
            RangePolicy::FIXED,
        )
        .unwrap();
    doc.commit();
    assert_eq!(
        table_features(&doc).required,
        REQUIRED_TEXT_FORMATTING | REQUIRED_TEXT_EMPHASIS
    );
    doc.remove_text_format(id).unwrap();
    doc.commit();
    assert_eq!(
        table_features(&doc).required,
        REQUIRED_TEXT_FORMATTING | REQUIRED_TEXT_EMPHASIS
    );
    let bytes = Package::new(&doc, DocumentId([42; 16]), PersistenceMode::History)
        .unwrap()
        .save()
        .unwrap();
    let required = u64::from_le_bytes(bytes[28..36].try_into().unwrap());
    assert_ne!(
        required & !(REQUIRED_TEXT_FORMATTING | (1 << 8)),
        0,
        "an old reader sees an unknown required feature"
    );
    Package::open(&bytes, 2, Limits::default(), &MigrationRegistry::builtin()).unwrap();
    let only_style = Document::new(3).unwrap();
    only_style
        .define_style(
            "unused",
            &Style {
                color: Some([0, 0, 0, 0]),
                ..Default::default()
            },
        )
        .unwrap();
    only_style.commit();
    assert_eq!(table_features(&only_style).required, REQUIRED_TEXT_EMPHASIS);
}
