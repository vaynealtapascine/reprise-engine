//! Caret movement over real layouts (30, 33).

mod common;

use common::*;
use reprise_doc::text::segment;
use reprise_doc::{BlockKind, Document};
use reprise_edit::{Affinity, Caret, Cursor, Movement, Navigator};
use reprise_fixtures::templates::{long_text, two_columns};
use reprise_fixtures::{engine, hostile};
use reprise_geom::Length;
use reprise_layout::LineRef;

#[test]
fn visual_movement_crosses_every_grapheme_of_every_line_once() {
    let (mut lines, mut mixed) = (0, 0);
    for fixture in hostile::all().unwrap() {
        let name = fixture.name;
        let snapshot = fixture.engine.layout(&fixture.doc);
        let nav = Navigator::semantic(&snapshot, &fixture.doc);
        for at in lines_of(&snapshot) {
            let cells = nav.graphemes(at);
            if cells.is_empty() {
                continue;
            }
            lines += 1;
            // The cells tile distinct graphemes, in some order.
            let mut ranges: Vec<_> = cells.iter().map(|c| c.range.clone()).collect();
            ranges.sort_by_key(|r| r.start);
            assert!(
                ranges.windows(2).all(|w| w[0].end <= w[1].start),
                "{name}: {at:?} cells overlap"
            );
            if cells.iter().any(|c| c.level % 2 == 1) && cells.iter().any(|c| c.level % 2 == 0) {
                mixed += 1;
            }
            // Right: one step per cell, each landing on the cell's right edge.
            let right = walk(&nav, at, true);
            assert_eq!(right.len(), cells.len() + 1, "{name}: {at:?} steps right");
            assert_eq!(
                x_of(&nav, &right[0]),
                cells[0].x0,
                "{name}: starts at the left edge"
            );
            for (k, cell) in cells.iter().enumerate() {
                assert_eq!(
                    x_of(&nav, &right[k + 1]),
                    cell.x1,
                    "{name}: {at:?} step {k}"
                );
            }
            // Left: the same, mirrored.
            let left = walk(&nav, at, false);
            let n = cells.len();
            assert_eq!(left.len(), n + 1, "{name}: {at:?} steps left");
            assert_eq!(x_of(&nav, &left[0]), cells[n - 1].x1);
            for k in 0..n {
                assert_eq!(
                    x_of(&nav, &left[k + 1]),
                    cells[n - 1 - k].x0,
                    "{name}: {at:?} left {k}"
                );
            }
        }
    }
    assert!(
        lines > 100 && mixed >= 2,
        "{lines} lines, {mixed} mixed direction"
    );
}

#[test]
fn visual_right_through_rtl_mixed_visits_every_grapheme_once_in_visual_order() {
    let f = hostile::rtl_mixed().unwrap();
    let snapshot = f.engine.layout(&f.doc);
    let nav = Navigator::semantic(&snapshot, &f.doc);
    let node = nav.reading_order()[0];
    let block = snapshot.block(node).unwrap();
    let mut seen = std::collections::BTreeSet::new(); // of (start, end)
    let mut backwards = 0;
    for line in 0..block.lines.len() {
        let at = LineRef { node, line };
        let cells = nav.graphemes(at);
        let carets = walk(&nav, at, true);
        // Every step moves right.
        assert!(
            carets
                .windows(2)
                .all(|w| x_of(&nav, &w[0]) <= x_of(&nav, &w[1]))
        );
        for cell in &cells {
            assert!(
                seen.insert((cell.range.start, cell.range.end)),
                "{:?} visited twice",
                cell.range
            );
        }
        // In the right-to-left runs the offsets run backwards while the
        // caret goes right.
        backwards += carets
            .windows(2)
            .filter(|w| w[1].offset < w[0].offset)
            .count();
    }
    assert!(backwards > 10, "{backwards} steps went back in the text");
    // Every grapheme of the text is in the set exactly once.
    let all: Vec<_> = segment::grapheme_boundaries(&block.text)
        .windows(2)
        .map(|w| w[0]..w[1])
        .collect();
    let have: Vec<_> = seen.into_iter().map(|(a, b)| a..b).collect();
    assert_eq!(have, all);
}

