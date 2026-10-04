//! Pass 1: flow (24). Text threads through the frames of the main flow in the
//! order the template declares them, page after page. A paragraph that doesn't
//! fit what is left of a frame continues, from the line where it stopped, in
//! the next one. Annotations are composed and left for the relation pass to
//! place.
//!
//! Termination is bounded twice over. Every pass round the loop either places
//! a line or moves on to the next frame, and the number of pages is capped by
//! the engine's page limit, which is reported when it is hit. A line taller
//! than every frame is placed overflowing, so it can't make the flow look for
//! a frame forever.

use std::ops::Range;

use reprise_compose::{
    Break, ComposeRequest, GeometryProvider, LineFragment, Measure, break_opportunities,
};
use reprise_diag::Severity;
use reprise_doc::{BlockKind, ComputedStyle, Document, NodeId};
use reprise_font::FaceId;
use reprise_geom::{FrameSpace, Length, Point, Rect};
use reprise_shape::{Item, ParagraphInput, ShapedText, Shaper, StyleRun, itemize, visual_order};

use crate::region::Bounded;
use crate::template::ResolvedTemplate;
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

/// Lays out every block, places the paragraphs and returns the blocks that
/// wait for a relation to place them.
pub(crate) fn run(
    engine: &Engine,
    doc: &Document,
    template: &ResolvedTemplate,
    snapshot: &mut LayoutSnapshot,
) -> Vec<Pending> {
    let thread = template.main_thread();
    let max_depth = thread
        .iter()
        .map(|&t| template.frames[t].depth)
        .max()
        .unwrap_or_default();
    let thread_is_empty = thread.is_empty();
    let mut flow = Flow {
        engine,
        template,
        snapshot,
        thread,
        max_depth,
        page_base: Vec::new(),
        fills: Vec::new(),
        page: 0,
        pos: 0,
        // No frame to flow through: unreachable, but nothing may panic.
        limit_hit: thread_is_empty,
    };
    // Even an empty document gets a page.
    flow.new_page();

    let mut pending = Vec::new();
    for node in doc.blocks() {
        let kind = match doc.block(node) {
            Ok(b) => b.kind,
            Err(e) => {
                flow.snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    codes::MALFORMED_BLOCK,
                    Subject::Node(node),
                    e.to_string(),
                ));
                continue;
            }
        };
        match kind {
            BlockKind::Paragraph => flow.paragraph(doc, node),
            BlockKind::Annotation => pending.extend(flow.annotation(doc, node)),
        }
    }
    pending
}

/// How much of a frame the flow has used.
#[derive(Clone, Copy)]
struct Fill {
    /// Where the next block starts, spacing included.
    used: Length,
    /// Nothing is in the frame yet.
    empty: bool,
}

struct Flow<'a> {
    engine: &'a Engine,
    template: &'a ResolvedTemplate,
    snapshot: &'a mut LayoutSnapshot,
    /// Template indices of the main flow's frames, in threading order.
    thread: Vec<usize>,
    /// The deepest frame in the thread: a line taller than this fits nowhere.
    max_depth: Length,
    /// The snapshot index of each page's first frame.
    page_base: Vec<usize>,
    /// By snapshot frame index.
    fills: Vec<Fill>,
    /// Where the flow is: a page and a position in the thread.
    page: usize,
    pos: usize,
    limit_hit: bool,
}

