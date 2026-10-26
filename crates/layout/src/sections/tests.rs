use super::*;
use crate::{
    PageKind,
    incremental::{LayoutSession, Viewport},
};
use reprise_doc::{BlockKind, Dim, FrameTemplate, LengthExpr, Style};
use reprise_font::{Face, FontStore};
use reprise_geom::{Length, Point};

fn setup() -> (Engine, Document) {
    let mut fonts = FontStore::default();
    fonts.add(
        Face::from_bytes(
            include_bytes!("../../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
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
fn props(entries: &[(&str, Value)]) -> SectionProperties {
    entries
        .iter()
        .map(|(k, v)| ((*k).into(), v.clone()))
        .collect()
}
fn first(doc: &Document, entries: &[(&str, Value)]) {
    doc.write_section_properties(SectionId::First, "0", &props(entries), &[])
        .unwrap();
}
fn boundary(doc: &Document, node: NodeId, kind: BreakKind, entries: &[(&str, Value)]) {
    let mut own = props(entries);
    own.insert("break".into(), json!(kind.token()));
    let life = doc.mint_lifetime(node).unwrap();
    doc.activate_start(node, &life, kind, &own).unwrap();
}
fn block(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "s", text).unwrap()
}

#[test]
fn portrait_landscape_portrait_preserves_physical_margins_and_owners() {
    let (engine, doc) = setup();
    doc.set_page_setup_patch(reprise_doc::page_setup::PageSetupPatch {
        width: Some(Length(70000)),
        height: Some(Length(90000)),
        top: Some(Length(3000)),
        right: Some(Length(3000)),
        bottom: Some(Length(3000)),
        left: Some(Length(3000)),
    })
    .unwrap();
    first(
        &doc,
        &[
            ("width", json!(60000)),
            ("height", json!(80000)),
            ("left", json!(4000)),
            ("right", json!(5000)),
            ("top", json!(6000)),
            ("bottom", json!(7000)),
        ],
    );
    let a = block(&doc, "a");
    let b = block(&doc, "b");
    let c = block(&doc, "c");
    boundary(
        &doc,
        b,
        BreakKind::Section,
        &[
            ("orientation", json!("landscape")),
            ("numbering.restart", json!("-4")),
        ],
    );
    boundary(&doc, c, BreakKind::Section, &[]);
    let s = engine.layout(&doc);
    let pages = &s.sections.as_ref().unwrap().pages;
    assert_eq!(pages.len(), 3);
    assert_eq!(
        pages.iter().map(|p| p.section).collect::<Vec<_>>(),
        vec![SectionId::First, SectionId::Block(b), SectionId::Block(c)]
    );
    // Geometry is based on the document default, not the preceding first's own patch.
    assert_eq!(
        (s.pages[0].width, s.pages[0].height),
        (Length(60000), Length(80000))
    );
    assert!(s.pages[1].width > s.pages[1].height);
    assert!(s.pages[2].width < s.pages[2].height);
    assert_eq!(
        pages.iter().map(|p| p.counter).collect::<Vec<_>>(),
        vec![1, -4, -3]
    );
    assert_eq!(
        s.frames[s.block(a).unwrap().lines[0].frame]
            .to_page
            .apply(Point::origin()),
        Point::new(Length(4000), Length(6000))
    );
}

#[test]
fn parity_is_physical_fillers_are_outgoing_and_counted() {
    let (mut engine, doc) = setup();
    first(&doc, &[("numbering.restart", json!("2"))]);
    block(&doc, "a");
    let b = block(&doc, "b");
    boundary(
        &doc,
        b,
        BreakKind::Section,
        &[
            ("start", json!("odd-page")),
            ("numbering.restart", json!("17")),
            ("filler.regions", json!("running")),
        ],
    );
    let s = engine.layout(&doc);
    let p = &s.sections.as_ref().unwrap().pages;
    assert_eq!(
        p.iter().map(|p| (p.kind, p.counter)).collect::<Vec<_>>(),
        vec![
            (PageKind::Content, 2),
            (PageKind::Filler, 3),
            (PageKind::Content, 17)
        ]
    );
    assert_eq!(p[1].section, SectionId::First);
    assert!(p[1].bands.is_empty());
    assert!(!s.frames.iter().any(|f| f.page == 1));
    assert_eq!(s.frames[s.block(b).unwrap().lines[0].frame].page, 2);
    engine.flow.max_pages = 1;
    let capped = engine.layout(&doc);
    assert_eq!(capped.pages.len(), 1);
    assert!(
        capped
            .diagnostics_with("layout.page-limit")
            .next()
            .is_some()
    );
}

#[test]
fn manual_breaks_step_the_column_thread_and_the_current_section() {
    let (engine, doc) = setup();
    first(&doc, &[("columns.count", json!(2))]);
    let a = block(&doc, "a");
    let b = block(&doc, "b");
    let c = block(&doc, "c");
    let d = block(&doc, "d");
    boundary(&doc, b, BreakKind::Column, &[]);
    boundary(&doc, c, BreakKind::Column, &[]);
    boundary(&doc, d, BreakKind::Page, &[]);
    let s = engine.layout(&doc);
    let frames: Vec<_> = [a, b, c, d]
        .iter()
        .map(|n| s.block(*n).unwrap().lines[0].frame)
        .collect();
    assert!(frames[0] < frames[1]);
    assert_eq!(
        frames.iter().map(|i| s.frames[*i].page).collect::<Vec<_>>(),
        vec![0, 0, 1, 2]
    );
    assert!(
        s.sections
            .as_ref()
            .unwrap()
            .pages
            .iter()
            .all(|p| p.section == SectionId::First)
    );
}

#[test]
fn first_body_boundary_ignores_start_and_does_not_make_an_empty_first_page() {
    let (engine, doc) = setup();
    let a = block(&doc, "a");
    first(&doc, &[("numbering.format", json!("upper-roman"))]);
    boundary(
        &doc,
        a,
        BreakKind::Section,
        &[
            ("start", json!("even-page")),
            ("numbering.restart", json!("0")),
        ],
    );
    let s = engine.layout(&doc);
    let p = &s.sections.as_ref().unwrap().pages;
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].section, SectionId::Block(a));
    assert_eq!(p[0].counter, 0);
    assert_eq!(
        engine.section_state(&doc)[&SectionId::Block(a)]["numbering"]["format"],
        "upper-roman"
    );
}

#[test]
fn exact_unequal_rtl_columns_have_rules_but_no_rule_frames() {
    let (engine, doc) = setup();
    first(
        &doc,
        &[
            ("width", json!(10003)),
            ("height", json!(50000)),
            ("left", json!(0)),
            ("right", json!(0)),
            ("top", json!(0)),
            ("bottom", json!(0)),
            ("columns.widths", json!("1,2,3")),
            ("columns.gap", json!(10)),
            ("columns.rule", json!(7)),
            ("direction", json!("rtl")),
        ],
    );
    block(&doc, "a");
    let s = engine.layout(&doc);
    let m = s.sections.as_ref().unwrap();
    let band = &m.pages[0].bands[0];
    assert_eq!(
        band.frames
            .iter()
            .map(|i| s.frames[*i].rect.width.0)
            .collect::<Vec<_>>(),
        vec![1664, 3328, 4991]
    );
    assert_eq!(
        band.frames
            .iter()
            .map(|i| s.frames[*i].to_page.apply(Point::origin()).x.0)
            .collect::<Vec<_>>(),
        vec![8339, 5001, 0]
    );
    assert_eq!(m.rules.len(), 2);
    assert_eq!(m.rules[0].rect.origin.x, Length(8330));
    assert_eq!(m.rules[0].rect.width, Length(7));
    assert_eq!(
        s.to_display_list(0, crate::DisplayOptions::default())
            .items
            .len(),
        s.frames.len() + 2
    );
}

#[test]
fn generated_columns_replace_transform_but_preserve_non_main_frames() {
    let (engine, doc) = setup();
    let mut main = FrameTemplate::new(
        "body",
        FrameRole::Flow("main".into()),
        (Dim::pt(0), Dim::pt(0)),
        (Dim::pt(100), Dim::pt(100)),
    );
    main.transform.mirror_x = true;
    let template = PageTemplate::new("spatial", Dim::pt(100), Dim::pt(100))
        .with_frame(main)
        .with_frame(FrameTemplate::new(
            "margin",
            FrameRole::Margin,
            (Dim::pt(80), Dim::pt(0)),
            (Dim::pt(20), Dim::pt(100)),
        ));
    doc.set_page_template(&template).unwrap();
    first(&doc, &[("columns.gap", json!(1))]);
    block(&doc, "a");
    let s = engine.layout(&doc);
    assert_eq!(s.diagnostics_with("section.template").count(), 1);
    assert_eq!(
        s.frames
            .iter()
            .filter(|f| f.role == FrameRole::Margin)
            .count(),
        1
    );
    assert_eq!(
        s.frames
            .iter()
            .find(|f| f.role == FrameRole::Margin)
            .unwrap()
            .to_page
            .apply(Point::origin())
            .x,
        Length::from_pt(80)
    );
    assert_eq!(
        s.sections.as_ref().unwrap().pages[0].bands[0].frames.len(),
        1
    );
}

#[test]
fn counter_saturates_and_serializes_as_exact_signed_decimal() {
    let (engine, doc) = setup();
    first(&doc, &[("numbering.restart", json!(i64::MAX.to_string()))]);
    block(&doc, "a");
    let b = block(&doc, "b");
    boundary(&doc, b, BreakKind::Page, &[]);
    let s = engine.layout(&doc);
    assert_eq!(s.diagnostics_with("section.page-counter").count(), 1);
    let json: Value = serde_json::from_str(&s.to_json()).unwrap();
    assert_eq!(
        json["sections"]["pages"][0]["counter"],
        i64::MAX.to_string()
    );
    assert_eq!(
        json["sections"]["pages"][1]["counter"],
        i64::MAX.to_string()
    );
    assert_eq!(json["sections"]["pages"][0]["section"], "first");
}

#[test]
fn section_geometry_matches_incremental_jobs_with_unit_budgets() {
    let (engine, doc) = setup();
    first(&doc, &[("columns.count", json!(2))]);
    let a = block(&doc, "prefix");
    let b = block(&doc, "landscape");
    let c = block(&doc, "suffix");
    boundary(
        &doc,
        b,
        BreakKind::Section,
        &[("orientation", json!("landscape"))],
    );
    boundary(&doc, c, BreakKind::Section, &[]);
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..100));
    assert_eq!(job.step(0).unwrap().used, 0);
    for _ in 0..10000 {
        if job.step(1).unwrap().complete {
            break;
        }
    }
    assert_eq!(job.complete().unwrap().unwrap(), engine.layout(&doc));
    drop(job);
    doc.block(b).unwrap().text.insert(0, "edited ").unwrap();
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
    assert!(session.counters().reused_compositions > 0);
    assert_eq!(engine.layout(&doc).block(a).unwrap().lines[0].frame, 0);
}

