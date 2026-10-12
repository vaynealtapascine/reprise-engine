use reprise_doc::{BlockKind, Document, Domain, NewBlock};

#[test]
#[allow(clippy::disallowed_methods)] // The facade's public compatibility aliases.
fn public_body_aliases_agree_with_explicit_domains() {
    let doc = Document::new(1).unwrap();
    let body = doc.append_block(BlockKind::Paragraph, "", "body").unwrap();
    doc.create_story(&[NewBlock::new(BlockKind::Paragraph, "", "story")])
        .unwrap();
    assert_eq!(doc.blocks(), [body]);
    assert_eq!(doc.children(None), doc.top(Domain::Body));
    assert_eq!(doc.document_order(), doc.order(Domain::Body));
}
