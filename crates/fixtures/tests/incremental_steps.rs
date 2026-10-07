//! Finer job steps and worker executors (`docs/incremental.md`, "Work units and
//! the budget" and "Native workers"). Every executor must give the same steps,
//! partial views, counters and final snapshot as `Serial`, under random budgets
//! including 1; and no single step may do more than its documented bound of
//! work, measured with counters.

use std::sync::Arc;

use reprise_doc::{BlockKind, Document, PersistenceMode};
use reprise_fixtures::{hostile, spike};
use reprise_layout::incremental::{
    Coverage, JobError, LayoutSession, PREFETCH_BLOCKS, SHAPE_CHUNK_BYTES, Step, StepWork,
    TABLE_ROW_GROUP, Viewport, WorkCounters,
};
use reprise_layout::workers::{Serial, Task, Threads, Workers};
use reprise_layout::{Engine, LayoutSnapshot};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*: deterministic on every platform.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

/// Runs every task, in a seeded shuffled order: a completion order no real
/// executor is obliged to avoid.
struct Shuffled(u64);

impl Workers for Shuffled {
    fn run<'a>(&self, tasks: Vec<Task<'a>>) {
        let mut tasks: Vec<Option<Task<'a>>> = tasks.into_iter().map(Some).collect();
        let mut rng = Rng(self.0 ^ tasks.len() as u64 ^ 0x5DEE_CE66);
        let mut order: Vec<usize> = (0..tasks.len()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, rng.below(i + 1));
        }
        for i in order {
            if let Some(task) = tasks.get_mut(i).and_then(Option::take) {
                task();
            }
        }
    }
}

fn executors() -> Vec<(String, Arc<dyn Workers>)> {
    let many = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(3, 16);
    vec![
        ("serial".into(), Arc::new(Serial)),
        ("threads(1)".into(), Arc::new(Threads(1))),
        ("threads(2)".into(), Arc::new(Threads(2))),
        (format!("threads({many})"), Arc::new(Threads(many))),
        ("shuffled".into(), Arc::new(Shuffled(0xC0FF_EE11))),
    ]
}

/// Everything a host can observe after each step.
#[derive(Debug, PartialEq)]
struct Observed {
    step: Step,
    coverage: Coverage,
    partial: LayoutSnapshot,
    counters: WorkCounters,
    work: StepWork,
}

const BUDGETS: [usize; 7] = [1, 1, 2, 3, 5, 16, 64];

/// Drives a job with seeded random budgets on `workers`, recording every step.
fn trace(
    engine: &Engine,
    doc: &Document,
    workers: Arc<dyn Workers>,
    seed: u64,
) -> (Vec<Observed>, LayoutSnapshot) {
    let mut session = LayoutSession::new(engine);
    session.set_workers(workers);
    let mut job = session.start(doc, Viewport::Pages(0..1));
    let mut rng = Rng(seed);
    let mut steps = Vec::new();
    for _ in 0..1_000_000 {
        let budget = BUDGETS[rng.below(BUDGETS.len())];
        let step = job.step(budget).unwrap();
        assert!(
            step.used <= budget && step.used >= 1,
            "{step:?} for {budget}"
        );
        let view = job.partial().unwrap();
        steps.push(Observed {
            step: step.clone(),
            coverage: view.coverage().clone(),
            partial: view.snapshot().clone(),
            counters: job.counters(),
            work: job.step_work(),
        });
        if step.complete {
            return (steps, job.complete().unwrap().unwrap());
        }
    }
    panic!("the job did not finish");
}

#[test]
fn every_executor_steps_identically_on_every_fixture() {
    let mut cases: Vec<(String, Engine, Document)> = hostile::all()
        .unwrap()
        .into_iter()
        .map(|f| (f.name.to_string(), f.engine, f.doc))
        .collect();
    cases.push((
        "spike".into(),
        reprise_fixtures::engine(),
        spike::document().unwrap().doc,
    ));
    for (i, (name, engine, doc)) in cases.iter().enumerate() {
        let seed = 0x9E37_79B9_7F4A_7C15 ^ (i as u64 + 1);
        let full = engine.layout(doc);
        let mut reference = None;
        for (label, workers) in executors() {
            let (steps, complete) = trace(engine, doc, workers, seed);
            assert_eq!(complete, full, "{name} on {label}");
            match &reference {
                None => reference = Some(steps),
                Some(expected) => assert!(
                    steps == *expected,
                    "{name}: {label} observed different steps than serial"
                ),
            }
        }
    }
}

