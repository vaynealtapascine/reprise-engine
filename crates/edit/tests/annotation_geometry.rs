//! The page geometry of annotations: rectangles per line fragment, in
//! rotated, mirrored, right-to-left and vertical frames, across page breaks,
//! and for orphaned points. The rectangles must agree with the navigator's
//! selection geometry (30), which is what the editor draws.
use reprise_doc::annotation::{AnnotationData, CommentAnchor, LinkTarget};
use reprise_doc::{BlockKind, Document, NodeId};
use reprise_edit::{Caret, Navigator, Selection};
use reprise_fixtures::hostile;
use reprise_fixtures::spike::define_styles;
use reprise_fixtures::templates::{long_text, two_columns};
use reprise_fixtures::{PEER, engine};
use reprise_layout::LayoutSnapshot;
use reprise_layout::span::{MAX_SPAN_RECTS, SpanGeometry};

fn url(u: &str) -> AnnotationData {
    AnnotationData::Link(LinkTarget::Url { url: u.into() })
}

fn comment(t: &str) -> AnnotationData {
    AnnotationData::Comment(CommentAnchor { thread: t.into() })
}

/// A seeded sequence, so the ranges chosen are the same everywhere.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

/// `span_geometry` equals the navigator's selection rectangles for ranges
/// between valid caret positions of every block.
fn assert_parity(name: &str, doc: &Document, snapshot: &LayoutSnapshot, ranges: usize) {
    let nav = Navigator::semantic(snapshot, doc);
    let mut rng = Lcg(0x5eed);
    let mut checked = 0usize;
    for &node in nav.reading_order() {
        let offsets: Vec<usize> = nav.caret_positions(node).iter().map(|c| c.offset).collect();
        if offsets.len() < 2 {
            continue;
        }
        for _ in 0..ranges {
            let (a, b) = (
                offsets[rng.next(offsets.len())],
                offsets[rng.next(offsets.len())],
            );
            let (a, b) = (a.min(b), a.max(b));
            if a == b {
                continue;
            }
            let selection = Selection {
                anchor: Caret::new(node, a),
                focus: Caret::new(node, b),
            };
            let expected: Vec<_> = nav
                .selection_rects(&selection)
                .into_iter()
                .map(|r| (r.page, r.rect, r.line))
                .collect();
            let got = snapshot.span_geometry(&[(node, a..b)]);
            let got: Vec<_> = got
                .rects
                .iter()
                .map(|r| (r.page, r.bounds, r.line))
                .collect();
            assert_eq!(got, expected, "{name}: {node:?} {a}..{b}");
            checked += 1;
        }
    }
    assert!(checked > 0, "{name}: some ranges were compared");
}

#[test]
fn rectangles_equal_the_navigators_in_every_kind_of_frame() {
    for fixture in [
        hostile::rtl_mixed().unwrap(),
        hostile::transformed_rtl().unwrap(),
        hostile::vertical_rl().unwrap(),
        hostile::spiral_text().unwrap(),
        hostile::combining_marks().unwrap(),
        hostile::bidi_override_ligature().unwrap(),
        hostile::justified_bidi().unwrap(),
        hostile::display_text_clusters().unwrap(),
    ] {
        let snapshot = fixture.engine.layout(&fixture.doc);
        assert_parity(fixture.name, &fixture.doc, &snapshot, 40);
    }
}

fn paragraph_doc(texts: &[&str]) -> (Document, Vec<NodeId>) {
    let doc = Document::new(PEER).unwrap();
    define_styles(&doc).unwrap();
    let ids = texts
        .iter()
        .map(|t| doc.append_block(BlockKind::Paragraph, "body", t).unwrap())
        .collect();
    doc.commit();
    (doc, ids)
}

#[test]
fn an_annotation_across_a_page_break_has_rectangles_on_both_pages() {
    let doc = Document::new(PEER).unwrap();
    define_styles(&doc).unwrap();
    doc.set_page_template(&two_columns()).unwrap();
    let text = long_text(6);
    let p = doc
        .append_block(BlockKind::Paragraph, "body", &text)
        .unwrap();
    doc.commit();
    let snapshot = engine().layout(&doc);
    assert!(snapshot.pages.len() >= 2, "the text needs several pages");
    // From the middle of the first page's text to the middle of the second's.
    let block = snapshot.block(p).unwrap();
    let on_page = |page: usize| {
        block
            .lines
            .iter()
            .filter(|l| snapshot.frames[l.frame].page == page)
            .cloned()
            .collect::<Vec<_>>()
    };
    let first = on_page(0);
    let second = on_page(1);
    let start = first[first.len() / 2].text.start;
    let end = second[second.len() / 2].text.end;
    let id = doc.add_annotation(p, start..end, &comment("t1")).unwrap();
    doc.commit();
    let snapshot = engine().layout(&doc);
    let geometry = snapshot.annotation_geometry(&doc, id).unwrap();
    assert!(!geometry.stale && !geometry.truncated);
    let pages: std::collections::BTreeSet<usize> = geometry.rects.iter().map(|r| r.page).collect();
    assert!(pages.contains(&0) && pages.contains(&1), "{pages:?}");
    // The top is the first page and the smallest y on it.
    let top = geometry.top.unwrap();
    assert_eq!(top.page, 0);
    let min = geometry
        .rects
        .iter()
        .filter(|r| r.page == 0)
        .map(|r| r.bounds.origin.y)
        .min()
        .unwrap();
    assert_eq!(top.y, min);
    // Several frames (columns) are involved, each rectangle names its own.
    let frames: std::collections::BTreeSet<usize> =
        geometry.rects.iter().map(|r| r.frame).collect();
    assert!(frames.len() >= 2);
    for r in &geometry.rects {
        assert_eq!(snapshot.frames[r.frame].page, r.page);
        assert!(r.frame_rect.width.0 > 0);
    }
    // And it agrees with the editor's selection geometry.
    assert_parity("page break", &doc, &snapshot, 30);
}

