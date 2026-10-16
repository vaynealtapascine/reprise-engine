//! Running regions beside the other things that share a page: continuous
//! bands, vertical text, notes and floats.
use super::accept_tests::run_text;
use super::layout_tests::*;
use crate::PageKind;
use crate::incremental::LayoutSession;
use reprise_doc::sections::BreakKind;
use reprise_doc::{BlockKind, Dim, FrameRole, FrameTemplate, PageTemplate};
use reprise_geom::{Length, Point};
use serde_json::json;

fn small_page(doc: &reprise_doc::Document, width: i32, height: i32, margin: i32) {
    doc.set_page_setup_patch(reprise_doc::page_setup::PageSetupPatch {
        width: Some(Length::from_pt(width)),
        height: Some(Length::from_pt(height)),
        top: Some(Length::from_pt(margin)),
        right: Some(Length::from_pt(margin)),
        bottom: Some(Length::from_pt(margin)),
        left: Some(Length::from_pt(margin)),
    })
    .unwrap();
}

fn y_of(s: &crate::LayoutSnapshot, frame: usize) -> Length {
    s.frames[frame].to_page.apply(Point::origin()).y
}

#[test]
fn bands_start_below_the_header_and_stop_above_the_footer() {
    let build = |regions: bool| {
        let (engine, doc) = setup();
        small_page(&doc, 200, 200, 12);
        let mut keys = vec![
            ("columns.count", json!(1)),
            ("header.distance", json!(4096)),
            ("footer.distance", json!(4096)),
        ];
        if regions {
            let head = story(&doc, &["h", "h", "h"]);
            let foot = story(&doc, &["f"]);
            keys.push(("region.header.default", bind(head)));
            keys.push(("region.footer.default", bind(foot)));
        }
        first(&doc, &keys);
        body(&doc, "a");
        let b = body(&doc, "b\nb\nb\nb");
        let c = body(&doc, "c");
        boundary(
            &doc,
            b,
            BreakKind::Section,
            &[
                ("start", json!("continuous")),
                ("columns.count", json!(2)),
                ("columns.gap", json!(10240)),
            ],
        );
        boundary(
            &doc,
            c,
            BreakKind::Section,
            &[("start", json!("continuous")), ("columns.count", json!(1))],
        );
        let s = engine.layout(&doc);
        assert_eq!(s, LayoutSession::new(&engine).layout(&doc).unwrap());
        s
    };
    let plain = build(false);
    let with = build(true);
    let (a, b) = (
        &plain.sections.as_ref().unwrap().pages[0],
        &with.sections.as_ref().unwrap().pages[0],
    );
    assert_eq!(a.bands.len(), 3);
    assert_eq!(b.bands.len(), 3);
    // Header: 4 + three 12 pt lines and two 8 pt gaps = 56 against a 12 pt
    // margin takes 44 pt; the footer 4 + 12 = 16 takes 4 pt.
    assert_eq!(b.reserve, (Length::from_pt(44), Length::from_pt(4)));
    // Every band moves down by the header's share, keeps its balanced depth,
    // and the last stays above the footer.
    for (p, w) in a.bands.iter().zip(&b.bands) {
        if p.balanced {
            assert_eq!(w.depth, p.depth, "balancing is unchanged");
            assert!(w.balanced);
        } else {
            // The ragged last band is what the header and footer left.
            assert_eq!(w.depth, p.depth - Length::from_pt(48));
        }
        assert_eq!(w.offset, p.offset);
        assert_eq!(w.body.origin.y, p.body.origin.y + Length::from_pt(44));
        for (pf, wf) in p.frames.iter().zip(&w.frames) {
            assert_eq!(y_of(&with, *wf), y_of(&plain, *pf) + Length::from_pt(44));
        }
    }
    let last = b.bands.last().unwrap();
    assert!(last.body.origin.y + last.body.height <= Length::from_pt(200 - 12 - 4));
    // The body keeps the order of its own flow and the regions are instances.
    assert_eq!(instances(&with, "header").len(), 1);
    assert_eq!(run_text(&with, 0, "header").unwrap(), "h|h|h");
}