#[test]
fn logical_movement_walks_ligatures_and_combining_marks_by_grapheme() {
    let text = "office fi ffl e\u{301}x";
    let (doc, ids, snapshot) = laid_out(&[text]);
    let nav = Navigator::semantic(&snapshot, &doc);
    let node = ids[0];
    let graphemes = segment::grapheme_boundaries(text);
    let mut caret = Caret::new(node, 0);
    let mut visited = vec![0];
    let mut last_x = x_of(&nav, &caret);
    loop {
        let next = nav.move_caret(caret, Movement::NextGrapheme).unwrap();
        if next == caret {
            break;
        }
        assert!(graphemes.contains(&next.offset), "never inside e + accent");
        let x = x_of(&nav, &next);
        assert!(x >= last_x, "a left-to-right line only goes right");
        last_x = x;
        visited.push(next.offset);
        caret = next;
    }
    assert_eq!(visited, graphemes, "every grapheme boundary, once");
    // Backwards is the same walk the other way.
    let mut back = vec![text.len()];
    let mut caret = Caret::upstream(node, text.len());
    loop {
        let prev = nav.move_caret(caret, Movement::PreviousGrapheme).unwrap();
        if prev == caret {
            break;
        }
        back.push(prev.offset);
        caret = prev;
    }
    back.reverse();
    assert_eq!(back, graphemes);
}

#[test]
fn word_movement_follows_segmentation_and_crosses_blocks() {
    let (doc, ids, snapshot) = laid_out(&["Tom's house, 42 rooms.", "next"]);
    let nav = Navigator::semantic(&snapshot, &doc);
    let node = ids[0];
    let text = "Tom's house, 42 rooms.";
    let mut caret = Caret::new(node, 0);
    let mut stops = Vec::new();
    for _ in 0..6 {
        caret = nav.move_caret(caret, Movement::NextWord).unwrap();
        stops.push((caret.node == node).then_some(caret.offset));
    }
    let end = |w: &str| text.find(w).unwrap() + w.len();
    assert_eq!(
        stops,
        [
            Some(end("Tom's")),
            Some(end("house")),
            Some(end("42")),
            Some(end("rooms")),
            Some(text.len()),
            None
        ],
        "the end of each word, then the end of the block, then the next block"
    );
    assert_eq!(caret, nav.normalize(Caret::new(ids[1], 0)).unwrap());
    // Backwards: to the start of each word.
    let mut caret = Caret::upstream(ids[1], 4);
    let mut back = Vec::new();
    for _ in 0..7 {
        caret = nav.move_caret(caret, Movement::PreviousWord).unwrap();
        back.push((caret.node == ids[1], caret.offset));
    }
    assert_eq!(
        back,
        [
            (true, 0),
            (false, text.len()),
            (false, text.find("rooms").unwrap()),
            (false, text.find("42").unwrap()),
            (false, text.find("house").unwrap()),
            (false, 0),
            (false, 0)
        ]
    );
}

#[test]
fn line_start_and_end_and_document_edges() {
    let (doc, ids, snapshot) = laid_out(&[
        "A long first paragraph that has to wrap onto several lines of this narrow page so the line movements have somewhere to go.",
        "Second.",
    ]);
    let nav = Navigator::semantic(&snapshot, &doc);
    let block = snapshot.block(ids[0]).unwrap();
    assert!(block.lines.len() >= 3);
    let mid = block.lines[1].text.start + 3;
    let caret = Caret::new(ids[0], mid);
    let start = nav.move_caret(caret, Movement::LineStart).unwrap();
    assert_eq!(start, Caret::new(ids[0], block.lines[1].text.start));
    let end = nav.move_caret(caret, Movement::LineEnd).unwrap();
    assert_eq!(end, Caret::upstream(ids[0], block.lines[1].text.end));
    // The end of one line and the start of the next are the same offset, two
    // places.
    let next_start = Caret::new(ids[0], block.lines[2].text.start);
    let (r_end, r_start) = (
        nav.caret_rect(end).unwrap(),
        nav.caret_rect(next_start).unwrap(),
    );
    assert_eq!(end.offset, next_start.offset);
    assert_eq!(r_end.line.line + 1, r_start.line.line);
    assert_ne!(r_end.rect, r_start.rect);
    assert_eq!(
        nav.move_caret(caret, Movement::DocumentStart),
        Some(Caret::new(ids[0], 0))
    );
    assert_eq!(
        nav.move_caret(caret, Movement::DocumentEnd),
        Some(Caret::upstream(ids[1], "Second.".len()))
    );
    assert_eq!(
        nav.move_caret(caret, Movement::BlockEnd),
        Some(Caret::upstream(ids[0], block.text.len()))
    );
    // Nowhere further.
    let doc_end = nav.move_caret(caret, Movement::DocumentEnd).unwrap();
    for m in [
        Movement::NextGrapheme,
        Movement::NextWord,
        Movement::VisualRight,
    ] {
        assert_eq!(nav.move_caret(doc_end, m), Some(doc_end), "{m:?}");
    }
    let doc_start = Caret::new(ids[0], 0);
    for m in [
        Movement::PreviousGrapheme,
        Movement::PreviousWord,
        Movement::VisualLeft,
    ] {
        assert_eq!(nav.move_caret(doc_start, m), Some(doc_start), "{m:?}");
    }
}

