//! Where a span of text is on the pages: rectangles per line fragment, for
//! annotations (links, comment anchors) and anything else that needs the page
//! geometry of a byte range (`docs/annotations.md`, decisions 20, 30, 33).
//!
//! A span is a byte range of a block's text. Its geometry is one rectangle
//! for each stretch of each line fragment that is drawn contiguously, so a
//! line of mixed direction can give several for one logical range. Every
//! rectangle is given three ways:
//!
//! -   `frame_rect`, in the frame's logical [`FrameSpace`], where the line
//!     boxes are;
//! -   `bounds`, the axis-aligned page-space bounding box, for coarse
//!     hit-testing and for aligning a sidebar;
//! -   `corners`, the four page-space corners of the frame rectangle, so a
//!     rotated, mirrored, vertical or spiral frame hit-tests exactly.
//!
//! Cells are placed as the editing kernel's navigator places them (clusters
//! split evenly between their graphemes), and a parity test pins the two
//! together: the rectangles here equal `Navigator::selection_rects`.
//!
//! Everything is integer arithmetic. Results are bounded: at most
//! [`MAX_SPAN_RECTS`] rectangles for a span and [`MAX_PAGE_LINKS`] links for a
//! page, reported as `truncated`.
//!
//! The per-line facts read here are small and fixed: a line fragment's
//! `text` bytes, `rect`, `frame` and visual-order `runs` (each with its
//! `range`, `level`, `x`, `width`, `combined` and glyph clusters and
//! advances). Features that put things on lines (list markers, inline
//! objects) should keep those meaning the same.

use std::collections::BTreeMap;
use std::ops::Range;

use reprise_diag::Code;
use reprise_doc::annotation::{AnnotationId, AnnotationState, Kind, LinkTarget};
use reprise_doc::{Document, NodeId};
use reprise_geom::{FrameSpace, Length, PageSpace, Point, Rect, div_round};
use reprise_text::segment;
use serde::Serialize;

use crate::{BlockLayout, LayoutSnapshot, LineLayout, LineRef, PositionedRun};

/// The document given to an annotation query is not the one the snapshot was
/// laid out from, so byte offsets would not match: nothing is placed.
pub const ANNOTATION_REVISION: Code = Code::new("layout.annotation-revision");

/// The most rectangles one span gives.
pub const MAX_SPAN_RECTS: usize = 4096;
/// The most links one page lists.
pub const MAX_PAGE_LINKS: usize = 4096;

/// One stretch of one line fragment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SpanRect {
    /// Index into [`LayoutSnapshot::pages`].
    pub page: usize,
    /// Index into [`LayoutSnapshot::frames`].
    pub frame: usize,
    pub line: LineRef,
    /// The stretch in its frame's logical space: the line box's block extent
    /// across the stretch's inline extent.
    pub frame_rect: Rect<FrameSpace>,
    /// The axis-aligned page bounds of `frame_rect`.
    pub bounds: Rect<PageSpace>,
    /// The page positions of `frame_rect`'s corners: inline start and block
    /// start, inline end and block start, inline end and block end, inline
    /// start and block end.
    pub corners: [Point<PageSpace>; 4],
    /// On a derived copy of a repeated table header row, not the authored line.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub repeated: bool,
}

/// Where a span starts, for aligning a sidebar entry with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SpanTop {
    /// The first page the span has a rectangle on.
    pub page: usize,
    /// The smallest page-space y of the span's rectangles on that page.
    pub y: Length,
}

/// The page geometry of a span.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SpanGeometry {
    /// In logical order of pieces and lines, then in visual order along a line.
    pub rects: Vec<SpanRect>,
    pub top: Option<SpanTop>,
    /// More than [`MAX_SPAN_RECTS`] rectangles matched.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Pieces whose block or text is not on any page (left out of layout, or
    /// text with no line): they have no rectangle.
    #[serde(skip_serializing_if = "is_zero")]
    pub unplaced: usize,
    /// The document was not the snapshot's: nothing was placed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl SpanGeometry {
    fn finish(mut self) -> SpanGeometry {
        self.top = self
            .rects
            .iter()
            .map(|r| (r.page, r.bounds.origin.y))
            .min()
            .map(|(page, y)| SpanTop { page, y });
        self
    }
}

