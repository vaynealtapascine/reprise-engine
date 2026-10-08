use reprise_doc::{BlockKind, Document, LengthExpr, Style};
use reprise_fixtures::{hostile, spike};
use reprise_geom::Length;
use reprise_layout::{Engine, incremental::LayoutSession};

fn check(session: &mut LayoutSession<'_>, engine: &Engine, doc: &Document, label: &str) {
    doc.commit();
    assert_eq!(session.layout(doc).unwrap(), engine.layout(doc), "{label}");
}

#[test]
fn shared_flow_typing_only_reshapes_the_changed_paragraph() {
    let engine = reprise_fixtures::engine();
    let doc = Document::new(1).unwrap();
    let head = doc
        .append_block(BlockKind::Paragraph, "", &"A short paragraph. ".repeat(24))
        .unwrap();
    let mut paragraphs = vec![head];
    for _ in 1..24 {
        let next = doc
            .split_block(*paragraphs.last().unwrap(), "A short paragraph. ".len())
            .unwrap();
        paragraphs.push(next);
    }
    let mut session = LayoutSession::new(&engine);
    check(&mut session, &engine, &doc, "shared flow initial");
    doc.block(paragraphs[12])
        .unwrap()
        .text
        .insert(2, "changed ")
        .unwrap();
    check(&mut session, &engine, &doc, "shared flow typing");
    let work = session.counters();
    assert_eq!(work.shapes, 1, "{work:?}");
    assert_eq!(work.compositions, 1, "{work:?}");
    let tail = doc.split_block(paragraphs[12], 10).unwrap();
    check(&mut session, &engine, &doc, "shared flow split");
    doc.join_blocks(paragraphs[12], tail).unwrap();
    check(&mut session, &engine, &doc, "shared flow join");
}
fn edits(engine: &Engine, doc: &Document, label: &str) {
    let mut session = LayoutSession::new(engine);
    check(&mut session, engine, doc, label);
    let mut seed = 0x52657072697365_u64;
    for turn in 0..32 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let nodes: Vec<_> = doc
            .document_order()
            .into_iter()
            .filter(|&n| doc.block(n).is_ok())
            .collect();
        let Some(&node) = nodes.get((seed as usize) % nodes.len().max(1)) else {
            continue;
        };
        let block = doc.block(node).unwrap();
        match turn % 16 {
            0 => {
                block
                    .text
                    .insert(0, "edited \u{e9} \u{5d0}\u{5d1} ")
                    .unwrap();
            }
            1 => {
                let text = block.text.to_string();
                if let Some(c) = text.chars().next() {
                    block.text.delete(0..c.len_utf8()).unwrap();
                }
            }
            2 => {
                doc.set_overrides(
                    node,
                    &Style {
                        size: Some(LengthExpr::Pt(Length::from_pt(12))),
                        ..Style::default()
                    },
                )
                .unwrap();
            }
            3 => {
                doc.define_style(
                    "body",
                    &Style {
                        size: Some(LengthExpr::Pt(Length::from_pt(10 + (seed % 5) as i32))),
                        ..Style::default()
                    },
                )
                .unwrap();
            }
            4 => {
                let peer = doc.fork(2).unwrap();
                peer.block(node).unwrap().text.insert(0, "peer ").unwrap();
                block.text.insert(0, "local ").unwrap();
                doc.merge(&peer).unwrap();
            }
            5 => {
                doc.append_block(
                    BlockKind::Paragraph,
                    "body",
                    "new paragraph crosses pages ".repeat(10).as_str(),
                )
                .unwrap();
            }
            6 => {
                doc.set_overrides(node, &Style::default()).unwrap();
            }
            7 => {
                let n = doc
                    .append_block(BlockKind::Paragraph, "body", "temporary")
                    .unwrap();
                doc.delete_block(n).unwrap();
            }
            8 => {
                doc.add_relation(
                    &engine.schemas,
                    &reprise_doc::Relation::new(reprise_doc::relation::builtin::REFERENCE)
                        .owned_by(node)
                        .target("to", reprise_doc::Target::Node(node)),
                )
                .unwrap();
            }
            9 => {
                if let Some((id, _)) = doc.relations().last() {
                    doc.delete_relation(*id).unwrap();
                }
            }
            10 => {
                doc.set_page_template(&reprise_fixtures::templates::two_columns())
                    .unwrap();
            }
            11 => {
                doc.store_raw_page_template("broken", "{unbalanced")
                    .unwrap();
                doc.use_page_template("broken").unwrap();
            }
            12 => {
                doc.set_page_template(&reprise_fixtures::templates::responsive_columns())
                    .unwrap();
                let text = block.text.to_string();
                let at = text
                    .char_indices()
                    .map(|(at, _)| at)
                    .nth(text.chars().count() / 2)
                    .unwrap_or(text.len());
                let suffix = text.get(at..).unwrap();
                block.text.delete(at..text.len()).unwrap();
                let n = doc
                    .append_block(BlockKind::Paragraph, "body", suffix)
                    .unwrap();
                doc.supersede(node, n).unwrap();
            }
            13 => {
                let paragraphs: Vec<_> = doc
                    .blocks()
                    .into_iter()
                    .filter(|&n| {
                        matches!(doc.table_role(n), Ok(None))
                            && doc.kind_of(n) == Some(BlockKind::Paragraph)
                    })
                    .collect();
                if let [.., first, second] = paragraphs.as_slice() {
                    let a = doc.block(*first).unwrap();
                    let b = doc.block(*second).unwrap();
                    a.text.insert(a.text.len(), &b.text.to_string()).unwrap();
                    doc.supersede(*second, *first).unwrap();
                    doc.delete_block(*second).unwrap();
                }
            }
            14 => {
                let n = doc
                    .append_block(BlockKind::Paragraph, "body", "reading cycle endpoint")
                    .unwrap();
                doc.add_relation(&engine.schemas, &reprise_doc::reading::before(node, n))
                    .unwrap();
                doc.add_relation(&engine.schemas, &reprise_doc::reading::before(n, node))
                    .unwrap();
            }
            _ => {
                for (id, relation) in doc.relations() {
                    if relation.is_ok_and(|r| r.schema == reprise_doc::reading::READING_ORDER) {
                        doc.delete_relation(id).unwrap();
                    }
                }
                let peer = doc.fork(2).unwrap();
                peer.define_style(
                    "body",
                    &Style {
                        size: Some(LengthExpr::Pt(Length::from_pt(9))),
                        ..Style::default()
                    },
                )
                .unwrap();
                peer.set_page_template(&reprise_fixtures::templates::two_columns())
                    .unwrap();
                doc.merge(&peer).unwrap();
            }
        }
        check(&mut session, engine, doc, &format!("{label}, edit {turn}"));
    }
}
#[test]
fn every_hostile_fixture_and_spike_stays_equivalent_after_edits() {
    for fixture in hostile::all().unwrap() {
        edits(&fixture.engine, &fixture.doc, fixture.name);
    }
    let spike = spike::document().unwrap();
    edits(&reprise_fixtures::engine(), &spike.doc, "spike");
}

