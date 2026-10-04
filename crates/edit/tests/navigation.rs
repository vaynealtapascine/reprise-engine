//! Caret geometry and hit testing over real layouts (30).

mod common;

use common::*;
use reprise_edit::{Affinity, Caret, Movement, Navigator};
use reprise_fixtures::hostile;
use reprise_geom::{Length, Point};
use reprise_layout::LineRef;

#[test]
fn rect_then_hit_gives_the_caret_back_in_every_hostile_fixture() {
    let (mut exact, mut coincident) = (0, 0);
    for fixture in hostile::all().unwrap() {
        let name = fixture.name;
        let snapshot = fixture.engine.layout(&fixture.doc);
        let nav = Navigator::semantic(&snapshot, &fixture.doc);
        let carets = all_carets(&nav);
        for (caret, rect) in &carets {
            assert_eq!(
                nav.normalize(*caret),
                Some(*caret),
                "{name}: {caret:?} is normal"
            );
            let hit = nav
                .hit(rect.page, rect.point())
                .unwrap_or_else(|| panic!("{name}: {caret:?} is hit"));
            let back = nav.caret_rect(hit.caret).expect("hit has geometry");
            assert_eq!(
                (back.page, back.rect),
                (rect.page, rect.rect),
                "{name}: rect-hit-rect preserves the visual position"
            );
            // Carets drawn in exactly the same place (zero-height lines
            // stacked on one another, a note over text) can't be told apart
            // by geometry: the hit must be one of them.
            let twins: Vec<&Caret> = carets
                .iter()
                .filter(|(_, r)| r.page == rect.page && r.rect == rect.rect)
                .map(|(c, _)| c)
                .collect();
            if twins.len() == 1 {
                assert_eq!(hit.caret, *caret, "{name}: rect then hit, {rect:?}");
                assert_eq!(hit.line, rect.line, "{name}: same line");
                exact += 1;
            } else {
                assert!(
                    twins.contains(&&hit.caret),
                    "{name}: {caret:?} -> {:?}",
                    hit.caret
                );
                coincident += 1;
            }
        }
    }
    assert!(
        exact > 1000 && coincident > 0,
        "both unique and coincident positions covered: {exact}, {coincident}"
    );
}

#[test]
fn bidi_boundaries_have_two_carets_and_the_affinity_picks_one() {
    let f = hostile::rtl_mixed().unwrap();
    let snapshot = f.engine.layout(&f.doc);
    let nav = Navigator::semantic(&snapshot, &f.doc);
    let node = nav.reading_order()[0];
    let text = &snapshot.block(node).unwrap().text;
    // The boundary between "then " and the Hebrew word.
    let he = text.find('\u{5E2}').unwrap();
    let before = nav.caret_rect(Caret::upstream(node, he)).unwrap();
    let after = nav.caret_rect(Caret::new(node, he)).unwrap();
    assert_eq!(before.line, after.line);
    assert_ne!(
        before.x, after.x,
        "an LTR run meeting an RTL run is a split caret"
    );
    assert_eq!((before.level, after.level), (0, 1));
    // The Hebrew run's start is its right edge, so it is drawn to the right.
    assert!(after.x > before.x);
    // Each is its own place: hit testing gives each back.
    for (c, r) in [
        (Caret::upstream(node, he), before),
        (Caret::new(node, he), after),
    ] {
        assert_eq!(
            nav.hit(r.page, r.point()).unwrap().caret,
            nav.normalize(c).unwrap()
        );
    }
}

#[test]
fn hitting_a_ligature_splits_it_by_grapheme() {
    let text = "xx office";
    let (doc, ids, snapshot) = laid_out(&[text]);
    let nav = Navigator::semantic(&snapshot, &doc);
    let node = ids[0];
    let at = LineRef { node, line: 0 };
    let ffi = text.find("ffi").unwrap();
    let cells = nav.graphemes(at);
    let cell = |from: usize| {
        cells
            .iter()
            .find(|c| c.range.start == from)
            .unwrap()
            .clone()
    };
    let (a, b, c) = (cell(ffi), cell(ffi + 1), cell(ffi + 2));
    let frame = &snapshot.frames[snapshot.block(node).unwrap().lines[0].frame];
    let line = &snapshot.block(node).unwrap().lines[0];
    let offset_at = |x: Length| {
        let p = frame.to_page.apply(Point::new(
            x,
            line.rect.origin.y + line.rect.height.mul_ratio(1, 2),
        ));
        nav.hit(0, p).unwrap().caret.offset
    };
    // The three graphemes of the ligature share its width.
    assert_eq!((a.x1, b.x1), (b.x0, c.x0), "no gaps");
    let widths = [a.x1 - a.x0, b.x1 - b.x0, c.x1 - c.x0].map(|w| w.0);
    assert!(
        widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 1,
        "{widths:?}"
    );
    let quarter =
        |cell: &reprise_edit::GraphemeCell, n: i32| cell.x0 + (cell.x1 - cell.x0).mul_ratio(n, 4);
    assert_eq!(offset_at(a.x0 + Length(1)), ffi);
    assert_eq!(offset_at(quarter(&a, 1)), ffi);
    assert_eq!(offset_at(quarter(&a, 3)), ffi + 1);
    assert_eq!(offset_at(quarter(&b, 3)), ffi + 2);
    assert_eq!(offset_at(quarter(&c, 1)), ffi + 2);
    assert_eq!(offset_at(c.x1 - Length(1)), ffi + 3);
}

