mod common;
use common::*;
use reprise_doc::relation::builtin;
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, Dim, Document, Domain, InlineObject, NodeId, PageTemplate, Param, Relation, Target,
};
use reprise_layout::incremental::{JobError, LayoutSession, Pass, Viewport};
use reprise_layout::{Engine, LayoutSnapshot};

fn bounded(engine: &Engine, doc: &Document, budget: usize) -> LayoutSnapshot {
    let mut session = LayoutSession::new(engine);
    let mut state = session.start(doc, Viewport::Pages(0..usize::MAX)).suspend();
    let mut composed = 0;
    for i in 0..100_000 {
        let mut job = session.resume(doc, state).unwrap();
        let step = job.step(budget).unwrap();
        let w = job.step_work();
        let c = job.counters();
        assert!(step.used <= budget, "{i}: {step:?}");
        assert!(w.digest_blocks <= 64 * budget, "{i}: {w:?}");
        assert!(w.digest_inspected <= 64 * budget, "{i}: {w:?}");
        assert!(c.running_compositions - composed <= budget, "{i}: {w:?}");
        assert_eq!(c.running_inline, 0, "{i}: {c:?}");
        composed = c.running_compositions;
        if step.complete {
            return job.complete().unwrap().unwrap();
        }
        state = job.suspend();
    }
    panic!("did not finish");
}

#[test]
fn several_hosts_and_table_cell_hosts_are_bounded_and_resume() {
    for in_table in [false, true] {
        let (engine, doc) = setup();
        let story = doc.create_story(&[]).unwrap();
        let cell = if in_table {
            let table = doc
                .append_table(two_columns([
                    reprise_doc::ColumnWidth::Content,
                    reprise_doc::ColumnWidth::Content,
                ]))
                .unwrap();
            let row = doc.append_table_row(table, false).unwrap();
            let cell = doc.append_table_cell(row, 0).unwrap();
            doc.move_block(table, Some(story), 0).unwrap();
            Some(cell)
        } else {
            None
        };
        for _ in 0..4 {
            let mut tail = match cell {
                Some(cell) => doc
                    .append_cell_block(cell, BlockKind::Paragraph, "s", "x")
                    .unwrap(),
                None => doc
                    .append_story_block(story, BlockKind::Paragraph, "s", "x")
                    .unwrap(),
            };
            for _ in 0..129 {
                tail = doc.split_block(tail, 0).unwrap();
            }
        }
        bind_header(&doc, story);
        block(&doc, "body");
        doc.commit();
        let expected = engine.layout(&doc);
        for budget in [1, 3, 7] {
            assert_eq!(bounded(&engine, &doc, budget), expected, "table={in_table}");
        }
    }
}

fn mixed() -> (Engine, Document, NodeId) {
    let (mut engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(120))
            .with_frame(frame("main", 0, 0, 200, 120))
            .with_frame(notes_frame(0, 30, 200, 90)),
    )
    .unwrap();
    let head = story(&doc, "head ");
    let hb = doc.top(Domain::Story(head))[0];
    doc.insert_inline(hb, 5, &InlineObject::field("page-number"))
        .unwrap();
    bind(
        &doc,
        &[
            ("region.header.default", head),
            ("region.footer.default", head),
        ],
    );
    let anchor = block(&doc, "anchor");
    let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
    let note = doc
        .append_block(BlockKind::Annotation, "s", &vec!["note"; 500].join("\n"))
        .unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::NOTE)
            .owned_by(note)
            .target("anchor", Target::Range(range))
            .param("placement", Param::Text("page".into())),
    )
    .unwrap();
    let hash = engine
        .assets
        .insert(&include_bytes!("../../../fixtures/images/red-1x1.png")[..])
        .unwrap();
    let img = doc
        .append_image("s", &reprise_doc::image::ImageData::new(hash), "float")
        .unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::FLOAT)
            .owned_by(img)
            .target("anchor", Target::Range(range))
            .param("side", Param::Text("top".into())),
    )
    .unwrap();
    doc.commit();
    (engine, doc, hb)
}

#[test]
fn notes_floats_two_slots_and_suspension_keep_every_composition_charged() {
    let (engine, doc, _) = mixed();
    let expected = engine.layout(&doc);
    assert!(expected.pages.len() > 64);
    for budget in [1, 7] {
        assert_eq!(bounded(&engine, &doc, budget), expected);
    }
}

#[test]
fn cancellation_and_staleness_between_mixed_feedback_units() {
    for stale in [false, true] {
        let (engine, doc, hb) = mixed();
        let mut session = LayoutSession::new(&engine);
        let mut job = session.start(&doc, Viewport::Pages(0..usize::MAX));
        let mut feedback_units = 0;
        loop {
            let step = job.step(1).unwrap();
            if step.pass == Pass::RegionFeedback && job.step_work().region_units > 0 {
                feedback_units += 1;
            }
            if feedback_units == 40 {
                break;
            }
            assert!(!step.complete);
        }
        if stale {
            doc.block(hb).unwrap().text.insert(0, "changed ").unwrap();
            doc.commit();
            assert!(matches!(job.step(1), Err(JobError::Stale)));
        } else {
            job.cancel();
            assert!(matches!(job.step(1), Err(JobError::Cancelled)));
        }
        drop(job);
        assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
    }
}