#[test]
fn vertical_columns_are_shortened_and_shifted_by_the_regions() {
    let build = |regions: bool| {
        let (engine, doc) = setup();
        small_page(&doc, 300, 200, 20);
        let mut keys = vec![
            ("writing-mode", json!("vertical-rl")),
            ("header.distance", json!(4096)),
            ("footer.distance", json!(4096)),
        ];
        if regions {
            let head = story(&doc, &["h", "h", "h", "h", "h"]);
            let foot = story(&doc, &["f", "f"]);
            keys.push(("region.header.default", bind(head)));
            keys.push(("region.footer.default", bind(foot)));
        }
        first(&doc, &keys);
        let n = body(
            &doc,
            "vertical text that has to wrap onto a few columns of the page",
        );
        (engine.layout(&doc), n)
    };
    let (plain, n) = build(false);
    let (with, m) = build(true);
    let frame = |s: &crate::LayoutSnapshot, n| s.block(n).unwrap().lines[0].frame;
    let (pf, wf) = (frame(&plain, n), frame(&with, m));
    let reserve = with.sections.as_ref().unwrap().pages[0].reserve;
    // 4 + five 12 pt lines and four 8 pt gaps = 96 against a 20 pt margin;
    // 4 + 12 + 8 + 12 = 36 against 20.
    assert_eq!(reserve, (Length::from_pt(76), Length::from_pt(16)));
    // The inline axis runs down the page: the column starts lower and is shorter.
    assert_eq!(y_of(&with, wf), y_of(&plain, pf) + Length::from_pt(76));
    assert_eq!(
        with.frames[wf].rect.width,
        plain.frames[pf].rect.width - Length::from_pt(92)
    );
}

#[test]
fn notes_do_not_overlap_a_header_and_continuation_pages_show_regions() {
    use reprise_doc::relation::builtin;
    use reprise_doc::text::RangePolicy;
    use reprise_doc::{Param, Relation, Target};
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("notes", Dim::pt(200), Dim::pt(120))
            .with_frame(FrameTemplate::new(
                "main",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(200), Dim::pt(120)),
            ))
            .with_frame(FrameTemplate::new(
                "notes",
                FrameRole::Notes,
                (Dim::pt(0), Dim::pt(30)),
                (Dim::pt(200), Dim::pt(90)),
            )),
    )
    .unwrap();
    let head = story(&doc, &["head", "head"]);
    let foot = story(&doc, &["foot"]);
    first(
        &doc,
        &[
            ("columns.count", json!(2)),
            ("columns.gap", json!(8192)),
            ("header.distance", json!(0)),
            ("footer.distance", json!(0)),
            ("region.header.default", bind(head)),
            ("region.footer.default", bind(foot)),
        ],
    );
    let a = body(&doc, &["a"; 8].join("\n"));
    let b = body(&doc, "b");
    boundary(
        &doc,
        b,
        BreakKind::Section,
        &[("start", json!("continuous")), ("columns.count", json!(1))],
    );
    for (anchor, count) in [(a, 20), (b, 8)] {
        let note = doc
            .append_block(BlockKind::Annotation, "s", &vec!["note"; count].join("\n"))
            .unwrap();
        let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
        doc.add_relation(
            &engine.schemas,
            &Relation::new(builtin::NOTE)
                .owned_by(note)
                .target("anchor", Target::Range(range))
                .param("placement", Param::Text("page".into())),
        )
        .unwrap();
    }
    doc.commit();
    let s = engine.layout(&doc);
    let metadata = s.sections.as_ref().unwrap();
    assert!(metadata.pages.len() > 2, "{:?}", s.diagnostics);
    assert!(
        s.diagnostics_with("layout.region-cycle").next().is_none(),
        "{:?}",
        s.diagnostics
    );
    for (page, d) in metadata.pages.iter().enumerate() {
        // Content pages and continuation pages alike show the owner's regions.
        assert_eq!(
            run_text(&s, page, "header").as_deref(),
            Some("head|head"),
            "page {page} ({:?})",
            d.kind
        );
        assert_eq!(run_text(&s, page, "footer").as_deref(), Some("foot"));
    }
    assert!(
        metadata
            .pages
            .iter()
            .any(|p| p.kind == PageKind::NoteContinuation)
    );
    // No body line starts above the header's reservation.
    for block in s.blocks.iter().filter(|x| [a, b].contains(&x.node)) {
        for line in &block.lines {
            let page = s.frames[line.frame].page;
            let top = metadata.pages[page].reserve.0;
            let y = s.frames[line.frame].to_page.apply(line.rect.origin).y;
            assert!(
                y >= top,
                "page {page}: line at {y:?} above the header's {top:?}"
            );
        }
    }
    assert_eq!(s, LayoutSession::new(&engine).layout(&doc).unwrap());
}
