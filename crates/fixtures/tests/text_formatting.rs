use reprise_doc::{
    BlockKind, Document,
    formatting::{TextFeature, TextStyle},
    text::RangePolicy,
};
use reprise_geom::Length;
use reprise_layout::incremental::LayoutSession;

#[test]
fn mixed_sizes_shape_separately_and_invalidate_only_the_changed_paragraph() {
    let engine = reprise_fixtures::engine();
    let doc = Document::new(1).unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "", "small LARGE small")
        .unwrap();
    doc.append_block(BlockKind::Paragraph, "", "unaffected paragraph")
        .unwrap();
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    session.layout(&doc).unwrap();
    let mut undo = doc.undo_stack();
    doc.format_text(
        a,
        6..11,
        &TextStyle {
            size: Some(Length::from_pt(24)),
            ..TextStyle::default()
        },
        RangePolicy::EXPANDING,
    )
    .unwrap();
    doc.commit_step();
    let layout = session.layout(&doc).unwrap();
    assert_eq!(layout, engine.layout(&doc));
    assert_eq!(session.counters().shapes, 1);
    let block = layout.blocks.iter().find(|b| b.node == a).unwrap();
    let runs: Vec<_> = block.lines.iter().flat_map(|l| &l.runs).collect();
    assert!(
        runs.iter()
            .any(|r| r.range == (6..11) && r.size == Length::from_pt(24))
    );
    assert!(runs.iter().any(|r| r.size == Length::from_pt(10)));
    assert!(
        block
            .lines
            .iter()
            .all(|l| l.rect.height >= Length::from_pt(24))
    );
    assert!(undo.undo().unwrap());
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
    assert!(undo.redo().unwrap());
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
}

#[test]
fn formatting_across_flow_breaks_keeps_local_byte_offsets_and_bidi_context() {
    let engine = reprise_fixtures::engine();
    let doc = Document::new(1).unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "", "ab אבג office")
        .unwrap();
    doc.format_text(
        a,
        1..doc.block(a).unwrap().text.len(),
        &TextStyle {
            size: Some(Length::from_pt(16)),
            language: Some("he".into()),
            features: Some(vec![TextFeature {
                tag: *b"liga",
                value: 0,
            }]),
            ..TextStyle::default()
        },
        RangePolicy::EXPANDING,
    )
    .unwrap();
    let b = doc.split_block(a, 3).unwrap();
    doc.commit();
    let layout = engine.layout(&doc);
    let tail = layout.blocks.iter().find(|block| block.node == b).unwrap();
    assert_eq!(tail.text, "אבג office");
    assert!(
        tail.lines
            .iter()
            .flat_map(|l| &l.runs)
            .all(|r| r.size == Length::from_pt(16) && r.range.end <= tail.text.len())
    );
    assert!(
        tail.lines
            .iter()
            .flat_map(|l| &l.runs)
            .any(|r| r.level % 2 == 1)
    );
    assert!(
        !layout
            .diagnostics
            .iter()
            .any(|d| d.code == "shape.bad-style-run")
    );
}
