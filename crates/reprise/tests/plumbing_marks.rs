//! Mark facts (edge, target, diagnostic) and the bounded diagnostics query.
use reprise::*;

fn session() -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: "1".into(),
        }))
        .unwrap()
}
fn apply(s: &mut DocumentSession, commands: Vec<Command>) -> Payload<Applied> {
    s.apply(&Payload::new(Transaction { commands })).unwrap()
}
fn finish(s: &mut DocumentSession) {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    for _ in 0..10_000 {
        if job.step(s, 1000).unwrap().data.complete {
            return;
        }
    }
    panic!("layout did not finish in bounded steps");
}
fn paragraph(text: &str, index: u32) -> Command {
    Command::InsertBlock {
        parent: None,
        index,
        block_kind: BlockKind::Paragraph,
        text: text.into(),
        style: Style::default(),
    }
}
fn anchor(
    node: &str,
    at: u32,
    edge: LineEdge,
    target: &str,
    target_at: u32,
    te: AnchorEdge,
) -> Command {
    Command::AddAnchor {
        node: node.into(),
        at,
        edge,
        target: target.into(),
        target_at,
        target_edge: te,
    }
}
fn anchors(s: &DocumentSession) -> Vec<Mark> {
    s.marks(0)
        .unwrap()
        .data
        .marks
        .into_iter()
        .filter(|m| m.kind == MarkKind::Anchor)
        .collect()
}

#[test]
fn applied_pin_reports_edge_target_and_no_diagnostic() {
    let mut s = session();
    let n = apply(&mut s, vec![paragraph("ab", 0)]).data.blocks[0].clone();
    apply(
        &mut s,
        vec![
            Command::InsertTab {
                node: n.clone(),
                at: 1,
            },
            Command::InsertLineBreak {
                node: n.clone(),
                at: 3,
            },
        ],
    );
    let id = apply(
        &mut s,
        vec![anchor(&n, 4, LineEdge::End, &n, 1, AnchorEdge::GapEnd)],
    )
    .data
    .relations[0]
        .clone();
    finish(&mut s);
    let marks = anchors(&s);
    let m = marks
        .iter()
        .find(|m| m.relation.as_ref() == Some(&id))
        .unwrap();
    assert!(m.applied);
    assert_eq!(m.edge, Some(LineEdge::End));
    assert_eq!(m.target_node.as_deref(), Some(n.as_str()));
    assert_eq!(m.target_offset, Some(1));
    assert_eq!(m.target_edge, Some(AnchorEdge::GapEnd));
    assert_eq!(m.diagnostic, None);
}

#[test]
fn cycle_reports_its_code() {
    let mut s = session();
    let ids = apply(&mut s, vec![paragraph("one", 0), paragraph("two", 1)])
        .data
        .blocks;
    let (a, b) = (ids[0].clone(), ids[1].clone());
    let rels = apply(
        &mut s,
        vec![
            anchor(&a, 0, LineEdge::Start, &b, 0, AnchorEdge::LineEnd),
            anchor(&b, 0, LineEdge::Start, &a, 0, AnchorEdge::LineEnd),
        ],
    )
    .data
    .relations;
    finish(&mut s);
    let marks = anchors(&s);
    assert_eq!(marks.len(), 2);
    for m in &marks {
        assert!(!m.applied);
        assert_eq!(m.diagnostic.as_deref(), Some("relation.alignment-cycle"));
        assert!(rels.contains(m.relation.as_ref().unwrap()));
    }
    // The same fact through the bounded query, by relation.
    let list = s
        .diagnostics_for(&Payload::new(DiagnosticFilter {
            subject: Some(rels[0].clone()),
            code_prefix: Some("relation.alignment".into()),
            ..DiagnosticFilter::default()
        }))
        .unwrap()
        .data;
    assert_eq!(list.diagnostics.len(), 1);
    assert!(!list.truncated);
    assert_eq!(list.diagnostics[0].code, "relation.alignment-cycle");
}

#[test]
fn out_of_reach_pin_is_applied_but_flagged() {
    let mut s = session();
    let long = "m".repeat(30);
    let ids = apply(&mut s, vec![paragraph(&long, 0), paragraph(&long, 1)])
        .data
        .blocks;
    let (a, b) = (ids[0].clone(), ids[1].clone());
    // Start of the second line at the end of the first: it cannot fit.
    apply(
        &mut s,
        vec![anchor(&b, 0, LineEdge::Start, &a, 0, AnchorEdge::LineEnd)],
    );
    finish(&mut s);
    let marks = anchors(&s);
    assert_eq!(marks.len(), 1);
    assert_eq!(
        marks[0].diagnostic.as_deref(),
        Some("relation.alignment-outside")
    );
    assert!(marks[0].applied);
}

#[test]
fn limit_reports_its_code() {
    let mut s = session();
    let mut commands = vec![paragraph("target", 0)];
    for i in 1..=258 {
        commands.push(paragraph("x", i));
    }
    let blocks = apply(&mut s, commands).data.blocks;
    let target = blocks[0].clone();
    let pins: Vec<_> = blocks[1..]
        .iter()
        .map(|b| anchor(b, 0, LineEdge::Start, &target, 0, AnchorEdge::Position))
        .collect();
    apply(&mut s, pins);
    finish(&mut s);
    let list = s
        .diagnostics_for(&Payload::new(DiagnosticFilter {
            code_prefix: Some("relation.alignment-limit".into()),
            limit: Some(3),
            ..DiagnosticFilter::default()
        }))
        .unwrap()
        .data;
    assert_eq!(list.diagnostics.len(), 3);
    assert!(list.truncated, "258 pins exceed the 256-line domain");
}

#[test]
fn diagnostics_filter_is_bounded_and_validated() {
    let s = session();
    let ok = s
        .diagnostics_for(&Payload::new(DiagnosticFilter::default()))
        .unwrap();
    assert!(ok.data.diagnostics.is_empty());
    let bad = Payload {
        version: 9,
        data: DiagnosticFilter::default(),
    };
    assert_eq!(
        s.diagnostics_for(&bad).unwrap_err().code(),
        "bindings.version"
    );
    let none = s
        .diagnostics_for(&Payload::new(DiagnosticFilter {
            limit: Some(0),
            ..DiagnosticFilter::default()
        }))
        .unwrap();
    assert!(none.data.diagnostics.is_empty());
}
