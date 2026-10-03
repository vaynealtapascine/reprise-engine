//! Layout: from an authored [`Document`] to a derived [`LayoutSnapshot`] and a
//! [`DisplayList`] (decisions 05, 24, 26, 28 and 37).
//!
//! The spike lays out one page in two passes:
//! 1. Flow: paragraphs stack down the main column.
//! 2. Relations: annotations are placed beside the line their relation targets,
//!    which is only known after pass 1. That ordering is the staged-pass rule (26).
//!
//! Layout never fails as a whole. Whatever can't be laid out is left out and
//! reported in `diagnostics` (37).

use std::ops::Range;

use reprise_compose::{BreakReason, ComposeRequest, Composer, Greedy, Measure};
use reprise_display::{Color, DisplayList, Glyph, Item, Layer};
use reprise_doc::{
    BlockKind, ComputedStyle, Document, NodeId, RangeState, RelationId, RelationKind, Revision,
    Target,
};
use reprise_font::{FaceId, FontStore};
use reprise_geom::{FrameSpace, Length, PageSpace, Point, Rect, Transform};
use reprise_shape::{AdapterInfo, HarfRust, ShapeRequest, ShapedGlyph, ShapingAdapter};
use serde::{Deserialize, Serialize};

/// Page geometry for the spike: one page, a main column and a margin column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSettings {
    pub width: Length,
    pub height: Length,
    pub margin_top: Length,
    pub margin_left: Length,
    pub column_width: Length,
    pub gutter: Length,
    pub margin_column_width: Length,
    pub paragraph_spacing: Length,
    pub annotation_spacing: Length,
}

impl Default for PageSettings {
    fn default() -> Self {
        PageSettings {
            width: Length::from_pt(420),
            height: Length::from_pt(300),
            margin_top: Length::from_pt(36),
            margin_left: Length::from_pt(36),
            column_width: Length::from_pt(220),
            gutter: Length::from_pt(18),
            margin_column_width: Length::from_pt(110),
            paragraph_spacing: Length::from_pt(8),
            annotation_spacing: Length::from_pt(4),
        }
    }
}

/// The engine configuration: fonts, the shaping adapter and the composer.
/// All of it is an input to the determinism guarantee (38).
pub struct Engine {
    pub fonts: FontStore,
    pub shaper: Box<dyn ShapingAdapter>,
    pub composer: Box<dyn Composer>,
    pub page: PageSettings,
}