fn long_paragraph(words: usize) -> Document {
    let doc = Document::new(1).unwrap();
    spike::define_styles(&doc).unwrap();
    let vocabulary = [
        "the ", "hallway ", "is ", "longer ", "inside ", "than ", "out ",
    ];
    let text: String = (0..words)
        .map(|i| vocabulary[i % vocabulary.len()])
        .collect();
    doc.append_block(BlockKind::Paragraph, "body", &text)
        .unwrap();
    doc.append_block(BlockKind::Paragraph, "body", "after the long one")
        .unwrap();
    doc.commit();
    doc
}

/// The per-step bound of `docs/incremental.md` for a step of `budget` units.
/// `requests_per_unit` is 1 for flow paragraphs (one chunk per unit) and the
/// cells of one row group for tables (each cell is a short preparation).
fn assert_bounded(
    work: &StepWork,
    budget: usize,
    requests_per_unit: usize,
    composer_calls_per_unit: usize,
    paragraph_bytes: usize,
    label: &str,
) {
    assert!(work.units <= budget, "{label}: {work:?}");
    // Speculation adds at most L - 1 short preparations of one chunk each.
    let requests = budget * requests_per_unit + PREFETCH_BLOCKS - 1;
    assert!(work.adapter_requests <= requests, "{label}: {work:?}");
    assert!(
        work.largest_request <= SHAPE_CHUNK_BYTES,
        "{label}: {work:?}"
    );
    assert!(
        work.shaped_bytes <= requests * SHAPE_CHUNK_BYTES,
        "{label}: {work:?}"
    );
    assert!(
        work.composer_calls <= budget * composer_calls_per_unit,
        "{label}: {work:?}"
    );
    // One whole-paragraph linear scan per unit at most: itemisation, break
    // analysis or a composer call's preprocessing (documented, not chunked).
    assert!(work.largest_scan <= paragraph_bytes, "{label}: {work:?}");
    assert!(
        work.table_rows <= budget * TABLE_ROW_GROUP,
        "{label}: {work:?}"
    );
}

