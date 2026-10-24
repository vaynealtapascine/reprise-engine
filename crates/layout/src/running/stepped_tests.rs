//! S7c: the regions of a page are made in units of their own, under the job's
//! budget (sections §11, "Scheduling"). A page's regions used to be composed
//! inside the unit that made the page, and the first page's inside
//! `LayoutJob::new`, so a budget of 1 bounded neither.
use super::compose::{DIGEST_CHUNK, MAX_ENTRIES};
use super::field_tests::{P, story_of};
use super::layout_tests::*;
use crate::incremental::{JobError, LayoutSession, Pass, StepWork, Viewport, WorkCounters};
use crate::{Engine, LayoutSnapshot, PageKind};
use reprise_doc::sections::BreakKind;
use reprise_doc::{Document, NodeId};
use serde_json::json;

/// What a step charged, what it did, and whether it ran the region
/// allocation, which composes a whole document's notes and floats in one unit
/// of its own.
type Work = (usize, StepWork, bool);

/// Runs a job to the end `budget` units at a time, with the work of each step.
fn stepped(
    engine: &Engine,
    doc: &Document,
    budget: usize,
) -> (LayoutSnapshot, Vec<Work>, WorkCounters) {
    stepped_on(&mut LayoutSession::new(engine), doc, budget)
}

/// As [`stepped`], on a session that may have laid the document out before.
fn stepped_on(
    session: &mut LayoutSession<'_>,
    doc: &Document,
    budget: usize,
) -> (LayoutSnapshot, Vec<Work>, WorkCounters) {
    let mut job = session.start(doc, Viewport::Pages(0..usize::MAX));
    let mut works = Vec::new();
    let mut allocating = false;
    for _ in 0..4_000_000 {
        let step = job.step(budget).unwrap();
        works.push((step.used, job.step_work(), allocating));
        allocating = step.pass == Pass::RegionFeedback;
        if step.complete {
            let snapshot = job.complete().unwrap().unwrap();
            return (snapshot, works, job.counters());
        }
    }
    panic!("a job of budget {budget} did not finish");
}

/// Every step charged at most the budget and did a bounded amount of region
/// work, and the flow never made a composition itself.
fn assert_bounded(works: &[Work], counters: &WorkCounters, budget: usize) {
    assert_bounded_steps(works, budget);
    assert_eq!(counters.running_inline, 0, "{counters:?}");
}

/// The per-step bounds alone: what a job that had to fall back to composing
/// regions inline (see the hostile tests at the end) still holds to.
fn assert_bounded_steps(works: &[Work], budget: usize) {
    for (i, (used, w, allocating)) in works.iter().enumerate() {
        assert!(*used <= budget.max(1), "step {i} charged {used}: {w:?}");
        if budget == 1 {
            assert!(w.region_blocks <= 1, "step {i}: {w:?}");
            assert!(w.digest_blocks <= DIGEST_CHUNK, "step {i}: {w:?}");
            assert!(w.pages_made <= 1, "step {i}: {w:?}");
            if *allocating {
                continue;
            }
            assert!(w.composer_calls <= 1, "step {i}: {w:?}");
            // A body block prepares its short neighbours with it (the flow's
            // speculation); a region unit shapes one request at most.
            if w.region_units > 0 {
                assert!(w.adapter_requests <= 1, "step {i}: {w:?}");
            }
        }
    }
}

fn all_budgets(engine: &Engine, doc: &Document) -> LayoutSnapshot {
    let expected = engine.layout(doc);
    for budget in [1usize, 2, 3, 7, 64, usize::MAX / 2] {
        let (snapshot, works, counters) = stepped(engine, doc, budget);
        assert_eq!(snapshot, expected, "budget {budget}");
        assert_bounded(&works, &counters, budget);
    }
    expected
}

/// A header, a footer and a background on `count` one-paragraph pages.
fn furnished(count: usize) -> (Engine, Document, NodeId, NodeId) {
    let (engine, doc) = setup();
    let head = story(&doc, &["Running head", "second paragraph of the head"]);
    let foot = story(&doc, &["Footer"]);
    let mark = story(&doc, &["DRAFT"]);
    first(
        &doc,
        &[
            ("region.header.default", bind(head)),
            ("region.footer.default", bind(foot)),
            ("region.background.default", bind(mark)),
        ],
    );
    pages(&doc, count);
    doc.commit();
    (engine, doc, head, foot)
}

