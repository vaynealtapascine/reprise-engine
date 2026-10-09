use reprise_doc::{
    BlockKind, Document, SchemaRegistry, Style,
    marks::{Alignment, AnchorEdge, LineEdge, TabStop, TabStops},
};
use reprise_edit::{Affinity, Caret, Command, Editor, Navigator};
use reprise_geom::{Length, Point};
use reprise_layout::{LineRef, RelationStatus, marks::MarkKind};

fn make_doc(text: &str) -> (Document, reprise_doc::NodeId) {
    let doc = Document::new(1).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "", text).unwrap();
    doc.commit();
    (doc, node)
}
fn tabs(position: Option<Length>, alignment: Alignment) -> TabStops {
    TabStops {
        interval: Length::from_pt(36),
        stops: vec![TabStop {
            position,
            alignment,
            leader: None,
        }],
    }
}
#[test]
fn forced_breaks_include_empty_tail_and_caret_hits() {
    for text in ["", "\n", "a\n", "a\n\nb", "a\r\nb", "a\u{2028}"] {
        let (doc, node) = make_doc(text);
        let snapshot = reprise_fixtures::engine().layout(&doc);
        let block = snapshot.block(node).unwrap();
        let breaks = text
            .chars()
            .filter(|&c| c == '\n' || c == '\u{2028}')
            .count();
        assert_eq!(block.lines.len(), breaks + 1, "{text:?}");
        let nav = Navigator::semantic(&snapshot, &doc);
        for offset in reprise_doc::text::segment::grapheme_boundaries(text) {
            let caret = Caret {
                node,
                offset,
                affinity: Affinity::Downstream,
            };
            let rect = nav.caret_rect(caret).unwrap();
            let hit = nav.hit(rect.page, rect.point()).unwrap();
            let again = nav.caret_rect(hit.caret).unwrap();
            assert_eq!(rect, again, "{text:?} at {offset}");
        }
        assert_eq!(
            snapshot
                .marks(0)
                .iter()
                .filter(|m| m.kind == MarkKind::LineBreak)
                .count(),
            breaks
        );
    }
}