impl Flow<'_> {
    /// Makes the next page from the template, unless the limit is reached.
    fn new_page(&mut self) -> bool {
        let limit = self.engine.flow.max_pages.max(1) as usize;
        if self.snapshot.pages.len() >= limit {
            return false;
        }
        let page = self.snapshot.pages.len();
        self.page_base.push(self.snapshot.frames.len());
        self.snapshot.pages.push(PageLayout {
            width: self.template.width,
            height: self.template.height,
        });
        for frame in &self.template.frames {
            self.snapshot.frames.push(FrameLayout {
                name: frame.name.clone(),
                role: frame.role.clone(),
                page,
                to_page: frame.to_page(),
                rect: frame.rect(),
            });
            self.fills.push(Fill {
                used: Length::ZERO,
                empty: true,
            });
        }
        true
    }

    /// The snapshot index of the frame the flow is in.
    fn frame_index(&self) -> usize {
        self.page_base[self.page] + self.thread[self.pos]
    }

    /// Moves on to the next frame of the thread, or the first of a new page.
    /// False when the page limit stops it, which is reported once.
    fn advance(&mut self) -> bool {
        if self.pos + 1 < self.thread.len() {
            self.pos += 1;
            return true;
        }
        if self.new_page() {
            self.page = self.snapshot.pages.len() - 1;
            self.pos = 0;
            return true;
        }
        if !self.limit_hit {
            self.limit_hit = true;
            self.snapshot.diagnostics.push(Diagnostic::new(
                Severity::Error,
                codes::PAGE_LIMIT,
                Subject::Document,
                format!(
                    "the text needs more than {} pages; the rest was left out",
                    self.snapshot.pages.len()
                ),
            ));
        }
        false
    }

    fn paragraph(&mut self, doc: &Document, node: NodeId) {
        let engine = self.engine;
        let subject = Subject::Node(node);
        let Some(prepared) = prepare(engine, doc, node, &mut self.snapshot.diagnostics) else {
            return;
        };
        let len = prepared.text.len();
        if self.limit_hit {
            unplaced(&mut self.snapshot.diagnostics, &subject, 0..len);
            return;
        }

        // A line taller than every frame fits nowhere; placing it overflowing
        // is better than leaving the text out or hunting through pages.
        let oversize = prepared.style.line_height > self.max_depth;
        let mut lines = Vec::new();
        let mut start = 0;
        let mut done = false;
        let mut stalls = 0;
        let mut forced = false;
        let mut overflowed = false;
        while !done {
            let index = self.frame_index();
            let fill = self.fills[index];
            let template = self.template;
            let frame = &template.frames[self.thread[self.pos]];
            let y = if fill.empty { Length::ZERO } else { fill.used };
            let measure = Measure(frame.width);
            let bounded = Bounded {
                inner: &measure,
                depth: frame.depth,
            };
            let unbounded = oversize || forced;
            let geometry: &dyn GeometryProvider = if unbounded { &measure } else { &bounded };
            let composed = prepared.compose(
                engine,
                geometry,
                index,
                start,
                y,
                &subject,
                &mut self.snapshot.diagnostics,
            );
            if composed.lines.is_empty() {
                // Nothing fit here. That is ordinary for a frame that is
                // already partly full. A whole page of empty frames that can't
                // take a line means the geometry will never allow one: place it
                // anyway, overflowing.
                stalls += usize::from(fill.empty);
                if stalls > self.thread.len() && !forced {
                    forced = true;
                } else if !self.advance() {
                    break;
                }
                continue;
            }
            if unbounded && !overflowed {
                overflowed = true;
                self.snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Warning,
                    codes::FRAME_OVERFLOW,
                    subject.clone(),
                    "a line is taller than every frame of the main flow; placed overflowing",
                ));
            }
            stalls = 0;
            forced = false;
            lines.extend(composed.lines);
            let spacing = engine.flow.paragraph_spacing;
            match composed.rest {
                None => {
                    done = true;
                    self.fills[index] = Fill {
                        used: composed.block_end + spacing,
                        empty: false,
                    };
                }
                Some(rest) => {
                    start = rest;
                    self.fills[index] = Fill {
                        used: composed.block_end,
                        empty: false,
                    };
                    if !self.advance() {
                        break;
                    }
                }
            }
        }

        if !done {
            unplaced(&mut self.snapshot.diagnostics, &subject, start..len);
        }
        if !lines.is_empty() {
            self.snapshot.blocks.push(prepared.into_block(lines));
        }
    }

    /// Composes an annotation for a relation to place. The page it lands on
    /// isn't known yet, so it is composed at the margin frame's width, or the
    /// main frame's when the template has no margin frame (then no relation
    /// can place it, and that is reported when one tries).
    fn annotation(&mut self, doc: &Document, node: NodeId) -> Option<Pending> {
        let engine = self.engine;
        let subject = Subject::Node(node);
        let prepared = prepare(engine, doc, node, &mut self.snapshot.diagnostics)?;
        let width = self.template.margin_width().unwrap_or_else(|| {
            self.thread
                .first()
                .map_or(Length::ZERO, |&t| self.template.frames[t].width)
        });
        let composed = prepared.compose(
            engine,
            &Measure(width),
            0, // Provisional: the relation that places the block sets the frame.
            0,
            Length::ZERO,
            &subject,
            &mut self.snapshot.diagnostics,
        );
        if let Some(rest) = composed.rest {
            unplaced(
                &mut self.snapshot.diagnostics,
                &subject,
                rest..prepared.text.len(),
            );
        }
        Some(Pending {
            extent: composed.block_end,
            block: prepared.into_block(composed.lines),
        })
    }
}

