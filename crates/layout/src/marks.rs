//! Read-only formatting marks in page units. This query paints nothing.
use std::collections::BTreeMap;

use reprise_doc::{
    NodeId, RelationId,
    marks::{Alignment, is_line_break},
};
use reprise_geom::{FrameSpace, Length, PageSpace, Point};
use serde::Serialize;

use crate::{BlockLayout, LayoutSnapshot, LineLayout, LineRef, RelationStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarkKind {
    ParagraphEnd,
    LineBreak,
    Gap,
    Alignment,
    Anchor,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Mark {
    pub kind: MarkKind,
    pub node: NodeId,
    pub offset: usize,
    pub line: usize,
    pub from: Point<PageSpace>,
    pub to: Option<Point<PageSpace>>,
    pub target_page: Option<usize>,
    pub alignment: Option<Alignment>,
    pub relation: Option<RelationId>,
    pub state: Option<RelationStatus>,
    pub applied: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchorGuide {
    pub source: LineRef,
    pub offset: usize,
    pub from: Point<FrameSpace>,
    pub target: Option<(LineRef, Point<FrameSpace>)>,
}

/// Logical content edges, including expanded tabs, excluding hanging whitespace.
pub(crate) fn edges(block: &BlockLayout, line: &LineLayout) -> (Length, Length) {
    let left = line
        .runs
        .iter()
        .map(|r| r.x)
        .min()
        .unwrap_or(line.rect.origin.x);
    let right = left + line.width;
    if block.base_level % 2 == 1 {
        (right, left)
    } else {
        (left, right)
    }
}

/// Glyph cluster edges use the navigator's proportional grapheme subdivision.
/// At bidi junctions the following cluster's edge wins (downstream).
fn positions(
    block: &BlockLayout,
    line: &LineLayout,
) -> BTreeMap<usize, (Option<Length>, Option<Length>)> {
    let mut points = BTreeMap::<usize, (Option<Length>, Option<Length>)>::new();
    for run in &line.runs {
        if run.combined {
            let (a, z) = if run.level % 2 == 1 {
                (run.x + run.width, run.x)
            } else {
                (run.x, run.x + run.width)
            };
            points.entry(run.range.start).or_default().1 = Some(a);
            points.entry(run.range.end).or_default().0 = Some(z);
            continue;
        }
        let mut groups = BTreeMap::<usize, (Length, Length)>::new();
        let mut pen = run.x;
        for g in &run.glyphs {
            let x = pen;
            pen += g.advance;
            groups
                .entry(g.cluster as usize)
                .and_modify(|v| v.1 = pen)
                .or_insert((x, pen));
        }
        for (&cluster, &(left, right)) in &groups {
            let end = groups
                .range(cluster.saturating_add(1)..)
                .next()
                .map_or(run.range.end, |(&n, _)| n);
            let raw = block.text.get(cluster..end).unwrap_or_default();
            let bounds = reprise_doc::text::segment::grapheme_boundaries(raw);
            let count = bounds.len().saturating_sub(1).max(1);
            for (index, &relative) in bounds.iter().enumerate() {
                let part = (right - left).mul_ratio(
                    i32::try_from(index).unwrap_or(i32::MAX),
                    i32::try_from(count).unwrap_or(i32::MAX),
                );
                let x = if run.level % 2 == 1 {
                    right - part
                } else {
                    left + part
                };
                let point = points.entry(cluster.saturating_add(relative)).or_default();
                if index == count {
                    point.0 = Some(x);
                } else {
                    point.1 = Some(x);
                }
            }
        }
    }
    points
}
pub(crate) fn position(block: &BlockLayout, line: &LineLayout, at: usize) -> Length {
    position_in(block, line, at, &positions(block, line))
}
fn position_in(
    block: &BlockLayout,
    line: &LineLayout,
    at: usize,
    points: &BTreeMap<usize, (Option<Length>, Option<Length>)>,
) -> Length {
    points
        .get(&at)
        .and_then(|(up, down)| down.or(*up))
        .unwrap_or_else(|| {
            let (start, end) = edges(block, line);
            if at <= line.text.start { start } else { end }
        })
}

impl LayoutSnapshot {
    pub fn marks(&self, page: usize) -> Vec<Mark> {
        if self.pages.get(page).is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for block in &self.blocks {
            if block.image.is_some() {
                continue;
            }
            for (index, line) in block.lines.iter().enumerate() {
                let Some(frame) = self.frame(line.frame).filter(|f| f.page == page) else {
                    continue;
                };
                let points = positions(block, line);
                let point = |at| {
                    frame.to_page.apply(Point::new(
                        position_in(block, line, at, &points),
                        line.baseline,
                    ))
                };
                let mark = |kind, offset, from, to| Mark {
                    kind,
                    node: block.node,
                    offset,
                    line: index,
                    from,
                    to,
                    target_page: None,
                    alignment: None,
                    relation: None,
                    state: None,
                    applied: true,
                };
                for (relative, c) in block
                    .text
                    .get(line.text.clone())
                    .unwrap_or_default()
                    .char_indices()
                {
                    let at = line.text.start.saturating_add(relative);
                    if c == '\t' {
                        out.push(mark(
                            MarkKind::Gap,
                            at,
                            point(at),
                            Some(point(at.saturating_add(1))),
                        ));
                    } else if is_line_break(c) {
                        // CRLF is one authored break, not two labels.
                        if c == '\n' && block.text.get(..at).is_some_and(|s| s.ends_with('\r')) {
                            continue;
                        }
                        out.push(mark(MarkKind::LineBreak, at, point(at), None));
                    }
                }
                if line.text.end == block.text.len()
                    && index.saturating_add(1) == block.lines.len()
                    && (line.text.is_empty()
                        || !block.text.chars().next_back().is_some_and(is_line_break))
                {
                    out.push(mark(
                        MarkKind::ParagraphEnd,
                        block.text.len(),
                        point(block.text.len()),
                        None,
                    ));
                }
                if let Some(alignment) = line.alignment {
                    let mut label = mark(
                        MarkKind::Alignment,
                        line.text.start,
                        frame
                            .to_page
                            .apply(Point::new(edges(block, line).0, line.rect.origin.y)),
                        None,
                    );
                    label.alignment = Some(alignment);
                    out.push(label);
                }
            }
        }
        for relation in &self.relations {
            let Some(guide) = &relation.anchor else {
                continue;
            };
            let Some(line) = self.line(guide.source) else {
                continue;
            };
            let Some(frame) = self.frame(line.frame).filter(|f| f.page == page) else {
                continue;
            };
            let target = guide.target.and_then(|(at, point)| {
                self.line(at)
                    .and_then(|l| self.frame(l.frame))
                    .map(|f| (f.page, f.to_page.apply(point)))
            });
            out.push(Mark {
                kind: MarkKind::Anchor,
                node: guide.source.node,
                offset: guide.offset,
                line: guide.source.line,
                from: frame.to_page.apply(guide.from),
                to: target.map(|(_, p)| p),
                target_page: target.map(|(p, _)| p),
                alignment: None,
                relation: Some(relation.id),
                state: Some(relation.status),
                applied: relation.applied,
            });
        }
        out
    }
}