#[test]
fn vertical_layout_uses_physical_asymmetric_margins_and_logical_columns() {
    for (mode, x) in [("vertical-rl", 55000), ("vertical-lr", 4000)] {
        let (engine, doc) = setup();
        first(
            &doc,
            &[
                ("width", json!(60000)),
                ("height", json!(80000)),
                ("left", json!(4000)),
                ("right", json!(5000)),
                ("top", json!(6000)),
                ("bottom", json!(7000)),
                ("columns.count", json!(2)),
                ("columns.rule", json!(4096)),
                ("writing-mode", json!(mode)),
            ],
        );
        block(&doc, "vertical");
        let s = engine.layout(&doc);
        let m = s.sections.as_ref().unwrap();
        let indices = &m.pages[0].bands[0].frames;
        assert_eq!(s.frames[indices[0]].rect.width, Length(24284));
        assert_eq!(s.frames[indices[0]].rect.height, Length(51000));
        assert_eq!(
            s.frames[indices[0]].to_page.apply(Point::origin()),
            Point::new(Length(x), Length(6000))
        );
        assert_eq!(
            s.frames[indices[1]].to_page.apply(Point::origin()).y,
            Length(48716)
        );
        assert_eq!(m.rules[0].rect.origin.y, Length(37452));
        assert_eq!(m.rules[0].rect.width, Length(51000));
    }
}

