//! Logical selections, bidi geometry and policy-free gesture operations.
mod common;

use common::*;
use reprise_doc::text::segment;
use reprise_edit::{Caret, Movement, Navigator, Selection};
use reprise_fixtures::hostile;
use reprise_layout::LineRef;

#[test]
fn ranges_are_logical_and_direction_independent_across_blocks() {
    let (doc, ids, snapshot) = laid_out(&["office", "", "e\u{301}nd"]);
    let nav = Navigator::semantic(&snapshot, &doc);
    let selection = Selection {
        anchor: Caret::new(ids[2], 3),
        focus: Caret::new(ids[0], 2),
    };
    let ranges = nav.selection_ranges(&selection);
    assert_eq!(
        ranges
            .iter()
            .map(|r| (r.node, r.bytes.clone()))
            .collect::<Vec<_>>(),
        [(ids[0], 2..6), (ids[1], 0..0), (ids[2], 0..3)]
    );
    assert_eq!(
        ranges,
        nav.selection_ranges(&Selection {
            anchor: selection.focus,
            focus: selection.anchor
        })
    );
    let collapsed = Selection::collapsed(selection.focus);
    assert!(collapsed.is_collapsed());
    assert_eq!(nav.selection_ranges(&collapsed)[0].bytes, 2..2);
    assert!(nav.selection_rects(&collapsed).is_empty());
    for offset in [1, usize::MAX] {
        let bad = Selection {
            anchor: Caret::new(ids[2], offset),
            ..selection
        };
        assert!(nav.selection_ranges(&bad).is_empty());
        assert!(nav.selection_rects(&bad).is_empty());
    }
}

#[test]
fn a_logical_bidi_range_can_have_discontiguous_geometry() {
    let (doc, ids, snapshot) = laid_out(&["abc \u{5D0}\u{5D1}\u{5D2} xyz"]);
    let node = ids[0];
    let nav = Navigator::semantic(&snapshot, &doc);
    // The LTR prefix and only the logically first Hebrew character: that
    // character is visually at the right end of the reversed Hebrew run.
    let selection = Selection {
        anchor: Caret::new(node, 0),
        focus: Caret::upstream(node, 6),
    };
    assert_eq!(nav.selection_ranges(&selection)[0].bytes, 0..6);
    let rects = nav.selection_rects(&selection);
    assert_eq!(rects.len(), 2);
    assert!(rects[0].rect.max_x() < rects[1].rect.origin.x);
    let all = nav.select_all().unwrap();
    assert_eq!(
        nav.selection_rects(&all).len(),
        1,
        "adjacent cells coalesce"
    );
}

#[test]
fn gestures_select_words_gaps_lines_and_blocks_without_changing_text() {
    let (doc, ids, snapshot) = laid_out(&["Tom's   office\ne\u{301}nd"]);
    let nav = Navigator::semantic(&snapshot, &doc);
    let node = ids[0];
    let bytes = |s: Selection| nav.selection_ranges(&s)[0].bytes.clone();
    assert_eq!(bytes(nav.select_word(Caret::new(node, 2)).unwrap()), 0..5);
    assert_eq!(bytes(nav.select_word(Caret::new(node, 6)).unwrap()), 5..8);
    assert_eq!(bytes(nav.select_word(Caret::new(node, 9)).unwrap()), 8..14);
    assert_eq!(bytes(nav.select_line(Caret::new(node, 9)).unwrap()), 0..14);
    assert_eq!(bytes(nav.select_block(Caret::new(node, 9)).unwrap()), 0..20);
    assert_eq!(bytes(nav.select_all().unwrap()), 0..20);
    assert_eq!(
        doc.block(node).unwrap().text.to_string(),
        "Tom's   office\ne\u{301}nd"
    );
}

#[test]
fn ligature_selection_covers_only_the_selected_grapheme_share() {
    let (doc, ids, snapshot) = laid_out(&["office e\u{301}"]);
    let nav = Navigator::semantic(&snapshot, &doc);
    let node = ids[0];
    let cell = nav
        .graphemes(LineRef { node, line: 0 })
        .into_iter()
        .find(|c| c.range == (2..3))
        .unwrap();
    let selection = Selection {
        anchor: Caret::new(node, 2),
        focus: Caret::upstream(node, 3),
    };
    let rects = nav.selection_rects(&selection);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0].rect.width, cell.x1 - cell.x0);
    assert!(
        nav.select_word(Caret::new(node, 9)).is_none(),
        "inside a combining grapheme"
    );
}

#[test]
fn empty_documents_and_blocks_have_defined_navigation_and_selection() {
    let (doc, _, snapshot) = laid_out(&[]);
    let nav = Navigator::semantic(&snapshot, &doc);
    assert!(nav.reading_order().is_empty());
    assert!(nav.hit(0, pt(0, 0)).is_none());
    assert!(nav.select_all().is_none());

    let (doc, ids, snapshot) = laid_out(&["", "next", ""]);
    let nav = Navigator::semantic(&snapshot, &doc);
    for node in [ids[0], ids[2]] {
        let caret = Caret::new(node, 0);
        assert_eq!(nav.caret_positions(node), [caret]);
        for m in ALL_MOVEMENTS {
            assert!(nav.move_caret(caret, m).is_some());
        }
        for selection in [
            nav.select_word(caret),
            nav.select_line(caret),
            nav.select_block(caret),
        ] {
            let selection = selection.unwrap();
            assert!(selection.is_collapsed());
            assert!(nav.selection_rects(&selection).is_empty());
        }
        let r = nav.caret_rect(caret).unwrap();
        assert_eq!(nav.hit(r.page, r.point()).unwrap().caret, caret);
    }
    assert_eq!(
        nav.move_caret(Caret::new(ids[0], 0), Movement::NextGrapheme),
        Some(Caret::new(ids[1], 0))
    );
    assert_eq!(
        nav.move_caret(Caret::new(ids[2], 0), Movement::PreviousGrapheme),
        Some(Caret::upstream(ids[1], 4))
    );
    assert!(nav.line_after(ids[0], usize::MAX).is_none());
    assert!(nav.line_before(ids[0], usize::MAX).is_none());
}

#[test]
fn selections_cover_each_drawn_grapheme_on_every_hostile_fixture() {
    for fixture in hostile::all().unwrap() {
        let snapshot = fixture.engine.layout(&fixture.doc);
        let nav = Navigator::semantic(&snapshot, &fixture.doc);
        for at in lines_of(&snapshot) {
            let block = snapshot.block(at.node).unwrap();
            let line = &block.lines[at.line];
            let frame = snapshot.frame(line.frame).unwrap();
            for cell in nav.graphemes(at) {
                assert!(segment::is_grapheme_boundary(&block.text, cell.range.start));
                let selection = Selection {
                    anchor: Caret::new(at.node, cell.range.start),
                    focus: Caret::upstream(at.node, cell.range.end),
                };
                assert_eq!(nav.selection_ranges(&selection)[0].bytes, cell.range);
                let rects = nav.selection_rects(&selection);
                if cell.x0 == cell.x1 {
                    assert!(rects.is_empty());
                    continue;
                }
                let expected = frame.to_page.bounds(&reprise_geom::Rect::new(
                    reprise_geom::Point::new(cell.x0, line.rect.origin.y),
                    cell.x1 - cell.x0,
                    line.rect.height,
                ));
                assert!(
                    rects
                        .iter()
                        .any(|r| r.page == frame.page && r.rect == expected),
                    "{}: {:?} {:?}",
                    fixture.name,
                    at,
                    selection
                );
            }
        }
    }
}
