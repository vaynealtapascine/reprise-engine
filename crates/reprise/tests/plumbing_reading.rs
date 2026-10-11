//! Reading-order steps carry byte ranges that slice the block text.
use reprise::*;
use std::collections::BTreeMap;

fn build(texts: &[&str]) -> DocumentSession {
    let mut s = Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: "1".into(),
        }))
        .unwrap();
    let commands = texts
        .iter()
        .enumerate()
        .map(|(i, t)| Command::InsertBlock {
            parent: None,
            index: i as u32,
            block_kind: BlockKind::Paragraph,
            text: (*t).into(),
            style: Style::default(),
        })
        .collect();
    s.apply(&Payload::new(Transaction { commands })).unwrap();
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    while !job.step(&mut s, 1000).unwrap().data.complete {}
    s
}

#[test]
fn ranges_slice_the_text_and_tile_each_block() {
    let long = "wrapping words in a narrow measure ".repeat(40);
    let rtl = "אבג דהו זחט ".repeat(30);
    let mixed = "emoji 👩‍👧 café \t tab\nbreak ".repeat(20);
    let s = build(&[&long, "", &rtl, &mixed, "x"]);
    let blocks: BTreeMap<_, _> = s
        .state()
        .unwrap()
        .data
        .blocks
        .into_iter()
        .map(|b| (b.id, b.text))
        .collect();
    let steps = s.reading_order().unwrap().data;
    assert!(steps.len() > 5, "the long paragraphs wrap");
    let mut covered: BTreeMap<String, usize> = BTreeMap::new();
    let mut last_line: BTreeMap<String, u32> = BTreeMap::new();
    for step in &steps {
        let text = &blocks[&step.node];
        let (start, end) = (step.start as usize, step.end as usize);
        assert!(start <= end && end <= text.len(), "{step:?}");
        assert!(
            text.get(start..end).is_some(),
            "a range ends on character boundaries: {step:?}"
        );
        // Lines of one block come in order and tile it with no gap.
        let at = covered.entry(step.node.clone()).or_insert(0);
        assert_eq!(*at, start, "lines tile the block: {step:?}");
        *at = end;
        let line = last_line.insert(step.node.clone(), step.line);
        assert!(line.is_none_or(|l| l < step.line));
    }
    for (node, text) in &blocks {
        assert_eq!(covered.get(node).copied().unwrap_or(0), text.len(), "{node}");
    }
}

#[test]
fn editing_retires_the_steps_until_layout_returns() {
    let mut s = build(&["abc"]);
    assert!(!s.reading_order().unwrap().data.is_empty());
    let node = s.state().unwrap().data.blocks[0].id.clone();
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertText {
            node,
            at: 0,
            text: "x".into(),
        }],
    }))
    .unwrap();
    assert_eq!(s.reading_order().unwrap_err().code(), "bindings.layout-required");
}
