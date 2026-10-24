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

/// Bytes of a flow host's text decoded for a unit of reading (`DECODE_UNIT_BYTES`).
const DECODE_UNIT: usize = 4096;

/// The most text of a flow host a digest decodes (`MAX_HOST_BYTES`): 1 MiB and
/// the three bytes of break marker each of 4,096 blocks adds.
const MAX_HOST_BYTES: usize = (1 << 20) + 3 * 4096;

/// The most look-ahead units one decode charges: the units of reading a host
/// of `MAX_HOST_BYTES` costs, in the 64 a look-ahead unit holds.
const MAX_DECODE_UNITS: usize = (MAX_HOST_BYTES / DECODE_UNIT).div_ceil(CHUNK);

/// The steps of a job of budget 1, as `(units the step charged, its work)`.
fn first_charged(
    engine: &reprise_layout::Engine,
    doc: &Document,
    steps: usize,
) -> Vec<(usize, StepWork)> {
    let mut session = LayoutSession::new(engine);
    let mut job = session.start(doc, Viewport::Pages(0..usize::MAX));
    let mut out = Vec::new();
    for _ in 0..steps {
        let step = job.step(1).unwrap();
        out.push((step.used, job.step_work()));
        if step.complete {
            break;
        }
    }
    out
}

/// What decoding one host, whole, may be: one atomic unit of the digest.
fn assert_an_atomic_decode(works: &[(usize, StepWork)], decode: usize, bytes: usize) {
    let (used, w) = &works[decode];
    assert!(w.digest_decoded >= bytes, "{w:?}");
    // Capped, and charged what it cost in that step: no debt for a later one.
    assert!(w.digest_decoded <= MAX_HOST_BYTES, "{w:?}");
    let cost = (w.digest_decoded / DECODE_UNIT).div_ceil(CHUNK);
    assert!(cost > 1 && cost <= MAX_DECODE_UNITS, "{cost} units: {w:?}");
    assert_eq!((*used, w.units), (cost, cost), "{w:?}");
    // Nothing else ran in it: the host's first block was taken, no other child
    // looked at, nothing composed, shaped or placed.
    assert_eq!(
        (
            w.digest_blocks,
            w.digest_inspected,
            w.region_blocks,
            w.composer_calls,
            w.adapter_requests,
            w.pages_made,
            w.table_rows,
        ),
        (1, 0, 0, 0, 0, 0, 0),
        "{w:?}"
    );
    // No other step charges more than its unit, and the story is composing two
    // steps after the decode at the latest (the digest finishing, then the
    // first block): no step is spent paying a debt, since a decode is paid for
    // when it is done, never after.
    for (i, (used, w)) in works.iter().enumerate() {
        if i != decode {
            assert_eq!(*used, 1, "step {i}: {w:?}");
        }
    }
    let composing = works[decode + 1..]
        .iter()
        .position(|(_, w)| w.region_blocks > 0)
        .expect("the story is composed");
    assert!(
        composing <= 2,
        "composing {composing} steps after the decode"
    );
}

#[test]
fn decoding_a_long_host_is_one_atomic_step_that_charges_its_cost() {
    // A paragraph of 1,000,000 bytes (within what a story reads) is decoded in
    // one go. That is the one unit that costs more than the units of a step: it
    // decodes at most `MAX_HOST_BYTES`, takes the host's first block and does
    // nothing else, and charges what it cost in that step. The steps after it
    // read on at once.
    let (engine, doc) = setup();
    story_with_a_long_paragraph(&doc, 1_000_000);
    let works = first_charged(&engine, &doc, 12);
    let decode = works
        .iter()
        .position(|(_, w)| w.digest_decoded >= 1_000_000)
        .expect("the long paragraph is decoded");
    assert_an_atomic_decode(&works[..12], decode, 1_000_000);
    // The steps before it only looked, and read nothing dear.
    assert!(works[..decode].iter().all(|(_, w)| w.digest_decoded < 100));
}

#[test]
fn the_largest_host_a_story_reads_is_decoded_in_five_units_at_most() {
    // One paragraph of the whole of a story's text: the dearest decode there is
    // short of a host of many paragraphs, and what a table cell's costs too.
    let (engine, doc) = setup();
    let story = doc
        .create_story(&[NewBlock::new(
            BlockKind::Paragraph,
            "s",
            &"z".repeat(1 << 20),
        )])
        .unwrap();
    bind_header(&doc, story);
    block(&doc, "body");
    doc.commit();
    let works = first_charged(&engine, &doc, 12);
    let decode = works
        .iter()
        .position(|(_, w)| w.digest_decoded >= 1 << 20)
        .expect("the paragraph is decoded");
    assert_an_atomic_decode(&works[..12], decode, 1 << 20);
    assert_eq!(works[decode].0, (1 << 20) / DECODE_UNIT / CHUNK);
    assert!(works[decode].0 <= MAX_DECODE_UNITS);
}

