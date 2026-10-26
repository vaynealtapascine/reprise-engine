//! Astra's review of Stage B part 2, finding 5: a running story reads at most
//! 4,096 blocks and 1 MiB of text, and a table's rows, cells and cell blocks
//! are part of that budget, charged before any of them is prepared for
//! composition. Reading a story is also bounded work: a story of 100,000
//! blocks costs what its first 4,096 do, not what its length does.
mod common;
use common::*;
use reprise_doc::{BlockKind, Document, NewBlock, NodeId};
use reprise_layout::Engine;
use reprise_layout::incremental::{LayoutSession, StepWork, Viewport};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The most paragraphs the shaper was asked to shape at once, and how many
/// requests began with `prefix`.
struct Spy {
    largest: Arc<AtomicUsize>,
    prefixed: Arc<AtomicUsize>,
    prefix: &'static str,
}

impl reprise_shape::ShapingAdapter for Spy {
    fn info(&self) -> reprise_shape::AdapterInfo {
        reprise_shape::HarfRust.info()
    }
    fn shape(&self, r: &reprise_shape::ShapeRequest<'_>) -> Vec<reprise_shape::ShapedGlyph> {
        self.largest.fetch_max(r.text.len(), Ordering::SeqCst);
        if r.text.starts_with(self.prefix) {
            self.prefixed.fetch_add(1, Ordering::SeqCst);
        }
        // Avoid shaping the hostile megabyte; observe the forbidden read itself.
        if r.text.len() > (1 << 20) {
            Vec::new()
        } else {
            reprise_shape::HarfRust.shape(r)
        }
    }
}

fn spied(engine: &mut Engine, prefix: &'static str) -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let largest = Arc::new(AtomicUsize::new(0));
    let prefixed = Arc::new(AtomicUsize::new(0));
    engine.shaper = Box::new(Spy {
        largest: largest.clone(),
        prefixed: prefixed.clone(),
        prefix,
    });
    (largest, prefixed)
}

/// A table of one row and one cell holding `texts` as blocks, first in `story`,
/// which is bound as the header. The body gets a paragraph so there is a page.
fn cell_table_in(doc: &Document, story: NodeId, texts: impl Iterator<Item = String>) {
    let table = doc
        .append_table(two_columns([
            reprise_doc::ColumnWidth::Content,
            reprise_doc::ColumnWidth::Content,
        ]))
        .unwrap();
    let row = doc.append_table_row(table, false).unwrap();
    let cell = doc.append_table_cell(row, 0).unwrap();
    for text in texts {
        doc.append_cell_block(cell, BlockKind::Paragraph, "s", &text)
            .unwrap();
    }
    doc.move_block(table, Some(story), 0).unwrap();
    bind_header(doc, story);
    block(doc, "body");
}

fn one_cell_table(doc: &Document, texts: impl Iterator<Item = String>) {
    let story = doc.create_story(&[]).unwrap();
    cell_table_in(doc, story, texts);
}

#[test]
fn running_table_respects_story_text_read_limit() {
    let (mut engine, doc) = setup();
    let (largest, _) = spied(&mut engine, "");
    let g = grid(&doc, 1);
    doc.block(g.blocks[0][0])
        .unwrap()
        .text
        .insert(0, &"x".repeat((1 << 20) + 1))
        .unwrap();
    doc.commit();
    engine.layout(&doc);
    let read = largest.load(Ordering::SeqCst);
    assert!(
        read <= (1 << 20),
        "table bypasses running story text limit: {read}"
    );
}

#[test]
fn a_table_with_more_cell_blocks_than_the_story_budget_is_never_prepared() {
    let (mut engine, doc) = setup();
    let (_, cells) = spied(&mut engine, "cell");
    one_cell_table(&doc, (0..4097).map(|i| format!("cell{i}")));
    doc.commit();
    let s = engine.layout(&doc);
    assert_eq!(
        cells.load(Ordering::SeqCst),
        0,
        "no cell of a table past the 4,096-block budget is shaped"
    );
    // The story is cut, and says so.
    assert!(
        s.diagnostics
            .iter()
            .any(|d| d.code.as_str() == "layout.running-clipped"),
        "{:?}",
        s.diagnostics
    );
}

#[test]
fn a_table_within_the_budget_is_composed() {
    let (mut engine, doc) = setup();
    let (_, cells) = spied(&mut engine, "cell");
    one_cell_table(&doc, (0..4000).map(|i| format!("cell{i}")));
    doc.commit();
    engine.layout(&doc);
    assert!(cells.load(Ordering::SeqCst) > 0, "the table is still read");
}

/// The work of the first `steps` steps of a job of budget 1.
fn first_steps(engine: &Engine, doc: &Document, steps: usize) -> Vec<StepWork> {
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

// The digest looks at 64 child IDs a unit (`DIGEST_CHUNK`).
const CHUNK: usize = 64;

#[test]
fn top_level_blocks_and_table_cells_share_one_story_budget() {
    // 3,003 nodes of table (container, row, cell, 3,000 blocks), then 1,200
    // paragraphs: the story is cut at 4,096 whole nodes, not at 4,096 top-level
    // blocks and 3,003 more.
    let (engine, doc) = setup();
    let tail: Vec<_> = (0..1200)
        .map(|i| NewBlock::new(BlockKind::Paragraph, "s", &format!("tail{i}")))
        .collect();
    let story = doc.create_story(&tail).unwrap();
    cell_table_in(&doc, story, (0..3000).map(|i| format!("cell{i}")));
    doc.commit();
    let works = first_steps(&engine, &doc, 90);
    let kept: usize = works.iter().map(|w| w.digest_blocks).sum();
    let looked: usize = works.iter().map(|w| w.digest_inspected).sum();
    assert_eq!(kept, 4096, "the story is cut at the budget");
    assert!(looked <= 4096 + 2 * CHUNK, "{looked} IDs looked at");
    assert!(works.iter().all(|w| w.digest_inspected <= CHUNK));
}

#[test]
fn a_story_of_a_hundred_thousand_blocks_costs_its_budget_to_read() {
    let (engine, doc) = setup();
    let blocks: Vec<_> = (0..100_000)
        .map(|i| NewBlock::new(BlockKind::Paragraph, "s", &format!("line {i}")))
        .collect();
    let story = doc.create_story(&blocks).unwrap();
    bind_header(&doc, story);
    block(&doc, "body");
    doc.commit();
    let works = first_steps(&engine, &doc, 90);
    let kept: usize = works.iter().map(|w| w.digest_blocks).sum();
    let looked: usize = works.iter().map(|w| w.digest_inspected).sum();
    assert_eq!(kept, 4096);
    assert!(
        looked <= 4096 + 2 * CHUNK,
        "{looked} IDs looked at to read 4,096 blocks of 100,000"
    );
    assert!(works.iter().all(|w| w.digest_inspected <= CHUNK));
}