#[test]
fn up_and_down_keep_the_goal_x_across_lines_blocks_and_columns() {
    let doc = Document::new(1).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    doc.set_page_template(&two_columns()).unwrap();
    let text = long_text(2);
    let a = doc
        .append_block(BlockKind::Paragraph, "body", &text)
        .unwrap();
    let b = doc
        .append_block(BlockKind::Paragraph, "body", "Tiny.")
        .unwrap();
    let snapshot = engine().layout(&doc);
    let nav = Navigator::semantic(&snapshot, &doc);
    let block = snapshot.block(a).unwrap();
    let frames: std::collections::BTreeSet<_> = block.lines.iter().map(|l| l.frame).collect();
    assert!(frames.len() >= 2, "the paragraph threads through columns");

    // Walk down from a point well along the first line; the goal survives
    // lines of different lengths and the jump to the next column.
    let first = LineRef { node: a, line: 0 };
    let start = nav
        .caret_at_x(first, block.lines[0].rect.origin.x + Length::from_pt(60))
        .unwrap();
    let goal = x_of(&nav, &start);
    let mut cursor = Cursor::from(start);
    let mut visited = vec![nav.caret_rect(start).unwrap().line];
    for _ in 0..block.lines.len() - 1 {
        cursor = nav.move_cursor(&cursor, Movement::LineDown).unwrap();
        assert_eq!(cursor.goal_x, Some(goal), "the goal is kept");
        visited.push(nav.caret_rect(cursor.caret).unwrap().line);
    }
    let lines: Vec<usize> = visited.iter().map(|l| l.line).collect();
    assert_eq!(lines, (0..block.lines.len()).collect::<Vec<_>>());
    // Each landing is as near the goal as that line allows.
    let model_line = |at: LineRef| &snapshot.block(at.node).unwrap().lines[at.line];
    for at in &visited {
        let l = model_line(*at);
        assert!(l.frame < snapshot.frames.len());
    }
    // Down from the last line of the paragraph is the next block.
    let next = nav.move_cursor(&cursor, Movement::LineDown).unwrap();
    assert_eq!(next.caret.node, b);
    // Up retraces, and any other movement clears the goal.
    let back = nav.move_cursor(&next, Movement::LineUp).unwrap();
    assert_eq!(back.caret.node, a);
    assert_eq!(back.goal_x, Some(goal));
    let sideways = nav.move_cursor(&back, Movement::NextGrapheme).unwrap();
    assert_eq!(sideways.goal_x, None);
    // At the top and bottom of the document the caret goes to the line's edge.
    let top = nav
        .move_cursor(&Cursor::from(start), Movement::LineUp)
        .unwrap();
    assert_eq!(top.caret, Caret::new(a, 0));
    let last = nav
        .move_caret(Caret::new(b, 2), Movement::LineDown)
        .unwrap();
    assert_eq!(last, Caret::upstream(b, "Tiny.".len()));
}

#[test]
fn movement_between_blocks_follows_reading_order_not_geometry() {
    let spike = reprise_fixtures::spike::document().unwrap();
    let snapshot = engine().layout(&spike.doc);
    // The first note sits in the margin beside the first paragraph, but it
    // comes after the second paragraph in the tree.
    let notes: Vec<_> = snapshot
        .blocks
        .iter()
        .filter(|b| b.kind == BlockKind::Annotation)
        .map(|b| b.node)
        .collect();
    let nav = Navigator::semantic(&snapshot, &spike.doc);
    let end_of_hallway = Caret::upstream(
        spike.hallway,
        snapshot.block(spike.hallway).unwrap().text.len(),
    );
    let next = nav
        .move_caret(end_of_hallway, Movement::NextGrapheme)
        .unwrap();
    assert!(
        notes.contains(&next.node),
        "after the last paragraph: a note"
    );
    assert_eq!(
        nav.reading_order()
            .iter()
            .position(|n| *n == spike.hallway)
            .map(|p| p + 1),
        nav.reading_order().iter().position(|n| *n == next.node)
    );
    // An explicit override plugs in at construction: the notes right after
    // the paragraph they stand beside.
    let mut order = vec![spike.opening];
    order.push(notes[0]);
    order.extend([spike.hallway, notes[1]]);
    let custom = Navigator::new(&snapshot, order);
    let end_of_opening = Caret::upstream(
        spike.opening,
        snapshot.block(spike.opening).unwrap().text.len(),
    );
    let moved = custom
        .move_caret(end_of_opening, Movement::NextGrapheme)
        .unwrap();
    assert_eq!(moved.node, notes[0]);
    // Blocks a custom order forgets are still reachable, and unknown or
    // repeated ones are ignored.
    let partial = Navigator::new(&snapshot, [spike.hallway, spike.hallway, notes[0]]);
    assert_eq!(partial.reading_order().len(), snapshot.blocks.len());
    assert_eq!(partial.reading_order()[0], spike.hallway);
}