#[test]
fn a_job_composes_nothing_until_it_is_stepped() {
    let (engine, doc, _, _) = furnished(3);
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..10));
    // `LayoutJob::new` used to make the first page, and with it three
    // compositions.
    let c = job.counters();
    assert_eq!((c.running_compositions, c.running_inline), (0, 0), "{c:?}");
    let view = job.partial().unwrap();
    assert!(view.snapshot().pages.is_empty(), "no page before a unit");
    let before = view.snapshot().to_json();
    // Budget 0 is a checked no-op.
    assert_eq!(job.step(0).unwrap().used, 0);
    assert_eq!(before, job.partial().unwrap().snapshot().to_json());
    assert_eq!(job.counters().running_compositions, 0);
    // One unit is a bounded piece of the first region, not the first page.
    let step = job.step(1).unwrap();
    assert_eq!(step.used, 1);
    let w = job.step_work();
    assert_eq!(w.pages_made, 0, "{w:?}");
    assert_eq!(job.counters().running_compositions, 0);
    assert!(job.partial().unwrap().snapshot().pages.is_empty());
    // And it gets there.
    let mut steps = 1;
    while !job.step(1).unwrap().complete {
        steps += 1;
        assert!(steps < 100_000);
    }
    let c = job.counters();
    assert_eq!(
        c.running_compositions, 3,
        "header, footer, background: {c:?}"
    );
    assert_eq!(c.running_inline, 0);
}

#[test]
fn each_step_does_a_bounded_amount_of_region_work() {
    let (engine, doc, _, _) = furnished(5);
    let (snapshot, works, counters) = stepped(&engine, &doc, 1);
    assert_eq!(snapshot, engine.layout(&doc));
    assert_bounded(&works, &counters, 1);
    // The work was done, and it was counted.
    let units: usize = works.iter().map(|(_, w, _)| w.region_units).sum();
    let blocks: usize = works.iter().map(|(_, w, _)| w.region_blocks).sum();
    let digests: usize = works.iter().map(|(_, w, _)| w.digest_blocks).sum();
    let pages: usize = works.iter().map(|(_, w, _)| w.pages_made).sum();
    assert!(units >= 3, "{units}");
    assert_eq!(
        blocks,
        2 + 1 + 1,
        "the head's two paragraphs, the footer, the mark"
    );
    assert_eq!(digests, 4, "four story blocks read once");
    assert_eq!(pages, 5);
    assert_eq!(counters.running_compositions, 3);
}

#[test]
fn tall_headers_and_footers_step_to_the_same_snapshot() {
    let (engine, doc) = setup();
    let tall = "word ".repeat(4000);
    let head = story(&doc, &[&tall, "second"]);
    let foot = story(&doc, &[&tall]);
    first(
        &doc,
        &[
            ("region.header.default", bind(head)),
            ("region.footer.default", bind(foot)),
        ],
    );
    pages(&doc, 3);
    doc.commit();
    let s = all_budgets(&engine, &doc);
    assert!(instances(&s, "header").iter().all(|r| r.clipped));
}

#[test]
fn one_story_in_several_slots_and_pages_steps_to_the_same_snapshot() {
    let (engine, doc) = setup();
    let both = story_of(
        &doc,
        &[
            P::T("page "),
            P::F("page-number"),
            P::T(" of "),
            P::F("page-count"),
        ],
    );
    let mark = story(&doc, &["WATERMARK"]);
    first(
        &doc,
        &[
            ("region.header.default", bind(both)),
            ("region.footer.default", bind(both)),
            ("region.background.default", bind(mark)),
        ],
    );
    pages(&doc, 14);
    doc.commit();
    let s = all_budgets(&engine, &doc);
    assert_eq!(instances(&s, "header").len(), 14);
}

#[test]
fn a_hundred_thousand_paragraph_story_steps_to_the_same_snapshot() {
    let (engine, doc) = setup();
    let texts: Vec<String> = (0..100_000).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let head = story(&doc, &refs);
    first(&doc, &[("region.header.default", bind(head))]);
    pages(&doc, 2);
    doc.commit();
    let expected = engine.layout(&doc);
    // Budgets 1 and 64 only: the story is read 64 blocks a unit.
    for budget in [1usize, 64] {
        let (snapshot, works, counters) = stepped(&engine, &doc, budget);
        assert_eq!(snapshot, expected, "budget {budget}");
        assert_bounded(&works, &counters, budget);
        let read: usize = works.iter().map(|(_, w, _)| w.digest_blocks).sum();
        assert_eq!(read, 4096, "the bounded prefix, once");
    }
    assert!(instances(&expected, "header").iter().all(|r| r.clipped));
}

#[test]
fn a_one_mebibyte_paragraph_steps_to_the_same_snapshot() {
    let (engine, doc) = setup();
    let big = "word ".repeat((1 << 20) / 5);
    let head = story(&doc, &[&big]);
    first(&doc, &[("region.header.default", bind(head))]);
    pages(&doc, 2);
    doc.commit();
    let expected = engine.layout(&doc);
    let (snapshot, works, counters) = stepped(&engine, &doc, 1);
    assert_eq!(snapshot, expected);
    assert_bounded(&works, &counters, 1);
    // The paragraph is shaped in chunks, a request a unit.
    let shaped = works.iter().map(|(_, w, _)| w.shaped_bytes).max().unwrap();
    assert!(shaped <= crate::incremental::SHAPE_CHUNK_BYTES, "{shaped}");
    let requests: usize = works.iter().map(|(_, w, _)| w.adapter_requests).sum();
    assert!(requests > 32, "{requests}");
}

