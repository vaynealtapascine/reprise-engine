//! Inline objects and fields in layout (`docs/inline.md`): metrics, per-line
//! heights, wrapping, clamping, field resolution and its feedback loop, and
//! how the objects sit in right-to-left, vertical and repeated content.

use reprise_display::Item;
use reprise_doc::{InlineAlign, InlinePatch, NodeId, WritingMode};
use reprise_edit::Navigator;
use reprise_fixtures::inline::{self, Part, Scenario};
use reprise_geom::Length;
use reprise_layout::incremental::LayoutSession;
use reprise_layout::{AtomLayout, DisplayOptions, FieldInstance, LayoutSnapshot};

fn pt(points: i32) -> Length {
    Length::from_pt(points)
}

/// The subject's atoms with the index of their line, in line order.
fn atoms<'a>(snapshot: &'a LayoutSnapshot, s: &Scenario) -> Vec<(usize, &'a AtomLayout)> {
    snapshot
        .block(s.subject)
        .map(|b| {
            b.lines
                .iter()
                .enumerate()
                .flat_map(|(i, l)| l.atoms.iter().map(move |a| (i, a)))
                .collect()
        })
        .unwrap_or_default()
}

/// Lays `s` out, checks the incremental layout agrees, and records it.
fn laid_out(s: &Scenario) -> LayoutSnapshot {
    let snapshot = s.engine.layout(&s.doc);
    assert_eq!(
        LayoutSession::new(&s.engine).layout(&s.doc).unwrap(),
        snapshot,
        "{}: incremental layout agrees with full layout",
        s.name
    );
    assert_eq!(snapshot.to_json(), s.engine.layout(&s.doc).to_json());
    insta::assert_snapshot!(s.name, snapshot.to_json());
    snapshot
}

fn codes(snapshot: &LayoutSnapshot) -> Vec<String> {
    snapshot
        .diagnostics
        .iter()
        .filter(|d| d.severity != reprise_diag::Severity::Info)
        .map(|d| d.code.as_str().to_string())
        .collect()
}

fn region_passes(s: &Scenario) -> usize {
    let mut session = LayoutSession::new(&s.engine);
    session.layout(&s.doc).unwrap();
    session.counters().region_passes
}

#[test]
fn a_tall_image_grows_only_its_own_line() {
    let s = inline::tall_image().unwrap();
    let snapshot = laid_out(&s);
    assert!(codes(&snapshot).is_empty(), "{:?}", snapshot.diagnostics);
    let block = snapshot.block(s.subject).unwrap();
    let found = atoms(&snapshot, &s);
    assert_eq!(found.len(), 1);
    let (line, image) = found[0];
    assert_eq!(image.rect.height, pt(36));
    assert_eq!(image.rect.width, pt(24));
    // The image stands on the baseline and the line is as tall as it needs.
    let tall = &block.lines[line];
    assert_eq!(tall.baseline, tall.rect.origin.y + pt(36));
    assert_eq!(image.rect.origin.y + image.rect.height, tall.baseline);
    assert!(tall.rect.height > pt(36) && tall.rect.height < pt(40));
    // Every other line keeps the strut's height.
    for (i, l) in block.lines.iter().enumerate() {
        if i != line {
            assert_eq!(l.rect.height, pt(12), "line {i}");
        }
    }
    // The lines below start where the tall line ends.
    assert_eq!(
        block.lines[line + 1].rect.origin.y,
        tall.rect.origin.y + tall.rect.height
    );
    assert_eq!(image.text(), "a tall picture");
}

#[test]
fn an_image_that_does_not_fit_the_rest_of_the_line_wraps_whole() {
    let s = inline::image_wraps_at_line_end().unwrap();
    let snapshot = laid_out(&s);
    assert!(codes(&snapshot).is_empty(), "{:?}", snapshot.diagnostics);
    let found = atoms(&snapshot, &s);
    assert_eq!(found.len(), 1);
    let (line, image) = found[0];
    assert_eq!(
        line, 1,
        "the first line is full, so the image starts the second"
    );
    assert_eq!(image.rect.origin.x, Length::ZERO);
    let block = snapshot.block(s.subject).unwrap();
    assert!(block.lines[0].atoms.is_empty());
    // The break before it is a break opportunity around the object, not a
    // split of its character.
    assert!(block.lines[1].text.start == block.lines[0].text.end);
    assert!(block.text[block.lines[1].text.clone()].starts_with('\u{fffc}'));
}

