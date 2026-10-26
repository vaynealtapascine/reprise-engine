use reprise_doc::relation::builtin;
use reprise_doc::sections::{BreakKind, SectionId};
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, Dim, Document, Domain, FrameRole, FrameTemplate, LengthExpr, NewBlock, NodeId,
    PageTemplate, Param, Relation, Style, Target,
};
use reprise_font::{Face, FontStore};
use reprise_geom::Length;
use reprise_layout::Engine;
use serde_json::json;
fn setup() -> (Engine, Document) {
    let mut fonts = FontStore::default();
    fonts.add(
        Face::from_bytes(
            include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
        )
        .unwrap(),
    );
    let engine = Engine::new(fonts);
    let doc = Document::new(1).unwrap();
    doc.define_style(
        "s",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(10))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(12))),
            ..Default::default()
        },
    )
    .unwrap();
    (engine, doc)
}
fn frame(name: &str, x: i32, y: i32, w: i32, h: i32) -> FrameTemplate {
    FrameTemplate::new(
        name,
        FrameRole::Flow("main".into()),
        (Dim::pt(x), Dim::pt(y)),
        (Dim::pt(w), Dim::pt(h)),
    )
}
fn block(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "s", text).unwrap()
}
#[test]
fn budget_one_does_not_compose_many_running_instances_inline() {
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(120))
            .with_frame(frame("main", 0, 0, 200, 120))
            .with_frame(FrameTemplate::new(
                "notes",
                FrameRole::Notes,
                (Dim::pt(0), Dim::pt(30)),
                (Dim::pt(200), Dim::pt(90)),
            )),
    )
    .unwrap();
    let head = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "s", "head ")])
        .unwrap();
    let hb = doc.top(Domain::Story(head))[0];
    doc.insert_inline(hb, 5, &reprise_doc::InlineObject::field("page-number"))
        .unwrap();
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[
            ("header.distance".into(), json!(0)),
            ("region.header.default".into(), json!(head.to_string())),
        ]
        .into(),
        &[],
    )
    .unwrap();
    let body = block(&doc, "a");
    let note = doc
        .append_block(BlockKind::Annotation, "s", &vec!["note"; 700].join("\n"))
        .unwrap();
    let range = doc.add_range(body, 0..1, RangePolicy::FIXED).unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::NOTE)
            .owned_by(note)
            .target("anchor", Target::Range(range))
            .param("placement", Param::Text("page".into())),
    )
    .unwrap();
    doc.commit();
    let expected = engine.layout(&doc);
    let mut session = reprise_layout::incremental::LayoutSession::new(&engine);
    let mut job = session.start(
        &doc,
        reprise_layout::incremental::Viewport::Pages(0..usize::MAX),
    );
    assert_eq!(job.counters().running_compositions, 0);
    assert_eq!(job.step(0).unwrap().used, 0);
    let mut prior = 0;
    let mut max_inline = 0;
    for step_no in 0..100000 {
        let step = job.step(1).unwrap();
        let current = job.counters().running_inline;
        let delta = current - prior;
        prior = current;
        if delta > max_inline {
            max_inline = delta;
            eprintln!(
                "step={step_no}, used={}, inline_regions={delta}, work={:?}",
                step.used,
                job.step_work()
            );
        }
        if step.complete {
            break;
        }
    }
    assert_eq!(job.complete().unwrap().unwrap(), expected);
    eprintln!(
        "pages={}, maximum inline compositions in budget-one step={max_inline}",
        expected.pages.len()
    );
    assert_eq!(max_inline, 0, "whole running stories bypassed stepped work");
}
#[test]
fn running_field_kinds_revalidate_in_one_persistent_session() {
    let (engine, doc) = setup();
    let p = block(&doc, "one");
    let q = block(&doc, "two");
    let life = doc.mint_lifetime(q).unwrap();
    doc.activate_start(q, &life, BreakKind::Page, &Default::default())
        .unwrap();
    let head = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "s", "")])
        .unwrap();
    let hb = doc.top(Domain::Story(head))[0];
    let fields = [
        reprise_doc::InlineObject::field("text").with("text", "original"),
        reprise_doc::InlineObject::field("page-number"),
        reprise_doc::InlineObject::field("page-count"),
        reprise_doc::InlineObject::field("page-of")
            .with("target", doc.field_target(q, 0).unwrap().as_str()),
    ];
    for f in fields {
        let end = doc.block(hb).unwrap().text.len();
        doc.insert_inline(hb, end, &f).unwrap();
    }
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[
            ("region.header.default".into(), json!(head.to_string())),
            ("region.footer.default".into(), json!(head.to_string())),
        ]
        .into(),
        &[],
    )
    .unwrap();
    doc.commit();
    let mut session = reprise_layout::incremental::LayoutSession::new(&engine);
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
    let id = doc.inline_objects(hb).unwrap()[0].id;
    doc.update_inline(
        id,
        &reprise_doc::InlinePatch {
            align: None,
            params: [("text".into(), Some("changed text field".into()))].into(),
        },
    )
    .unwrap();
    doc.commit();
    assert_eq!(
        session.layout(&doc).unwrap(),
        engine.layout(&doc),
        "text params"
    );
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[
            ("numbering.restart".into(), json!(12)),
            ("numbering.format".into(), json!("upper-roman")),
        ]
        .into(),
        &[],
    )
    .unwrap();
    doc.commit();
    assert_eq!(
        session.layout(&doc).unwrap(),
        engine.layout(&doc),
        "page-number labels"
    );
    doc.deactivate_start(q).unwrap();
    doc.commit();
    assert_eq!(
        session.layout(&doc).unwrap(),
        engine.layout(&doc),
        "page-count and page-of shrink"
    );
    doc.delete_block(p).unwrap();
    doc.commit();
    assert_eq!(
        session.layout(&doc).unwrap(),
        engine.layout(&doc),
        "deleted prefix"
    );
}
#[test]
fn one_physical_flow_host_cannot_bypass_digest_step_budget() {
    let (engine, doc) = setup();
    let story = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "s", "x")])
        .unwrap();
    let mut tail = doc.top(Domain::Story(story))[0];
    for _ in 0..130 {
        tail = doc.split_block(tail, 0).unwrap();
    }
    block(&doc, "body");
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[("region.header.default".into(), json!(story.to_string()))].into(),
        &[],
    )
    .unwrap();
    doc.commit();
    assert_eq!(doc.top(Domain::Story(story)).len(), 131);
    let mut session = reprise_layout::incremental::LayoutSession::new(&engine);
    let mut job = session.start(&doc, reprise_layout::incremental::Viewport::Pages(0..10));
    let step = job.step(1).unwrap();
    let work = job.step_work();
    eprintln!("split flow host: charged={}, work={work:?}", step.used);
    assert!(
        work.digest_blocks <= 64,
        "one physical host expanded into all its paragraphs in one digest step"
    );
}