#[test]
fn a_trailing_caret_continues_to_the_next_frame_without_inventing_marks() {
    let (doc, node) = make_doc("a\n");
    doc.set_overrides(
        node,
        &Style {
            size: Some(reprise_doc::LengthExpr::Pt(Length::from_pt(10))),
            line_height: Some(reprise_doc::LengthExpr::Pt(Length::from_pt(12))),
            ..Default::default()
        },
    )
    .unwrap();
    let frame = reprise_fixtures::templates::flow_frame(
        "one",
        reprise_doc::Dim::pt(0),
        reprise_doc::Dim::pt(0),
        reprise_doc::Dim::pt(100),
        reprise_doc::Dim::pt(12),
    );
    doc.define_page_template(
        &reprise_doc::PageTemplate::new(
            "one",
            reprise_doc::Dim::pt(100),
            reprise_doc::Dim::pt(100),
        )
        .with_frame(frame),
    )
    .unwrap();
    doc.use_page_template("one").unwrap();
    doc.commit();
    let mut engine = reprise_fixtures::engine();
    engine.flow.max_pages = 2;
    let snapshot = engine.layout(&doc);
    assert_eq!(snapshot.pages.len(), 2);
    assert_eq!(snapshot.block(node).unwrap().lines[1].text, 2..2);
    assert!(
        !snapshot
            .marks(0)
            .iter()
            .any(|m| m.kind == MarkKind::ParagraphEnd)
    );
    assert!(
        snapshot
            .marks(1)
            .iter()
            .any(|m| m.kind == MarkKind::ParagraphEnd)
    );
    engine.flow.max_pages = 1;
    let snapshot = engine.layout(&doc);
    assert!(
        snapshot
            .diagnostics_with("layout.text-unplaced")
            .next()
            .is_some()
    );
    assert!(
        !snapshot
            .marks(0)
            .iter()
            .any(|m| m.kind == MarkKind::ParagraphEnd)
    );
}
#[test]
fn tabs_align_fields_and_fill_to_interval_end() {
    for alignment in [Alignment::Start, Alignment::Centre, Alignment::End] {
        let (doc, node) = make_doc("a\tfield\nnext");
        let stop = Length::from_pt(100);
        doc.set_overrides(
            node,
            &Style {
                tabs: Some(tabs(Some(stop), alignment)),
                ..Default::default()
            },
        )
        .unwrap();
        doc.commit();
        let snapshot = reprise_fixtures::engine().layout(&doc);
        let line = &snapshot.block(node).unwrap().lines[0];
        let gap = snapshot
            .marks(0)
            .into_iter()
            .find(|m| m.kind == MarkKind::Gap)
            .unwrap();
        let frame = snapshot.frame(line.frame).unwrap();
        let after = frame.to_page.inverse().unwrap().apply(gap.to.unwrap()).x;
        let before = frame.to_page.inverse().unwrap().apply(gap.from).x;
        assert!(after > before);
        let field: Length = line
            .runs
            .iter()
            .flat_map(|r| &r.glyphs)
            .filter(|g| (2..7).contains(&(g.cluster as usize)))
            .map(|g| g.advance)
            .sum();
        let offset = match alignment {
            Alignment::Start => Length::ZERO,
            Alignment::Centre => field.mul_ratio(1, 2),
            Alignment::End => field,
        };
        assert_eq!(after + offset, stop);
        let nav = Navigator::semantic(&snapshot, &doc);
        let hit = nav
            .hit(
                0,
                Point::new(gap.to.unwrap().x - Length(1), gap.to.unwrap().y),
            )
            .unwrap();
        assert_eq!(hit.caret.offset, 2);
    }
    let (doc, node) = make_doc("left\tright");
    doc.set_overrides(
        node,
        &Style {
            tabs: Some(tabs(None, Alignment::End)),
            ..Default::default()
        },
    )
    .unwrap();
    doc.commit();
    let snapshot = reprise_fixtures::engine().layout(&doc);
    let line = &snapshot.block(node).unwrap().lines[0];
    assert_eq!(line.width, line.available.width());
}
#[test]
fn concurrent_tab_break_and_deleted_target_converge_with_undo() {
    let (doc, node) = make_doc("first\nsecond");
    let target = doc
        .append_block(BlockKind::Paragraph, "", "target")
        .unwrap();
    doc.commit();
    let peer = doc.fork(2).unwrap();
    let mut a = Editor::new(doc, SchemaRegistry::builtin());
    let mut b = Editor::new(peer, SchemaRegistry::builtin());
    a.apply_command(Command::InsertTab { node, at: 5 }).unwrap();
    b.apply_command(Command::InsertLineBreak { node, at: 5 })
        .unwrap();
    a.apply_command(Command::AddAnchor {
        node,
        at: 0,
        edge: LineEdge::Start,
        target,
        target_at: 0,
        target_edge: AnchorEdge::Position,
    })
    .unwrap();
    b.apply_command(Command::DeleteBlock { node: target })
        .unwrap();
    a.merge(b.document()).unwrap();
    b.merge(a.document()).unwrap();
    assert_eq!(
        a.document().block(node).unwrap().text.to_string(),
        b.document().block(node).unwrap().text.to_string()
    );
    let engine = reprise_fixtures::engine();
    let layout = engine.layout(a.document());
    assert_eq!(layout, engine.layout(b.document()));
    assert_eq!(layout.relations[0].status, RelationStatus::Missing);
    assert_eq!(
        layout
            .marks(0)
            .iter()
            .find(|m| m.kind == MarkKind::Anchor)
            .unwrap()
            .state,
        Some(RelationStatus::Missing)
    );
    assert!(a.undo().unwrap()); // anchor
    assert!(a.undo().unwrap()); // tab, retaining peer's newline
    assert!(
        a.document()
            .block(node)
            .unwrap()
            .text
            .to_string()
            .contains("\n\n")
    );
    assert!(a.redo().unwrap());
    assert!(a.redo().unwrap());
}
#[test]
fn pins_follow_reflow_and_cycles_are_bounded() {
    let (doc, node) = make_doc("word word word word word word\nsource");
    let at = doc
        .block(node)
        .unwrap()
        .text
        .to_string()
        .find("source")
        .unwrap();
    let id = doc
        .pin_line(node, at, LineEdge::Start, node, 4, AnchorEdge::Position)
        .unwrap();
    doc.commit();
    let mut engine = reprise_fixtures::engine();
    for width in [100, 420] {
        engine.medium.width = Length::from_pt(width);
        let layout = engine.layout(&doc);
        let r = layout.relation(id).unwrap();
        assert!(r.applied);
        let m = layout
            .marks(0)
            .into_iter()
            .find(|m| m.relation == Some(id))
            .unwrap();
        assert_eq!(m.from.x, m.to.unwrap().x);
    }
    let (doc, a) = make_doc("first");
    let b = doc
        .append_block(BlockKind::Paragraph, "", "second")
        .unwrap();
    let ids = reprise_fixtures::marks::cycle(&doc, a, b).unwrap();
    doc.commit();
    let layout = engine.layout(&doc);
    assert!(ids.iter().all(|&id| !layout.relation(id).unwrap().applied));
    assert_eq!(
        layout.diagnostics_with("relation.alignment-cycle").count(),
        2
    );
}
#[test]
fn per_line_defaults_and_marks_never_mutate_display() {
    let (doc, node) = make_doc("first\nsecond\nthird");
    doc.align_line(node, 6, Alignment::End).unwrap();
    doc.commit();
    let engine = reprise_fixtures::engine();
    let snapshot = engine.layout(&doc);
    let lines = &snapshot.block(node).unwrap().lines;
    assert_eq!(lines[0].rect.origin.x, Length::ZERO);
    assert!(lines[1].runs[0].x > Length::ZERO);
    assert_eq!(lines[2].rect.origin.x, Length::ZERO);
    let bytes = doc.export(reprise_doc::PersistenceMode::History);
    let display = snapshot.to_display_list(0, Default::default());
    for _ in 0..20 {
        assert!(
            snapshot
                .marks(0)
                .iter()
                .any(|m| m.kind == MarkKind::Alignment)
        );
    }
    assert_eq!(display, snapshot.to_display_list(0, Default::default()));
    assert_eq!(bytes, doc.export(reprise_doc::PersistenceMode::History));
    assert_eq!(
        snapshot.line_containing(node, 6),
        Some(LineRef { node, line: 1 })
    );
}
#[test]
fn thousands_extremes_and_vertical_tabs_terminate() {
    let (doc, node) = make_doc(&"\t".repeat(4000));
    doc.set_overrides(
        node,
        &Style {
            tabs: Some(TabStops {
                interval: Length(1),
                stops: Vec::new(),
            }),
            ..Default::default()
        },
    )
    .unwrap();
    doc.commit();
    let snapshot = reprise_fixtures::engine().layout(&doc);
    assert_eq!(
        snapshot
            .marks(0)
            .iter()
            .filter(|m| m.kind == MarkKind::Gap)
            .count(),
        4000
    );
    for length in [Length::MIN, Length::ZERO, Length(-1)] {
        assert!(
            TabStops {
                interval: length,
                stops: Vec::new()
            }
            .validate()
            .is_err()
        );
    }
    for mode in [
        reprise_doc::WritingMode::VerticalRl,
        reprise_doc::WritingMode::VerticalLr,
    ] {
        let (doc, node) = make_doc("one\ttwo\nthree");
        let mut frame = reprise_fixtures::templates::flow_frame(
            "v",
            reprise_doc::Dim::pt(20),
            reprise_doc::Dim::pt(20),
            reprise_doc::Dim::pt(160),
            reprise_doc::Dim::pt(200),
        );
        frame.writing_mode = mode;
        let template = reprise_doc::PageTemplate::new(
            "v",
            reprise_doc::Dim::pt(300),
            reprise_doc::Dim::pt(300),
        )
        .with_frame(frame);
        doc.define_page_template(&template).unwrap();
        doc.use_page_template("v").unwrap();
        doc.commit();
        let snapshot = reprise_fixtures::engine().layout(&doc);
        let gap = snapshot
            .marks(0)
            .into_iter()
            .find(|m| m.kind == MarkKind::Gap)
            .unwrap();
        assert_eq!(gap.from.x, gap.to.unwrap().x);
        assert!(gap.to.unwrap().y > gap.from.y);
        assert_eq!(snapshot.block(node).unwrap().lines.len(), 2);
    }
    let (doc, node) = make_doc("אב\tגד");
    doc.set_overrides(
        node,
        &Style {
            tabs: Some(tabs(Some(Length::from_pt(100)), Alignment::Start)),
            ..Default::default()
        },
    )
    .unwrap();
    doc.commit();
    let snapshot = reprise_fixtures::engine().layout(&doc);
    let gap = snapshot
        .marks(0)
        .into_iter()
        .find(|m| m.kind == MarkKind::Gap)
        .unwrap();
    assert!(gap.from.x > gap.to.unwrap().x);
}