use reprise_doc::{Relation, Target};
use reprise_layout::incremental::{Computation, Dependency, JobError, Viewport};

fn paragraphs(count: usize) -> Document {
    let doc = Document::new(1).unwrap();
    spike::define_styles(&doc).unwrap();
    doc.set_page_template(&reprise_fixtures::templates::two_columns())
        .unwrap();
    for _ in 0..count {
        doc.append_block(BlockKind::Paragraph, "body", "a tiny paragraph")
            .unwrap();
    }
    doc.commit();
    doc
}

#[test]
fn one_edit_in_a_thousand_paragraphs_only_shapes_and_composes_that_paragraph() {
    let engine = reprise_fixtures::engine();
    let doc = paragraphs(1000);
    let nodes = doc.blocks();
    let mut session = LayoutSession::new(&engine);
    check(&mut session, &engine, &doc, "initial thousand");
    let node = nodes[500];
    doc.block(node).unwrap().text.insert(0, " ").unwrap();
    check(&mut session, &engine, &doc, "single edit");
    let counters = session.counters();
    assert_eq!(counters.shapes, 1, "{counters:?}");
    assert_eq!(counters.style_resolutions, 1, "{counters:?}");
    assert_eq!(counters.compositions, 1, "{counters:?}");
    assert_eq!(counters.reflowed_pages.len(), 1, "{counters:?}");
    assert_eq!(counters.reused_compositions, 999, "{counters:?}");
    assert!(counters.composer_calls <= 2, "{counters:?}");
    let graph = session.graph();
    assert!(
        graph
            .why_recomputed(&Computation::Compose(node))
            .contains(&Dependency::Text(node))
    );
    assert!(
        graph
            .why_recomputed(&Computation::Compose(nodes[499]))
            .is_empty()
    );
    assert!(
        graph
            .dependencies(&Computation::Line(reprise_layout::LineRef {
                node,
                line: 0
            }))
            .contains(&Dependency::Style("body".into()))
    );
    assert!(
        graph
            .dependents(&Dependency::Text(node))
            .contains(&Computation::Compose(node))
    );
    assert!(
        graph
            .dependents(&Dependency::Text(node))
            .iter()
            .any(|u| matches!(u, Computation::Page(_)))
    );
    doc.define_style(
        "unused",
        &Style {
            size: Some(LengthExpr::Pt(Length::MAX)),
            ..Style::default()
        },
    )
    .unwrap();
    check(&mut session, &engine, &doc, "unrelated style");
    let counters = session.counters();
    assert_eq!(counters.shapes, 0, "{counters:?}");
    assert_eq!(counters.composer_calls, 0, "{counters:?}");
    assert_eq!(counters.relation_passes, 0, "{counters:?}");
    assert_eq!(counters.reading_order_passes, 0, "{counters:?}");
    assert!(
        session
            .graph()
            .dependents(&Dependency::Style("unused".into()))
            .is_empty()
    );
}