#[test]
fn unreadable_properties_and_unknown_breaks_are_retained_and_diagnostics_sort_by_body_order() {
    let (engine, doc) = setup();
    doc.set_geometry_mode("pageless").unwrap();
    first(
        &doc,
        &[
            ("mode", json!("paged")),
            ("width", json!(-1)),
            ("columns.widths", json!(" ".repeat(1025))),
            ("direction", json!(false)),
            ("template", json!("missing")),
            ("numbering.format", json!("bullet")),
            ("region.future.default", json!("")),
        ],
    );
    block(&doc, "a");
    let b = block(&doc, "b");
    boundary(&doc, b, BreakKind::Page, &[]);
    let life = doc
        .start_records()
        .into_iter()
        .find(|r| r.id == SectionId::Block(b))
        .unwrap()
        .life
        .unwrap();
    doc.write_section_properties(
        SectionId::Block(b),
        &life,
        &props(&[("break", json!("future"))]),
        &[],
    )
    .unwrap();
    let revision = doc.revision();
    let s = engine.layout(&doc);
    assert_eq!(revision, doc.revision());
    assert_eq!(s.pages.len(), 1);
    assert_eq!(s.diagnostics_with("section.mode-unsupported").count(), 0);
    assert_eq!(s.diagnostics_with("section.region-slot").count(), 1);
    assert!(
        s.diagnostics
            .iter()
            .any(|d| d.subject == Subject::Node(b) && d.code.as_str() == "section.property")
    );
    let keys: Vec<_> = s
        .diagnostics
        .iter()
        .filter(|d| d.subject == Subject::Document && d.code.as_str().starts_with("section."))
        .map(|d| d.message.split(':').next().unwrap().to_owned())
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    assert_eq!(
        engine.section_state(&doc)[&SectionId::First]["numbering"]["format"],
        "decimal"
    );
}