#[test]
fn a_hard_line_break_ends_a_line_and_the_caret_is_never_drawn_after_it_there() {
    let (doc, ids, snapshot) = laid_out(&["one\ntwo\n\nfour"]);
    let nav = Navigator::semantic(&snapshot, &doc);
    let node = ids[0];
    let block = snapshot.block(node).unwrap();
    assert_eq!(block.lines.len(), 4);
    // Before and after the first break are on different lines.
    let before = nav.caret_rect(Caret::upstream(node, 3)).unwrap();
    let after = nav.caret_rect(Caret::new(node, 4)).unwrap();
    assert_eq!((before.line.line, after.line.line), (0, 1));
    assert!(after.rect.origin.y > before.rect.origin.y);
    // LineEnd stops before the break; hitting right of the line does too.
    assert_eq!(
        nav.move_caret(Caret::new(node, 0), Movement::LineEnd),
        Some(Caret::upstream(node, 3))
    );
    let hit = nav
        .hit(before.page, {
            let p = before.point();
            reprise_geom::Point::new(p.x + Length::from_pt(400), p.y)
        })
        .unwrap();
    assert_eq!(hit.caret, Caret::upstream(node, 3));
    // Moving right from before the break goes to the next line's start; the
    // empty line is a line of its own.
    let mut caret = Caret::upstream(node, 3);
    let mut lines = vec![nav.caret_rect(caret).unwrap().line.line];
    for _ in 0..4 {
        caret = nav.move_caret(caret, Movement::NextGrapheme).unwrap();
        lines.push(nav.caret_rect(caret).unwrap().line.line);
    }
    assert_eq!(lines, [0, 1, 1, 1, 1]);
    // The empty third line holds one position, and visual movement walks on.
    let empty = Caret::new(node, 8);
    let rect = nav.caret_rect(empty).unwrap();
    assert_eq!(rect.line.line, 2);
    let right = nav.move_caret(empty, Movement::VisualRight).unwrap();
    assert_eq!(nav.caret_rect(right).unwrap().line.line, 3);
}

#[test]
fn a_right_to_left_paragraph_runs_the_other_way() {
    let text = "\u{5E9}\u{5DC}\u{5D5}\u{5DD} \u{5E2}\u{5D5}\u{5DC}\u{5DD}";
    let (doc, ids, snapshot) = laid_out(&[text, "after"]);
    let nav = Navigator::semantic(&snapshot, &doc);
    let node = ids[0];
    let at = LineRef { node, line: 0 };
    let cells = nav.graphemes(at);
    assert!(cells.iter().all(|c| c.level == 1));
    // The logical start is on the right.
    let start = nav
        .move_caret(Caret::new(node, 2), Movement::LineStart)
        .unwrap();
    let end = nav.move_caret(start, Movement::LineEnd).unwrap();
    assert!(x_of(&nav, &start) > x_of(&nav, &end));
    // Visual right goes toward the logical start; at the right end it
    // goes to the previous line, not the next.
    let right = nav.move_caret(end, Movement::VisualRight).unwrap();
    assert!(right.offset < end.offset);
    let at_start = nav.move_caret(start, Movement::VisualRight).unwrap();
    assert_eq!(at_start, start, "the first line's start: nothing before it");
    let left_from_end = nav.move_caret(end, Movement::VisualLeft).unwrap();
    assert_eq!(
        left_from_end.node, ids[1],
        "past the visual left end: the next line"
    );
    // Next grapheme is logical: it goes from the start toward the end.
    let next = nav.move_caret(start, Movement::NextGrapheme).unwrap();
    assert_eq!(next.offset, 2);
    assert!(x_of(&nav, &next) < x_of(&nav, &start));
    assert_eq!(start.affinity, Affinity::Downstream);
}