#[test]
fn an_empty_body_still_gets_its_one_page_and_its_regions() {
    let (engine, doc) = setup();
    let head = story(&doc, &["HEAD"]);
    first(&doc, &[("region.header.default", bind(head))]);
    doc.commit();
    let s = all_budgets(&engine, &doc);
    assert_eq!(s.pages.len(), 1);
    assert_eq!(instances(&s, "header").len(), 1);
}

#[test]
fn starts_and_fillers_make_one_page_a_unit() {
    let (engine, doc) = setup();
    let head = story_of(&doc, &[P::T("p"), P::F("page-number")]);
    first(
        &doc,
        &[
            ("region.header.default", bind(head)),
            ("region.header.even", bind(head)),
            ("region.even", json!(true)),
        ],
    );
    let nodes: Vec<_> = (0..6).map(|i| body(&doc, &format!("s{i}"))).collect();
    // An odd-page start after one page makes a filler, then the content page;
    // a continuous start and a column break make none.
    boundary(
        &doc,
        nodes[1],
        BreakKind::Section,
        &[("start", json!("even-page"))],
    );
    boundary(&doc, nodes[2], BreakKind::Page, &[]);
    boundary(
        &doc,
        nodes[3],
        BreakKind::Section,
        &[
            ("start", json!("odd-page")),
            ("filler.regions", json!("running")),
        ],
    );
    boundary(
        &doc,
        nodes[4],
        BreakKind::Section,
        &[("start", json!("continuous"))],
    );
    boundary(&doc, nodes[5], BreakKind::Column, &[]);
    doc.commit();
    let s = all_budgets(&engine, &doc);
    let kinds: Vec<_> = s
        .sections
        .as_ref()
        .unwrap()
        .pages
        .iter()
        .map(|p| p.kind)
        .collect();
    assert!(kinds.contains(&PageKind::Filler), "{kinds:?}");
}

#[test]
fn cancellation_mid_region_rejects_everything() {
    let (engine, doc) = setup();
    let texts: Vec<String> = (0..300).map(|i| format!("head line {i}")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let head = story(&doc, &refs);
    first(&doc, &[("region.header.default", bind(head))]);
    pages(&doc, 3);
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..1000));
    let mut blocks = 0;
    for _ in 0..100_000 {
        job.step(1).unwrap();
        blocks += job.step_work().region_blocks;
        // Inside the header's composition: blocks started, none finished.
        if blocks >= 5 {
            break;
        }
    }
    assert!(blocks >= 5);
    assert_eq!(
        job.counters().running_compositions,
        0,
        "mid-way through the region"
    );
    assert!(job.partial().unwrap().snapshot().pages.is_empty());
    job.cancel();
    assert!(matches!(job.step(1), Err(JobError::Cancelled)));
    assert!(matches!(job.partial(), Err(JobError::Cancelled)));
    assert!(matches!(job.complete(), Err(JobError::Cancelled)));
    // The session lays out the same document afterwards, whole.
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
}