#[test]
fn a_decode_is_alone_in_its_step_whatever_the_step_had_done_before_it() {
    // The reader meets the dearest host there is, a block embedded in the
    // paragraph in front of it, after the step has spent 63 of its 64 units on
    // sixty-two paragraphs and the look at that host: with one unit left, the
    // step must stop in front of it, and the next step, which starts afresh,
    // decodes it alone. Otherwise the step costs 63 + 258 units, six.
    let (engine, doc) = setup();
    let mut blocks: Vec<NewBlock> = (0..62)
        .map(|i| NewBlock::new(BlockKind::Paragraph, "s", &format!("p{i}")))
        .collect();
    blocks.push(NewBlock::new(BlockKind::Paragraph, "s", "ab"));
    let story = doc.create_story(&blocks).unwrap();
    let host = doc.top(reprise_doc::Domain::Story(story))[62];
    doc.split_block(host, 1).unwrap();
    let long = doc
        .append_block(BlockKind::Paragraph, "s", &"z".repeat(MAX_HOST_BYTES))
        .unwrap();
    doc.move_block(long, Some(story), 63).unwrap();
    bind_header(&doc, story);
    block(&doc, "body");
    doc.commit();
    let top = doc.top(reprise_doc::Domain::Story(story));
    assert_eq!(top.len(), 65);
    assert_eq!(
        top[62..64],
        [host, long],
        "the long block is embedded in the host's flow"
    );
    let works = first_charged(&engine, &doc, 12);
    let decode = works
        .iter()
        .position(|(_, w)| w.digest_decoded >= MAX_HOST_BYTES)
        .expect("the long block is decoded");
    let (used, w) = &works[decode];
    assert_eq!(*used, MAX_DECODE_UNITS, "{w:?}");
    assert_eq!(w.units, MAX_DECODE_UNITS, "{w:?}");
    // Nothing but the decode: no look at a child, no block of the sixty-three
    // before it (the block is more text than a story reads, so none is taken
    // from it either, and the story is cut in front of it), no composition.
    assert_eq!(
        (w.digest_blocks, w.digest_inspected, w.composer_calls),
        (0, 0, 0),
        "{w:?}"
    );
    for (i, (used, w)) in works.iter().enumerate() {
        if i != decode {
            assert_eq!(*used, 1, "step {i}: {w:?}");
        }
    }
    // The steps before it read the sixty-three and nothing dear.
    assert!(
        works[..decode]
            .iter()
            .all(|(_, w)| w.digest_decoded < DECODE_UNIT)
    );
}

#[test]
fn a_cell_block_decode_is_an_atomic_step_too() {
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
    doc.append_cell_block(cell, BlockKind::Paragraph, "s", &"z".repeat(1_000_000))
        .unwrap();
    doc.move_block(table, Some(story), 0).unwrap();
    bind_header(&doc, story);
    block(&doc, "body");
    doc.commit();
    let works = first_charged(&engine, &doc, 12);
    let decode = works
        .iter()
        .position(|(_, w)| w.digest_decoded >= 1_000_000)
        .expect("the cell block is decoded");
    let (used, w) = &works[decode];
    assert!(w.digest_decoded <= MAX_HOST_BYTES, "{w:?}");
    let cost = (w.digest_decoded / DECODE_UNIT).div_ceil(CHUNK);
    assert_eq!((*used, w.units), (cost, cost), "{w:?}");
    assert!(cost > 1 && cost <= MAX_DECODE_UNITS, "{w:?}");
    // Nothing else ran in it, and every other step charged its one unit.
    assert_eq!(
        (
            w.digest_inspected,
            w.composer_calls,
            w.adapter_requests,
            w.pages_made
        ),
        (0, 0, 0, 0),
        "{w:?}"
    );
    for (i, (used, w)) in works.iter().enumerate() {
        if i != decode {
            assert_eq!(*used, 1, "step {i}: {w:?}");
        }
    }
    // The whole job is the plain layout, with its decode charged where it was.
    let (snapshot, _) = run(&engine, &doc);
    assert_eq!(snapshot, engine.layout(&doc));
}