#[test]
fn a_hebrew_link_sits_where_the_hebrew_is_drawn() {
    let fixture = hostile::rtl_mixed().unwrap();
    let doc = &fixture.doc;
    let snapshot = fixture.engine.layout(doc);
    let node = snapshot.blocks[0].node;
    let text = snapshot.block(node).unwrap().text.clone();
    let from = text.find('\u{5E2}').unwrap();
    let to = from + "\u{5E2}\u{5D1}\u{5E8}\u{5D9}\u{5EA}".len();
    let id = doc
        .add_annotation(node, from..to, &url("https://he.example"))
        .unwrap();
    doc.commit();
    let snapshot = fixture.engine.layout(doc);
    let geometry = snapshot.annotation_geometry(doc, id).unwrap();
    assert!(!geometry.rects.is_empty());
    let first = &geometry.rects[0];
    // The word is right-to-left: its run is drawn from the line's far side of
    // the preceding English, as a rectangle of its own width.
    let line = snapshot.line(first.line).unwrap();
    let run = line
        .runs
        .iter()
        .find(|r| r.range.contains(&from))
        .expect("a run holds the Hebrew");
    assert_eq!(run.level % 2, 1);
    assert!(first.frame_rect.origin.x >= run.x);
    assert!(first.frame_rect.max_x() <= run.x + run.width);
    assert_parity("rtl link", doc, &snapshot, 10);
}

#[test]
fn vertical_and_rotated_frames_give_exact_corners() {
    let fixture = hostile::vertical_rl().unwrap();
    let doc = &fixture.doc;
    let snapshot = fixture.engine.layout(doc);
    let node = snapshot.blocks[0].node;
    let id = doc
        .add_annotation(node, 4..30, &url("https://v.example"))
        .unwrap();
    doc.commit();
    let snapshot = fixture.engine.layout(doc);
    let geometry = snapshot.annotation_geometry(doc, id).unwrap();
    assert!(!geometry.rects.is_empty());
    for r in &geometry.rects {
        // A column of vertical text: the inline axis runs down the page.
        assert!(r.bounds.height.0 > r.bounds.width.0, "{r:?}");
        let xs = r.corners.map(|c| c.x.0);
        let ys = r.corners.map(|c| c.y.0);
        assert_eq!(r.bounds.origin.x.0, *xs.iter().min().unwrap());
        assert_eq!(r.bounds.max_x().0, *xs.iter().max().unwrap());
        assert_eq!(r.bounds.origin.y.0, *ys.iter().min().unwrap());
        assert_eq!(r.bounds.max_y().0, *ys.iter().max().unwrap());
    }
    let rotated = hostile::transformed_rtl().unwrap();
    let snapshot = rotated.engine.layout(&rotated.doc);
    let node = snapshot.blocks[0].node;
    let id = rotated
        .doc
        .add_annotation(node, 0..20, &url("https://r.example"))
        .unwrap();
    rotated.doc.commit();
    let snapshot = rotated.engine.layout(&rotated.doc);
    let geometry = snapshot.annotation_geometry(&rotated.doc, id).unwrap();
    assert!(!geometry.rects.is_empty());
    // A quarter turn plus a mirror: the corners are not the bounds' corners
    // in the frame's own order, but they span exactly the bounds.
    for r in &geometry.rects {
        let xs = r.corners.map(|c| c.x.0);
        assert_eq!(r.bounds.origin.x.0, *xs.iter().min().unwrap());
        assert_eq!(r.bounds.max_x().0, *xs.iter().max().unwrap());
    }
}

#[test]
fn an_orphaned_comment_is_one_zero_width_point() {
    let (doc, ids) = paragraph_doc(&["keep this sentence here"]);
    let p = ids[0];
    let id = doc.add_annotation(p, 5..18, &comment("t1")).unwrap();
    doc.commit();
    let before = engine().layout(&doc);
    let whole = before.annotation_geometry(&doc, id).unwrap();
    assert!(whole.rects.iter().all(|r| r.frame_rect.width.0 > 0));
    doc.delete_text(p, 4..18).unwrap();
    doc.commit();
    let snapshot = engine().layout(&doc);
    let point = snapshot.annotation_geometry(&doc, id).unwrap();
    assert_eq!(point.rects.len(), 1);
    assert_eq!(point.rects[0].frame_rect.width.0, 0);
    assert_eq!(point.unplaced, 0);
    // The point is where the text before it ends.
    let before = snapshot.span_geometry(&[(p, 0..4)]);
    assert_eq!(
        point.rects[0].frame_rect.origin.x,
        before.rects[0].frame_rect.max_x()
    );
    assert!(point.top.is_some());
}

