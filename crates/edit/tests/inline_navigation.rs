//! Carets, selection, hit testing and deletion over inline objects
//! (`docs/inline.md`), in left-to-right and mixed-direction lines.

mod common;

use common::*;
use reprise_doc::text::OBJECT;
use reprise_edit::{Caret, Command, Editor, Movement, Navigator, Selection, Transaction};
use reprise_fixtures::hostile;
use reprise_geom::Length;
use reprise_layout::{LayoutSnapshot, LineRef};

const WIDTH: usize = OBJECT.len_utf8();

/// Every object on the snapshot's lines: its block, line and atom.
fn all_atoms(snapshot: &LayoutSnapshot) -> Vec<(LineRef, &reprise_layout::AtomLayout)> {
    snapshot
        .blocks
        .iter()
        .flat_map(|b| {
            b.lines.iter().enumerate().flat_map(move |(line, l)| {
                l.atoms
                    .iter()
                    .map(move |a| (LineRef { node: b.node, line }, a))
            })
        })
        .collect()
}

#[test]
fn carets_stop_before_and_after_an_object_and_never_inside() {
    let f = hostile::inline_bidi().unwrap();
    let snapshot = f.engine.layout(&f.doc);
    let nav = Navigator::semantic(&snapshot, &f.doc);
    let atoms = all_atoms(&snapshot);
    assert!(atoms.len() >= 8, "{} objects", atoms.len());
    for (at, atom) in atoms {
        let stops: Vec<usize> = nav
            .caret_positions(at.node)
            .into_iter()
            .map(|c| c.offset)
            .collect();
        assert!(stops.contains(&atom.at), "before {atom:?}");
        assert!(stops.contains(&(atom.at + WIDTH)), "after {atom:?}");
        assert!(!stops.contains(&(atom.at + 1)) && !stops.contains(&(atom.at + 2)));
        // The object is one cell as wide as its box.
        let cells = nav.graphemes(at);
        let cell = cells
            .iter()
            .find(|c| c.range == (atom.at..atom.at + WIDTH))
            .unwrap_or_else(|| panic!("no cell for {atom:?} in {cells:?}"));
        assert_eq!(cell.x1 - cell.x0, atom.rect.width);
    }
}

#[test]
fn a_visual_walk_crosses_each_object_in_one_step_in_both_directions() {
    let f = hostile::inline_bidi().unwrap();
    let snapshot = f.engine.layout(&f.doc);
    let nav = Navigator::semantic(&snapshot, &f.doc);
    let mut crossed = 0;
    for (at, _) in all_atoms(&snapshot) {
        let cells = nav.graphemes(at);
        let objects: Vec<_> = cells
            .iter()
            .filter(|c| {
                snapshot
                    .block(at.node)
                    .is_some_and(|b| b.text.get(c.range.clone()) == Some(&OBJECT.to_string()[..]))
            })
            .collect();
        for right in [true, false] {
            let carets = walk(&nav, at, right);
            // One step per cell, and a step across an object is exactly as
            // wide as the object.
            assert_eq!(carets.len(), cells.len() + 1);
            for pair in carets.windows(2) {
                let (a, b) = (x_of(&nav, &pair[0]), x_of(&nav, &pair[1]));
                let across = (b - a).0.abs();
                if objects
                    .iter()
                    .any(|c| (c.x1 - c.x0).0 == across && c.x0 >= a.min(b) && c.x1 <= a.max(b))
                {
                    crossed += 1;
                }
            }
        }
    }
    assert!(crossed >= 16, "{crossed} object crossings");
}

#[test]
fn logical_steps_jump_the_whole_object() {
    let f = hostile::inline_bidi().unwrap();
    let snapshot = f.engine.layout(&f.doc);
    let nav = Navigator::semantic(&snapshot, &f.doc);
    for (at, atom) in all_atoms(&snapshot) {
        let before = Caret::new(at.node, atom.at);
        let after = nav.move_caret(before, Movement::NextGrapheme).unwrap();
        assert_eq!(after.offset, atom.at + WIDTH, "{atom:?}");
        let back = nav.move_caret(after, Movement::PreviousGrapheme).unwrap();
        assert_eq!(back.offset, atom.at);
    }
}

