//! Oracles over layout snapshots and rendered output.
//!
//! These take finished results and say whether they honour the contracts:
//! diagnostics are documented, lines cover their text, frames are on real
//! pages, reading order visits every placed line once, and output is
//! byte-identical when produced twice.

use std::collections::BTreeSet;

use reprise_display::{pdf, png, svg};
use reprise_doc::Document;
use reprise_doc::text::segment;
use reprise_layout::{DisplayOptions, Engine, LayoutSnapshot, LineRef, Subject};

use crate::codes;
use crate::ensure;
use crate::input::fnv;
use crate::scenario::{RENDER_DEBUG, RENDER_PDF, RENDER_PDF_ORDERED, RENDER_PNG, RENDER_SVG};
use crate::violation::R;

/// Every code is in the `docs/contracts.md` table.
pub fn documented<'a>(what: &str, codes_seen: impl IntoIterator<Item = &'a str>) -> R {
    for code in codes_seen {
        ensure!(
            codes::is_documented(code),
            "undocumented-code",
            "{what} reported `{code}`, which docs/contracts.md doesn't list"
        );
    }
    Ok(())
}

pub fn diagnostics(snapshot: &LayoutSnapshot) -> R {
    documented(
        "layout",
        snapshot.diagnostics.iter().map(|d| d.code.as_str()),
    )
}

/// The structural invariants the hostile suite checks on every fixture.
pub fn structure(snapshot: &LayoutSnapshot) -> R {
    for block in &snapshot.blocks {
        let node = block.node;
        ensure!(!block.lines.is_empty(), "block-has-a-line", "{node} has no line");
        let unplaced = snapshot
            .diagnostics_with("layout.text-unplaced")
            .any(|d| d.subject == Subject::Node(node));
        let mut at = 0;
        for line in &block.lines {
            ensure!(
                line.text.start == at,
                "lines-contiguous",
                "{node}: line starts at {} after {at}",
                line.text.start
            );
            ensure!(
                segment::is_grapheme_boundary(&block.text, line.text.start),
                "lines-on-graphemes",
                "{node}: line starts inside a grapheme at {}",
                line.text.start
            );
            ensure!(
                line.rect.width.0 >= 0 && line.rect.height.0 >= 0,
                "line-size",
                "{node}: negative line size {:?}",
                line.rect
            );
            ensure!(
                line.rect.origin.y.0 >= 0,
                "line-below-frame-top",
                "{node}: line above its frame at {:?}",
                line.rect.origin.y
            );
            ensure!(
                snapshot.frame(line.frame).is_some(),
                "line-frame-exists",
                "{node}: line in missing frame {}",
                line.frame
            );
            for run in &line.runs {
                ensure!(
                    run.range.start >= line.text.start && run.range.end <= line.text.end,
                    "run-in-line",
                    "{node}: run {:?} outside line {:?}",
                    run.range,
                    line.text
                );
                for glyph in &run.glyphs {
                    ensure!(
                        run.range.contains(&(glyph.cluster as usize)),
                        "glyph-in-run",
                        "{node}: glyph cluster {} outside run {:?}",
                        glyph.cluster,
                        run.range
                    );
                }
            }
            at = line.text.end;
        }
        if !unplaced {
            ensure!(
                at == block.text.len(),
                "lines-cover-text",
                "{node}: lines end at {at}, text is {} bytes",
                block.text.len()
            );
        }
        // The query API agrees with the lines.
        let placed = block.lines.last().map_or(0, |l| l.text.end);
        for offset in (0..=placed).filter(|&i| block.text.is_char_boundary(i)) {
            let found = snapshot.line_containing(node, offset);
            ensure!(
                found.is_some(),
                "query-line-containing",
                "{node}: no line contains byte {offset}"
            );
        }
    }
    ensure!(!snapshot.pages.is_empty(), "has-a-page", "layout made no page");
    let mut page = 0;
    for (i, frame) in snapshot.frames.iter().enumerate() {
        ensure!(
            frame.page < snapshot.pages.len(),
            "frame-on-a-page",
            "frame {i} is on page {}, of {}",
            frame.page,
            snapshot.pages.len()
        );
        ensure!(
            frame.page >= page,
            "frames-in-page-order",
            "frame {i} is on page {} after page {page}",
            frame.page
        );
        page = frame.page;
    }
    Ok(())
}