#[test]
fn staleness_mid_region_rejects_the_job_and_leaves_nothing_behind() {
    let (engine, doc) = setup();
    let texts: Vec<String> = (0..300).map(|i| format!("head line {i}")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let head = story(&doc, &refs);
    first(&doc, &[("region.header.default", bind(head))]);
    pages(&doc, 3);
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    let mut job = session.start(&doc, Viewport::Pages(0..1000));
    let mut blocks = 0;
    for _ in 0..100_000 {
        if blocks >= 5 {
            break;
        }
        job.step(1).unwrap();
        blocks += job.step_work().region_blocks;
    }
    assert!(blocks >= 5, "the job reached the middle of the region");
    assert_eq!(job.counters().running_compositions, 0);
    // The story changes under the half-made region.
    let first_block = doc.top(reprise_doc::Domain::Story(head))[0];
    doc.block(first_block)
        .unwrap()
        .text
        .insert(0, "EDITED ")
        .unwrap();
    doc.commit();
    assert!(matches!(job.step(1), Err(JobError::Stale)));
    assert!(matches!(job.partial(), Err(JobError::Stale)));
    assert!(matches!(job.complete(), Err(JobError::Stale)));
    // The next job starts clean: nothing of the half-made composition is
    // reused, and the edit shows on every page.
    let fresh = session.layout(&doc).unwrap();
    assert_eq!(fresh, engine.layout(&doc));
    assert!(
        instances(&fresh, "header")
            .iter()
            .all(|r| text_of(r).starts_with("EDITED head line 0")),
    );
}

#[test]
fn a_memoised_transition_waits_for_its_regions_and_is_still_reused() {
    // Paragraphs that overflow into new pages make those pages themselves, so
    // their memoised transitions are judged by what the regions of those pages
    // come to. With the page's number in the header each page has regions of
    // its own, which have to be made before the paragraph can be reused.
    for numbered in [false, true] {
        for budget in [1usize, 5] {
            let (engine, doc) = setup();
            let head = if numbered {
                story_of(&doc, &[P::T("head "), P::F("page-number")])
            } else {
                story(&doc, &["Running head"])
            };
            first(&doc, &[("region.header.default", bind(head))]);
            // Each paragraph runs over about three pages.
            for i in 0..8 {
                body(
                    &doc,
                    &format!("paragraph {i}: {}", "overflowing text ".repeat(700)),
                );
            }
            doc.commit();
            let mut warm = LayoutSession::new(&engine);
            let before = warm.layout(&doc).unwrap();
            assert!(before.pages.len() >= 16, "{} pages", before.pages.len());
            // The header's text changes and its height does not: every body
            // transition is reused.
            let block = doc.top(reprise_doc::Domain::Story(head))[0];
            doc.block(block).unwrap().text.insert(0, "Y").unwrap();
            doc.commit();
            let mut job = warm.start(&doc, Viewport::Pages(0..usize::MAX));
            let mut works = Vec::new();
            let mut allocating = false;
            for _ in 0..1_000_000 {
                let step = job.step(budget).unwrap();
                works.push((step.used, job.step_work(), allocating));
                allocating = step.pass == Pass::RegionFeedback;
                if step.complete {
                    break;
                }
            }
            let snapshot = job.complete().unwrap().unwrap();
            let c = job.counters();
            let what = format!(
                "numbered {numbered}, budget {budget}, {} pages: {c:?}",
                snapshot.pages.len()
            );
            assert_eq!(snapshot, engine.layout(&doc), "{what}");
            assert_eq!(c.compositions, 0, "bodies are reused: {what}");
            // A header with the page's number is a composition per page, and
            // the feedback pass flows the document a second time.
            if numbered {
                assert!(c.running_compositions >= snapshot.pages.len(), "{what}");
            } else {
                assert_eq!(c.running_compositions, 1, "{what}");
            }
            assert_bounded(&works, &c, budget);
        }
    }
}

#[test]
fn text_that_overflows_into_new_pages_finds_each_pages_regions_ready() {
    let (engine, doc) = setup();
    // Every page's header and footer read its own number, so each page needs
    // regions of its own, made before the paragraph that overflows into it.
    let head = story_of(&doc, &[P::T("head "), P::F("page-number")]);
    let foot = story_of(
        &doc,
        &[P::F("page-number"), P::T(" of "), P::F("page-count")],
    );
    first(
        &doc,
        &[
            ("region.header.default", bind(head)),
            ("region.footer.default", bind(foot)),
        ],
    );
    for i in 0..120 {
        body(
            &doc,
            &format!("paragraph {i}: {}", "overflowing text ".repeat(30)),
        );
    }
    doc.commit();
    let s = all_budgets(&engine, &doc);
    assert!(s.pages.len() >= 8, "{} pages", s.pages.len());
    let numbers: Vec<_> = (0..s.pages.len())
        .map(|p| super::field_tests::shown(super::field_tests::on(&s, p, "header")))
        .collect();
    assert_eq!(numbers[3], ["4"], "each page shows its own number");
}

#[test]
fn pages_the_region_allocation_makes_have_their_regions_made_by_the_look_ahead() {
    use reprise_doc::relation::builtin;
    use reprise_doc::text::RangePolicy;
    use reprise_doc::{
        BlockKind, Dim, FrameRole, FrameTemplate, PageTemplate, Param, Relation, Target,
    };
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(120))
            .with_frame(FrameTemplate::new(
                "main",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(200), Dim::pt(120)),
            ))
            .with_frame(FrameTemplate::new(
                "notes",
                FrameRole::Notes,
                (Dim::pt(0), Dim::pt(30)),
                (Dim::pt(200), Dim::pt(90)),
            )),
    )
    .unwrap();
    let head = story_of(&doc, &[P::T("head "), P::F("page-number")]);
    first(
        &doc,
        &[
            ("header.distance", json!(0)),
            ("region.header.default", bind(head)),
        ],
    );
    let a = body(&doc, "a");
    // A note far longer than a page: its continuation pages are made by the
    // region allocation, each with a header of its own number.
    let note = doc
        .append_block(
            BlockKind::Annotation,
            "s",
            &vec!["note"; 150].join(
                "
",
            ),
        )
        .unwrap();
    let range = doc.add_range(a, 0..1, RangePolicy::FIXED).unwrap();
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
    let pages = expected.pages.len();
    assert!(pages >= 20, "{pages} pages");
    assert!(
        expected
            .sections
            .as_ref()
            .unwrap()
            .pages
            .iter()
            .any(|p| p.kind == PageKind::NoteContinuation)
    );
    for budget in [1usize, 7] {
        let (snapshot, works, counters) = stepped(&engine, &doc, budget);
        assert_eq!(snapshot, expected, "budget {budget}");
        assert_bounded(&works, &counters, budget);
        // Every page's header was made by the look-ahead, in units.
        assert!(counters.region_passes >= 1);
        let units: usize = works.iter().map(|(_, w, _)| w.region_units).sum();
        assert!(units >= pages, "{units} region units for {pages} pages");
    }
}