#[test]
fn step_work_is_bounded_for_a_long_paragraph() {
    let engine = reprise_fixtures::engine();
    let words = std::env::var("REPRISE_LONG_PARAGRAPH_WORDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let doc = long_paragraph(words);
    let bytes = doc.block(doc.blocks()[0]).unwrap().text.to_string().len();
    assert!(bytes > 3 * SHAPE_CHUNK_BYTES, "{bytes}");
    let mut session = LayoutSession::new(&engine);
    session.set_workers(Arc::new(Threads(4)));
    let mut job = session.start(&doc, Viewport::Pages(0..1));
    let budget = 8;
    let mut units_to_first_page = None;
    let mut total = 0usize;
    let mut peak = StepWork::default();
    loop {
        let step = job.step(budget).unwrap();
        let work = job.step_work();
        assert_bounded(&work, budget, 1, 1, bytes, "long paragraph");
        // A unit scans its paragraph at most twice (break analysis, then the
        // first composer call); speculation scans only short paragraphs.
        assert!(work.scanned_bytes <= 2 * budget * bytes, "{work:?}");
        total += step.used;
        peak.adapter_requests = peak.adapter_requests.max(work.adapter_requests);
        peak.composer_calls = peak.composer_calls.max(work.composer_calls);
        peak.largest_request = peak.largest_request.max(work.largest_request);
        if step.viewport_ready && units_to_first_page.is_none() {
            units_to_first_page = Some(total);
            // The first page shows while the paragraph is still in progress.
            let view = job.partial().unwrap();
            assert!(view.coverage().pages.contains(&0));
            assert!(!view.snapshot().blocks.is_empty());
            assert!(!view.coverage().complete);
        }
        if step.complete {
            break;
        }
    }
    // The first page needs every chunk shaped (composers read the whole
    // shaped paragraph), plus itemisation, break analysis and one fragment.
    let chunks = bytes.div_ceil(SHAPE_CHUNK_BYTES);
    let first = units_to_first_page.unwrap();
    assert!(
        first <= chunks + 2 * budget,
        "{first} units for {chunks} chunks"
    );
    assert!(peak.adapter_requests > 1, "shaping batches ran: {peak:?}");
    assert!(peak.largest_request <= SHAPE_CHUNK_BYTES);
    assert_eq!(job.complete().unwrap().unwrap(), engine.layout(&doc));
}

fn long_table(rows: usize) -> Document {
    let doc = Document::new(1).unwrap();
    spike::define_styles(&doc).unwrap();
    let table = doc
        .append_table(reprise_doc::TableColumns {
            columns: (0..2)
                .map(|_| reprise_doc::Column {
                    width: reprise_doc::ColumnWidth::Proportional(1),
                })
                .collect(),
        })
        .unwrap();
    for i in 0..rows {
        let row = doc.append_table_row(table, i == 0).unwrap();
        for column in 0..2u32 {
            let cell = doc.append_table_cell(row, column).unwrap();
            doc.append_cell_block(cell, BlockKind::Paragraph, "body", &format!("row {i}"))
                .unwrap();
        }
    }
    doc.commit();
    doc
}

#[test]
fn step_work_is_bounded_for_a_five_thousand_row_table() {
    let mut engine = reprise_fixtures::engine();
    engine.flow.max_pages = 10_000;
    let doc = long_table(5_000);
    let full = engine.layout(&doc);
    for (label, workers) in [
        ("serial", Arc::new(Serial) as Arc<dyn Workers>),
        ("threads", Arc::new(Threads(3))),
    ] {
        let mut session = LayoutSession::new(&engine);
        session.set_workers(workers);
        let mut job = session.start(&doc, Viewport::Pages(0..1));
        let budget = 4;
        let mut most_rows = 0;
        let mut most_composer_calls = 0;
        loop {
            let step = job.step(budget).unwrap();
            let work = job.step_work();
            assert_bounded(&work, budget, TABLE_ROW_GROUP * 2, 2 * 4, 64, label);
            most_rows = most_rows.max(work.table_rows);
            most_composer_calls = most_composer_calls.max(work.composer_calls);
            if step.complete {
                break;
            }
        }
        assert!(most_rows > 0 && most_rows <= budget * TABLE_ROW_GROUP);
        // A placed row group composes each of its two cells, at most a few
        // times per frame it is tried in; one group per unit.
        assert!(
            most_composer_calls <= budget * 2 * 4,
            "{most_composer_calls}"
        );
        assert_eq!(job.complete().unwrap().unwrap(), full, "{label}");
    }
}

#[test]
fn edits_between_fragments_of_one_paragraph_are_stale_with_workers() {
    let engine = reprise_fixtures::engine();
    let doc = long_paragraph(20_000);
    let mut session = LayoutSession::new(&engine);
    session.set_workers(Arc::new(Threads(4)));
    let mut job = session.start(&doc, Viewport::Pages(0..1));
    // Inside the long paragraph's preparation, then inside its composition.
    for _ in 0..12 {
        job.step(1).unwrap();
    }
    let view = job.partial().unwrap();
    doc.block(doc.blocks()[0])
        .unwrap()
        .text
        .insert(0, "edited ")
        .unwrap();
    assert_eq!(job.step(1), Err(JobError::Stale));
    assert!(matches!(view.publish(&doc), Err(JobError::Stale)));
    drop(job);
    // The cleared memos still give the reference result.
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
}

#[test]
fn remote_merges_reuse_memos_for_untouched_paragraphs() {
    let engine = reprise_fixtures::engine();
    let doc = Document::new(1).unwrap();
    spike::define_styles(&doc).unwrap();
    for i in 0..200 {
        doc.append_block(BlockKind::Paragraph, "body", &format!("paragraph {i}"))
            .unwrap();
    }
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    session.set_workers(Arc::new(Threads(2)));
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
    for (round, peer) in [
        doc.fork(2).unwrap(),
        Document::import(&doc.export(PersistenceMode::History), 3).unwrap(),
    ]
    .into_iter()
    .enumerate()
    {
        let target = peer.blocks()[100 + round];
        peer.block(target).unwrap().text.insert(0, "x").unwrap();
        peer.commit();
        doc.merge(&peer).unwrap();
        doc.commit();
        assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
        let counters = session.counters();
        assert_eq!(counters.shapes, 1, "round {round}: {counters:?}");
        assert_eq!(counters.style_resolutions, 1, "round {round}: {counters:?}");
        assert_eq!(counters.compositions, 1, "round {round}: {counters:?}");
        assert_eq!(
            counters.reused_compositions, 199,
            "round {round}: {counters:?}"
        );
    }
}