#[test]
fn relation_edits_leave_body_composition_cached() {
    let fixture = spike::document().unwrap();
    let engine = reprise_fixtures::engine();
    let mut session = LayoutSession::new(&engine);
    check(&mut session, &engine, &fixture.doc, "spike initial");
    let id = fixture
        .doc
        .add_relation(
            &engine.schemas,
            &Relation::new(reprise_doc::relation::builtin::REFERENCE)
                .owned_by(fixture.opening)
                .target("to", Target::Node(fixture.opening)),
        )
        .unwrap();
    check(&mut session, &engine, &fixture.doc, "relation added");
    let counters = session.counters();
    assert_eq!(counters.shapes, 0, "{counters:?}");
    assert_eq!(counters.compositions, 0, "{counters:?}");
    assert_eq!(counters.composer_calls, 0, "{counters:?}");
    assert_eq!(counters.relation_passes, 1);
    assert!(
        session
            .graph()
            .dependents(&Dependency::Node(fixture.opening))
            .contains(&Computation::Relation(id))
    );
    fixture
        .doc
        .define_style("unused", &Style::default())
        .unwrap();
    check(
        &mut session,
        &engine,
        &fixture.doc,
        "unused style with annotations",
    );
    assert_eq!(session.counters().composer_calls, 0);
    assert_eq!(session.counters().relation_passes, 0);
}

#[test]
fn viewport_first_partial_then_finishing_matches_full_layout() {
    let engine = reprise_fixtures::engine();
    let doc = paragraphs(1000);
    let full = engine.layout(&doc);
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..1));
    let before = job.counters();
    let step = job.step(0).unwrap();
    assert_eq!(step.used, 0);
    assert_eq!(job.counters(), before);
    let step = job.step(64).unwrap();
    assert!(step.viewport_ready && !step.complete, "{step:?}");
    assert!(
        step.used < 64,
        "viewport demand yields before the background budget"
    );
    let partial = job.partial().unwrap();
    assert!(!partial.coverage().complete);
    assert!(partial.coverage().pages.contains(&0));
    assert_eq!(partial.revision(), &doc.revision());
    assert!(partial.publish(&doc).is_ok());
    for block in &partial.snapshot().blocks {
        for line in &block.lines {
            let complete_line = full
                .block(block.node)
                .unwrap()
                .lines
                .iter()
                .find(|l| l.text == line.text)
                .unwrap();
            assert_eq!(line, complete_line);
            assert_eq!(partial.snapshot().frame(line.frame), full.frame(line.frame));
        }
    }
    for _ in 0..2000 {
        if job.step(7).unwrap().complete {
            break;
        }
    }
    assert_eq!(job.complete().unwrap().unwrap(), full);
}