// Hostile tests of the paths where a stepped job could compose a region inline,
// whole. `Cursor::look` lets a page make a region itself when the look-ahead
// made it and the memo still does not have it (an eviction); that is correct
// and counted in `running_inline`, and the tests that provoke it with a memo
// that evicts everything assert it is nonzero. Region feedback never does: the
// allocation asks for every composition it lacks, the look-ahead makes them a
// unit at a time and holds them for it, and a need that comes back after being
// made (the memo lost it under the job) ends the job. A job whose memo was
// taken by another cannot be resumed at all (`incremental_tests.rs`).

/// A note far longer than a page, on a page template with a notes area, under a
/// header showing its page number: every continuation page the region
/// allocation makes needs a header of its own.
fn endless_note(lines: usize) -> (Engine, Document) {
    use reprise_doc::relation::builtin;
    use reprise_doc::text::RangePolicy;
    use reprise_doc::{
        BlockKind, Dim, FrameRole, FrameTemplate, PageTemplate, Param, Relation, Target,
    };
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(120))
            .with_frame(FrameTemplate::new(
                "main",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(200), Dim::pt(120)),
            ))
            .with_frame(FrameTemplate::new(
                "notes",
                FrameRole::Notes,
                (Dim::pt(0), Dim::pt(30)),
                (Dim::pt(200), Dim::pt(90)),
            )),
    )
    .unwrap();
    let head = story_of(&doc, &[P::T("head "), P::F("page-number")]);
    first(
        &doc,
        &[
            ("header.distance", json!(0)),
            ("region.header.default", bind(head)),
        ],
    );
    let a = body(&doc, "a");
    let note = doc
        .append_block(
            BlockKind::Annotation,
            "s",
            &vec!["note"; lines].join(
                "
",
            ),
        )
        .unwrap();
    let range = doc.add_range(a, 0..1, RangePolicy::FIXED).unwrap();
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

/// Runs a job on `session` at budget 1 up to the middle of region feedback:
/// the first step after which `held` compositions are held for the allocation.
fn into_feedback<'j, 'e>(
    session: &'j mut LayoutSession<'e>,
    doc: &'j Document,
    held: usize,
) -> crate::incremental::LayoutJob<'j, 'e> {
    let memo = session.running_memo();
    let mut job = session.start(doc, Viewport::Pages(0..usize::MAX));
    for _ in 0..4_000_000 {
        job.step(1).unwrap();
        if memo.held_count() >= held {
            return job;
        }
    }
    panic!("the job never held {held} compositions");
}

#[test]
fn region_feedback_over_more_pages_than_the_memo_keeps_composes_nothing_inline() {
    // Continuation pages the region allocation makes, each with a header of its
    // own number: the allocation asks for every one of them in a pass, they are
    // made a unit at a time and held, past the memo's 64, until an allocation
    // finds them all. Nothing is composed inside the allocation unit.
    let (engine, doc) = endless_note(700);
    let expected = engine.layout(&doc);
    let pages = expected.pages.len();
    assert!(pages > MAX_ENTRIES + 8, "{pages} pages");
    for budget in [1usize, 7] {
        let mut session = LayoutSession::new(&engine);
        let memo = session.running_memo();
        let mut job = session.start(&doc, Viewport::Pages(0..usize::MAX));
        let (mut works, mut made, mut most_held) = (Vec::new(), 0usize, 0usize);
        let mut allocating = false;
        let snapshot = loop {
            let step = job.step(budget).unwrap();
            let work = job.step_work();
            let c = job.counters();
            // A composition is finished by a look-ahead unit, one a step at
            // most for a budget of 1, and never by the flow or the allocation.
            let delta = c.running_compositions - made;
            made = c.running_compositions;
            assert!(delta == 0 || work.region_units > 0, "{work:?}");
            if budget == 1 {
                assert!(delta <= 1, "{delta} compositions in one unit: {work:?}");
            }
            assert_eq!(c.running_inline, 0, "{c:?}");
            most_held = most_held.max(memo.held_count());
            works.push((step.used, work, allocating));
            allocating = step.pass == Pass::RegionFeedback;
            if step.complete {
                break job.complete().unwrap().unwrap();
            }
        };
        assert_eq!(snapshot, expected, "budget {budget}");
        assert_bounded_steps(&works, budget);
        // More compositions were kept for the allocation than the memo holds,
        // and all were handed back to it when the allocation had them.
        assert!(most_held > MAX_ENTRIES, "{most_held} held at most");
        assert_eq!(memo.held_count(), 0);
        assert_eq!(job.counters().running_inline, 0);
    }
}