impl Engine {
    pub fn new(fonts: FontStore) -> Engine {
        Engine {
            fonts,
            shaper: Box::new(HarfRust),
            composer: Box::new(Greedy),
            page: PageSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineLayout {
    pub text: Range<usize>,
    /// The line's text, for reading snapshots.
    pub preview: String,
    /// The line box.
    pub rect: Rect<PageSpace>,
    pub baseline: Length,
    pub width: Length,
    pub break_reason: BreakReason,
    #[serde(skip)]
    glyphs: Vec<ShapedGlyph>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockLayout {
    pub node: NodeId,
    pub kind: BlockKind,
    pub face: FaceId,
    pub style: ComputedStyle,
    pub frame: Rect<PageSpace>,
    pub lines: Vec<LineLayout>,
}

/// How a relation resolved (15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelationStatus {
    Valid,
    /// The target range lost an end and was rebound to the nearest position.
    Rebound,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationLayout {
    pub id: RelationId,
    pub kind: RelationKind,
    pub source: NodeId,
    pub status: RelationStatus,
    /// The block and line index the query matched.
    pub target_line: Option<(NodeId, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub node: Option<NodeId>,
    pub message: String,
}

/// Everything layout derived from one document revision. Can be thrown away
/// and recomputed at any time (05).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutSnapshot {
    pub revision: Revision,
    pub adapter: AdapterInfo,
    pub composer: String,
    pub page: PageSettings,
    pub blocks: Vec<BlockLayout>,
    pub relations: Vec<RelationLayout>,
    pub diagnostics: Vec<Diagnostic>,
}

impl LayoutSnapshot {
    pub fn block(&self, node: NodeId) -> Option<&BlockLayout> {
        self.blocks.iter().find(|b| b.node == node)
    }

    /// The `LineContaining` query: the line holding byte `at` of `node`.
    /// A position at a line's end belongs to that line, not the next.
    pub fn line_containing(&self, node: NodeId, at: usize) -> Option<usize> {
        let block = self.block(node)?;
        block
            .lines
            .iter()
            .position(|l| at < l.text.end || (at == l.text.end && l.text.end == block_end(block)))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("snapshots serialize")
    }
}

fn block_end(block: &BlockLayout) -> usize {
    block.lines.last().map_or(0, |l| l.text.end)
}

struct Laid {
    block: BlockLayout,
    height: Length,
}

impl Engine {
    pub fn layout(&self, doc: &Document) -> LayoutSnapshot {
        let page = self.page;
        let mut snapshot = LayoutSnapshot {
            revision: doc.revision(),
            adapter: self.shaper.info(),
            composer: self.composer.name().into(),
            page,
            blocks: Vec::new(),
            relations: Vec::new(),
            diagnostics: Vec::new(),
        };

        // Pass 1: flow.
        let mut y = page.margin_top;
        let mut annotations = Vec::new();
        for node in doc.blocks() {
            let Some(kind) = doc.block(node).ok().map(|b| b.kind) else {
                continue;
            };
            let width = match kind {
                BlockKind::Paragraph => page.column_width,
                BlockKind::Annotation => page.margin_column_width,
            };
            let Some(laid) = self.lay_block(doc, node, width, &mut snapshot.diagnostics) else {
                continue;
            };
            match kind {
                BlockKind::Paragraph => {
                    let origin = Point::new(page.margin_left, y);
                    y += laid.height + page.paragraph_spacing;
                    snapshot.blocks.push(place(laid, origin));
                }
                BlockKind::Annotation => annotations.push(laid),
            }
        }

        // Pass 2: relations. Each annotation follows its target line; if it
        // would overlap the previous annotation it is pushed down.
        let margin_x = page.margin_left + page.column_width + page.gutter;
        let mut next_free = Length::ZERO;
        for (id, relation) in doc.relations() {
            let RelationKind::Follow = relation.kind;
            let Target::LineContaining { range } = &relation.target;
            let mut result = RelationLayout {
                id: id.clone(),
                kind: relation.kind,
                source: relation.source,
                status: RelationStatus::Missing,
                target_line: None,
            };
            let (status, node, bytes) = match doc.resolve_range(range) {
                RangeState::Valid { node, bytes } => (RelationStatus::Valid, node, bytes),
                RangeState::Rebound { node, bytes } => (RelationStatus::Rebound, node, bytes),
                RangeState::Missing { .. } => {
                    snapshot.diagnostics.push(Diagnostic {
                        node: Some(relation.source),
                        message: format!(
                            "relation {}: target range {} is gone; source not placed",
                            id.0, range.0
                        ),
                    });
                    snapshot.relations.push(result);
                    continue;
                }
            };
            let line = snapshot.line_containing(node, bytes.start);
            let Some(pos) = annotations
                .iter()
                .position(|a| a.block.node == relation.source)
            else {
                snapshot.diagnostics.push(Diagnostic {
                    node: Some(relation.source),
                    message: format!(
                        "relation {}: source is not an annotation, or is already placed",
                        id.0
                    ),
                });
                snapshot.relations.push(result);
                continue;
            };
            let Some(line) = line else {
                snapshot.diagnostics.push(Diagnostic {
                    node: Some(node),
                    message: format!("relation {}: no line contains byte {}", id.0, bytes.start),
                });
                snapshot.relations.push(result);
                continue;
            };
            let target = &snapshot
                .block(node)
                .expect("line_containing found it")
                .lines[line];
            let want = target.rect.origin.y;
            let at = if want < next_free {
                snapshot.diagnostics.push(Diagnostic {
                    node: Some(relation.source),
                    message: format!(
                        "relation {}: pushed down to avoid the previous annotation",
                        id.0
                    ),
                });
                next_free
            } else {
                want
            };
            let laid = annotations.remove(pos);
            next_free = at + laid.height + page.annotation_spacing;
            snapshot.blocks.push(place(laid, Point::new(margin_x, at)));
            result.status = status;
            result.target_line = Some((node, line));
            snapshot.relations.push(result);
        }
        for left in annotations {
            snapshot.diagnostics.push(Diagnostic {
                node: Some(left.block.node),
                message: "annotation has no relation placing it; not drawn".into(),
            });
        }
        snapshot
    }

    /// Shapes and composes one block at the frame origin.
    fn lay_block(
        &self,
        doc: &Document,
        node: NodeId,
        width: Length,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Laid> {
        let block = doc.block(node).ok()?;
        let style = match doc.computed_style(node) {
            Ok(s) => s,
            Err(e) => {
                diagnostics.push(Diagnostic {
                    node: Some(node),
                    message: e.to_string(),
                });
                return None;
            }
        };
        let Some(face) = self.fonts.by_family(&style.family) else {
            diagnostics.push(Diagnostic {
                node: Some(node),
                message: format!("no face for family {:?}; block not laid out", style.family),
            });
            return None;
        };
        let text = block.text.to_string();
        let shaped = self.shaper.shape(&ShapeRequest {
            text: &text,
            face,
            size: style.size,
            direction: Default::default(),
        });
        let composition = self.composer.compose(&ComposeRequest {
            text: &text,
            shaped: &shaped,
            line_height: style.line_height,
            geometry: &Measure(width),
        });
        diagnostics.extend(
            composition
                .diagnostics
                .into_iter()
                .map(|message| Diagnostic {
                    node: Some(node),
                    message,
                }),
        );

        // Half-leading: the glyphs' ascent and descent are centred in the line box.
        let m = face.metrics();
        let ascent = Length::from_font_units(m.ascent, style.size, m.units_per_em);
        let descent = Length::from_font_units(m.descent, style.size, m.units_per_em);
        let half_leading = (style.line_height - ascent - descent).mul_ratio(1, 2);

        let lines: Vec<LineLayout> = composition
            .lines
            .iter()
            .map(|l| {
                let top = l.block_offset;
                LineLayout {
                    text: l.text.clone(),
                    preview: text[l.text.clone()].trim_end().to_string(),
                    rect: Rect::new(
                        Point::new(l.available.start, top),
                        l.available.width(),
                        style.line_height,
                    ),
                    baseline: top + half_leading + ascent,
                    width: l.width,
                    break_reason: l.break_reason,
                    glyphs: shaped.glyphs[l.glyphs.clone()].to_vec(),
                }
            })
            .collect();
        let height = style.line_height.mul_ratio(lines.len() as i32, 1);
        Some(Laid {
            block: BlockLayout {
                node,
                kind: block.kind,
                face: face.id().clone(),
                style,
                frame: Rect::new(Point::origin(), width, height),
                lines,
            },
            height,
        })
    }
}

/// Moves a block laid out at the frame origin to `origin` on the page.
fn place(laid: Laid, origin: Point<PageSpace>) -> BlockLayout {
    let to_page: Transform<FrameSpace, PageSpace> = Transform::translate(origin.x, origin.y);
    let mut block = laid.block;
    let frame_point = |p: Point<PageSpace>| to_page.apply(Point::<FrameSpace>::new(p.x, p.y));
    block.frame.origin = origin;
    for line in &mut block.lines {
        line.rect.origin = frame_point(line.rect.origin);
        line.baseline += origin.y;
    }
    block
}

/// Display list options.
#[derive(Clone, Copy, Debug, Default)]
pub struct DisplayOptions {
    /// Draw frames, line boxes, baselines and relation links (39).
    pub debug: bool,
}

const FRAME: Color = Color(40, 110, 220, 160);
const LINE_BOX: Color = Color(40, 110, 220, 60);
const BASELINE: Color = Color(220, 60, 60, 110);
const LINK: Color = Color(30, 160, 90, 200);

impl LayoutSnapshot {
    pub fn to_display_list(&self, options: DisplayOptions) -> DisplayList {
        let mut items = Vec::new();
        for block in &self.blocks {
            if options.debug {
                items.push(Item::Rect {
                    rect: block.frame,
                    fill: None,
                    stroke: Some(FRAME),
                    layer: Layer::Debug,
                });
            }
            for line in &block.lines {
                if options.debug {
                    items.push(Item::Rect {
                        rect: line.rect,
                        fill: None,
                        stroke: Some(LINE_BOX),
                        layer: Layer::Debug,
                    });
                    items.push(Item::Line {
                        from: Point::new(line.rect.origin.x, line.baseline),
                        to: Point::new(line.rect.origin.x + line.rect.width, line.baseline),
                        color: BASELINE,
                        layer: Layer::Debug,
                    });
                }
                let mut x = line.rect.origin.x;
                let glyphs = line
                    .glyphs
                    .iter()
                    .map(|g| {
                        let glyph = Glyph {
                            id: g.id,
                            x: x + g.x_offset,
                            y: line.baseline - g.y_offset,
                        };
                        x += g.advance;
                        glyph
                    })
                    .collect();
                items.push(Item::Glyphs {
                    face: block.face.clone(),
                    size: block.style.size,
                    color: Color::BLACK,
                    glyphs,
                });
            }
        }
        if options.debug {
            for rel in &self.relations {
                let (Some((node, line)), Some(source)) = (rel.target_line, self.block(rel.source))
                else {
                    continue;
                };
                let Some(target) = self.block(node).map(|b| &b.lines[line]) else {
                    continue;
                };
                items.push(Item::Line {
                    from: Point::new(target.rect.origin.x + target.width, target.baseline),
                    to: Point::new(
                        source.frame.origin.x,
                        source
                            .lines
                            .first()
                            .map_or(source.frame.origin.y, |l| l.baseline),
                    ),
                    color: LINK,
                    layer: Layer::Debug,
                });
            }
        }
        DisplayList {
            width: self.page.width,
            height: self.page.height,
            items,
        }
    }
}