#[test]
fn hit_testing_between_lines_outside_frames_and_at_extremes() {
    let (doc, ids, snapshot) = laid_out(&[
        "First paragraph with enough words in it to wrap onto a second line or even a third one if it must.",
        "Short.",
    ]);
    let nav = Navigator::semantic(&snapshot, &doc);
    let block = snapshot.block(ids[0]).unwrap();
    assert!(block.lines.len() >= 2);
    let frame = &snapshot.frames[block.lines[0].frame];
    let to_page = |x: Length, y: Length| frame.to_page.apply(Point::new(x, y));
    let (l0, l1) = (&block.lines[0], &block.lines[1]);
    // Between two lines: the line below the gap's start.
    let between = to_page(l0.rect.origin.x + Length::from_pt(5), l1.rect.origin.y);
    assert_eq!(nav.hit(0, between).unwrap().caret.node, ids[0]);
    // Far above, below, left and right of everything.
    let above = nav.hit(0, pt(Length::from_pt(100).0, -1_000_000)).unwrap();
    assert_eq!(
        above.line,
        LineRef {
            node: ids[0],
            line: 0
        }
    );
    assert!(!above.inside);
    let below = nav.hit(0, pt(Length::from_pt(100).0, 100_000_000)).unwrap();
    assert_eq!(below.line.node, ids[1]);
    let left = nav
        .hit(
            0,
            to_page(Length(-5_000_000), l0.rect.origin.y + Length(10)),
        )
        .unwrap();
    assert_eq!(left.caret, Caret::new(ids[0], 0));
    let right = nav
        .hit(
            0,
            to_page(Length(50_000_000), l0.rect.origin.y + Length(10)),
        )
        .unwrap();
    assert_eq!(right.caret.offset, block.lines[0].text.end);
    assert_eq!(
        right.caret.affinity,
        Affinity::Upstream,
        "the end of a wrapped line"
    );
    // The extremes of the coordinate space.
    for (x, y) in [
        (i32::MIN, i32::MIN),
        (i32::MAX, i32::MAX),
        (i32::MIN, i32::MAX),
        (i32::MAX, i32::MIN),
        (0, i32::MIN),
        (i32::MAX, 0),
    ] {
        let hit = nav.hit(0, pt(x, y)).expect("some caret");
        assert!(nav.normalize(hit.caret).is_some());
        assert!(!hit.inside);
    }
    // A page that doesn't exist.
    assert!(nav.hit(99, pt(0, 0)).is_none());
    let _ = Movement::NextGrapheme;
}

#[test]
fn a_page_with_no_text_hits_the_nearest_text() {
    let (doc, ids, snapshot) = laid_out(&["One.", "Two."]);
    let page = snapshot.pages[0];
    // A blank page after the text, one with a frame but no text.
    let mut after = snapshot.clone();
    after.pages.push(page);
    after.pages.push(page);
    let mut frame = after.frames[0].clone();
    frame.page = 2;
    after.frames.push(frame);
    let nav = Navigator::semantic(&after, &doc);
    let end = nav.hit(1, pt(0, 0)).unwrap();
    assert_eq!(
        end.caret.node, ids[1],
        "the end of the text before the page"
    );
    assert_eq!(end.caret.offset, "Two.".len());
    assert!(!end.inside);
    assert_eq!(nav.hit(2, pt(5, 5)).unwrap().caret, end.caret);
    // Text only after the page.
    let mut later = snapshot.clone();
    later.pages.insert(0, page);
    for f in &mut later.frames {
        f.page += 1;
    }
    let nav = Navigator::semantic(&later, &doc);
    assert_eq!(nav.hit(0, pt(0, 0)).unwrap().caret, Caret::new(ids[0], 0));
}