#[test]
fn cancellation_in_region_feedback_leaves_nothing_held_for_the_next_job() {
    let (engine, doc) = endless_note(700);
    let expected = engine.layout(&doc);
    let mut session = LayoutSession::new(&engine);
    let memo = session.running_memo();
    let mut job = into_feedback(&mut session, &doc, 10);
    assert!(memo.held_count() >= 10);
    job.cancel();
    assert!(matches!(job.step(1), Err(JobError::Cancelled)));
    assert!(matches!(job.partial(), Err(JobError::Cancelled)));
    assert!(matches!(job.complete(), Err(JobError::Cancelled)));
    drop(job);
    // The next job starts clean, and lays out what a plain layout does.
    assert_eq!(session.layout(&doc).unwrap(), expected);
    assert_eq!(memo.held_count(), 0);
}

#[test]
fn staleness_in_region_feedback_rejects_the_job_and_forgets_what_was_held() {
    let (engine, doc) = endless_note(700);
    let before = engine.layout(&doc);
    let mut session = LayoutSession::new(&engine);
    let memo = session.running_memo();
    let mut job = into_feedback(&mut session, &doc, 10);
    assert!(memo.held_count() >= 10);
    // The header changes under the compositions held for the allocation.
    let head = doc.stories()[0];
    let block = doc.top(reprise_doc::Domain::Story(head))[0];
    doc.block(block).unwrap().text.insert(0, "EDITED ").unwrap();
    doc.commit();
    assert!(matches!(job.step(1), Err(JobError::Stale)));
    assert!(matches!(job.partial(), Err(JobError::Stale)));
    assert!(matches!(job.complete(), Err(JobError::Stale)));
    drop(job);
    assert_eq!(memo.held_count(), 0, "a stale job keeps nothing");
    let fresh = session.layout(&doc).unwrap();
    assert_eq!(fresh, engine.layout(&doc));
    assert_ne!(fresh, before);
}

#[test]
fn a_need_that_comes_back_after_it_was_made_ends_the_job_and_composes_nothing() {
    // The memo loses what was held for the allocation after every step of
    // region feedback, so what was made is gone when the allocation runs again.
    // Waiting for the same thing again would never end, and composing it in the
    // allocation is the escape the units exist to remove: the job ends, stale,
    // and the next job (which makes its own) lays out what a plain layout does.
    let (engine, doc) = endless_note(300);
    let expected = engine.layout(&doc);
    let mut session = LayoutSession::new(&engine);
    let memo = session.running_memo();
    let mut job = session.start(&doc, Viewport::Pages(0..usize::MAX));
    let mut steps = 0usize;
    let error = loop {
        match job.step(1) {
            Ok(step) => assert!(!step.complete, "a job that lost what it made finished"),
            Err(error) => break error,
        }
        steps += 1;
        assert!(steps < 100_000, "the job did not end");
        memo.lose_held();
    };
    assert_eq!(error, JobError::Stale);
    assert_eq!(job.counters().running_inline, 0);
    assert!(matches!(job.partial(), Err(JobError::Stale)));
    assert!(matches!(job.complete(), Err(JobError::Stale)));
    assert!(matches!(job.step(1), Err(JobError::Stale)));
    drop(job);
    assert_eq!(session.layout(&doc).unwrap(), expected);
    assert_eq!(session.counters().running_inline, 0);
}

#[test]
fn a_job_whose_holds_another_job_took_cannot_resume_and_composes_nothing() {
    // The compositions held for an allocation are the session's memo's, and
    // starting a job drops them. A continuation suspended in the middle of
    // region feedback has lost what it was waiting for; resuming it would have
    // its allocation compose the whole of it inline in one step. It is refused
    // instead, however unchanged the document is, and the memo it leaves is the
    // newer job's.
    let (engine, doc) = endless_note(700);
    let expected = engine.layout(&doc);
    let mut session = LayoutSession::new(&engine);
    let memo = session.running_memo();
    let state = into_feedback(&mut session, &doc, 10).suspend();
    assert!(memo.held_count() >= 10);
    // Starting a job, and dropping it at once, is enough.
    drop(session.start(&doc, Viewport::Pages(0..1)));
    assert_eq!(memo.held_count(), 0);
    assert!(matches!(session.resume(&doc, state), Err(JobError::Stale)));
    // Of two jobs started, the earlier one's continuation is the stale one.
    let first = session
        .start(&doc, Viewport::Pages(0..usize::MAX))
        .suspend();
    let second = session
        .start(&doc, Viewport::Pages(0..usize::MAX))
        .suspend();
    assert!(matches!(session.resume(&doc, first), Err(JobError::Stale)));
    // Refusing the old one took nothing from the newer: it runs to the end.
    let mut state = second;
    let snapshot = loop {
        let mut job = session.resume(&doc, state).unwrap();
        if job.step(1).unwrap().complete {
            break job.complete().unwrap().unwrap();
        }
        assert_eq!(job.counters().running_inline, 0);
        state = job.suspend();
    };
    assert_eq!(snapshot, expected);
    assert_eq!(session.counters().running_inline, 0);
}

