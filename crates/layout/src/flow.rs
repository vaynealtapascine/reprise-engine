//! Pass 1: flow. Sets up the page and its frames, then lays out every block:
//! paragraphs stack down the main frame; annotations are composed and left
//! for the relation pass to place.

use reprise_compose::{ComposeRequest, LineFragment, Measure, break_opportunities};
use reprise_diag::Severity;
use reprise_doc::{BlockKind, Document, NodeId};
use reprise_font::FaceId;
use reprise_geom::{FrameSpace, Length, Point, Rect, Transform};
use reprise_shape::{ParagraphInput, Shaper, StyleRun, itemize, visual_order};

use crate::{
    BlockLayout, Diagnostic, Engine, FrameLayout, LayoutSnapshot, LineLayout, PageLayout,
    PositionedRun, Subject, codes,
};

/// A block composed but not yet placed.
pub(crate) struct Pending {
    pub block: BlockLayout,
    /// How far the block extends down the block axis.
    pub extent: Length,
}

/// The frames of the spike's page.
pub(crate) const MAIN: usize = 0;
pub(crate) const MARGIN: usize = 1;

/// Lays out every block, places the paragraphs and returns the blocks that
/// wait for a relation to place them.
pub(crate) fn run(engine: &Engine, doc: &Document, snapshot: &mut LayoutSnapshot) -> Vec<Pending> {
    let page = engine.page;
    snapshot.pages.push(PageLayout {
        width: page.width,
        height: page.height,
    });
    let depth = page.height - page.margin_top - page.margin_top;
    let margin_x = page.margin_left + page.column_width + page.gutter;
    for (name, x, width) in [
        ("main", page.margin_left, page.column_width),
        ("margin", margin_x, page.margin_column_width),
    ] {
        snapshot.frames.push(FrameLayout {
            name: name.into(),
            page: 0,
            to_page: Transform::translate(x, page.margin_top),
            rect: Rect::new(Point::origin(), width, depth),
        });
    }

    let mut y = Length::ZERO;
    let mut pending = Vec::new();
    for node in doc.blocks() {
        let kind = match doc.block(node) {
            Ok(b) => b.kind,
            Err(e) => {
                snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    codes::MALFORMED_BLOCK,
                    Subject::Node(node),
                    e.to_string(),
                ));
                continue;
            }
        };
        match kind {
            BlockKind::Paragraph => {
                let frame = &snapshot.frames[MAIN];
                let width = frame.rect.width;
                let depth = frame.rect.height;
                let Some(laid) =
                    compose_block(engine, doc, node, MAIN, width, y, &mut snapshot.diagnostics)
                else {
                    continue;
                };
                if y + laid.extent > depth {
                    snapshot.diagnostics.push(Diagnostic::new(
                        Severity::Warning,
                        codes::FRAME_OVERFLOW,
                        Subject::Node(node),
                        "runs past the bottom of the main frame",
                    ));
                }
                y += laid.extent + page.paragraph_spacing;
                snapshot.blocks.push(laid.block);
            }
            BlockKind::Annotation => {
                let width = snapshot.frames[MARGIN].rect.width;
                pending.extend(compose_block(
                    engine,
                    doc,
                    node,
                    MARGIN,
                    width,
                    Length::ZERO,
                    &mut snapshot.diagnostics,
                ));
            }
        }
    }
    pending
}