#[test]
fn an_image_wider_than_its_frame_is_scaled_to_the_frame_and_reported() {
    let s = inline::image_wider_than_frame().unwrap();
    let snapshot = laid_out(&s);
    assert_eq!(codes(&snapshot), ["layout.image-size"]);
    let (_, image) = atoms(&snapshot, &s)[0];
    assert_eq!(
        image.rect.width,
        pt(100),
        "clamped to the frame's inline extent"
    );
    assert_eq!(image.rect.height, pt(10), "the aspect ratio is kept");
    let block = snapshot.block(s.subject).unwrap();
    for line in &block.lines {
        assert!(line.rect.max_x() <= pt(100));
    }
}

#[test]
fn page_numbers_name_their_pages_and_settle_in_one_pass() {
    let s = inline::page_numbers().unwrap();
    let snapshot = laid_out(&s);
    assert!(snapshot.pages.len() >= 3, "{} pages", snapshot.pages.len());
    assert!(codes(&snapshot).is_empty(), "{:?}", snapshot.diagnostics);
    let found = atoms(&snapshot, &s);
    assert!(!found.is_empty());
    let block = snapshot.block(s.subject).unwrap();
    for (line, atom) in found {
        let page = snapshot.frame(block.lines[line].frame).unwrap().page;
        assert_eq!(atom.text(), (page + 1).to_string());
        assert_eq!(atom.instance, FieldInstance::Single);
    }
    // The text is the strut's height: a field adds nothing to its line.
    assert!(block.lines.iter().all(|l| l.rect.height == pt(12)));
    assert_eq!(region_passes(&s), 1, "the page being composed is the page");
}

#[test]
fn a_forward_page_of_and_a_page_count_settle_in_two_passes() {
    let s = inline::page_of_forward().unwrap();
    let snapshot = laid_out(&s);
    assert!(codes(&snapshot).is_empty(), "{:?}", snapshot.diagnostics);
    let last = snapshot.blocks.last().unwrap();
    let target_page = snapshot.frame(last.lines[0].frame).unwrap().page + 1;
    assert!(target_page > 3, "the target is pages away");
    let texts: Vec<_> = atoms(&snapshot, &s).iter().map(|(_, a)| a.text()).collect();
    assert_eq!(
        texts,
        [target_page.to_string(), snapshot.pages.len().to_string()]
    );
    // The first pass guesses; the second composes with what the first found.
    assert_eq!(region_passes(&s), 2);
}

#[test]
fn a_page_of_whose_target_is_gone_keeps_its_text_and_says_so() {
    let s = inline::page_of_forward().unwrap();
    // Delete the target paragraph: the persistent position is gone.
    let order = s.doc.document_order();
    let target = *order.last().unwrap();
    s.doc.delete_block(target).unwrap();
    s.doc.commit();
    let snapshot = s.engine.layout(&s.doc);
    assert!(
        snapshot
            .diagnostics_with("layout.field-target")
            .next()
            .is_some(),
        "{:?}",
        snapshot.diagnostics
    );
    assert!(
        snapshot
            .diagnostics_with("layout.field-target")
            .all(|d| d.severity == reprise_diag::Severity::Warning)
    );
    // Layout still places everything and still terminates.
    assert!(!snapshot.pages.is_empty());
}

#[test]
fn objects_in_a_right_to_left_paragraph_sit_in_visual_order() {
    let s = inline::rtl_with_atoms().unwrap();
    let snapshot = laid_out(&s);
    assert!(codes(&snapshot).is_empty(), "{:?}", snapshot.diagnostics);
    let block = snapshot.block(s.subject).unwrap();
    let line = &block.lines[0];
    assert_eq!(line.atoms.len(), 2);
    // Visual order is left to right, whatever the logical order is.
    assert!(line.atoms[0].rect.origin.x < line.atoms[1].rect.origin.x);
    let (field, image) = (&line.atoms[0], &line.atoms[1]);
    assert_eq!(field.kind, "field");
    assert_eq!(image.kind, "image");
    // In logical order the image came first, so in a right-to-left line it
    // is the further right.
    assert!(image.at < field.at);
    // The field's own text is left to right: a page number keeps its digits.
    assert_eq!(field.text(), "1");
    for a in &line.atoms {
        assert!(a.rect.max_x() <= line.rect.max_x() && a.rect.origin.x >= line.rect.origin.x);
    }
}

