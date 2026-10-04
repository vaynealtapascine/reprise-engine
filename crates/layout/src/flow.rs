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
use reprise_doc::context::{Extent, ResolutionContext};
use reprise_doc::{BlockKind, ComputedStyle, Document, NodeId};
use reprise_font::FaceId;
use reprise_geom::{FrameSpace, Length, Point, Rect};
use reprise_shape::{
    Item, ParagraphInput, ShapedText, Shaper, StyleRun, itemize, reorder_line, visual_order,
};

use crate::region::Bounded;
use crate::template::{ResolvedFrame, ResolvedTemplate};
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
        let mut preparation_notes = Vec::new();
        let mut ctx = self.paragraph_context();
        let Some(mut prepared) = prepare(engine, doc, node, &ctx, &mut preparation_notes) else {
            self.snapshot.diagnostics.extend(preparation_notes);
            return;
        };
        let len = prepared.text.len();
        if self.limit_hit {
            self.snapshot.diagnostics.extend(preparation_notes);
            unplaced(&mut self.snapshot.diagnostics, &subject, 0..len);
            return;
        }

        // A line taller than every frame fits nowhere; placing it overflowing
        // is better than leaving the text out or hunting through pages.
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
            let unbounded = prepared.style.line_height > self.max_depth || forced;
            let geometry: &dyn GeometryProvider = if unbounded { &measure } else { &bounded };
            let mut composition_notes = Vec::new();
            let composed = prepared.compose(
                engine,
                geometry,
                index,
                start,
                y,
                &subject,
                &mut composition_notes,
            );
            if composed.lines.is_empty() {
                self.snapshot.diagnostics.extend(composition_notes);
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
                if lines.is_empty() {
                    // No line has been placed yet: resolve against the new
                    // candidate starting frame, discarding provisional notes.
                    ctx = self.paragraph_context();
                    preparation_notes.clear();
                    let Some(next) = prepare(engine, doc, node, &ctx, &mut preparation_notes)
                    else {
                        self.snapshot.diagnostics.extend(preparation_notes);
                        return;
                    };
                    prepared = next;
                }
                continue;
            }
            self.snapshot.diagnostics.append(&mut preparation_notes);
            self.snapshot.diagnostics.extend(composition_notes);
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

        self.snapshot.diagnostics.extend(preparation_notes);
        if !done {
            unplaced(&mut self.snapshot.diagnostics, &subject, start..len);
        }
        if !lines.is_empty() {
            self.snapshot.blocks.push(prepared.into_block(lines));
        }
    }

    /// Style is frozen once the block places its first line. A continuation
    /// keeps that starting frame's context even when later widths differ.
    fn paragraph_context(&self) -> ResolutionContext {
        let frame = self
            .thread
            .get(self.pos)
            .and_then(|&i| self.template.frames.get(i));
        resolution_context(
            self.engine,
            self.template,
            frame,
            frame.map_or(Length::ZERO, |f| f.width),
        )
    }

    /// Composes an annotation for a relation to place. The page it lands on
    /// isn't known yet, so it is composed at the margin frame's width, or the
    /// main frame's when the template has no margin frame (then no relation
    /// can place it, and that is reported when one tries).
    fn annotation(&mut self, doc: &Document, node: NodeId) -> Option<Pending> {
        let engine = self.engine;
        let subject = Subject::Node(node);
        let frame = self
            .template
            .frames
            .iter()
            .find(|f| f.role == reprise_doc::FrameRole::Margin)
            .or_else(|| {
                self.thread
                    .first()
                    .and_then(|&i| self.template.frames.get(i))
            });
        let width = self
            .template
            .margin_width()
            .unwrap_or_else(|| frame.map_or(Length::ZERO, |f| f.width));
        let ctx = resolution_context(engine, self.template, frame, width);
        let prepared = prepare(engine, doc, node, &ctx, &mut self.snapshot.diagnostics)?;
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

/// All pages currently use the same resolved template. Named bases include
/// every frame on that page; block height stays indefinite until composed.
fn resolution_context(
    engine: &Engine,
    template: &ResolvedTemplate,
    frame: Option<&ResolvedFrame>,
    width: Length,
) -> ResolutionContext {
    let mut ctx = ResolutionContext::default()
        .with_medium(Extent::definite(engine.medium.width, engine.medium.height))
        .with_page(Extent::definite(template.width, template.height));
    for f in &template.frames {
        ctx = ctx.with_named_frame(&f.name, Extent::definite(f.width, f.depth));
    }
    if let Some(f) = frame {
        ctx = ctx.with_current_frame(&f.name, Extent::definite(f.width, f.depth));
    }
    ctx.with_block(Extent::auto_height(width))
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
    levels: Vec<u8>,
    base_level: u8,
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
    ctx: &ResolutionContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Prepared> {
    let subject = Subject::Node(node);
    let block = doc.block(node).ok()?;
    let style = match doc.computed_style_with(node, ctx, &engine.functions) {
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
    diagnostics.extend(
        style
            .notes
            .iter()
            .cloned()
            .map(|note| Diagnostic::from_note(note, subject.clone())),
    );
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
        levels: itemized.levels,
        base_level: itemized.base_level,
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
                .map(|l| line_layout(engine, self, frame, l, subject, diagnostics))
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
    prepared: &Prepared,
    frame: usize,
    fragment: LineFragment,
    subject: &Subject,
    diagnostics: &mut Vec<Diagnostic>,
) -> LineLayout {
    let text = &prepared.text;
    let fallback = prepared.fallback.as_ref();
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

    let visual_runs = match reorder_line(
        text,
        &fragment.runs,
        &prepared.levels,
        fragment.text.clone(),
        prepared.base_level,
    ) {
        Ok(runs) => runs,
        Err(note) => {
            diagnostics.push(Diagnostic::from_note(note, subject.clone()));
            let levels: Vec<u8> = fragment.runs.iter().map(|r| r.level).collect();
            visual_order(&levels)
                .into_iter()
                .filter_map(|i| fragment.runs.get(i).cloned())
                .collect()
        }
    };
    let mut x = fragment.available.start;
    let runs = visual_runs
        .into_iter()
        .map(|run| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use reprise_compose::{Adjustment, BreakReason, Explanation, Interval};
    use reprise_font::FontStore;

    #[test]
    fn preparation_uses_the_engines_function_registry() {
        use reprise_doc::expr::{Dim, Value};
        use reprise_doc::function::{Builtin, Signature};
        use reprise_doc::{Authored, Expr, Property, Style};
        let mut engine = Engine::new(FontStore::default());
        engine
            .functions
            .register(
                "custom-size",
                Builtin::new(Signature::new(&[], Dim::Length), |_| {
                    Ok(Value::Length(Length::from_pt(12)))
                }),
            )
            .unwrap();
        let doc = Document::new(1).unwrap();
        let mut style = Style::default();
        style.set(
            Property::Size,
            Authored::Expr(Expr::parse("custom-size()").unwrap()),
        );
        doc.define_style("custom", &style).unwrap();
        let node = doc
            .append_block(BlockKind::Paragraph, "custom", "")
            .unwrap();
        let mut notes = Vec::new();
        let prepared = prepare(
            &engine,
            &doc,
            node,
            &ResolutionContext::default(),
            &mut notes,
        )
        .unwrap();
        assert_eq!(prepared.style.size, Length::from_pt(12));
        assert!(notes.is_empty());
    }

    #[test]
    fn context_keeps_extreme_definite_bases_and_indefinite_block_height() {
        use reprise_doc::context::{Axis, Basis, Level, Resolved};
        let mut engine = Engine::new(FontStore::default());
        engine.medium = reprise_doc::Medium::new(Length::MIN, Length::MAX);
        let frame = ResolvedFrame {
            name: "extreme".into(),
            role: reprise_doc::FrameRole::Margin,
            x: Length::MIN,
            y: Length::MAX,
            width: Length::MAX,
            depth: Length::ZERO,
        };
        let template = ResolvedTemplate {
            name: "extreme".into(),
            source: crate::TemplateSource::Document,
            width: Length::MAX,
            height: Length::ZERO,
            frames: vec![frame.clone()],
        };
        let ctx = resolution_context(&engine, &template, Some(&frame), frame.width);
        assert_eq!(
            ctx.basis(&Basis::frame("extreme", Axis::Width)),
            Resolved::Definite(Length::MAX)
        );
        assert_eq!(
            ctx.basis(&Basis::exact(Level::Frame, Axis::Height)),
            Resolved::Definite(Length::ZERO)
        );
        assert_eq!(
            ctx.basis(&Basis::exact(Level::Block, Axis::Height)),
            Resolved::Indefinite
        );
        assert_eq!(
            ctx.basis(&Basis::exact(Level::Medium, Axis::Width)),
            Resolved::Definite(Length::MIN)
        );
        let absent = resolution_context(&engine, &template, None, Length::ZERO);
        assert_eq!(absent.frame, Extent::UNRESOLVED);
    }

    #[test]
    fn malformed_line_levels_report_and_fall_back_without_panicking() {
        let engine = Engine::new(FontStore::default());
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "").unwrap();
        let mut diagnostics = Vec::new();
        let mut prepared = prepare(
            &engine,
            &doc,
            node,
            &ResolutionContext::default(),
            &mut diagnostics,
        )
        .unwrap();
        prepared.levels = vec![127];
        let fragment = LineFragment {
            text: 0..0,
            runs: Vec::new(),
            width: Length::ZERO,
            available: Interval::new(Length::MIN, Length::MAX),
            line: 0,
            block_offset: Length::ZERO,
            height: Length::MAX,
            explanation: Explanation {
                reason: BreakReason::End,
                score: None,
                adjustment: Adjustment::default(),
                reshaped: false,
            },
        };
        let line = line_layout(
            &engine,
            &prepared,
            0,
            fragment,
            &Subject::Node(node),
            &mut diagnostics,
        );
        assert!(line.runs.is_empty());
        let note = diagnostics
            .iter()
            .find(|d| d.code.as_str() == "shape.bad-line")
            .unwrap();
        assert_eq!(note.severity, Severity::Warning);
        assert_eq!(note.subject, Subject::Node(node));
    }
}