/// Shapes and composes one block into `frame`, starting `block_start` down it.
pub(crate) fn compose_block(
    engine: &Engine,
    doc: &Document,
    node: NodeId,
    frame: usize,
    width: Length,
    block_start: Length,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Pending> {
    let subject = Subject::Node(node);
    let block = doc.block(node).ok()?;
    let style = match doc.computed_style(node) {
        Ok(s) => s,
        Err(e) => {
            diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::STYLE,
                subject,
                e.to_string(),
            ));
            return None;
        }
    };
    for property in &style.clamped {
        diagnostics.push(Diagnostic::new(
            Severity::Warning,
            codes::STYLE_CLAMPED,
            subject.clone(),
            format!("{property} resolved to a negative length; clamped to zero"),
        ));
    }
    let text = block.text.to_string();
    let styles = [StyleRun {
        range: 0..text.len(),
        families: vec![style.family.clone()],
        size: style.size,
        language: None,
        features: Vec::new(),
    }];
    let itemized = itemize(
        &ParagraphInput {
            text: &text,
            styles: &styles,
            direction: None,
        },
        &engine.fonts,
    );
    diagnostics.extend(
        itemized
            .notes
            .into_iter()
            .map(|n| Diagnostic::from_note(n, subject.clone())),
    );
    if itemized.items.is_empty() && !text.is_empty() {
        return None; // Nothing could be shaped; itemisation said why.
    }
    let shaper = Shaper {
        text: &text,
        items: &itemized.items,
        fonts: &engine.fonts,
        adapter: engine.shaper.as_ref(),
    };
    let shaped = shaper.shape();
    let composition = engine.composer.compose(&ComposeRequest {
        text: &text,
        shaped: &shaped,
        reshape: &shaper,
        breaks: &break_opportunities(&text),
        line_height: style.line_height,
        geometry: &Measure(width),
        start: 0,
        block_start,
    });
    diagnostics.extend(
        composition
            .notes
            .into_iter()
            .map(|n| Diagnostic::from_note(n, subject.clone())),
    );
    if let Some(rest) = composition.rest {
        let mut d = Diagnostic::new(
            Severity::Error,
            codes::TEXT_UNPLACED,
            subject.clone(),
            format!("bytes {rest}..{} were not placed", text.len()),
        );
        d.bytes = Some(rest..text.len());
        diagnostics.push(d);
    }

    // Empty lines take their vertical metrics from the style's own face.
    let fallback = engine
        .fonts
        .by_family(&style.family)
        .map(|f| (f.id().clone(), style.size));
    let lines = composition
        .lines
        .into_iter()
        .map(|l| line_layout(engine, &text, frame, l, fallback.as_ref()))
        .collect();
    Some(Pending {
        block: BlockLayout {
            node,
            kind: block.kind,
            style,
            text,
            lines,
        },
        extent: composition.block_end - block_start,
    })
}

fn line_layout(
    engine: &Engine,
    text: &str,
    frame: usize,
    fragment: LineFragment,
    fallback: Option<&(FaceId, Length)>,
) -> LineLayout {
    // Half-leading: the tallest ascent and descent on the line are centred
    // in the line box.
    let metrics = |face: &FaceId, size: Length| {
        engine.fonts.get(face).ok().map(|f| {
            let m = f.metrics();
            (
                Length::from_font_units(m.ascent, size, m.units_per_em),
                Length::from_font_units(m.descent, size, m.units_per_em),
            )
        })
    };
    let mut extents: Vec<(Length, Length)> = fragment
        .runs
        .iter()
        .filter_map(|r| metrics(&r.face, r.size))
        .collect();
    if extents.is_empty() {
        extents.extend(fallback.and_then(|(face, size)| metrics(face, *size)));
    }
    let ascent = extents.iter().map(|e| e.0).max().unwrap_or_default();
    let descent = extents.iter().map(|e| e.1).max().unwrap_or_default();
    let half_leading = (fragment.height - ascent - descent).mul_ratio(1, 2);

    let levels: Vec<u8> = fragment.runs.iter().map(|r| r.level).collect();
    let mut x = fragment.available.start;
    let runs = visual_order(&levels)
        .into_iter()
        .map(|i| {
            let run = &fragment.runs[i];
            let width = run.width();
            let placed = PositionedRun {
                range: run.range.clone(),
                face: run.face.clone(),
                size: run.size,
                level: run.level,
                x,
                width,
                glyphs: run.glyphs.clone(),
            };
            x += width;
            placed
        })
        .collect();
    let rect: Rect<FrameSpace> = Rect::new(
        Point::new(fragment.available.start, fragment.block_offset),
        fragment.available.width(),
        fragment.height,
    );
    LineLayout {
        frame,
        preview: text[fragment.text.clone()].trim_end().to_string(),
        text: fragment.text,
        rect,
        baseline: fragment.block_offset + half_leading + ascent,
        width: fragment.width,
        explanation: fragment.explanation,
        runs,
    }
}

/// Moves a pending block `by` down the block axis of its frame.
pub(crate) fn shift(mut block: BlockLayout, by: Length) -> BlockLayout {
    for line in &mut block.lines {
        line.rect.origin.y += by;
        line.baseline += by;
    }
    block
}