/// One link on a page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PageLink {
    pub id: AnnotationId,
    pub target: LinkTarget,
    /// Its rectangles on this page, in logical order.
    pub rects: Vec<SpanRect>,
}

/// The links drawn on one page, in the order their text first appears on it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PageLinks {
    pub page: usize,
    pub links: Vec<PageLink>,
    /// More than [`MAX_PAGE_LINKS`] links, or a link's rectangles hit the
    /// span limit.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The document was not the snapshot's: nothing was listed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

impl LayoutSnapshot {
    /// The geometry of text spans, given as `(block, bytes)` pieces in logical
    /// order. An empty `bytes` is a point (a collapsed comment): one
    /// zero-width rectangle where the point is.
    ///
    /// This takes no document, so it cannot tell whether the byte offsets were
    /// made for this snapshot: use [`LayoutSnapshot::annotation_geometry`] for
    /// that check.
    pub fn span_geometry(&self, pieces: &[(NodeId, Range<usize>)]) -> SpanGeometry {
        let mut resolver = SpanResolver::new(self);
        resolver.geometry(pieces)
    }

    /// The geometry of one annotation, or `None` when the document has no
    /// readable live annotation with that ID. `stale` when `doc` is not the
    /// revision this snapshot was made from.
    pub fn annotation_geometry(
        &self,
        doc: &Document,
        id: AnnotationId,
    ) -> Option<SpanGeometry> {
        self.annotation_geometries(doc, &[id])
            .into_iter()
            .next()
            .map(|(_, geometry)| geometry)
    }

    /// The geometry of several annotations at once, sharing the work of
    /// finding each block's graphemes and cells (a comment sidebar asks for
    /// all of them). IDs without a readable live annotation are left out; the
    /// result is in the order of `ids`.
    pub fn annotation_geometries(
        &self,
        doc: &Document,
        ids: &[AnnotationId],
    ) -> Vec<(AnnotationId, SpanGeometry)> {
        let view = doc.annotation_view();
        let stale = self.revision != doc.revision();
        let mut resolver = SpanResolver::new(self);
        let mut out = Vec::new();
        for &id in ids.iter().take(reprise_doc::annotation::MAX_QUERY_RESULTS) {
            let Some(annotation) = view.get(id) else {
                continue;
            };
            if stale {
                out.push((
                    id,
                    SpanGeometry {
                        stale: true,
                        ..SpanGeometry::default()
                    },
                ));
                continue;
            }
            let pieces: Vec<(NodeId, Range<usize>)> = match &annotation.state {
                AnnotationState::Active { pieces, .. } => {
                    pieces.iter().map(|p| (p.node, p.bytes.clone())).collect()
                }
                AnnotationState::Orphaned {
                    node: Some(node),
                    at,
                } => vec![(*node, *at..*at)],
                AnnotationState::Orphaned { node: None, .. } | AnnotationState::Gone => {
                    Vec::new()
                }
            };
            let mut geometry = resolver.geometry(&pieces);
            if pieces.is_empty() {
                geometry.unplaced = 1;
            }
            out.push((id, geometry));
        }
        out
    }