#[test]
fn objects_in_a_vertical_frame_are_upright_and_centred_on_the_baseline() {
    let s = inline::vertical_with_atoms().unwrap();
    let snapshot = laid_out(&s);
    assert!(codes(&snapshot).is_empty(), "{:?}", snapshot.diagnostics);
    let found = atoms(&snapshot, &s);
    let (line_index, image) = found.iter().find(|(_, a)| a.kind == "image").unwrap();
    // In frame space the inline advance is the picture's own height and the
    // line carries its width across, half either side of the baseline.
    assert_eq!(image.rect.width, pt(24));
    assert_eq!(image.rect.height, pt(16));
    let block = snapshot.block(s.subject).unwrap();
    let line = &block.lines[*line_index];
    assert_eq!(
        image.rect.origin.y + image.rect.height.mul_ratio(1, 2),
        line.baseline
    );
    // On the page the picture is upright: 16 wide and 24 tall.
    let navigator = Navigator::new(&snapshot, [s.subject]);
    let rect = navigator.atom_rect(s.subject, image.at).unwrap();
    assert_eq!((rect.rect.width, rect.rect.height), (pt(16), pt(24)));
    // And its display item is not turned with the column.
    let list = snapshot.to_display_list(0, DisplayOptions::default());
    fn images(items: &[Item], inside_group: bool, out: &mut Vec<bool>) {
        for item in items {
            match item {
                Item::Image { .. } => out.push(inside_group),
                Item::Group { items, .. } => images(items, true, out),
                _ => {}
            }
        }
    }
    let mut found = Vec::new();
    images(&list.items, false, &mut found);
    assert_eq!(found.len(), 1);
}

#[test]
fn a_repeated_table_header_resolves_its_fields_for_each_page() {
    let s = inline::repeated_header_page_numbers().unwrap();
    let snapshot = laid_out(&s);
    assert!(codes(&snapshot).is_empty(), "{:?}", snapshot.diagnostics);
    // The authored header is on page 1 and says so.
    let (_, authored) = atoms(&snapshot, &s)[0];
    assert_eq!(authored.text(), "1");
    assert_eq!(authored.instance, FieldInstance::Single);
    // Each continuation page has its own copy, composed for that page.
    let copies: Vec<_> = snapshot
        .repeated_headers
        .iter()
        .map(|h| {
            let page = snapshot.frame(h.frame).unwrap().page;
            let atom = h
                .blocks
                .iter()
                .flat_map(|b| &b.lines)
                .flat_map(|l| &l.atoms)
                .next()
                .unwrap();
            (page, atom.text(), atom.instance)
        })
        .collect();
    assert!(copies.len() >= 3, "{copies:?}");
    for (page, text, instance) in copies {
        assert_eq!(text, (page + 1).to_string());
        assert_eq!(instance, FieldInstance::of_page(page));
    }
    assert_eq!(region_passes(&s), 1);
}

#[test]
fn a_change_of_alignment_moves_the_object_against_the_baseline() {
    let s = inline::tall_image().unwrap();
    let before = s.engine.layout(&s.doc);
    let (_, image) = atoms(&before, &s)[0];
    let entry = s.doc.inline_objects(s.subject).unwrap().remove(0);
    let baseline = image.baseline;
    for (align, check) in [
        (InlineAlign::Middle, 0),
        (InlineAlign::TextTop, 1),
        (InlineAlign::TextBottom, 2),
    ] {
        s.doc
            .update_inline(
                entry.id,
                &InlinePatch {
                    align: Some(align),
                    ..Default::default()
                },
            )
            .unwrap();
        s.doc.commit();
        let after = s.engine.layout(&s.doc);
        let (line, moved) = atoms(&after, &s)[0];
        let block = after.block(s.subject).unwrap();
        let line = &block.lines[line];
        match check {
            // Centred a quarter em above the baseline.
            0 => assert_eq!(
                moved.rect.origin.y + moved.rect.height.mul_ratio(1, 2),
                line.baseline - pt(10).mul_ratio(1, 4)
            ),
            // The top is at the face's ascent.
            1 => assert!(moved.rect.origin.y >= line.rect.origin.y),
            // The bottom is at the face's descent below the baseline.
            _ => assert!(moved.rect.origin.y + moved.rect.height > line.baseline),
        }
        assert_ne!(moved.baseline, Length::ZERO);
    }
    assert_ne!(baseline, Length::ZERO);
}