#[test]
fn conflicting_and_oversized_domains_keep_complete_fallback_geometry() {
    let (doc, source) = make_doc("source");
    let target = doc
        .append_block(BlockKind::Paragraph, "", "target")
        .unwrap();
    let a = doc
        .pin_line(source, 0, LineEdge::Start, target, 1, AnchorEdge::Position)
        .unwrap();
    let b = doc
        .pin_line(source, 0, LineEdge::End, target, 2, AnchorEdge::Position)
        .unwrap();
    doc.commit();
    let snapshot = reprise_fixtures::engine().layout(&doc);
    for id in [a, b] {
        let relation = snapshot.relation(id).unwrap();
        assert_eq!(relation.status, RelationStatus::Ambiguous);
        assert!(!relation.applied);
        assert!(relation.anchor.as_ref().unwrap().target.is_some());
    }
    assert_eq!(
        snapshot.block(source).unwrap().lines[0].runs[0].x,
        Length::ZERO
    );

    let (doc, target) = make_doc("unconstrained");
    for _ in 0..257 {
        let source = doc.append_block(BlockKind::Paragraph, "", "line").unwrap();
        doc.pin_line(source, 0, LineEdge::Start, target, 1, AnchorEdge::Position)
            .unwrap();
    }
    doc.commit();
    let snapshot = reprise_fixtures::engine().layout(&doc);
    assert!(snapshot.relations.iter().all(|r| !r.applied));
    assert_eq!(
        snapshot
            .diagnostics_with("relation.alignment-limit")
            .count(),
        257
    );
    assert!(
        snapshot
            .blocks
            .iter()
            .flat_map(|b| &b.lines)
            .all(|l| l.runs[0].x == Length::ZERO)
    );
}

#[test]
fn incremental_marks_match_reference_after_property_and_anchor_changes() {
    let (doc, node) = make_doc("left\tright\nsource");
    let engine = reprise_fixtures::engine();
    let mut session = reprise_layout::incremental::LayoutSession::new(&engine);
    session.layout(&doc).unwrap();
    doc.set_tab_stops(node, &tabs(None, Alignment::End))
        .unwrap();
    doc.set_alignment(node, Alignment::Centre).unwrap();
    let pin = doc
        .pin_line(node, 11, LineEdge::Start, node, 4, AnchorEdge::GapEnd)
        .unwrap();
    doc.commit();
    let reference = engine.layout(&doc);
    let incremental = session.layout(&doc).unwrap();
    assert_eq!(incremental, reference);
    assert_eq!(incremental.marks(0), reference.marks(0));
    doc.delete_relation(pin).unwrap();
    doc.block(node).unwrap().text.insert(0, "more ").unwrap();
    doc.commit();
    let reference = engine.layout(&doc);
    let incremental = session.layout(&doc).unwrap();
    assert_eq!(incremental, reference);
    assert_eq!(incremental.marks(0), reference.marks(0));
}