#[test]
fn a_continuation_resumes_only_on_the_session_whose_memo_it_was_made_under() {
    let (engine, doc) = endless_note(300);
    let expected = engine.layout(&doc);
    let mut session = LayoutSession::new(&engine);
    let state = into_feedback(&mut session, &doc, 5).suspend();
    // Another session has none of its digests or holds.
    let mut other = LayoutSession::new(&engine);
    assert!(matches!(other.resume(&doc, state), Err(JobError::Stale)));
    // A layout is a job of its own and takes the memo as well.
    let state = into_feedback(&mut session, &doc, 5).suspend();
    assert_eq!(session.layout(&doc).unwrap(), expected);
    assert!(matches!(session.resume(&doc, state), Err(JobError::Stale)));
    // The memo carried into a new session (a host that cannot keep the
    // borrowed one) is the same memo: the continuation goes on there.
    let mut session = LayoutSession::new(&engine);
    let mut state = into_feedback(&mut session, &doc, 5).suspend();
    let mut session = LayoutSession::from_cache(&engine, session.into_cache());
    let snapshot = loop {
        let mut job = session.resume(&doc, state).unwrap();
        if job.step(3).unwrap().complete {
            break job.complete().unwrap().unwrap();
        }
        state = job.suspend();
    };
    assert_eq!(snapshot, expected);
    assert_eq!(session.counters().running_inline, 0);
}

#[test]
fn compositions_evicted_from_the_memo_are_made_again_in_units_not_inline() {
    // The memo keeps `MAX_ENTRIES` compositions of a story, and a header that
    // shows its page's number has one a page. So after a layout of more pages
    // than that, the early pages' compositions are gone from the memo while the
    // story's digest is unchanged. An edit at the top replays the transitions
    // after it, and each needs its pages' regions to judge that: those the memo
    // has lost are made again by the look-ahead, a unit at a time, before the
    // block that needs them, and not by the flow in the middle of a unit.
    let (engine, doc) = setup();
    let head = story_of(&doc, &[P::T("head "), P::F("page-number")]);
    first(&doc, &[("region.header.default", bind(head))]);
    let blocks: Vec<_> = (0..40)
        .map(|i| {
            body(
                &doc,
                &format!("paragraph {i}: {}", "overflowing text ".repeat(700)),
            )
        })
        .collect();
    doc.commit();
    let mut warm = LayoutSession::new(&engine);
    let pages = warm.layout(&doc).unwrap().pages.len();
    assert!(pages > MAX_ENTRIES + 8, "{pages} pages");
    // The edit keeps the body's shape: the same pages, and the same heights.
    let end = doc.block(blocks[0]).unwrap().text.len();
    doc.block(blocks[0]).unwrap().text.insert(end, ".").unwrap();
    doc.commit();
    let expected = engine.layout(&doc);
    assert_eq!(
        expected.pages.len(),
        pages,
        "the edit changed the page count"
    );
    let (snapshot, works, counters) = stepped_on(&mut warm, &doc, 1);
    assert_eq!(snapshot, expected);
    assert_bounded(&works, &counters, 1);
    // The memo kept what it could: some of its compositions served this job.
    assert!(counters.running_reused > 0, "{counters:?}");
    // What it had lost was made again. The flow confirms its fields with a
    // second pass, which finds the 64-entry memo flooded by the first when the
    // pages outnumber it, so a page's header is made at most twice.
    assert!(
        counters.running_compositions >= pages - MAX_ENTRIES,
        "{} compositions for {pages} pages: {counters:?}",
        counters.running_compositions
    );
    assert!(
        counters.running_compositions <= 2 * pages + 2,
        "{} compositions for {pages} pages: {counters:?}",
        counters.running_compositions
    );
    let units: usize = works.iter().map(|(_, w, _)| w.region_units).sum();
    assert!(
        units >= counters.running_compositions,
        "{units} region units: {counters:?}"
    );
}

/// What a job that loses its memo's compositions after every step did.
struct Evicted {
    snapshot: LayoutSnapshot,
    /// Each step's work, and whether the flow composed a region in it.
    steps: Vec<(Work, bool)>,
    counters: WorkCounters,
}