#[test]
fn resizing_an_object_relayouts_like_a_full_layout() {
    let (engine, doc, asset) = inline::page(120, 300, WritingMode::default()).unwrap();
    let picture = inline::paragraph(
        &doc,
        vec![
            Part::Text("Before "),
            Part::Object(inline::image(&asset, 24, 12, "picture")),
            Part::Text(" after, with enough words to wrap onto a second line."),
        ],
    )
    .unwrap();
    let others: Vec<NodeId> = (0..6)
        .map(|i| {
            inline::paragraph(
                &doc,
                vec![Part::Text(&format!("An unrelated paragraph number {i}."))],
            )
            .unwrap()
        })
        .collect();
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    let first = session.layout(&doc).unwrap();
    assert_eq!(first, engine.layout(&doc));
    let object = doc.inline_objects(picture).unwrap().remove(0);
    let width = |points: i32| InlinePatch {
        params: [("width".to_string(), Some(inline::pt(points)))].into(),
        ..Default::default()
    };
    for points in [60, 100, 24, 400, 0] {
        doc.update_inline(object.id, &width(points)).unwrap();
        doc.commit();
        let full = engine.layout(&doc);
        assert_eq!(session.layout(&doc).unwrap(), full, "width {points}");
        let work = session.counters();
        // The text is not shaped again for a resize (its characters did not
        // change), only composed again where the box moves lines.
        assert_eq!(work.shapes, 0, "width {points}: {work:?}");
        // A size seen before comes back from the composition cache.
        assert!(
            work.compositions + work.reused_compositions >= 1,
            "width {points}: {work:?}"
        );
        let width_now = full
            .block(picture)
            .unwrap()
            .lines
            .iter()
            .flat_map(|l| &l.atoms)
            .next();
        assert_eq!(
            width_now.map(|a| a.rect.width),
            Some(pt(points.min(120))),
            "clamped to the frame"
        );
    }
    // Back to the first size, everything is where it was.
    doc.update_inline(object.id, &width(24)).unwrap();
    doc.commit();
    let again = session.layout(&doc).unwrap();
    assert_eq!(again.blocks, first.blocks);
    assert_eq!(again.frames, first.frames);
    for node in others {
        assert!(first.block(node).is_some());
    }
}

#[test]
fn explore_many() {
    use std::time::Instant;
    for (n, kind) in [
        (2000usize, "field"),
        (2000, "image"),
        (10000, "field"),
        (10000, "image"),
    ] {
        let (engine, doc, asset) = inline::page(400, 400, WritingMode::default()).unwrap();
        let p = inline::paragraph(&doc, vec![Part::Text("start ")]).unwrap();
        let marks: String = std::iter::repeat_n(reprise_doc::text::OBJECT, n).collect();
        let object = if kind == "field" {
            inline::page_number()
        } else {
            inline::image(&asset, 2, 8, "x")
        };
        let t = Instant::now();
        let end = doc.block(p).unwrap().text.len();
        doc.insert_with_inline(p, end, &marks, &vec![object; n])
            .unwrap();
        doc.commit();
        let a = t.elapsed();
        let t = Instant::now();
        let snapshot = engine.layout(&doc);
        let b = t.elapsed();
        let t = Instant::now();
        let mut session = LayoutSession::new(&engine);
        session.layout(&doc).unwrap();
        let c = t.elapsed();
        let count: usize = snapshot
            .block(p)
            .unwrap()
            .lines
            .iter()
            .map(|l| l.atoms.len())
            .sum();
        println!(
            "{kind} n={n}: build {a:?} full layout {b:?} session {c:?}; {} lines {count} atoms {} pages",
            snapshot.block(p).unwrap().lines.len(),
            snapshot.pages.len()
        );
    }
}
