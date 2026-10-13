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

#[test]
fn facade_state_exposes_raw_sections_bound_and_orphan_stories_and_body_breaks() {
    use reprise_doc::sections::{BreakKind, SectionId};
    let doc = Document::new(1).unwrap();
    let body = doc
        .append_block(BlockKind::Paragraph, "body", "body")
        .unwrap();
    let story = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "body", "running")])
        .unwrap();
    let orphan = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "body", "orphan")])
        .unwrap();
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[
            (
                "region.header.default".into(),
                serde_json::json!(story.to_string()),
            ),
            ("width".into(), serde_json::json!(i64::MAX)),
            ("unknown.future".into(), serde_json::json!("retain")),
        ]
        .into(),
        &[],
    )
    .unwrap();
    let life = doc.mint_lifetime(body).unwrap();
    doc.activate_start(body, &life, BreakKind::Section, &Default::default())
        .unwrap();
    doc.commit();
    let bytes = reprise_format::Package::new(
        &doc,
        reprise_format::DocumentId([1; 16]),
        reprise_doc::PersistenceMode::History,
    )
    .unwrap()
    .save()
    .unwrap();
    let session = reprise::Workspace::new()
        .open(
            &reprise::Payload::new(reprise::Open {
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    let state = session.state().unwrap().data;
    assert_eq!(state.blocks.len(), 1);
    assert_eq!(state.blocks[0].starts.as_ref().unwrap().life, life);
    assert_eq!(state.sections.len(), 2);
    assert_eq!(state.sections[0].first_block, None);
    assert_eq!(state.sections[0].own["raw"]["width"], i64::MAX.to_string());
    assert_eq!(state.sections[0].unknown_keys, ["unknown.future"]);
    let bound = state
        .stories
        .iter()
        .find(|s| s.id == story.to_string())
        .unwrap();
    assert_eq!(bound.bound_by.len(), 2);
    assert_eq!(bound.blocks[0].text, "running");
    let unbound = state
        .stories
        .iter()
        .find(|s| s.id == orphan.to_string())
        .unwrap();
    assert!(unbound.bound_by.is_empty());
    assert_eq!(unbound.blocks[0].text, "orphan");
}