#[test]
fn every_movement_from_every_caret_of_every_fixture_lands_on_a_real_caret() {
    for fixture in hostile::all().unwrap() {
        let name = fixture.name;
        let snapshot = fixture.engine.layout(&fixture.doc);
        let nav = Navigator::semantic(&snapshot, &fixture.doc);
        for (caret, _) in all_carets(&nav) {
            for m in ALL_MOVEMENTS {
                let moved = nav
                    .move_caret(caret, m)
                    .unwrap_or_else(|| panic!("{name}: {m:?} from {caret:?}"));
                assert!(
                    nav.caret_rect(moved).is_some(),
                    "{name}: {m:?} from {caret:?} went to {moved:?}"
                );
            }
        }
        // And hitting anywhere gives a real caret.
        for page in 0..snapshot.pages.len() {
            for (x, y) in [
                (i32::MIN, i32::MIN),
                (i32::MAX, i32::MAX),
                (0, 0),
                (50_000, 90_000),
            ] {
                if let Some(hit) = nav.hit(page, pt(x, y)) {
                    assert!(nav.caret_rect(hit.caret).is_some(), "{name}");
                }
            }
        }
    }
}

#[test]
fn glyphless_and_malformed_spans_retain_logical_positions_without_panicking() {
    let (doc, ids, mut snapshot) = laid_out(&["office e\u{301}\u{200b}!"]);
    let node = ids[0];
    for run in &mut snapshot.blocks[0].lines[0].runs {
        run.glyphs.clear();
    }
    let nav = Navigator::semantic(&snapshot, &doc);
    let boundaries = reprise_doc::text::segment::grapheme_boundaries(&snapshot.blocks[0].text);
    let mut caret = Caret::new(node, 0);
    let mut visited = vec![0];
    for _ in 0..boundaries.len() {
        let next = nav.move_caret(caret, Movement::NextGrapheme).unwrap();
        if next == caret {
            break;
        }
        visited.push(next.offset);
        caret = next;
    }
    assert_eq!(visited, boundaries);
    for caret in nav.caret_positions(node) {
        let rect = nav.caret_rect(caret).unwrap();
        let hit = nav.hit(rect.page, rect.point()).unwrap();
        assert_eq!(nav.caret_rect(hit.caret).unwrap().rect, rect.rect);
    }
    drop(nav);
    // Snapshot corruption must not cause unchecked offsets or geometry panics.
    snapshot.blocks[0].lines[0].runs[0].range = 1..usize::MAX;
    let nav = Navigator::semantic(&snapshot, &doc);
    assert!(nav.hit(0, pt(i32::MAX, i32::MIN)).is_some());
    assert!(nav.caret_rect(Caret::new(node, usize::MAX)).is_none());
}

#[test]
fn blank_page_fallback_uses_logical_rtl_end_and_transforms_round_trip() {
    use reprise_geom::{Matrix, Transform};
    let (doc, ids, mut snapshot) = laid_out(&["\u{5D0}\u{5D1}\u{5D2}"]);
    snapshot.pages.push(snapshot.pages[0]);
    let nav = Navigator::semantic(&snapshot, &doc);
    assert_eq!(nav.hit(1, pt(0, 0)).unwrap().caret.offset, 6);
    drop(nav);
    for matrix in [
        Matrix::rotate_quarter(1),
        Matrix::rotate_quarter(3),
        Matrix::scale(reprise_geom::Fixed::from_int(-1), reprise_geom::Fixed::ONE),
    ] {
        let mut rotated = snapshot.clone();
        for frame in &mut rotated.frames {
            frame.to_page = Transform::new(matrix.then(&Matrix::translate(
                Length::from_pt(300),
                Length::from_pt(300),
            )));
        }
        let nav = Navigator::semantic(&rotated, &doc);
        for caret in nav.caret_positions(ids[0]) {
            let rect = nav.caret_rect(caret).unwrap();
            let hit = nav.hit(rect.page, rect.point()).unwrap();
            assert_eq!(nav.caret_rect(hit.caret).unwrap().rect, rect.rect);
        }
    }
}

#[test]
fn cluster_subdivision_adds_in_wide_integer_space_before_saturating() {
    let (doc, ids, mut snapshot) = laid_out(&["ffi"]);
    let run = &mut snapshot.blocks[0].lines[0].runs[0];
    let mut glyph = run.glyphs[0];
    glyph.cluster = 0;
    glyph.advance = Length::MAX;
    run.glyphs = vec![glyph, glyph, {
        let mut last = glyph;
        last.advance = Length(1);
        last
    }];
    run.range = 0..3;
    run.x = Length::MIN;
    let nav = Navigator::semantic(&snapshot, &doc);
    let cells = nav.graphemes(LineRef {
        node: ids[0],
        line: 0,
    });
    assert_eq!(cells.len(), 3);
    assert_eq!(cells[0].x0, Length::MIN);
    assert_eq!(cells[2].x1, Length::MAX);
    assert_eq!(cells[0].x1, cells[1].x0);
    assert_eq!(cells[1].x1, cells[2].x0);
}