    /// The links drawn on `page`: for each link with text on the page, its
    /// rectangles there, so the host can hit-test them and show a hover card.
    /// A derived copy of a repeated table header row lists its links too
    /// (`SpanRect::repeated`), since the copy is what the reader can click.
    ///
    /// This is the display list's companion: the display list is unchanged by
    /// annotations, and the underline and colour of a link are ordinary
    /// formatting the author (or Cmd+K) applied.
    pub fn page_links(&self, doc: &Document, page: usize) -> PageLinks {
        let mut out = PageLinks {
            page,
            ..PageLinks::default()
        };
        if self.revision != doc.revision() {
            out.stale = true;
            return out;
        }
        let view = doc.annotation_view();
        if view.is_empty() {
            return out;
        }
        let mut resolver = SpanResolver::new(self);
        let mut at: BTreeMap<AnnotationId, usize> = BTreeMap::new();
        let mut scan = |block: &BlockLayout, repeated: bool, out: &mut PageLinks| {
            if !block
                .lines
                .iter()
                .any(|l| self.frames.get(l.frame).is_some_and(|f| f.page == page))
            {
                return;
            }
            for (bytes, id) in view.pieces(block.node) {
                let Some(annotation) = view.get(*id) else {
                    continue;
                };
                let (Kind::Link, AnnotationState::Active { .. }) =
                    (annotation.kind(), &annotation.state)
                else {
                    continue;
                };
                let reprise_doc::annotation::AnnotationData::Link(target) = &annotation.data
                else {
                    continue;
                };
                let mut rects = Vec::new();
                let full = resolver.block_rects(block, bytes, repeated, &mut rects);
                rects.retain(|r| r.page == page);
                if rects.is_empty() {
                    continue;
                }
                let index = match at.get(id) {
                    Some(&i) => i,
                    None => {
                        if out.links.len() >= MAX_PAGE_LINKS {
                            out.truncated = true;
                            continue;
                        }
                        out.links.push(PageLink {
                            id: *id,
                            target: target.clone(),
                            rects: Vec::new(),
                        });
                        at.insert(*id, out.links.len() - 1);
                        out.links.len() - 1
                    }
                };
                let link = &mut out.links[index];
                let room = MAX_SPAN_RECTS.saturating_sub(link.rects.len());
                if rects.len() > room || !full {
                    out.truncated = true;
                }
                rects.truncate(room);
                link.rects.extend(rects);
            }
        };
        for block in &self.blocks {
            scan(block, false, &mut out);
        }
        for header in &self.repeated_headers {
            for block in &header.blocks {
                scan(block, true, &mut out);
            }
        }
        out
    }
}

/// A grapheme-sized stretch of a line: its bytes and where it is drawn.
#[derive(Clone, Debug)]
struct Cell {
    range: Range<usize>,
    x0: Length,
    x1: Length,
    /// The bidi embedding level of the run it was drawn in.
    level: u8,
}

/// What is derived from a block's text, once per call.
struct BlockInfo {
    boundaries: Vec<usize>,
}

impl BlockInfo {
    fn new(text: &str) -> BlockInfo {
        BlockInfo {
            boundaries: segment::grapheme_boundaries(text),
        }
    }

    fn is_boundary(&self, at: usize) -> bool {
        self.boundaries.binary_search(&at).is_ok()
    }

    /// The last boundary strictly before `at`.
    fn prev_boundary(&self, at: usize) -> Option<usize> {
        let i = self.boundaries.partition_point(|&b| b < at);
        i.checked_sub(1).map(|i| self.boundaries[i])
    }
}

/// A hard line break ends a line; nothing is drawn for it.
fn is_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Where text stops being drawn on line `index` of a block: the line's end,
/// or the start of the hard break that ends it when another line follows.
fn end_limit(block: &BlockLayout, info: &BlockInfo, index: usize) -> usize {
    let Some(line) = block.lines.get(index) else {
        return 0;
    };
    let end = line.text.end;
    if index + 1 >= block.lines.len() {
        return end;
    }
    match info.prev_boundary(end) {
        Some(start)
            if start >= line.text.start
                && block
                    .text
                    .get(start..end)
                    .is_some_and(|g| !g.is_empty() && g.chars().all(is_break)) =>
        {
            start
        }
        _ => end,
    }
}

