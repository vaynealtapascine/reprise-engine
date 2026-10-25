mod common;
use common::*;
use reprise_doc::relation::builtin;
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, Dim, Document, Domain, InlineObject, NewBlock, NodeId, PageTemplate, Param,
    Relation, Style, Target,
};
use reprise_layout::incremental::{
    JobError, LayoutSession, Pass, SHAPE_CHUNK_BYTES, StepWork, Viewport,
};
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

/// Resumes and steps `state` one unit at a time to the end.
fn finish_suspended(
    session: &mut LayoutSession<'_>,
    doc: &Document,
    mut state: reprise_layout::incremental::LayoutContinuation,
) -> LayoutSnapshot {
    for _ in 0..100_000 {
        let mut job = session.resume(doc, state).expect("the newest job resumes");
        let step = job.step(1).unwrap();
        assert_eq!(job.counters().running_inline, 0);
        if step.complete {
            return job.complete().unwrap().unwrap();
        }
        state = job.suspend();
    }
    panic!("did not finish");
}

#[test]
fn an_obsolete_continuation_of_an_older_revision_leaves_the_newer_job_alone() {
    // A job suspended, the document edited, a job on the new revision started
    // and suspended: the first's continuation is refused for its revision, and
    // refusing it must not take the memo the second job holds.
    let (engine, doc) = endless_note();
    let mut session = LayoutSession::new(&engine);
    let old = suspended_in_feedback(&mut session, &doc, 2);
    let body = doc.top(Domain::Body)[0];
    doc.block(body).unwrap().text.insert(0, "edit ").unwrap();
    doc.commit();
    let expected = engine.layout(&doc);
    let newer = suspended_in_feedback(&mut session, &doc, 3);
    let before = session.counters();
    assert!(before.units > 0);
    assert!(matches!(session.resume(&doc, old), Err(JobError::Stale)));
    assert_eq!(session.counters(), before, "the memo is the newer job's");
    assert_eq!(finish_suspended(&mut session, &doc, newer), expected);
}

#[test]
fn an_obsolete_continuation_resumed_with_another_document_leaves_the_newer_job_alone() {
    // The same, refused for its document: the continuation names one document
    // and is resumed against another while a newer job holds the memo.
    let (engine, doc) = endless_note();
    let expected = engine.layout(&doc);
    let other = Document::new(2).unwrap();
    let mut session = LayoutSession::new(&engine);
    let old = suspended_in_feedback(&mut session, &doc, 2);
    let newer = suspended_in_feedback(&mut session, &doc, 3);
    let before = session.counters();
    assert!(matches!(session.resume(&other, old), Err(JobError::Stale)));
    assert_eq!(session.counters(), before, "the memo is the newer job's");
    assert_eq!(finish_suspended(&mut session, &doc, newer), expected);
    // A continuation made on another session is not the memo's owner either.
    let mut session = LayoutSession::new(&engine);
    let newer = suspended_in_feedback(&mut session, &doc, 3);
    let mut elsewhere = LayoutSession::new(&engine);
    let foreign = suspended_in_feedback(&mut elsewhere, &doc, 2);
    let before = session.counters();
    assert!(matches!(
        session.resume(&other, foreign),
        Err(JobError::Stale)
    ));
    assert_eq!(session.counters(), before);
    assert_eq!(finish_suspended(&mut session, &doc, newer), expected);
}

#[test]
fn the_job_that_owns_the_memo_still_drops_it_when_its_document_is_not_the_one() {
    // The owner's behaviour is as it was: a document edit can race a job and
    // leave anything in the memo, so refusing the owner drops everything.
    for case in 0..2 {
        let (engine, doc) = endless_note();
        let other = Document::new(2).unwrap();
        let mut session = LayoutSession::new(&engine);
        let owner = suspended_in_feedback(&mut session, &doc, 3);
        assert!(session.counters().units > 0);
        if case == 0 {
            assert!(matches!(
                session.resume(&other, owner),
                Err(JobError::Stale)
            ));
        } else {
            let body = doc.top(Domain::Body)[0];
            doc.block(body).unwrap().text.insert(0, "edit ").unwrap();
            doc.commit();
            assert!(matches!(session.resume(&doc, owner), Err(JobError::Stale)));
        }
        assert_eq!(session.counters(), Default::default(), "case {case}");
        assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
    }
}