fn unplaced(diagnostics: &mut Vec<Diagnostic>, subject: &Subject, bytes: Range<usize>) {
    let mut d = Diagnostic::new(
        Severity::Error,
        codes::TEXT_UNPLACED,
        subject.clone(),
        format!("bytes {}..{} were not placed", bytes.start, bytes.end),
    );
    d.bytes = Some(bytes);
    diagnostics.push(d);
}

/// A block shaped and ready to compose into one or more regions.
struct Prepared {
    node: NodeId,
    kind: BlockKind,
    style: ComputedStyle,
    text: String,
    items: Vec<Item>,
    shaped: ShapedText,
    breaks: Vec<Break>,
    /// Empty lines take their vertical metrics from the style's own face.
    fallback: Option<(FaceId, Length)>,
}

/// What composing a block into one region produced.
struct Composed {
    lines: Vec<LineLayout>,
    /// Where the text continues, if the region ended before it did.
    rest: Option<usize>,
    block_end: Length,
}

/// Resolves a block's style and shapes its text. `None` when it can't be laid
/// out at all; the diagnostics say why.
fn prepare(
    engine: &Engine,
    doc: &Document,
    node: NodeId,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Prepared> {
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
    let shaped = Shaper {
        text: &text,
        items: &itemized.items,
        fonts: &engine.fonts,
        adapter: engine.shaper.as_ref(),
    }
    .shape();
    let fallback = engine
        .fonts
        .by_family(&style.family)
        .map(|f| (f.id().clone(), style.size));
    Some(Prepared {
        node,
        kind: block.kind,
        breaks: break_opportunities(&text),
        style,
        text,
        items: itemized.items,
        shaped,
        fallback,
    })
}

impl Prepared {
    /// Composes the text from byte `start` into one region, with its first
    /// line `block_start` down the frame's block axis. Lines are in frame
    /// `frame`.
    #[allow(clippy::too_many_arguments)]
    fn compose(
        &self,
        engine: &Engine,
        geometry: &dyn GeometryProvider,
        frame: usize,
        start: usize,
        block_start: Length,
        subject: &Subject,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Composed {
        let shaper = Shaper {
            text: &self.text,
            items: &self.items,
            fonts: &engine.fonts,
            adapter: engine.shaper.as_ref(),
        };
        let composition = engine.composer.compose(&ComposeRequest {
            text: &self.text,
            shaped: &self.shaped,
            reshape: &shaper,
            breaks: &self.breaks,
            line_height: self.style.line_height,
            geometry,
            start,
            block_start,
        });
        diagnostics.extend(
            composition
                .notes
                .into_iter()
                .map(|n| Diagnostic::from_note(n, subject.clone())),
        );
        Composed {
            lines: composition
                .lines
                .into_iter()
                .map(|l| line_layout(engine, &self.text, frame, l, self.fallback.as_ref()))
                .collect(),
            rest: composition.rest,
            block_end: composition.block_end,
        }
    }

    fn into_block(self, lines: Vec<LineLayout>) -> BlockLayout {
        BlockLayout {
            node: self.node,
            kind: self.kind,
            style: self.style,
            text: self.text,
            lines,
        }
    }
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
        preview: text
            .get(fragment.text.clone())
            .unwrap_or_default()
            .trim_end()
            .to_string(),
        text: fragment.text,
        rect,
        baseline: fragment.block_offset + half_leading + ascent,
        width: fragment.width,
        explanation: fragment.explanation,
        runs,
    }
}

/// Puts a block in `frame`, `by` down its block axis.
pub(crate) fn place(mut block: BlockLayout, frame: usize, by: Length) -> BlockLayout {
    for line in &mut block.lines {
        line.frame = frame;
        line.rect.origin.y += by;
        line.baseline += by;
    }
    block
}
