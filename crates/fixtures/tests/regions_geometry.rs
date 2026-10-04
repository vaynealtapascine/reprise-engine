//! Orchestrator review: workstreams 3b (floats and notes) and 5 (frame
//! transforms and writing modes) were built in parallel and never saw each
//! other. Here a float and notes nested three deep live in a rotated, mirrored
//! body frame and a vertical-rl notes frame. Layout must stay deterministic,
//! keep every line in an existing frame of the right role with an invertible
//! transform, render in every backend, and give a reading order that visits
//! every placed line exactly once.

use std::collections::BTreeSet;

use reprise_doc::relation::builtin::{FLOAT, NOTE};
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, Dim, Document, FrameRole, FrameTemplate, LengthExpr, MAIN_FLOW, NodeId,
    PageTemplate, Param, Relation, Rotation, SchemaRegistry, Style, Target, WritingMode,
};
use reprise_fixtures::spike::define_styles;
use reprise_fixtures::{PEER, engine};
use reprise_geom::Length;
use reprise_layout::DisplayOptions;

fn attach(doc: &Document, schema: reprise_doc::SchemaId, owner: NodeId, anchor: NodeId) {
    let range = doc.add_range(anchor, 1..2, RangePolicy::FIXED).unwrap();
    let mut relation = Relation::new(schema.clone())
        .owned_by(owner)
        .target("anchor", Target::Range(range));
    if schema == FLOAT {
        relation = relation.param("width", Param::Length(LengthExpr::Pt(Length::from_pt(30))));
    }
    doc.add_relation(&SchemaRegistry::builtin(), &relation)
        .unwrap();
}

#[test]
fn floats_and_nested_notes_in_transformed_frames() {
    let doc = Document::new(PEER).unwrap();
    define_styles(&doc).unwrap();
    doc.define_style(
        "note",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(8))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(10))),
            ..Default::default()
        },
    )
    .unwrap();
    let mut body = FrameTemplate::new(
        "body",
        FrameRole::Flow(MAIN_FLOW.into()),
        (Dim::pt(20), Dim::pt(20)),
        (Dim::pt(180), Dim::pt(160)),
    );
    body.transform.rotation = Rotation::Quarter(1);
    body.transform.mirror_x = true;
    let mut notes = FrameTemplate::new(
        "notes",
        FrameRole::Notes,
        (Dim::pt(220), Dim::pt(20)),
        (Dim::pt(80), Dim::pt(260)),
    );
    notes.writing_mode = WritingMode::VerticalRl;
    doc.set_page_template(
        &PageTemplate::new("strange", Dim::pt(320), Dim::pt(300))
            .with_frame(body)
            .with_frame(notes),
    )
    .unwrap();
    let text = "The house is larger on the inside than the outside, and the notes \
                about it keep growing. "
        .repeat(4);
    let p = doc
        .append_block(BlockKind::Paragraph, "body", &text)
        .unwrap();
    let float = doc
        .append_block(BlockKind::Annotation, "note", "A float beside it.")
        .unwrap();
    attach(&doc, FLOAT, float, p);
    let mut anchor = p;
    for depth in 1..=3 {
        let note = doc
            .append_block(
                BlockKind::Annotation,
                "note",
                &format!("Note at depth {depth}, which itself has a note."),
            )
            .unwrap();
        attach(&doc, NOTE, note, anchor);
        anchor = note;
    }
    doc.commit();

    let engine = engine();
    let snapshot = engine.layout(&doc);
    assert_eq!(snapshot, engine.layout(&doc), "deterministic");
    for frame in &snapshot.frames {
        assert!(frame.page < snapshot.pages.len(), "frame on a page");
        assert!(frame.to_page.inverse().is_some(), "invertible frame");
    }
    for block in &snapshot.blocks {
        for line in &block.lines {
            let frame = snapshot.frame(line.frame).expect("line in a frame");
            if block.kind == BlockKind::Paragraph {
                assert_eq!(frame.role, FrameRole::Flow(MAIN_FLOW.into()));
            }
        }
    }
    let lists = snapshot.to_display_lists(DisplayOptions {
        debug: true,
        ..Default::default()
    });
    for list in &lists {
        reprise_display::svg::render(list, &engine.fonts).unwrap();
        reprise_display::png::render(list, &engine.fonts, 1.0).unwrap();
    }
    reprise_display::pdf::render(&lists, &engine.fonts).unwrap();

    let placed: BTreeSet<reprise_layout::LineRef> = snapshot
        .blocks
        .iter()
        .flat_map(|b| {
            (0..b.lines.len()).map(move |line| reprise_layout::LineRef { node: b.node, line })
        })
        .collect();
    let order = snapshot.reading_order(&doc);
    let visited: Vec<_> = order.iter().map(|step| step.line).collect();
    let unique: BTreeSet<_> = visited.iter().copied().collect();
    assert_eq!(unique.len(), visited.len(), "no line is read twice");
    assert_eq!(unique, placed, "reading order visits every placed line");
}
