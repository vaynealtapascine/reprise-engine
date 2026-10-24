//! B3 fixes (the Astra B3 review), finding 2: reading a running story is
//! bounded work whatever its flow hosts hold. One physical child of a story
//! can stand for any number of paragraphs, and be any amount of text. The
//! digest charges what it reads (the paragraphs of a host one each, the
//! decoding of a host by its text), reads a host a piece a step, and does not
//! decode a host no story could read whole.
mod common;
use common::*;
use reprise_doc::{BlockKind, Document, NewBlock, NodeId};
use reprise_layout::LayoutSnapshot;
use reprise_layout::incremental::{LayoutSession, StepWork, Viewport};

/// The digest's units a step (`DIGEST_CHUNK`).
const CHUNK: usize = 64;

/// Runs a job of budget 1 to the end, with the work of each step.
fn run(engine: &reprise_layout::Engine, doc: &Document) -> (LayoutSnapshot, Vec<StepWork>) {
    let mut session = LayoutSession::new(engine);
    let mut job = session.start(doc, Viewport::Pages(0..usize::MAX));
    let mut works = Vec::new();
    for _ in 0..4_000_000 {
        let step = job.step(1).unwrap();
        works.push(job.step_work());
        if step.complete {
            assert_eq!(job.counters().running_inline, 0);
            return (job.complete().unwrap().unwrap(), works);
        }
    }
    panic!("the job did not finish");
}

/// The work of the first `steps` steps of a job of budget 1.
fn first_steps(engine: &reprise_layout::Engine, doc: &Document, steps: usize) -> Vec<StepWork> {
    let mut session = LayoutSession::new(engine);
    let mut job = session.start(doc, Viewport::Pages(0..usize::MAX));
    let mut works = Vec::new();
    for _ in 0..steps {
        let step = job.step(1).unwrap();
        works.push(job.step_work());
        if step.complete {
            break;
        }
    }
    works
}

fn total(works: &[StepWork], of: impl Fn(&StepWork) -> usize) -> usize {
    works.iter().map(of).sum()
}

#[test]
fn a_story_that_is_one_flow_host_of_many_paragraphs_is_read_a_chunk_a_step() {
    let (engine, doc) = setup();
    let story = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "s", "x")])
        .unwrap();
    let mut tail = doc.top(reprise_doc::Domain::Story(story))[0];
    for _ in 1..200 {
        tail = doc.split_block(tail, 0).unwrap();
    }
    bind_header(&doc, story);
    block(&doc, "body");
    doc.commit();
    assert_eq!(doc.top(reprise_doc::Domain::Story(story)).len(), 200);
    let (snapshot, works) = run(&engine, &doc);
    assert_eq!(snapshot, engine.layout(&doc));
    for (i, w) in works.iter().enumerate() {
        assert!(w.digest_blocks <= CHUNK, "step {i}: {w:?}");
        assert!(w.digest_inspected <= CHUNK, "step {i}: {w:?}");
    }
    // All two hundred paragraphs, from one physical child.
    assert_eq!(total(&works, |w| w.digest_blocks), 200);
    assert_eq!(total(&works, |w| w.digest_inspected), 1);
    assert!(works.iter().filter(|w| w.digest_blocks > 0).count() >= 200 / CHUNK);
}

/// A story of `head`, then one paragraph of `bytes` bytes, then `tail`.
fn story_with_a_long_paragraph(doc: &Document, bytes: usize) -> NodeId {
    let long = "z".repeat(bytes);
    let story = doc
        .create_story(&[
            NewBlock::new(BlockKind::Paragraph, "s", "head"),
            NewBlock::new(BlockKind::Paragraph, "s", &long),
            NewBlock::new(BlockKind::Paragraph, "s", "tail"),
        ])
        .unwrap();
    bind_header(doc, story);
    block(doc, "body");
    doc.commit();
    story
}

#[test]
fn a_host_no_story_could_read_whole_is_not_decoded() {
    // More text than the story reads (1 MiB) and the break markers its blocks
    // could add: the story is cut in front of it, and it is never decoded.
    let (engine, doc) = setup();
    story_with_a_long_paragraph(&doc, (1 << 20) + 3 * 4096 + 1);
    let (snapshot, works) = run(&engine, &doc);
    assert_eq!(snapshot, engine.layout(&doc));
    assert_eq!(total(&works, |w| w.digest_blocks), 1, "only `head` is read");
    assert!(
        total(&works, |w| w.digest_decoded) < 100,
        "{} bytes decoded",
        total(&works, |w| w.digest_decoded)
    );
}

#[test]
fn a_table_cell_holding_such_a_host_is_not_decoded_either() {
    let (engine, doc) = setup();
    let story = doc.create_story(&[]).unwrap();
    let table = doc
        .append_table(two_columns([
            reprise_doc::ColumnWidth::Content,
            reprise_doc::ColumnWidth::Content,
        ]))
        .unwrap();
    let row = doc.append_table_row(table, false).unwrap();
    let cell = doc.append_table_cell(row, 0).unwrap();
    doc.append_cell_block(cell, BlockKind::Paragraph, "s", "small")
        .unwrap();
    doc.append_cell_block(
        cell,
        BlockKind::Paragraph,
        "s",
        &"z".repeat((1 << 20) + 3 * 4096 + 1),
    )
    .unwrap();
    doc.move_block(table, Some(story), 0).unwrap();
    bind_header(&doc, story);
    block(&doc, "body");
    doc.commit();
    let (snapshot, works) = run(&engine, &doc);
    assert_eq!(snapshot, engine.layout(&doc));
    assert!(
        total(&works, |w| w.digest_decoded) < 100,
        "{} bytes decoded",
        total(&works, |w| w.digest_decoded)
    );
}

#[test]
fn decoding_a_long_host_is_a_step_of_its_own_and_is_paid_back_in_steps() {
    // A paragraph of 1,000,000 bytes (within what a story reads) is decoded in
    // one go. That costs a share of its text in units, the step that does it
    // does nothing else, and the steps after it pay the cost off before the
    // digest reads on.
    let (engine, doc) = setup();
    story_with_a_long_paragraph(&doc, 1_000_000);
    let works = first_steps(&engine, &doc, 12);
    let decode = works
        .iter()
        .position(|w| w.digest_decoded >= 1_000_000)
        .expect("the long paragraph is decoded");
    let w = &works[decode];
    assert_eq!(
        (w.digest_blocks, w.digest_inspected),
        (1, 0),
        "a step of its own: {w:?}"
    );
    // 244 units of decoding against a step of 64: the next steps read nothing.
    for (i, w) in works[decode + 1..decode + 3].iter().enumerate() {
        assert_eq!(
            (w.digest_blocks, w.digest_inspected, w.digest_decoded),
            (0, 0, 0),
            "step {} after the decode: {w:?}",
            i + 1
        );
        assert_eq!(w.region_units, 1, "{w:?}");
    }
    // The steps before it read `head`, one block, and nothing dear.
    assert!(works[..decode].iter().all(|w| w.digest_decoded < 100));
}