#[test]
fn jobs_and_yielded_results_reject_edits_merges_and_cancellation() {
    let engine = reprise_fixtures::engine();
    let doc = paragraphs(40);
    let node = doc.blocks()[0];
    let mut session = LayoutSession::new(&engine);
    {
        let mut job = session.start(&doc, Viewport::Pages(0..1));
        job.step(64).unwrap();
        let partial = job.partial().unwrap();
        let peer = doc.fork(2).unwrap();
        peer.block(node)
            .unwrap()
            .text
            .insert(0, "concurrent ")
            .unwrap();
        doc.merge(&peer).unwrap();
        assert_eq!(job.step(1), Err(JobError::Stale));
        assert_eq!(job.step(0), Err(JobError::Stale));
        assert!(matches!(partial.publish(&doc), Err(JobError::Stale)));
        assert!(matches!(job.complete(), Err(JobError::Stale)));
    }
    {
        let mut job = session.start(&doc, Viewport::Pages(0..1));
        job.step(64).unwrap();
        let partial = job.partial().unwrap();
        job.cancel();
        assert_eq!(job.step(1), Err(JobError::Cancelled));
        assert!(matches!(partial.publish(&doc), Err(JobError::Cancelled)));
    }
    {
        let mut job = session.start(&doc, Viewport::Pages(0..1));
        doc.block(node)
            .unwrap()
            .text
            .insert(0, "uncommitted edit ")
            .unwrap();
        assert_eq!(job.step(0), Err(JobError::Stale));
    }
    check(&mut session, &engine, &doc, "resumed after cancellation");
}

#[test]
fn every_fixture_has_identical_budgeted_completion_including_region_oscillation() {
    for fixture in hostile::all().unwrap() {
        let mut session = LayoutSession::new(&fixture.engine);
        let full = fixture.engine.layout(&fixture.doc);
        let mut job = session.start(&fixture.doc, Viewport::Pages(0..1));
        let mut complete = false;
        for _ in 0..20_000 {
            let step = job.step(3).unwrap();
            assert!(step.used <= 3);
            if step.complete {
                complete = true;
                break;
            }
        }
        assert!(complete, "{} terminates", fixture.name);
        assert_eq!(job.complete().unwrap().unwrap(), full, "{}", fixture.name);
        assert!(job.counters().region_passes <= 16);
    }
}

#[test]
fn empty_outside_and_extreme_rect_viewports_terminate_without_false_coverage() {
    let engine = reprise_fixtures::engine();
    for doc in [paragraphs(0), paragraphs(3)] {
        for viewport in [
            Viewport::Pages(usize::MAX..usize::MAX),
            Viewport::Pages(99..100),
            Viewport::Rect {
                page: usize::MAX,
                rect: reprise_geom::Rect::new(
                    reprise_geom::Point::new(Length::MIN, Length::MAX),
                    Length::MIN,
                    Length::MAX,
                ),
            },
        ] {
            let mut session = LayoutSession::new(&engine);
            let mut job = session.start(&doc, viewport);
            for _ in 0..100 {
                if job.step(1).unwrap().complete {
                    break;
                }
            }
            assert_eq!(job.complete().unwrap().unwrap(), engine.layout(&doc));
            assert!(job.partial().unwrap().coverage().complete);
        }
    }
}

#[test]
fn one_hundred_thousand_paragraphs_cover_first_page_within_fixed_budget() {
    let engine = reprise_fixtures::engine();
    let doc = paragraphs(100_000);
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..1));
    let step = job.step(64).unwrap();
    assert!(step.viewport_ready && !step.complete, "{step:?}");
    assert!(job.partial().unwrap().coverage().pages.contains(&0));
    assert!(job.counters().shapes < 64);
    job.cancel();
}

#[test]
fn region_viewport_is_explicitly_provisional_until_feedback_and_relations_finish() {
    let fixture = hostile::float_moves_its_anchor().unwrap();
    let mut session = LayoutSession::new(&fixture.engine);
    let mut job = session.start(&fixture.doc, Viewport::Pages(0..1));
    let step = job.step(64).unwrap();
    assert!(step.viewport_ready && !step.complete, "{step:?}");
    let partial = job.partial().unwrap();
    assert!(!partial.coverage().settled);
    assert!(!partial.coverage().complete);
    assert!(partial.coverage().pages.contains(&0));
    assert!(partial.snapshot().diagnostics.is_empty());
    for _ in 0..1000 {
        if job.step(2).unwrap().complete {
            break;
        }
    }
    let final_view = job.partial().unwrap();
    assert!(final_view.coverage().settled && final_view.coverage().complete);
    assert_eq!(final_view.snapshot(), &fixture.engine.layout(&fixture.doc));
}