/// Runs a job on `session` to the end at budget 1, emptying the running memo
/// of its compositions after every step. The look-ahead then makes a story,
/// installs it, and finds it gone: the state a memo at its cap leaves it in
/// when other requests of the same story arrive first.
fn stepped_evicting(session: &mut LayoutSession<'_>, doc: &Document, limit: usize) -> Evicted {
    let memo = session.running_memo();
    let mut job = session.start(doc, Viewport::Pages(0..usize::MAX));
    let mut steps = Vec::new();
    let mut allocating = false;
    let mut inline = 0;
    for _ in 0..limit {
        let step = job.step(1).unwrap();
        let now = job.counters().running_inline;
        steps.push(((step.used, job.step_work(), allocating), now > inline));
        inline = now;
        allocating = step.pass == Pass::RegionFeedback;
        if step.complete {
            let snapshot = job.complete().unwrap().unwrap();
            return Evicted {
                snapshot,
                steps,
                counters: job.counters(),
            };
        }
        memo.evict_compositions();
    }
    panic!("a job whose memo loses every composition did not finish in {limit} steps");
}

#[test]
fn a_memo_that_loses_what_the_look_ahead_just_made_does_not_loop() {
    // `Cursor::look` makes a page's regions ahead, then asks the memo again.
    // If the memo no longer has what was made, asking for the same thing a
    // second time would make it a second time, forever: the page makes its
    // regions itself instead, once, and that is counted.
    let (engine, doc) = setup();
    let head = story_of(&doc, &[P::T("head "), P::F("page-number")]);
    let foot = story_of(
        &doc,
        &[P::F("page-number"), P::T(" of "), P::F("page-count")],
    );
    first(
        &doc,
        &[
            ("region.header.default", bind(head)),
            ("region.footer.default", bind(foot)),
        ],
    );
    for i in 0..5 {
        body(
            &doc,
            &format!("paragraph {i}: {}", "overflowing text ".repeat(700)),
        );
    }
    doc.commit();
    let expected = engine.layout(&doc);
    let pages = expected.pages.len();
    assert!(pages >= 8, "{pages} pages");
    let (_, plain, plain_counters) = stepped(&engine, &doc, 1);
    assert_eq!(plain_counters.running_inline, 0);
    // Without the stall check the job never finishes: the limit is a few times
    // what a job whose memo keeps what it is given needs.
    let mut session = LayoutSession::new(&engine);
    let evicted = stepped_evicting(&mut session, &doc, 3 * plain.len() + 1_000);
    assert_eq!(evicted.snapshot, expected);
    let c = &evicted.counters;
    let made: usize = evicted
        .steps
        .iter()
        .map(|((_, w, _), _)| w.pages_made)
        .sum();
    // The fallback ran, and was counted: no more than the header and the footer
    // of each page that was made.
    assert!(c.running_inline > 0, "{c:?}");
    assert!(c.running_inline <= 2 * made, "{c:?}");
    // Only the step that made a page composed regions itself, once.
    let composing = evicted.steps.iter().filter(|(_, inline)| *inline).count();
    assert!(composing <= made, "{composing} steps, {made} pages made");
    // Every other step keeps the budget-1 bounds.
    let rest: Vec<_> = evicted
        .steps
        .iter()
        .filter(|(_, inline)| !inline)
        .map(|(work, _)| work.clone())
        .collect();
    assert_bounded_steps(&rest, 1);
}

#[test]
fn a_memoised_transition_whose_regions_keep_vanishing_is_redone_not_waited_for() {
    // A replayed transition waits for the regions of the pages it made, then
    // judges whether they are unchanged. If the memo keeps losing them, waiting
    // again would never end: the second time it is redone instead.
    let (engine, doc) = setup();
    let head = story_of(&doc, &[P::T("head "), P::F("page-number")]);
    first(&doc, &[("region.header.default", bind(head))]);
    let blocks: Vec<_> = (0..10)
        .map(|i| {
            body(
                &doc,
                &format!("paragraph {i}: {}", "overflowing text ".repeat(300)),
            )
        })
        .collect();
    doc.commit();
    // Two sessions that have laid the document out, to be edited the same way.
    let mut keeps = LayoutSession::new(&engine);
    let mut loses = LayoutSession::new(&engine);
    keeps.layout(&doc).unwrap();
    loses.layout(&doc).unwrap();
    let end = doc.block(blocks[0]).unwrap().text.len();
    doc.block(blocks[0]).unwrap().text.insert(end, ".").unwrap();
    doc.commit();
    let expected = engine.layout(&doc);
    assert!(expected.pages.len() >= 8);
    let (snapshot, plain, kept) = stepped_on(&mut keeps, &doc, 1);
    assert_eq!(snapshot, expected);
    assert_eq!(kept.running_inline, 0);
    let evicted = stepped_evicting(&mut loses, &doc, 3 * plain.len() + 1_000);
    assert_eq!(evicted.snapshot, expected);
    let lost = &evicted.counters;
    // With its memo the later paragraphs' transitions were replayed; without,
    // they were made again, and none was waited for twice.
    assert!(kept.reused_compositions > 0, "{kept:?}");
    assert!(
        lost.compositions > kept.compositions,
        "kept {kept:?}, lost {lost:?}"
    );
}