fn clamp_length(v: i64) -> Length {
    Length(v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
}

/// The cells of one run, in visual order. A cluster that holds several
/// graphemes (a ligature) is split evenly between them.
fn run_cells(run: &PositionedRun, info: &BlockInfo, out: &mut Vec<Cell>) {
    if run.combined {
        out.push(Cell {
            range: run.range.clone(),
            x0: run.x,
            x1: run.x + run.width,
            level: run.level,
        });
        return;
    }
    let rtl = run.level % 2 == 1;
    let range = &run.range;
    if range.start >= range.end {
        return;
    }
    // Consecutive glyphs of one cluster, in visual order, with their extent.
    let mut groups: Vec<(usize, Length, Length)> = Vec::new();
    let mut pen = run.x;
    for glyph in &run.glyphs {
        let cluster = glyph.cluster as usize;
        let from = pen;
        pen += glyph.advance;
        match groups.last_mut() {
            Some((c, _, x1)) if *c == cluster => *x1 = pen,
            _ => groups.push((cluster, from, pen)),
        }
    }
    groups.retain(|(c, ..)| range.contains(c) && info.is_boundary(*c));
    // A cluster runs to the next larger cluster, or to the run's end.
    let mut starts: Vec<usize> = groups.iter().map(|(c, ..)| *c).collect();
    starts.sort_unstable();
    starts.dedup();
    let end_of = |c: usize| {
        let i = starts.partition_point(|&s| s <= c);
        starts.get(i).copied().unwrap_or(range.end).min(range.end)
    };
    let mut cells: Vec<Cell> = Vec::new();
    for &(cluster, a, b) in &groups {
        let (lo, hi) = (a.min(b), a.max(b));
        let end = end_of(cluster);
        // The cluster's graphemes, as boundaries; the cluster's own edges
        // count even if a malformed shaper put them inside a grapheme.
        let mut edges = vec![cluster];
        let first = info.boundaries.partition_point(|&bd| bd <= cluster);
        edges.extend(
            info.boundaries
                .get(first..)
                .unwrap_or_default()
                .iter()
                .copied()
                .take_while(|&bd| bd < end),
        );
        edges.push(end);
        let n = edges.len() - 1;
        let width = i64::from(hi.0) - i64::from(lo.0);
        let at = |k: usize| {
            let share = div_round(width.saturating_mul(k as i64), n as i64);
            clamp_length(i64::from(lo.0).saturating_add(share))
        };
        for k in 0..n {
            let grapheme = if rtl { n - 1 - k } else { k };
            let (Some(&s), Some(&e)) = (edges.get(grapheme), edges.get(grapheme + 1)) else {
                continue;
            };
            cells.push(Cell {
                range: s..e,
                x0: at(k),
                x1: at(k + 1),
                level: run.level,
            });
        }
    }
    out.append(&mut cells);
}

/// The drawn cells of a line fragment, in visual order.
fn line_cells(block: &BlockLayout, info: &BlockInfo, index: usize, line: &LineLayout) -> Vec<Cell> {
    let limit = end_limit(block, info, index);
    let mut cells = Vec::new();
    for run in &line.runs {
        run_cells(run, info, &mut cells);
    }
    cells.retain(|c| {
        c.range.start >= line.text.start
            && c.range.end <= limit
            && info.is_boundary(c.range.start)
            && info.is_boundary(c.range.end)
    });
    cells
}

/// The x of a point on a line: the leading edge of the cell that starts
/// there, else the trailing edge of the cell that ends there, else the line's
/// start edge.
fn point_x(cells: &[Cell], at: usize, line: &LineLayout) -> Length {
    let rtl = |c: &Cell| c.level % 2 == 1;
    if let Some(cell) = cells.iter().find(|c| c.range.start == at) {
        return if rtl(cell) { cell.x1 } else { cell.x0 };
    }
    if let Some(cell) = cells.iter().find(|c| c.range.end == at) {
        return if rtl(cell) { cell.x0 } else { cell.x1 };
    }
    line.rect.origin.x
}

/// Resolves spans against one snapshot, remembering per-block work.
struct SpanResolver<'a> {
    snapshot: &'a LayoutSnapshot,
    /// Placed blocks by node, built on first use.
    blocks: Option<BTreeMap<NodeId, &'a BlockLayout>>,
    /// Block facts, keyed by the block layout's address (a key only; never
    /// iterated).
    info: BTreeMap<usize, BlockInfo>,
    /// Drawn cells of line fragments, keyed by block address and line.
    cells: BTreeMap<(usize, usize), Vec<Cell>>,
}

impl<'a> SpanResolver<'a> {
    fn new(snapshot: &'a LayoutSnapshot) -> Self {
        SpanResolver {
            snapshot,
            blocks: None,
            info: BTreeMap::new(),
            cells: BTreeMap::new(),
        }
    }