#[test]
fn a_suspended_job_cannot_resume_with_lost_holds_and_compose_inline() {
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(120))
            .with_frame(frame("main", 0, 0, 200, 120))
            .with_frame(notes_frame(0, 30, 200, 90)),
    )
    .unwrap();
    let head = story(&doc, "head ");
    let hb = doc.top(Domain::Story(head))[0];
    doc.insert_inline(hb, 5, &InlineObject::field("page-number"))
        .unwrap();
    bind_header(&doc, head);
    let anchor = block(&doc, "anchor");
    let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
    let note = doc
        .append_block(BlockKind::Annotation, "s", &vec!["note"; 700].join("\n"))
        .unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::NOTE)
            .owned_by(note)
            .target("anchor", Target::Range(range))
            .param("placement", Param::Text("page".into())),
    )
    .unwrap();
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..usize::MAX));
    let mut feedback_units = 0;
    for _ in 0..100_000 {
        let step = job.step(1).unwrap();
        if step.pass == Pass::RegionFeedback && job.step_work().region_units > 0 {
            feedback_units += 1;
        }
        if feedback_units >= 40 {
            break;
        }
        assert!(!step.complete);
    }
    assert_eq!(feedback_units, 40);
    let state = job.suspend();
    // Both operations are public. The original document and engine are unchanged.
    // Starting another job clears session-held dependencies but does not invalidate state.
    drop(session.start(&doc, Viewport::Pages(0..1)));
    let Ok(mut job) = session.resume(&doc, state) else {
        return;
    };
    for i in 0..100_000 {
        let Ok(step) = job.step(1) else {
            return;
        };
        let counters = job.counters();
        if counters.running_inline > 0 {
            eprintln!(
                "resumed step {i}, used={}, counters={counters:?}, work={:?}",
                step.used,
                job.step_work()
            );
        }
        assert_eq!(
            counters.running_inline, 0,
            "a public start/resume sequence reached the inline whole-story fallback"
        );
        if step.complete {
            return;
        }
    }
    panic!("did not finish");
}

/// A page with a note of 700 lines and a numbered running header: the
/// allocation asks for a header for every continuation page at once.
fn endless_note() -> (Engine, Document) {
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(120))
            .with_frame(frame("main", 0, 0, 200, 120))
            .with_frame(notes_frame(0, 30, 200, 90)),
    )
    .unwrap();
    let head = story(&doc, "head ");
    let hb = doc.top(Domain::Story(head))[0];
    doc.insert_inline(hb, 5, &InlineObject::field("page-number"))
        .unwrap();
    bind_header(&doc, head);
    let anchor = block(&doc, "anchor");
    let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
    let note = doc
        .append_block(BlockKind::Annotation, "s", &vec!["note"; 700].join("\n"))
        .unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::NOTE)
            .owned_by(note)
            .target("anchor", Target::Range(range))
            .param("placement", Param::Text("page".into())),
    )
    .unwrap();
    doc.commit();
    (engine, doc)
}

/// Steps a job at budget 1 until `units` look-ahead units of region feedback
/// have run, then suspends it.
fn suspended_in_feedback(
    session: &mut LayoutSession<'_>,
    doc: &Document,
    units: usize,
) -> reprise_layout::incremental::LayoutContinuation {
    let mut job = session.start(doc, Viewport::Pages(0..usize::MAX));
    let mut seen = 0;
    for _ in 0..100_000 {
        let step = job.step(1).unwrap();
        if step.pass == Pass::RegionFeedback && job.step_work().region_units > 0 {
            seen += 1;
        }
        if seen >= units {
            return job.suspend();
        }
        assert!(!step.complete);
    }
    panic!("never reached region feedback");
}

#[test]
fn a_continuation_of_a_replaced_job_is_refused_and_the_replacement_is_unharmed() {
    // The same sequence as the probe above, asserted exactly: starting a job
    // takes the session's digests and held compositions, so the earlier
    // continuation must not resume (it would compose them inline, whole).
    let (engine, doc) = endless_note();
    let expected = engine.layout(&doc);
    let mut session = LayoutSession::new(&engine);
    let stale = suspended_in_feedback(&mut session, &doc, 40);
    drop(session.start(&doc, Viewport::Pages(0..1)));
    assert!(matches!(session.resume(&doc, stale), Err(JobError::Stale)));
    // Refusing it left the session as it was: the next job is correct.
    assert_eq!(session.layout(&doc).unwrap(), expected);
    assert_eq!(session.counters().running_inline, 0);
    // A layout takes the memo as well (a session that has laid the document out
    // has its compositions, and needs none held, so each case has its own).
    let mut session = LayoutSession::new(&engine);
    let stale = suspended_in_feedback(&mut session, &doc, 40);
    assert_eq!(session.layout(&doc).unwrap(), expected);
    assert!(matches!(session.resume(&doc, stale), Err(JobError::Stale)));
    // And a continuation resumed with nothing started in between goes on.
    let mut session = LayoutSession::new(&engine);
    let mut state = suspended_in_feedback(&mut session, &doc, 40);
    let snapshot = loop {
        let mut job = session.resume(&doc, state).unwrap();
        let step = job.step(1).unwrap();
        assert_eq!(job.counters().running_inline, 0);
        if step.complete {
            break job.complete().unwrap().unwrap();
        }
        state = job.suspend();
    };
    assert_eq!(snapshot, expected);
}