#[test]
fn keep_lookahead_must_not_prepare_a_host_refused_by_the_digest() {
    let (engine, doc) = setup();
    let s = doc
        .create_story(&[
            NewBlock::new(BlockKind::Paragraph, "s", "head"),
            NewBlock::new(BlockKind::Paragraph, "s", &"z".repeat(1_100_000)),
        ])
        .unwrap();
    let head = doc.top(Domain::Story(s))[0];
    doc.set_overrides(
        head,
        &reprise_doc::Style {
            keep_with_next: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    bind_header(&doc, s);
    block(&doc, "body");
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..1));
    for i in 0..100 {
        let step = job.step(1).unwrap();
        let w = job.step_work();
        eprintln!("keep step {i}, used={}, {w:?}", step.used);
        assert!(
            w.shaped_bytes < 1_100_000,
            "lookahead prepared a refused host: {w:?}"
        );
        assert!(
            w.largest_request <= reprise_layout::incremental::SHAPE_CHUNK_BYTES,
            "{w:?}"
        );
        if step.complete {
            return;
        }
    }
    panic!("did not finish");
}

#[test]
fn omitted_host_keep_style_must_not_leave_running_memo_stale() {
    let (engine, doc) = setup();
    let s = doc
        .create_story(&[
            NewBlock::new(BlockKind::Paragraph, "s", "head"),
            NewBlock::new(BlockKind::Paragraph, "s", &"z ".repeat(550_000)),
        ])
        .unwrap();
    let nodes = doc.top(Domain::Story(s));
    doc.set_overrides(
        nodes[0],
        &reprise_doc::Style {
            keep_with_next: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    bind_header(&doc, s);
    block(&doc, "body");
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    let before = session.layout(&doc).unwrap();
    doc.set_overrides(
        nodes[1],
        &reprise_doc::Style {
            keep_together: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    doc.commit();
    let cached = session.layout(&doc).unwrap();
    let full = engine.layout(&doc);
    eprintln!(
        "before diagnostics={:?}; cached={:?}; full={:?}; counters={:?}",
        before.diagnostics,
        cached.diagnostics,
        full.diagnostics,
        session.counters()
    );
    assert_eq!(
        cached, full,
        "keep lookahead reads beyond the digest and its inputs are not memoized"
    );
}

/// A running header of paragraphs, each with its text and whether it keeps
/// with the next, and a body paragraph so the document has a page.
fn kept_header(blocks: &[(String, bool)]) -> (Engine, Document, NodeId, Vec<NodeId>) {
    let (engine, doc) = setup();
    let news: Vec<_> = blocks
        .iter()
        .map(|(text, _)| NewBlock::new(BlockKind::Paragraph, "s", text))
        .collect();
    let s = doc.create_story(&news).unwrap();
    let nodes = doc.top(Domain::Story(s));
    for ((_, keep), node) in blocks.iter().zip(&nodes) {
        if *keep {
            let style = Style {
                keep_with_next: Some(true),
                ..Default::default()
            };
            doc.set_overrides(*node, &style).unwrap();
        }
    }
    bind_header(&doc, s);
    block(&doc, "body");
    doc.commit();
    (engine, doc, s, nodes)
}

/// Runs a job to the end `budget` units a step, suspending and resuming
/// between every step, with the work of each.
fn stepped(engine: &Engine, doc: &Document, budget: usize) -> (LayoutSnapshot, Vec<StepWork>) {
    let mut session = LayoutSession::new(engine);
    let mut state = session.start(doc, Viewport::Pages(0..usize::MAX)).suspend();
    let mut works = Vec::new();
    for _ in 0..1_000_000 {
        let mut job = session.resume(doc, state).unwrap();
        let step = job.step(budget).unwrap();
        works.push(job.step_work());
        assert_eq!(job.counters().running_inline, 0);
        if step.complete {
            return (job.complete().unwrap().unwrap(), works);
        }
        state = job.suspend();
    }
    panic!("did not finish");
}

fn keep_unsatisfied(snapshot: &LayoutSnapshot) -> Vec<NodeId> {
    snapshot
        .diagnostics
        .iter()
        .filter(|d| d.code.as_str() == "layout.keep-unsatisfied")
        .filter_map(|d| match d.subject {
            reprise_layout::Subject::Node(n) => Some(n),
            _ => None,
        })
        .collect()
}

#[test]
fn a_keep_chain_of_long_members_is_checked_a_piece_a_unit() {
    // The first block keeps with a 100 KB paragraph that keeps with the next
    // and does not fit the header whole. Checking that is preparing it (seven
    // shaping requests, break analysis) and composing it: each a unit of its
    // own, not the one unit of the block that asks.
    let long = "word ".repeat(20_000);
    let (engine, doc, _, nodes) = kept_header(&[("head".into(), true), (long, true)]);
    let expected = engine.layout(&doc);
    assert_eq!(keep_unsatisfied(&expected), vec![nodes[0]]);
    let (snapshot, works) = stepped(&engine, &doc, 1);
    assert_eq!(snapshot, expected);
    let mut shaped = 0;
    for (i, w) in works.iter().enumerate() {
        assert!(w.composer_calls <= 1, "step {i}: {w:?}");
        assert!(w.adapter_requests <= 1, "step {i}: {w:?}");
        assert!(w.largest_request <= SHAPE_CHUNK_BYTES, "step {i}: {w:?}");
        assert!(w.digest_blocks <= 64, "step {i}: {w:?}");
        shaped += w.shaped_bytes;
    }
    // The member was prepared, over several steps, for the chain.
    assert!(shaped >= 100_000, "{shaped} bytes shaped");
    assert!(
        works.iter().filter(|w| w.adapter_requests == 1).count() >= 7,
        "{works:?}"
    );
}

#[test]
fn a_keep_chain_of_short_members_is_checked_a_composition_a_unit() {
    // Each member of the chain is composed in a unit of its own, and the
    // verdict is the one the whole layout comes to, with or without the unit
    // boundaries between.
    for tail_keeps in [false, true] {
        let blocks: Vec<_> = (0..6)
            .map(|i| (format!("line {i}"), i < 5 || tail_keeps))
            .collect();
        let (engine, doc, _, _) = kept_header(&blocks);
        let expected = engine.layout(&doc);
        for budget in [1, 2, 5] {
            let (snapshot, works) = stepped(&engine, &doc, budget);
            assert_eq!(snapshot, expected, "budget {budget}");
            if budget == 1 {
                for (i, w) in works.iter().enumerate() {
                    assert!(w.composer_calls <= 1, "step {i}: {w:?}");
                    assert!(w.adapter_requests <= 1, "step {i}: {w:?}");
                }
                // Six blocks, five chains of up to five members: more than the
                // blocks' own compositions.
                let composed: usize = works.iter().map(|w| w.composer_calls).sum();
                assert!(composed > 6 + 5, "{composed} composer calls");
            }
        }
    }
}

#[test]
fn a_keep_chain_in_a_running_story_reads_only_what_the_digest_took() {
    // The story's second block is cut by the read limits (no story can read
    // all of it). The first keeps with it; there is nothing to keep with. The
    // layout notes nothing of it, whether the refused block is edited or not,
    // and the cached layout is the full one.
    let refused = "z ".repeat(550_000);
    let (engine, doc, _, nodes) = kept_header(&[("head".into(), true), (refused, false)]);
    let mut session = LayoutSession::new(&engine);
    let before = session.layout(&doc).unwrap();
    assert_eq!(before, engine.layout(&doc));
    assert!(keep_unsatisfied(&before).is_empty());
    for edit in 0..3 {
        match edit {
            0 => doc.set_overrides(
                nodes[1],
                &Style {
                    keep_together: Some(true),
                    ..Default::default()
                },
            ),
            1 => doc.set_overrides(
                nodes[1],
                &Style {
                    keep_with_next: Some(true),
                    ..Default::default()
                },
            ),
            _ => doc.set_overrides(nodes[1], &Style::default()),
        }
        .unwrap();
        doc.commit();
        let cached = session.layout(&doc).unwrap();
        assert_eq!(cached, engine.layout(&doc), "edit {edit}");
        assert!(keep_unsatisfied(&cached).is_empty(), "edit {edit}");
    }
}

#[test]
fn editing_a_block_only_the_keep_lookahead_reads_re_lays_out_what_it_keeps_with() {
    // Block 1 is read by the first block's chain and is laid out below it: it
    // is in the digest. Its style decides the chain, so a cached layout after
    // the edit must be the full one, warning and all.
    let blocks = vec![
        ("head".to_owned(), true),
        ("the second paragraph ".repeat(80), false),
    ];
    let (engine, doc, _, nodes) = kept_header(&blocks);
    let mut session = LayoutSession::new(&engine);
    let before = session.layout(&doc).unwrap();
    assert_eq!(before, engine.layout(&doc));
    for keep_together in [true, false, true] {
        doc.set_overrides(
            nodes[1],
            &Style {
                keep_together: Some(keep_together),
                ..Default::default()
            },
        )
        .unwrap();
        doc.commit();
        let cached = session.layout(&doc).unwrap();
        assert_eq!(cached, engine.layout(&doc), "keep_together {keep_together}");
    }
}

/// Replaces Astra's `a_host_decode_must_fit_the_current_step_instead_of_
/// borrowing_future_steps`, which required a budget-one step never to decode
/// more than 64 units of text. The orchestrator ruled the decode of a host an
/// atomic unit instead (it cannot be split without making the CRDT layer's flow
/// reading incremental): at most `MAX_HOST_BYTES`, alone in its step, charged
/// in full in that step. The probe's fixture is kept; its assertions are these.
#[test]
fn a_host_decode_is_one_bounded_atomic_unit_that_charges_its_cost_in_its_step() {
    const MAX_HOST_BYTES: usize = (1 << 20) + 3 * 4096;
    const READING_UNIT_BYTES: usize = 4096;
    const READING_UNITS_PER_STEP: usize = 64;
    let (engine, doc) = setup();
    let s = doc
        .create_story(&[NewBlock::new(
            BlockKind::Paragraph,
            "s",
            &"z".repeat(1_000_000),
        )])
        .unwrap();
    bind_header(&doc, s);
    block(&doc, "body");
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..1));
    let mut decodes = 0;
    for i in 0..8 {
        let step = job.step(1).unwrap();
        let w = job.step_work();
        if w.digest_decoded == 0 {
            // Every other step is the one unit it was asked for.
            assert_eq!((step.used, w.units), (1, 1), "step {i}: {w:?}");
            continue;
        }
        decodes += 1;
        assert!(w.digest_decoded <= MAX_HOST_BYTES, "step {i}: {w:?}");
        // Charged in its own step, whole: no debt left for the steps after it.
        let cost = (w.digest_decoded / READING_UNIT_BYTES).div_ceil(READING_UNITS_PER_STEP);
        assert!(cost > 1 && cost <= 5, "step {i}: {cost} units, {w:?}");
        assert_eq!((step.used, w.units), (cost, cost), "step {i}: {w:?}");
        // And nothing else ran in it: the host's block, no other look, no
        // composition, shaping or page.
        assert_eq!(
            (
                w.digest_blocks,
                w.digest_inspected,
                w.region_blocks,
                w.composer_calls,
                w.adapter_requests,
                w.pages_made,
            ),
            (1, 0, 0, 0, 0, 0),
            "step {i}: {w:?}"
        );
    }
    assert_eq!(decodes, 1, "the host is decoded once");
}

// What a keep-with-next chain reads of the relations that own a block. The
// chain passes over an annotation and over an image a float or note owns, and
// ends at any other image or a table; a paragraph is a paragraph whatever
// relation names it owner, because the flow lays it out (the region pass calls
// such an owner invalid and places nothing). Whatever the chain reads must be
// in the key of what it keeps: the body's paragraph memo and the running
// digest. Each test edits relations through one session and compares every
// layout with a full layout of the same document, diagnostics included.

use reprise_doc::relation::SchemaId;
use reprise_doc::{ColumnWidth, LayoutQuery, RelationId};

fn owned_by(
    engine: &Engine,
    doc: &Document,
    schema: SchemaId,
    owner: NodeId,
    anchor: NodeId,
) -> RelationId {
    let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
    let relation = Relation::new(schema.clone()).owned_by(owner);
    let relation = match schema {
        s if s == builtin::NOTE => relation
            .target("anchor", Target::Range(range))
            .param("placement", Param::Text("page".into())),
        s if s == builtin::FLOAT => relation
            .target("anchor", Target::Range(range))
            .param("side", Param::Text("top".into())),
        _ => relation.target(
            "line",
            Target::Layout(LayoutQuery::LineContaining { range }),
        ),
    };
    doc.add_relation(&engine.schemas, &relation).unwrap()
}

/// Lays `doc` out in `session`; it must be what a full layout is. Returns the
/// blocks whose keep was not satisfied.
fn laid_out_as_a_full_layout_is(
    engine: &Engine,
    doc: &Document,
    session: &mut LayoutSession<'_>,
    what: &str,
) -> Vec<NodeId> {
    let cached = session.layout(doc).unwrap();
    let full = engine.layout(doc);
    assert!(
        cached == full,
        "{what}: the session's layout is not a full one"
    );
    keep_unsatisfied(&full)
}

/// A running header `head` (keeps with the next), the block `middle` makes,
/// and a paragraph of 5,000 bytes that keeps with the next and does not fit
/// the header: the chain from `head` fails, unless it ends at the middle block.
fn header_chain(
    middle: impl FnOnce(&mut Engine, &Document) -> NodeId,
) -> (Engine, Document, NodeId, NodeId) {
    let (mut engine, doc, story, nodes) =
        kept_header(&[("head".into(), true), ("word ".repeat(1000), true)]);
    let block = middle(&mut engine, &doc);
    doc.move_block(block, Some(story), 1).unwrap();
    doc.commit();
    (engine, doc, nodes[0], block)
}

#[test]
fn a_paragraph_a_relation_names_owner_is_still_a_paragraph_to_the_keep_chain() {
    // The re-review's finding: adding or removing a note or float relation
    // owned by the paragraph the chain looks at makes no difference to the
    // chain, in a session or not.
    for schema in [builtin::NOTE, builtin::FLOAT] {
        let (engine, doc, _, nodes) =
            kept_header(&[("head".into(), true), ("word ".repeat(1000), true)]);
        let anchor = doc.top(Domain::Body)[0];
        let mut session = LayoutSession::new(&engine);
        let plain = laid_out_as_a_full_layout_is(&engine, &doc, &mut session, "plain");
        assert_eq!(plain, vec![nodes[0]], "the chain from the head fails");
        let id = owned_by(&engine, &doc, schema.clone(), nodes[1], anchor);
        doc.commit();
        let owned = laid_out_as_a_full_layout_is(&engine, &doc, &mut session, "owner");
        assert_eq!(owned, plain, "{schema:?}");
        doc.delete_relation(id).unwrap();
        doc.commit();
        let gone = laid_out_as_a_full_layout_is(&engine, &doc, &mut session, "owner removed");
        assert_eq!(gone, plain, "{schema:?}");
    }
}

#[test]
fn a_body_paragraph_a_relation_names_owner_is_still_a_paragraph_to_the_keep_chain() {
    // The same in the body, whose paragraph memo has its own key: pages of ten
    // lines, a filler of eight, a head that keeps with a paragraph that does
    // not fit what is left of the page and keeps with the next.
    for schema in [builtin::NOTE, builtin::FLOAT] {
        let (engine, doc) = setup();
        doc.set_page_template(
            &PageTemplate::new("p", Dim::pt(200), Dim::pt(120))
                .with_frame(frame("main", 0, 0, 200, 120)),
        )
        .unwrap();
        block(&doc, &["fill"; 8].join("\n"));
        let head = block(&doc, "head");
        let long = block(&doc, &"word ".repeat(1000));
        for node in [head, long] {
            let keep = Style {
                keep_with_next: Some(true),
                ..Default::default()
            };
            doc.set_overrides(node, &keep).unwrap();
        }
        let anchor = block(&doc, "tail");
        doc.commit();
        let mut session = LayoutSession::new(&engine);
        let plain = laid_out_as_a_full_layout_is(&engine, &doc, &mut session, "plain");
        assert!(!plain.is_empty(), "the head's keep is not satisfied");
        let id = owned_by(&engine, &doc, schema.clone(), long, anchor);
        doc.commit();
        let owned = laid_out_as_a_full_layout_is(&engine, &doc, &mut session, "owner");
        assert_eq!(owned, plain, "{schema:?}");
        doc.delete_relation(id).unwrap();
        doc.commit();
        let gone = laid_out_as_a_full_layout_is(&engine, &doc, &mut session, "owner removed");
        assert_eq!(gone, plain, "{schema:?}");
    }
}

fn plain_image(engine: &mut Engine, doc: &Document) -> NodeId {
    let hash = engine
        .assets
        .insert(&include_bytes!("../../../fixtures/images/red-1x1.png")[..])
        .unwrap();
    doc.append_image("s", &reprise_doc::image::ImageData::new(hash), "image")
        .unwrap()
}

#[test]
fn an_image_a_float_or_note_owns_is_passed_over_by_the_keep_chain_in_both_directions() {
    // An image the chain reaches ends it as fitting; one a note or float owns
    // is left to the regions and the chain goes on to the paragraph after it,
    // which does not fit. So owning it flips the verdict, and the memo must
    // follow, whichever way the relation goes.
    for schema in [builtin::NOTE, builtin::FLOAT] {
        let (engine, doc, head, image) = header_chain(plain_image);
        let anchor = doc.top(Domain::Body)[0];
        let mut session = LayoutSession::new(&engine);
        let plain = laid_out_as_a_full_layout_is(&engine, &doc, &mut session, "plain");
        assert!(plain.is_empty());
        let id = owned_by(&engine, &doc, schema.clone(), image, anchor);
        doc.commit();
        let owned = laid_out_as_a_full_layout_is(&engine, &doc, &mut session, "owned");
        assert_eq!(owned, vec![head], "{schema:?}");
        doc.delete_relation(id).unwrap();
        doc.commit();
        let gone = laid_out_as_a_full_layout_is(&engine, &doc, &mut session, "owner removed");
        assert!(gone.is_empty(), "{schema:?}");
    }
}

#[test]
fn an_image_both_followed_and_owned_changes_the_keep_chain_when_only_the_owner_goes() {
    // The digest already holds that an image is left out of the story when a
    // relation places it, whichever one. The chain reads less than that, only
    // whether a float or note owns it: removing that relation leaves the image
    // placed by following a line, with the same verdict for the story and a
    // different one for the chain.
    let (engine, doc, head, image) = header_chain(plain_image);
    let anchor = doc.top(Domain::Body)[0];
    let mut session = LayoutSession::new(&engine);
    let mut at = |what: &str| laid_out_as_a_full_layout_is(&engine, &doc, &mut session, what);
    assert!(at("plain").is_empty());
    let follow = owned_by(&engine, &doc, builtin::FOLLOW, image, anchor);
    doc.commit();
    assert!(at("followed").is_empty());
    let float = owned_by(&engine, &doc, builtin::FLOAT, image, anchor);
    doc.commit();
    assert_eq!(at("followed and owned"), vec![head]);
    doc.delete_relation(float).unwrap();
    doc.commit();
    assert!(at("only followed").is_empty());
    doc.delete_relation(follow).unwrap();
    doc.commit();
    assert!(at("plain again").is_empty());
}

#[test]
fn a_table_a_relation_names_owner_still_ends_the_keep_chain() {
    // A table is laid out as one whatever owns its container, and the chain
    // ends at it. The container is no region owner the region pass can place.
    let (engine, doc, _, table) = header_chain(|_, doc| {
        let table = doc
            .append_table(two_columns([ColumnWidth::Content, ColumnWidth::Content]))
            .unwrap();
        let row = doc.append_table_row(table, false).unwrap();
        let cell = doc.append_table_cell(row, 0).unwrap();
        doc.append_cell_block(cell, BlockKind::Paragraph, "s", "cell")
            .unwrap();
        table
    });
    let anchor = doc.top(Domain::Body)[0];
    let mut session = LayoutSession::new(&engine);
    let mut at = |what: &str| laid_out_as_a_full_layout_is(&engine, &doc, &mut session, what);
    assert!(at("plain").is_empty());
    for schema in [builtin::NOTE, builtin::FLOAT] {
        let id = owned_by(&engine, &doc, schema.clone(), table, anchor);
        doc.commit();
        assert!(at("owner").is_empty(), "{schema:?}");
        doc.delete_relation(id).unwrap();
        doc.commit();
        assert!(at("owner removed").is_empty(), "{schema:?}");
    }
}