    fn block(&mut self, node: NodeId) -> Option<&'a BlockLayout> {
        let snapshot = self.snapshot;
        self.blocks
            .get_or_insert_with(|| {
                let mut map = BTreeMap::new();
                for block in &snapshot.blocks {
                    map.entry(block.node).or_insert(block);
                }
                map
            })
            .get(&node)
            .copied()
    }

    fn geometry(&mut self, pieces: &[(NodeId, Range<usize>)]) -> SpanGeometry {
        let mut out = SpanGeometry::default();
        let mut rects = Vec::new();
        for (node, bytes) in pieces {
            let Some(block) = self.block(*node) else {
                out.unplaced += 1;
                continue;
            };
            let before = rects.len();
            let complete = self.block_rects(block, bytes, false, &mut rects);
            if !complete {
                out.truncated = true;
            }
            if rects.len() == before {
                out.unplaced += 1;
            }
            if rects.len() >= MAX_SPAN_RECTS {
                rects.truncate(MAX_SPAN_RECTS);
                out.truncated = true;
                break;
            }
        }
        out.rects = rects;
        out.finish()
    }

    /// Appends the rectangles of `bytes` of `block`. False when a limit cut
    /// the list short.
    fn block_rects(
        &mut self,
        block: &BlockLayout,
        bytes: &Range<usize>,
        repeated: bool,
        out: &mut Vec<SpanRect>,
    ) -> bool {
        let key = block as *const BlockLayout as usize;
        let snapshot = self.snapshot;
        let info = self
            .info
            .entry(key)
            .or_insert_with(|| BlockInfo::new(&block.text));
        let mut complete = true;
        let point = bytes.is_empty();
        for (index, line) in block.lines.iter().enumerate() {
            let on_line = if point {
                line_holds_point(block, index, bytes.start)
            } else {
                line.text.start < bytes.end && bytes.start < line.text.end
            };
            if !on_line {
                continue;
            }
            let cells = self
                .cells
                .entry((key, index))
                .or_insert_with(|| line_cells(block, info, index, line));
            let Some(frame) = snapshot.frames.get(line.frame) else {
                continue;
            };
            let mut stretches: Vec<(Length, Length)> = Vec::new();
            if point {
                let x = point_x(cells, bytes.start, line);
                stretches.push((x, x));
            } else {
                for cell in cells.iter() {
                    let selected = cell.range.start < bytes.end && bytes.start < cell.range.end;
                    if !selected || cell.x0 == cell.x1 {
                        continue;
                    }
                    match stretches.last_mut() {
                        Some(last) if cell.x0 <= last.1 && cell.x1 >= last.0 => {
                            last.0 = last.0.min(cell.x0);
                            last.1 = last.1.max(cell.x1);
                        }
                        _ => stretches.push((cell.x0, cell.x1)),
                    }
                }
            }
            for (x0, x1) in stretches {
                if out.len() >= MAX_SPAN_RECTS {
                    complete = false;
                    break;
                }
                let rect = Rect::new(
                    Point::new(x0, line.rect.origin.y),
                    x1 - x0,
                    line.rect.height,
                );
                let corner = |x: Length, y: Length| frame.to_page.apply(Point::new(x, y));
                out.push(SpanRect {
                    page: frame.page,
                    frame: line.frame,
                    line: LineRef {
                        node: block.node,
                        line: index,
                    },
                    frame_rect: rect,
                    bounds: frame.to_page.bounds(&rect),
                    corners: [
                        corner(rect.origin.x, rect.origin.y),
                        corner(rect.max_x(), rect.origin.y),
                        corner(rect.max_x(), rect.max_y()),
                        corner(rect.origin.x, rect.max_y()),
                    ],
                    repeated,
                });
            }
        }
        complete
    }
}

/// A point belongs to the line that holds it; at a line break, the line after
/// it, except at the end of the block, which belongs to the last line.
fn line_holds_point(block: &BlockLayout, index: usize, at: usize) -> bool {
    let Some(line) = block.lines.get(index) else {
        return false;
    };
    if line.text.start <= at && at < line.text.end {
        return true;
    }
    index + 1 == block.lines.len() && at == line.text.end
}
