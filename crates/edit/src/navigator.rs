//! The navigator: caret geometry and hit testing over a layout snapshot (30).
//!
//! A [`Navigator`] is built for one [`LayoutSnapshot`] and answers questions
//! about it: where a caret is drawn, which caret a page point means, how a
//! caret moves ([`crate::movement`]) and what a selection covers
//! ([`crate::select`]). It never changes the document, and it works from the
//! snapshot alone, so a stale navigator is a navigator of an old layout, not
//! a wrong answer about the new one.
//!
//! # Reading order (33)
//!
//! Movement from block to block follows a *reading order*, not the geometry:
//! the snapshot's document reading order by default ([`Navigator::semantic`]),
//! including authored `reprise.reading-order` precedence.
//! [`Navigator::semantic`] consumes [`LayoutSnapshot::reading_order`], so
//! layout resolves document precedence before navigation. A caller-supplied
//! order plugs in at [`Navigator::new`], as a list of blocks.
//!
//! # What a caret position is
//!
//! A caret is a block, a byte offset on a grapheme boundary, and an
//! [`Affinity`]. Several carets can be drawn in the same place, and one offset
//! can be drawn in two places, so [`Navigator::normalize`] picks one
//! representative affinity per logical position. Hit testing returns a
//! deterministic representative when positions coincide; rect then hit then
//! rect preserves geometry, with exact identity where the position is unique.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use reprise_doc::{Document, NodeId};
use reprise_geom::{Length, PageSpace, Point, Rect};
use reprise_layout::{BlockLayout, LayoutSnapshot, LineLayout, LineRef};

use crate::caret::{Affinity, Caret, CaretRect, GraphemeCell, Hit};
use crate::model::{BlockInfo, LineModel, Stop, gap};

type InsideCandidate = (
    (i128, (usize, usize)),
    LineRef,
    Point<reprise_geom::FrameSpace>,
);

pub struct Navigator<'a> {
    pub(crate) snapshot: &'a LayoutSnapshot,
    /// The reading order: every block of the snapshot that has a line, once.
    pub(crate) order: Vec<NodeId>,
    pub(crate) rank: BTreeMap<NodeId, usize>,
    /// Index into `snapshot.blocks`.
    pub(crate) position: BTreeMap<NodeId, usize>,
    pub(crate) info: BTreeMap<NodeId, BlockInfo>,
    /// Each line's model, made the first time it is asked for.
    models: BTreeMap<NodeId, Vec<OnceLock<LineModel>>>,
}