/// Reading order visits every placed line exactly once, in a page that the
/// line's frame is on.
pub fn reading_order(doc: &Document, snapshot: &LayoutSnapshot) -> R {
    ensure!(
        snapshot.revision == doc.revision(),
        "snapshot-current",
        "snapshot is for another revision than the document"
    );
    let placed: BTreeSet<LineRef> = snapshot
        .blocks
        .iter()
        .flat_map(|b| (0..b.lines.len()).map(move |line| LineRef { node: b.node, line }))
        .collect();
    let order = snapshot.reading_order(doc);
    let mut seen = BTreeSet::new();
    for step in &order {
        ensure!(
            placed.contains(&step.line),
            "reading-order-placed",
            "reading order names {:?}, which isn't a placed line",
            step.line
        );
        ensure!(
            seen.insert(step.line),
            "reading-order-once",
            "reading order visits {:?} twice",
            step.line
        );
        let frame_page = snapshot
            .line(step.line)
            .and_then(|l| snapshot.frame(l.frame))
            .map(|f| f.page);
        ensure!(
            frame_page == Some(step.page),
            "reading-order-page",
            "step {:?} says page {}, its frame is on {frame_page:?}",
            step.line,
            step.page
        );
    }
    ensure!(
        seen.len() == placed.len(),
        "reading-order-complete",
        "reading order covers {} of {} placed lines",
        seen.len(),
        placed.len()
    );
    Ok(())
}

fn text<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, String> {
    result.map_err(|e| format!("{e:?}"))
}

/// Renders the snapshot the ways `flags` ask for and returns a transcript of
/// digests. Everything is produced twice and compared: the same inputs must
/// give identical display JSON, SVG, PNG and PDF bytes.
pub fn render(
    engine: &Engine,
    doc: &Document,
    snapshot: &LayoutSnapshot,
    flags: u8,
) -> R<Vec<u64>> {
    let options = DisplayOptions::default();
    let lists = snapshot.to_display_lists(options);
    let again = snapshot.to_display_lists(options);
    let mut transcript = Vec::new();
    for (list, twin) in lists.iter().zip(&again) {
        let json = list.to_json();
        ensure!(
            json == twin.to_json(),
            "display-json-repeatable",
            "display JSON differs between two builds of one snapshot"
        );
        transcript.push(fnv(json.as_bytes()));
    }
    ensure!(
        lists.len() == snapshot.pages.len(),
        "one-list-per-page",
        "{} display lists for {} pages",
        lists.len(),
        snapshot.pages.len()
    );
    if flags & RENDER_DEBUG != 0 {
        let debug = snapshot.to_display_lists(DisplayOptions {
            debug: true,
            ..options
        });
        for (page, (plain, overlaid)) in lists.iter().zip(&debug).enumerate() {
            ensure!(
                overlaid.content_only().to_json() == plain.to_json(),
                "debug-overlay-keeps-content",
                "page {page}: the debug overlay changed the content"
            );
        }
    }
    let first_pages = || lists.iter().take(3);
    if flags & RENDER_SVG != 0 {
        for (page, list) in first_pages().enumerate() {
            let a = text(svg::render_with_assets(list, &engine.fonts, &engine.assets));
            let b = text(svg::render_with_assets(list, &engine.fonts, &engine.assets));
            ensure!(a == b, "svg-repeatable", "page {page}: SVG differs between runs");
            transcript.push(fnv(format!("{a:?}").as_bytes()));
        }
    }
    if flags & RENDER_PNG != 0
        && let Some(list) = lists.first()
    {
        let a = text(png::render_with_assets(list, &engine.fonts, &engine.assets, 0.25));
        let b = text(png::render_with_assets(list, &engine.fonts, &engine.assets, 0.25));
        ensure!(a == b, "png-repeatable", "PNG differs between runs");
        transcript.push(fnv(format!("{a:?}").as_bytes()));
    }
    if lists.len() <= 12 {
        if flags & RENDER_PDF != 0 {
            let a = text(pdf::render_with_assets(&lists, &engine.fonts, &engine.assets));
            let b = text(pdf::render_with_assets(&lists, &engine.fonts, &engine.assets));
            ensure!(a == b, "pdf-repeatable", "PDF differs between runs");
            transcript.push(fnv(format!("{a:?}").as_bytes()));
        }
        if flags & RENDER_PDF_ORDERED != 0 {
            let order = snapshot.pdf_reading_order(doc);
            let a = text(pdf::render_ordered_with_assets(
                &lists,
                &engine.fonts,
                &engine.assets,
                &order,
            ));
            let b = text(pdf::render_ordered_with_assets(
                &lists,
                &engine.fonts,
                &engine.assets,
                &order,
            ));
            ensure!(
                a == b,
                "pdf-ordered-repeatable",
                "ordered PDF differs between runs"
            );
            transcript.push(fnv(format!("{a:?}").as_bytes()));
        }
    }
    Ok(transcript)
}