#[test]
fn endnote_continuations_use_the_last_pages_owner_and_responsive_geometry() {
    use reprise_doc::relation::builtin;
    use reprise_doc::text::RangePolicy;
    use reprise_doc::{Basis, Param, Relation, Target};
    let (engine, doc) = setup();
    let page_width = Dim::fraction(Basis::PageWidth, 1000);
    let page_height = Dim::fraction(Basis::PageHeight, 1000);
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(100), Dim::pt(60))
            .with_frame(FrameTemplate::new(
                "main",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (page_width, page_height),
            ))
            .with_frame(FrameTemplate::new(
                "notes",
                FrameRole::Notes,
                (Dim::pt(0), Dim::pt(0)),
                (page_width, page_height),
            )),
    )
    .unwrap();
    first(&doc, &[("columns.count", json!(1))]);
    let a = block(&doc, "anchor");
    let b = block(&doc, "body end");
    boundary(
        &doc,
        b,
        BreakKind::Section,
        &[
            ("width", json!(204800)),
            ("height", json!(102400)),
            ("numbering.restart", json!("7")),
        ],
    );
    let text = (0..20).map(|_| "note").collect::<Vec<_>>().join("\n");
    let note = doc.append_block(BlockKind::Annotation, "s", &text).unwrap();
    let range = doc.add_range(a, 0..1, RangePolicy::FIXED).unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::NOTE)
            .owned_by(note)
            .target("anchor", Target::Range(range))
            .param("placement", Param::Text("end".into())),
    )
    .unwrap();
    let s = engine.layout(&doc);
    let m = s.sections.as_ref().unwrap();
    assert!(m.pages.len() > 2);
    for (index, p) in m.pages.iter().enumerate().skip(2) {
        assert_eq!(p.section, SectionId::Block(b));
        assert_eq!(p.kind, PageKind::NoteContinuation);
        assert_eq!(p.counter, 7 + (index as i64) - 1);
        assert_eq!(
            (s.pages[index].width, s.pages[index].height),
            (Length::from_pt(200), Length::from_pt(100))
        );
    }
    let placed = s.block(note).unwrap();
    assert_eq!(placed.lines.last().unwrap().text.end, text.len());
    assert!(
        placed
            .lines
            .iter()
            .all(|l| s.frames[l.frame].rect.width == Length::from_pt(200))
    );
    let mut session = LayoutSession::new(&engine);
    assert_eq!(session.layout(&doc).unwrap(), s);
}
mod bands;
