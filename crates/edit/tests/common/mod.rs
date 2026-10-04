//! Helpers shared by the navigation tests.
#![allow(dead_code)]

use reprise_doc::{BlockKind, Document, NodeId};
use reprise_edit::{Caret, CaretRect, Movement, Navigator};
use reprise_fixtures::engine;
use reprise_geom::{Length, PageSpace, Point};
use reprise_layout::{LayoutSnapshot, LineRef};

pub fn doc_with(blocks: &[&str]) -> (Document, Vec<NodeId>) {
    let doc = Document::new(1).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    let ids = blocks
        .iter()
        .map(|t| doc.append_block(BlockKind::Paragraph, "body", t).unwrap())
        .collect();
    (doc, ids)
}

pub fn laid_out(blocks: &[&str]) -> (Document, Vec<NodeId>, LayoutSnapshot) {
    let (doc, ids) = doc_with(blocks);
    let snapshot = engine().layout(&doc);
    (doc, ids, snapshot)
}

pub fn pt(x: i32, y: i32) -> Point<PageSpace> {
    Point::new(Length(x), Length(y))
}

/// Every caret position of every block, with where it is drawn.
pub fn all_carets(nav: &Navigator) -> Vec<(Caret, CaretRect)> {
    nav.reading_order()
        .iter()
        .flat_map(|&n| nav.caret_positions(n))
        .map(|c| {
            (
                c,
                nav.caret_rect(c).expect("every caret position has a rect"),
            )
        })
        .collect()
}

pub fn lines_of(snapshot: &LayoutSnapshot) -> Vec<LineRef> {
    snapshot
        .blocks
        .iter()
        .flat_map(|b| (0..b.lines.len()).map(|line| LineRef { node: b.node, line }))
        .collect()
}

/// Walks a line from one visual end to the other with visual steps.
pub fn walk(nav: &Navigator, at: LineRef, right: bool) -> Vec<Caret> {
    walk_with(nav, at, right, false)
}

/// Traverse frame-inline graphemes irrespective of page rotation/mirroring.
pub fn walk_inline(nav: &Navigator, at: LineRef, forward: bool) -> Vec<Caret> {
    walk_with(nav, at, forward, true)
}

fn walk_with(nav: &Navigator, at: LineRef, right: bool, inline: bool) -> Vec<Caret> {
    let edge = if right { Length::MIN } else { Length::MAX };
    let hit = nav.caret_at_x(at, edge).expect("a line has a caret");
    // A point cannot distinguish coincident zero-width cells. Explicit
    // visual-edge movement identifies the junction and must not skip cells.
    let mut caret = nav
        .move_caret(
            hit,
            if inline && right {
                Movement::LineInlineStart
            } else if inline {
                Movement::LineInlineEnd
            } else if right {
                Movement::LineLeftmost
            } else {
                Movement::LineRightmost
            },
        )
        .unwrap();
    let mut visited = vec![caret];
    let movement = if inline && right {
        Movement::InlineForward
    } else if inline {
        Movement::InlineBackward
    } else if right {
        Movement::VisualRight
    } else {
        Movement::VisualLeft
    };
    for _ in 0..nav.graphemes(at).len() + 2 {
        let next = nav.move_caret(caret, movement).expect("a caret moves");
        if next == caret || nav.caret_rect(next).map(|r| r.line) != Some(at) {
            break;
        }
        visited.push(next);
        caret = next;
    }
    visited
}

pub fn x_of(nav: &Navigator, c: &Caret) -> Length {
    nav.caret_rect(*c).expect("a rect").x
}

pub const ALL_MOVEMENTS: [Movement; 20] = [
    Movement::NextGrapheme,
    Movement::PreviousGrapheme,
    Movement::NextWord,
    Movement::PreviousWord,
    Movement::VisualRight,
    Movement::VisualLeft,
    Movement::InlineForward,
    Movement::InlineBackward,
    Movement::LineUp,
    Movement::LineDown,
    Movement::LineStart,
    Movement::LineEnd,
    Movement::LineLeftmost,
    Movement::LineRightmost,
    Movement::LineInlineStart,
    Movement::LineInlineEnd,
    Movement::BlockStart,
    Movement::BlockEnd,
    Movement::DocumentStart,
    Movement::DocumentEnd,
];