impl<'a> Navigator<'a> {
    /// A navigator that reads blocks in `order`. Blocks the snapshot doesn't
    /// have, and repeats, are left out; blocks it has that `order` doesn't
    /// name follow, in snapshot order, so every laid-out block can be reached.
    pub fn new(snapshot: &'a LayoutSnapshot, order: impl IntoIterator<Item = NodeId>) -> Self {
        let position: BTreeMap<NodeId, usize> = snapshot
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.lines.is_empty())
            .map(|(i, b)| (b.node, i))
            .collect();
        let mut seen = BTreeSet::new();
        let mut ordered: Vec<NodeId> = order
            .into_iter()
            .filter(|n| position.contains_key(n) && seen.insert(*n))
            .collect();
        ordered.extend(
            snapshot
                .blocks
                .iter()
                .map(|b| b.node)
                .filter(|n| position.contains_key(n) && seen.insert(*n)),
        );
        let rank = ordered.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let info = position
            .iter()
            .map(|(n, &i)| {
                let block = &snapshot.blocks[i];
                (*n, BlockInfo::new(&block.text, block.base_level))
            })
            .collect();
        let models = position
            .iter()
            .map(|(n, &i)| {
                let lines = snapshot.blocks[i].lines.len();
                (*n, (0..lines).map(|_| OnceLock::new()).collect())
            })
            .collect();
        Navigator {
            snapshot,
            order: ordered,
            rank,
            position,
            info,
            models,
        }
    }

    /// A navigator using the snapshot's semantic reading order and authored
    /// precedence (33). Pass the same document revision used for layout.
    pub fn semantic(snapshot: &'a LayoutSnapshot, doc: &Document) -> Self {
        Navigator::new(
            snapshot,
            snapshot
                .reading_order(doc)
                .into_iter()
                .map(|step| step.line.node),
        )
    }

    pub fn snapshot(&self) -> &'a LayoutSnapshot {
        self.snapshot
    }

    /// The blocks in reading order.
    pub fn reading_order(&self) -> &[NodeId] {
        &self.order
    }

    pub(crate) fn block(&self, node: NodeId) -> Option<&'a BlockLayout> {
        self.position
            .get(&node)
            .and_then(|&i| self.snapshot.blocks.get(i))
    }

    pub(crate) fn info(&self, node: NodeId) -> Option<&BlockInfo> {
        self.info.get(&node)
    }

    /// The model of line `line` of `node`.
    pub(crate) fn line_model(
        &self,
        node: NodeId,
        line: usize,
    ) -> Option<(&LineModel, &'a LineLayout)> {
        let block = self.block(node)?;
        let layout = block.lines.get(line)?;
        let cell = self.models.get(&node)?.get(line)?;
        let info = self.info.get(&node)?;
        let model = cell.get_or_init(|| LineModel::new(block, info, line, layout));
        Some((model, layout))
    }

    /// The line that draws a caret at `offset` with `affinity`: the line
    /// holding the offset, and at a soft line break the earlier one for an
    /// upstream caret and the later one for a downstream one.
    pub(crate) fn line_of(&self, node: NodeId, offset: usize, affinity: Affinity) -> Option<usize> {
        let block = self.block(node)?;
        let info = self.info(node)?;
        let holding: Vec<usize> = (0..block.lines.len())
            .filter(|&i| {
                let line = &block.lines[i];
                line.text.start <= offset && offset <= crate::model::end_limit(block, info, i)
            })
            .collect();
        match (holding.first(), holding.last(), affinity) {
            (Some(&first), _, Affinity::Upstream) => Some(first),
            (_, Some(&last), Affinity::Downstream) => Some(last),
            _ => None,
        }
    }

    /// Whether `caret` names a place in the snapshot: its block is laid out
    /// and its offset is a grapheme boundary of the block's text.
    fn valid(&self, caret: &Caret) -> bool {
        self.info(caret.node)
            .is_some_and(|i| i.is_boundary(caret.offset))
    }

    /// The caret of `stop` on `node`.
    pub(crate) fn caret_of(&self, node: NodeId, stop: &Stop) -> Caret {
        Caret {
            node,
            offset: stop.offset,
            affinity: stop.affinity,
        }
    }

    /// The line and stop where `caret` is drawn.
    pub(crate) fn locate(&self, caret: &Caret) -> Option<(usize, &LineModel, Stop)> {
        if !self.valid(caret) {
            return None;
        }
        let line = self.line_of(caret.node, caret.offset, caret.affinity)?;
        let (model, _) = self.line_model(caret.node, line)?;
        let stop = model.locate(caret.offset, caret.affinity);
        Some((line, model, stop))
    }

    /// The representative of the place `caret` is drawn at. `None` when the
    /// caret isn't a place in this snapshot: its block isn't laid out, or its
    /// offset isn't a grapheme boundary.
    pub fn normalize(&self, caret: Caret) -> Option<Caret> {
        let (_, model, stop) = self.locate(&caret)?;
        Some(self.caret_of(caret.node, &model.canonical(stop)))
    }

    /// Where a caret is drawn: a page and a rectangle in page space.
    pub fn caret_rect(&self, caret: Caret) -> Option<CaretRect> {
        let (line, _, stop) = self.locate(&caret)?;
        let layout = self.block(caret.node)?.lines.get(line)?;
        let at = LineRef {
            node: caret.node,
            line,
        };
        let frame = self.snapshot.frame(layout.frame)?;
        let r = Rect::new(
            Point::new(stop.x, layout.rect.origin.y),
            Length::ZERO,
            layout.rect.height,
        );
        Some(CaretRect {
            page: frame.page,
            rect: frame.to_page.bounds(&r),
            line: at,
            x: stop.x,
            level: stop.level,
        })
    }

    /// Every place a caret can be drawn in `node`, as normalized carets in
    /// logical order. Upstream carets appear only where they differ from the
    /// downstream one: the ends of soft-wrapped lines and bidi boundaries.
    pub fn caret_positions(&self, node: NodeId) -> Vec<Caret> {
        let Some(block) = self.block(node) else {
            return Vec::new();
        };
        let mut found = BTreeSet::new();
        for line in 0..block.lines.len() {
            let Some((model, _)) = self.line_model(node, line) else {
                continue;
            };
            if model.stops.is_empty() {
                found.insert(self.caret_of(node, &model.empty_stop()));
            }
            for stop in &model.stops {
                found.insert(self.caret_of(node, &model.canonical(*stop)));
            }
        }
        found.into_iter().collect()
    }

    /// The graphemes of a line with their extents, in visual order.
    pub fn graphemes(&self, at: LineRef) -> Vec<GraphemeCell> {
        self.line_model(at.node, at.line)
            .map(|(m, _)| m.cells.clone())
            .unwrap_or_default()
    }

    /// The caret on line `at` nearest to `x` in the frame's space.
    pub fn caret_at_x(&self, at: LineRef, x: Length) -> Option<Caret> {
        let (model, _) = self.line_model(at.node, at.line)?;
        Some(self.caret_of(at.node, &model.stop_at_x(x)))
    }

    /// Hit testing: the caret a point on `page` means.
    ///
    /// -   A point inside a line gives the nearer edge of the grapheme under
    ///     it, so the caret goes where the pointer is, not always after the
    ///     character. A ligature is shared out evenly between its graphemes,
    ///     and a right-to-left run reads from its right edge.
    /// -   A point between lines or outside the frame gives the nearest line,
    ///     first by the block axis, then by the inline axis.
    /// -   A point outside every frame gives the nearest frame that has text,
    ///     measured in the frame's own space, so rotated and mirrored frames
    ///     work.
    /// -   A page with no text gives the end of the text before it, or the
    ///     start of the text after it if there is none before. `None` only if
    ///     the document has no text at all, or `page` doesn't exist.
    ///
    /// Any point works, including `Length::MIN` and `Length::MAX`.
    pub fn hit(&self, page: usize, point: Point<PageSpace>) -> Option<Hit> {
        if page >= self.snapshot.pages.len() {
            return None;
        }
        // Lines on this page, with the frames they are in.
        let mut by_frame: BTreeMap<usize, Vec<LineRef>> = BTreeMap::new();
        for block in &self.snapshot.blocks {
            if !self.position.contains_key(&block.node) {
                continue;
            }
            for (i, line) in block.lines.iter().enumerate() {
                if self
                    .snapshot
                    .frame(line.frame)
                    .is_some_and(|f| f.page == page)
                {
                    by_frame.entry(line.frame).or_default().push(LineRef {
                        node: block.node,
                        line: i,
                    });
                }
            }
        }
        // The point in each frame's own space. A frame that can't be inverted
        // can't be hit.
        let spaces: Vec<(usize, Point<reprise_geom::FrameSpace>)> = by_frame
            .keys()
            .filter_map(|&index| {
                let frame = self.snapshot.frame(index)?;
                Some((index, frame.to_page.inverse()?.apply(point)))
            })
            .collect();
        // Lines with their extent: the line box and whatever its text
        // overflows into. A point inside one is on that line, whichever frame
        // it also happens to be inside, so a caret drawn past the end of an
        // overflowing line is still hit on it.
        let extent = |r: &LineRef, p: &Point<reprise_geom::FrameSpace>| {
            let l = self.block(r.node).and_then(|b| b.lines.get(r.line))?;
            let (mut x0, mut x1) = (l.rect.origin.x, l.rect.max_x());
            for run in &l.runs {
                x0 = x0.min(run.x).min(run.x + run.width);
                x1 = x1.max(run.x).max(run.x + run.width);
            }
            let dy = gap(l.rect.origin.y, l.rect.max_y(), p.y);
            let dx = gap(x0, x1, p.x);
            Some((dy, dx))
        };
        let rank = |r: &LineRef| {
            (
                self.rank.get(&r.node).copied().unwrap_or(usize::MAX),
                r.line,
            )
        };
        // Several lines can contain the point (overflow, or a note drawn over
        // text): the one with a caret nearest the point wins, then the earlier
        // in reading order.
        let mut inside: Option<InsideCandidate> = None;
        for (frame, p) in &spaces {
            for r in &by_frame[frame] {
                let Some((model, _)) = self.line_model(r.node, r.line) else {
                    continue;
                };
                // Local distances from differently rotated/scaled strips are
                // not comparable. The page-space caret centre also accounts
                // for fixed-point inverse rounding at an overlapping strip.
                let caret = self.caret_of(r.node, &model.stop_at_x(p.x));
                let Some(rect) = self.caret_rect(caret) else {
                    continue;
                };
                let centre = rect.point();
                let dx = i128::from(centre.x.0) - i128::from(point.x.0);
                let dy = i128::from(centre.y.0) - i128::from(point.y.0);
                let distance = dx * dx + dy * dy;
                // Inverting a rounded transform can put an exact drawn edge
                // one unit outside its line. An exact page-space caret centre
                // remains a candidate, even then.
                if extent(r, p) != Some((0, 0)) && distance != 0 {
                    continue;
                }
                let key = (distance, rank(r));
                if inside.as_ref().is_none_or(|(k, ..)| key < *k) {
                    inside = Some((key, *r, *p));
                }
            }
        }
        let inside = inside.map(|(_, r, p)| (r, p));
        if spaces.is_empty() {
            return self.nearest_text(page);
        }
        let (line, p) = match inside {
            Some(found) => found,
            None => {
                // Outside every line: the nearest frame with text, then the
                // nearest line in it by block axis, then inline axis, then
                // reading order.
                let (frame, p) = spaces
                    .iter()
                    .min_by_key(|(index, p)| {
                        self.snapshot.frame(*index).map_or(i64::MAX, |f| {
                            gap(f.rect.origin.x, f.rect.max_x(), p.x).saturating_add(gap(
                                f.rect.origin.y,
                                f.rect.max_y(),
                                p.y,
                            ))
                        })
                    })
                    .copied()?;
                let line = by_frame[&frame].iter().copied().min_by_key(|r| {
                    let (dy, dx) = extent(r, &p).unwrap_or((i64::MAX, i64::MAX));
                    (dy, dx, rank(r))
                })?;
                (line, p)
            }
        };
        let inside = extent(&line, &p) == Some((0, 0));
        let (model, _) = self.line_model(line.node, line.line)?;
        let stop = model.stop_at_x(p.x);
        Some(Hit {
            caret: self.caret_of(line.node, &stop),
            line,
            page,
            inside,
        })
    }

    /// For a page with no text: the end of the nearest text before it, else
    /// the start of the nearest after it.
    fn nearest_text(&self, page: usize) -> Option<Hit> {
        let on = |p: usize| {
            self.order
                .iter()
                .filter_map(|n| self.block(*n))
                .flat_map(|b| {
                    b.lines.iter().enumerate().filter_map(move |(i, l)| {
                        self.snapshot
                            .frame(l.frame)
                            .is_some_and(|f| f.page == p)
                            .then_some(LineRef {
                                node: b.node,
                                line: i,
                            })
                    })
                })
                .collect::<Vec<_>>()
        };
        let (line, at_end) = (0..page)
            .rev()
            .find_map(|p| on(p).last().copied())
            .map(|l| (l, true))
            .or_else(|| {
                (page + 1..self.snapshot.pages.len())
                    .find_map(|p| on(p).first().copied())
                    .map(|l| (l, false))
            })?;
        let (model, _) = self.line_model(line.node, line.line)?;
        let stop = if at_end {
            model.canonical(model.locate(model.end_limit, Affinity::Upstream))
        } else {
            model.canonical(model.locate(model.start, Affinity::Downstream))
        };
        Some(Hit {
            caret: self.caret_of(line.node, &stop),
            line,
            page,
            inside: false,
        })
    }
}