#[test]
fn a_stale_snapshot_places_nothing_and_a_missing_annotation_is_none() {
    let (doc, ids) = paragraph_doc(&["hello there world"]);
    let p = ids[0];
    let id = doc
        .add_annotation(p, 0..5, &url("https://a.example"))
        .unwrap();
    doc.commit();
    let snapshot = engine().layout(&doc);
    doc.block(p).unwrap().text.insert(0, "x").unwrap();
    doc.commit();
    let stale = snapshot.annotation_geometry(&doc, id).unwrap();
    assert!(stale.stale && stale.rects.is_empty() && stale.top.is_none());
    assert!(snapshot.page_links(&doc, 0).stale);
    let fresh = engine().layout(&doc);
    assert!(
        fresh
            .annotation_geometry(&doc, reprise_doc::RangeId::parse("9@9").unwrap())
            .is_none()
    );
    // A link whose text was all deleted has no geometry (it is gone).
    doc.delete_text(p, 0..doc.block(p).unwrap().text.len())
        .unwrap();
    doc.commit();
    let fresh = engine().layout(&doc);
    assert!(fresh.annotation_geometry(&doc, id).is_none());
}

#[test]
fn page_links_list_each_link_once_per_page_with_its_rectangles() {
    let doc = Document::new(PEER).unwrap();
    define_styles(&doc).unwrap();
    doc.set_page_template(&two_columns()).unwrap();
    let text = long_text(6);
    let p = doc
        .append_block(BlockKind::Paragraph, "body", &text)
        .unwrap();
    doc.commit();
    let snapshot = engine().layout(&doc);
    let block = snapshot.block(p).unwrap();
    let first_page: Vec<_> = block
        .lines
        .iter()
        .filter(|l| snapshot.frames[l.frame].page == 0)
        .collect();
    let spanning = first_page.last().unwrap().text.start
        ..block.lines[block
            .lines
            .iter()
            .position(|l| snapshot.frames[l.frame].page == 1)
            .unwrap()
            + 1]
        .text
        .end;
    let small = doc
        .add_annotation(p, 0..4, &url("https://small.example"))
        .unwrap();
    let wide = doc
        .add_annotation(p, spanning, &url("https://wide.example"))
        .unwrap();
    let note = doc.add_annotation(p, 0..10, &comment("c")).unwrap();
    doc.commit();
    let snapshot = engine().layout(&doc);
    let zero = snapshot.page_links(&doc, 0);
    let one = snapshot.page_links(&doc, 1);
    let ids = |links: &reprise_layout::span::PageLinks| {
        links.links.iter().map(|l| l.id).collect::<Vec<_>>()
    };
    assert_eq!(ids(&zero), vec![small, wide], "comments are not links");
    assert_eq!(ids(&one), vec![wide]);
    assert!(!ids(&zero).contains(&note));
    for links in [&zero, &one] {
        for link in &links.links {
            assert!(link.rects.iter().all(|r| r.page == links.page));
            assert!(!link.rects.is_empty());
        }
    }
    // The wide link's rectangles across both pages are its whole geometry.
    let whole: SpanGeometry = snapshot.annotation_geometry(&doc, wide).unwrap();
    let combined: usize = [&zero, &one]
        .iter()
        .map(|l| l.links.iter().find(|x| x.id == wide).unwrap().rects.len())
        .sum();
    assert_eq!(combined, whole.rects.len());
    assert!(snapshot.page_links(&doc, 99).links.is_empty());
}

#[test]
fn geometry_is_bounded_and_deterministic() {
    let (doc, ids) = paragraph_doc(&[&"w ".repeat(6000)]);
    let p = ids[0];
    let id = doc.add_annotation(p, 0..12000, &comment("big")).unwrap();
    doc.commit();
    let snapshot = engine().layout(&doc);
    let a = snapshot.annotation_geometry(&doc, id).unwrap();
    let b = snapshot.annotation_geometry(&doc, id).unwrap();
    assert_eq!(a, b);
    assert!(a.rects.len() <= MAX_SPAN_RECTS);
    // Many annotations share one pass.
    let many: Vec<_> = (0..200)
        .map(|i| {
            doc.add_annotation(p, i * 4..i * 4 + 3, &comment("c"))
                .unwrap()
        })
        .collect();
    doc.commit();
    let snapshot = engine().layout(&doc);
    let all = snapshot.annotation_geometries(&doc, &many);
    assert_eq!(all.len(), many.len());
    for (id, geometry) in &all {
        assert_eq!(
            geometry,
            &snapshot.annotation_geometry(&doc, *id).unwrap(),
            "batch equals single"
        );
    }
}