#[test]
fn a_point_on_an_object_hits_its_node_and_the_nearer_edge() {
    let f = hostile::inline_bidi().unwrap();
    let snapshot = f.engine.layout(&f.doc);
    let nav = Navigator::semantic(&snapshot, &f.doc);
    for (at, atom) in all_atoms(&snapshot) {
        let rect = nav.atom_rect(at.node, atom.at).unwrap();
        assert_eq!(rect.line, at);
        let cell = nav
            .graphemes(at)
            .into_iter()
            .find(|c| c.range == (atom.at..atom.at + WIDTH))
            .unwrap();
        let frame = snapshot.frame(snapshot.line(at).unwrap().frame).unwrap();
        for quarters in [1, 3] {
            // A quarter and three quarters of the way across the object, left
            // to right in the frame.
            let x = cell.x0 + (cell.x1 - cell.x0).mul_ratio(quarters, 4);
            let y = atom.rect.origin.y + atom.rect.height.mul_ratio(1, 2);
            let hit = nav
                .hit(
                    rect.page,
                    frame.to_page.apply(reprise_geom::Point::new(x, y)),
                )
                .unwrap();
            assert_eq!(hit.caret.node, at.node);
            assert!(hit.inside, "{atom:?}");
            // The left edge is the start of a left-to-right object and the
            // end of a right-to-left one.
            let rtl = cell.level % 2 == 1;
            let left = quarters == 1;
            let expected = if left != rtl {
                atom.at
            } else {
                atom.at + WIDTH
            };
            assert_eq!(hit.caret.offset, expected, "{quarters}/4 of {atom:?}");
        }
    }
}

#[test]
fn a_selection_over_an_object_is_one_unit_with_a_rectangle() {
    let f = hostile::inline_bidi().unwrap();
    let snapshot = f.engine.layout(&f.doc);
    let nav = Navigator::semantic(&snapshot, &f.doc);
    for (at, atom) in all_atoms(&snapshot) {
        let selection = Selection {
            anchor: Caret::new(at.node, atom.at),
            focus: Caret::new(at.node, atom.at + WIDTH),
        };
        let ranges = nav.selection_ranges(&selection);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].bytes, atom.at..atom.at + WIDTH);
        // The selection's rectangles cover the object's own.
        let rect = nav.atom_rect(at.node, atom.at).unwrap();
        let covered: Length = nav
            .selection_rects(&selection)
            .iter()
            .filter(|r| r.page == rect.page)
            .map(|r| {
                let (lo, hi) = (
                    r.rect.origin.x.max(rect.rect.origin.x),
                    r.rect.max_x().min(rect.rect.max_x()),
                );
                (hi - lo).max(Length::ZERO)
            })
            .sum();
        assert!(covered >= rect.rect.width.mul_ratio(99, 100), "{atom:?}");
        // A selection never starts or ends inside the object's character.
        let word = nav.select_word(Caret::new(at.node, atom.at));
        if let Some(w) = word {
            for end in [w.anchor.offset, w.focus.offset] {
                assert!(end != atom.at + 1 && end != atom.at + 2);
            }
        }
    }
}

#[test]
fn an_object_has_no_rectangle_until_it_is_placed() {
    let f = hostile::inline_bidi().unwrap();
    let snapshot = f.engine.layout(&f.doc);
    let nav = Navigator::semantic(&snapshot, &f.doc);
    let node = nav.reading_order()[0];
    // Offset 1 is inside the first object's character; 4 is on a letter.
    assert!(nav.atom_rect(node, 1).is_none());
    assert!(nav.atom_rect(node, 5).is_none());
    assert!(nav.atom_rect(node, usize::MAX).is_none());
}

#[test]
fn backspace_deletes_an_object_and_undo_brings_it_back() {
    let f = hostile::inline_bidi().unwrap();
    let snapshot = f.engine.layout(&f.doc);
    let (at, atom) = all_atoms(&snapshot)
        .into_iter()
        .find(|(_, a)| a.kind == "image")
        .unwrap();
    let before = f.doc.inline_objects(at.node).unwrap();
    let removed = before.iter().find(|e| e.at == atom.at).unwrap().clone();
    // Backspace with the caret after the object steps back one grapheme and
    // deletes it.
    let nav = Navigator::semantic(&snapshot, &f.doc);
    let caret = Caret::new(at.node, atom.at + WIDTH);
    let start = nav.move_caret(caret, Movement::PreviousGrapheme).unwrap();
    assert_eq!(start.offset, atom.at);
    let mut editor = Editor::new(f.doc, reprise_doc::SchemaRegistry::builtin());
    editor
        .apply(&Transaction::new().with(Command::DeleteText {
            node: at.node,
            range: start.offset..caret.offset,
        }))
        .unwrap();
    let after = editor.document().inline_objects(at.node).unwrap();
    assert_eq!(after.len(), before.len() - 1);
    assert!(after.iter().all(|e| e.id != removed.id));
    let text = editor.document().block(at.node).unwrap().text.to_string();
    assert_eq!(text.matches(OBJECT).count(), after.len());
    // Undo restores the same object, with its parameters.
    assert!(editor.undo().unwrap());
    let restored = editor.document().inline_objects(at.node).unwrap();
    assert_eq!(restored, before);
    assert_eq!(
        editor.document().inline_object(removed.id).unwrap(),
        removed.object
    );
}
