use super::*;
use reprise_doc::{BlockKind, Document, Domain, NewBlock};
use reprise_font::{Face, FontStore};
use reprise_geom::{Length, Matrix};

fn setup() -> (Engine, Document) {
    let mut fonts = FontStore::default();
    fonts.add(
        Face::from_bytes(
            include_bytes!("../../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
        )
        .unwrap(),
    );
    (Engine::new(fonts), Document::new(1).unwrap())
}

#[test]
fn allocation_and_replay_share_one_atomic_page_limit() {
    let (mut engine, doc) = setup();
    engine.flow.max_pages = 1;
    let mut snapshot = engine.layout(&doc);
    let template = crate::template::resolve(&engine, &doc, &mut Vec::new());
    let before = snapshot.clone();
    assert!(!PageFactory::push(&engine, &template, &mut snapshot));
    assert!(!PageFactory::ensure(&engine, &template, &mut snapshot, 1));
    assert!(!PageFactory::replay(
        &engine,
        &mut snapshot,
        before.pages.clone(),
        before.frames.clone()
    ));
    assert_eq!(
        snapshot, before,
        "refused allocation never appends a partial page or frames"
    );
    assert!(PageFactory::ensure(&engine, &template, &mut snapshot, 0));
}

#[test]
fn logical_address_survives_an_unrelated_frame_index_shift() {
    let (engine, doc) = setup();
    let snapshot = engine.layout(&doc);
    let old = Catalog::from_snapshot(&snapshot);
    let address = FrameAddr::Main {
        page: 0,
        band: 0,
        column: 0,
    };
    let old_index = old.index(&address).unwrap();
    let entry = Entry::new(old.reference(old_index).unwrap(), Length(17));
    let mut shifted = snapshot.clone();
    let mut margin = shifted.frames[old_index].clone();
    margin.role = FrameRole::Margin;
    shifted.frames.insert(0, margin);
    let current = Catalog::from_snapshot(&shifted);
    let index = current.index(&address).unwrap();
    assert_eq!(index, old_index + 1);
    assert_eq!(
        entry.for_frame(current.reference(index).unwrap()),
        Some(&Length(17))
    );
}

#[test]
fn an_existing_address_requires_every_part_of_its_provenance() {
    let (engine, doc) = setup();
    let node = doc
        .append_block(BlockKind::Paragraph, "default", "owner")
        .unwrap();
    let snapshot = engine.layout(&doc);
    let catalog = Catalog::from_snapshot(&snapshot);
    let frame = catalog.reference(0).unwrap().clone();
    let entry = Entry::new(&frame, Length(17));
    let mut variants = vec![frame.clone(); 6];
    variants[0].provenance.section = SectionId::Block(node);
    variants[1].provenance.life = "99@1".into();
    variants[2].provenance.rect.width += Length(1);
    variants[3].provenance.to_page = Transform::new(Matrix::translate(Length(1), Length::ZERO));
    variants[4].provenance.writing_mode = WritingMode::VerticalRl;
    variants[5].provenance.running = Some(("header".into(), node));
    for changed in variants {
        assert_eq!(changed.address, frame.address);
        assert!(entry.for_frame(&changed).is_none());
        let mut plan = crate::regions::Plan::default();
        plan.reserve(&frame, Length(17));
        plan.exclude(&frame, frame.provenance.rect);
        assert!(plan.reservation_for(&changed).is_none());
        assert!(plan.exclusions_for(&changed).is_none());
        let mut next = plan.clone();
        next.reserve(&changed, Length(17));
        assert!(
            !plan.equivalent(&next),
            "changed provenance participates in convergence"
        );
    }
}

#[test]
fn a_story_on_one_frame_exhausts_without_creating_pages() {
    let (engine, doc) = setup();
    let body = doc
        .append_block(BlockKind::Paragraph, "default", "separate body")
        .unwrap();
    let story = doc
        .create_story(&[
            NewBlock::new(BlockKind::Paragraph, "default", &"story text ".repeat(4000)),
            NewBlock::new(BlockKind::Paragraph, "default", "omitted tail"),
        ])
        .unwrap();
    let nodes = doc.top(Domain::Story(story));
    let mut snapshot = engine.layout(&doc);
    snapshot.blocks.clear();
    snapshot.diagnostics.clear();
    let pages = snapshot.pages.clone();
    let frames = snapshot.frames.clone();
    let mut template = crate::template::resolve(&engine, &doc, &mut Vec::new());
    template.frames.truncate(1);
    let plan = crate::regions::Plan::default();
    let mut cursor = crate::flow::Cursor::new_in(
        &engine,
        &doc,
        &template,
        &mut snapshot,
        &plan,
        Domain::Story(story),
        FrameSource::OneFrame {
            index: 0,
            exhausted: false,
        },
    );
    for node in &nodes {
        cursor = cursor.step(&engine, &doc, &template, &mut snapshot, &plan, *node, None);
    }
    assert!(
        cursor
            .finish(&engine, &template, &mut snapshot, &plan)
            .is_empty()
    );
    assert_eq!(snapshot.pages, pages);
    assert_eq!(snapshot.frames, frames);
    assert!(snapshot.block(body).is_none());
    let first = snapshot.block(nodes[0]).unwrap();
    assert!(!first.lines.is_empty());
    assert!(first.lines.iter().all(|l| l.frame == 0));
    assert!(first.lines.last().unwrap().text.end < first.text.len());
    assert!(snapshot.block(nodes[1]).is_none());
    assert_eq!(snapshot.diagnostics_with("layout.page-limit").count(), 0);

    snapshot.blocks.clear();
    let cursor = crate::flow::Cursor::new_in(
        &engine,
        &doc,
        &template,
        &mut snapshot,
        &plan,
        Domain::Story(story),
        FrameSource::OneFrame {
            index: 0,
            exhausted: true,
        },
    );
    let cursor = cursor.step(
        &engine,
        &doc,
        &template,
        &mut snapshot,
        &plan,
        nodes[0],
        None,
    );
    cursor.finish(&engine, &template, &mut snapshot, &plan);
    assert!(
        snapshot.blocks.is_empty(),
        "an exhausted source cannot start again"
    );
    assert_eq!(snapshot.pages, pages);
}