#[test]
fn deleting_and_reinserting_identical_bytes_invalidates_anchor_answers_only() {
    let fixture = spike::document().unwrap();
    let engine = reprise_fixtures::engine();
    let mut session = LayoutSession::new(&engine);
    let before = session.layout(&fixture.doc).unwrap();
    let block = fixture.doc.block(fixture.hallway).unwrap();
    let text = block.text.to_string();
    let start = text.find("a corridor").unwrap();
    let bytes = start..start + "a corridor".len();
    block.text.delete(bytes.clone()).unwrap();
    block.text.insert(bytes.start, "a corridor").unwrap();
    assert_eq!(block.text.to_string(), text);
    check(
        &mut session,
        &engine,
        &fixture.doc,
        "same bytes with changed anchors",
    );
    assert_eq!(session.counters().shapes, 0);
    assert_eq!(session.counters().composer_calls, 0);
    assert_eq!(session.counters().relation_passes, 1);
    let after = session.layout(&fixture.doc).unwrap();
    assert_ne!(after.relations, before.relations);
}

#[test]
fn unused_style_does_not_reexecute_region_allocation() {
    let fixture = hostile::float_moves_its_anchor().unwrap();
    let mut session = LayoutSession::new(&fixture.engine);
    check(&mut session, &fixture.engine, &fixture.doc, "region before");
    fixture
        .doc
        .define_style(
            "unreferenced",
            &Style {
                line_height: Some(LengthExpr::Pt(Length::MIN)),
                ..Style::default()
            },
        )
        .unwrap();
    check(
        &mut session,
        &fixture.engine,
        &fixture.doc,
        "region unused style",
    );
    let counters = session.counters();
    assert_eq!(counters.shapes, 0, "{counters:?}");
    assert_eq!(counters.composer_calls, 0, "{counters:?}");
    assert_eq!(counters.region_passes, 0, "{counters:?}");
    assert_eq!(counters.relation_passes, 0, "{counters:?}");
    assert!(
        session
            .graph()
            .why_recomputed(&Computation::Regions)
            .is_empty()
    );
}

#[test]
fn equal_fixture_revisions_do_not_authorize_publication_to_another_document() {
    let engine = reprise_fixtures::engine();
    let doc = paragraphs(30);
    let other = paragraphs(30);
    assert_eq!(doc.revision(), other.revision());
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..1));
    job.step(64).unwrap();
    let view = job.partial().unwrap();
    assert!(matches!(view.publish(&other), Err(JobError::Stale)));
    assert!(view.publish(&doc).is_ok());
    job.cancel();
    drop(job);
    check(
        &mut session,
        &engine,
        &other,
        "switch to equal-revision document",
    );
}

#[test]
fn rectangle_demand_covers_its_page_before_background_completion() {
    let engine = reprise_fixtures::engine();
    let doc = paragraphs(200);
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(
        &doc,
        Viewport::Rect {
            page: 2,
            rect: reprise_geom::Rect::new(
                reprise_geom::Point::new(Length::from_pt(10), Length::from_pt(20)),
                Length::from_pt(80),
                Length::from_pt(60),
            ),
        },
    );
    let step = job.step(100).unwrap();
    assert!(step.viewport_ready && !step.complete, "{step:?}");
    let view = job.partial().unwrap();
    assert!(view.coverage().pages.contains(&2));
    assert!(view.coverage().settled && !view.coverage().complete);
    for _ in 0..300 {
        if job.step(5).unwrap().complete {
            break;
        }
    }
    assert_eq!(job.complete().unwrap().unwrap(), engine.layout(&doc));
}

#[test]
fn line_height_edits_change_composition_without_reitemizing_or_reshaping() {
    let engine = reprise_fixtures::engine();
    let doc = paragraphs(30);
    let node = doc.blocks()[0];
    let mut session = LayoutSession::new(&engine);
    check(&mut session, &engine, &doc, "height before");
    doc.set_overrides(
        node,
        &Style {
            line_height: Some(LengthExpr::Pt(Length::from_pt(18))),
            ..Style::default()
        },
    )
    .unwrap();
    check(&mut session, &engine, &doc, "height edit");
    let counters = session.counters();
    assert_eq!(counters.shapes, 0, "{counters:?}");
    assert_eq!(counters.itemizations, 0, "{counters:?}");
    assert!(counters.compositions > 0);
    let graph = session.graph();
    assert!(graph.why_recomputed(&Computation::Shape(node)).is_empty());
    assert!(
        graph
            .why_recomputed(&Computation::Style(node))
            .contains(&Dependency::Node(node))
    );
}
