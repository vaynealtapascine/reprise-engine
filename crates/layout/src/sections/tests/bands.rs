//! Acceptance tests for continuous bands and ordinary-flow reference fits.
use super::*;

fn page(doc: &Document, width: i32, height: i32, margin: i32) {
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
fn continuous(doc: &Document, node: NodeId, entries: &[(&str, Value)]) {
    let mut entries = entries.to_vec();
    entries.push(("start", json!("continuous")));
    boundary(doc, node, BreakKind::Section, &entries);
}
fn complete_by_units(engine: &Engine, doc: &Document) -> crate::LayoutSnapshot {
    let mut session = LayoutSession::new(engine);
    let mut job = session.start(doc, Viewport::Pages(0..1000));
    let before = job.partial().unwrap().snapshot().to_json();
    job.step(0).unwrap();
    assert_eq!(before, job.partial().unwrap().snapshot().to_json());
    for _ in 0..100_000 {
        let step = job.step(1).unwrap();
        let partial = job.partial().unwrap();
        assert!(
            partial
                .snapshot()
                .frames
                .iter()
                .all(|f| f.page < partial.snapshot().pages.len())
        );
        if let Some(metadata) = &partial.snapshot().sections {
            assert_eq!(metadata.pages.len(), partial.snapshot().pages.len());
            assert!(metadata.rules.iter().all(|r| r.page < metadata.pages.len()));
        }
        if step.complete {
            return job.complete().unwrap().unwrap();
        }
    }
    panic!("one-unit layout did not make bounded progress");
}

#[test]
fn one_two_one_columns_share_owner_and_exact_balanced_depths() {
    let (engine, doc) = setup();
    page(&doc, 200, 200, 12);
    first(&doc, &[("columns.count", json!(1))]);
    let a = block(&doc, "a");
    let b = block(&doc, "b\nb\nb\nb");
    let c = block(&doc, "c");
    continuous(
        &doc,
        b,
        &[
            ("columns.count", json!(2)),
            ("columns.gap", json!(10240)),
            ("columns.rule", json!(2048)),
            ("left", json!(30720)),
            ("right", json!(40960)),
            ("top", json!(61440)),
            ("bottom", json!(61440)),
            ("direction", json!("rtl")),
            ("numbering.restart", json!("-7")),
        ],
    );
    continuous(
        &doc,
        c,
        &[
            ("columns.count", json!(1)),
            ("left", json!(8192)),
            ("right", json!(9216)),
        ],
    );
    let snapshot = engine.layout(&doc);
    assert_eq!(snapshot.pages.len(), 1, "{:?}", snapshot.diagnostics);
    let metadata = snapshot.sections.as_ref().unwrap();
    let p = &metadata.pages[0];
    assert_eq!(p.section, SectionId::First);
    assert_eq!(p.counter, 1);
    assert_eq!(p.bands.len(), 3);
    assert_eq!(
        p.bands.iter().map(|b| b.frames.len()).collect::<Vec<_>>(),
        [1, 2, 1]
    );
    assert_eq!(
        p.bands.iter().map(|b| b.depth).collect::<Vec<_>>(),
        [
            Length::from_pt(12),
            Length::from_pt(24),
            Length::from_pt(140)
        ]
    );
    assert_eq!(
        p.bands.iter().map(|b| b.offset).collect::<Vec<_>>(),
        [Length::ZERO, Length::from_pt(12), Length::from_pt(36)]
    );
    assert!(p.bands[0].balanced && p.bands[1].balanced && !p.bands[2].balanced);
    assert_eq!(p.bands[1].body.origin.y, Length::from_pt(24));
    assert_eq!(p.bands[2].body.origin.y, Length::from_pt(48));
    assert_eq!(metadata.rules.len(), 1);
    assert_eq!(metadata.rules[0].rect.origin.x, Length::from_pt(94));
    assert_eq!(metadata.rules[0].rect.origin.y, Length::from_pt(24));
    assert_eq!(metadata.rules[0].rect.height, Length::from_pt(24));
    for node in [a, b, c] {
        assert!(snapshot.block(node).is_some());
    }
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn restart_is_pending_until_the_continuous_section_owns_content() {
    let (engine, doc) = setup();
    page(&doc, 180, 200, 10);
    first(&doc, &[("columns.count", json!(1))]);
    block(&doc, "a");
    let b = block(&doc, "b");
    continuous(
        &doc,
        b,
        &[
            ("columns.count", json!(1)),
            ("numbering.restart", json!("-8")),
        ],
    );
    let next = block(&doc, "next owned page");
    boundary(&doc, next, BreakKind::Page, &[]);
    let snapshot = engine.layout(&doc);
    let metadata = snapshot.sections.as_ref().unwrap();
    assert_eq!(metadata.pages.len(), 2);
    assert_eq!(metadata.pages[0].section, SectionId::First);
    assert_eq!(metadata.pages[0].counter, 1);
    assert_eq!(metadata.pages[0].bands.len(), 2);
    assert_eq!(metadata.pages[1].section, SectionId::Block(b));
    assert_eq!(metadata.pages[1].counter, -8);
    assert_eq!(metadata.pages[1].bands[0].offset, Length::ZERO);
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn incompatible_continuous_start_preserves_the_ragged_reference() {
    for entries in [
        vec![("width", json!(220 * 1024))],
        vec![("writing-mode", json!("vertical-rl"))],
    ] {
        let (engine, doc) = setup();
        page(&doc, 200, 200, 10);
        first(&doc, &[("columns.count", json!(2))]);
        block(&doc, "a");
        let b = block(&doc, "b");
        continuous(&doc, b, &entries);
        let snapshot = engine.layout(&doc);
        assert_eq!(snapshot.pages.len(), 2);
        assert!(!snapshot.sections.as_ref().unwrap().pages[0].bands[0].balanced);
        assert_eq!(
            snapshot
                .diagnostics_with("section.continuous-new-page")
                .count(),
            1
        );
    }
}

#[test]
fn thirty_third_band_advances_with_a_bounded_warning() {
    let (engine, doc) = setup();
    page(&doc, 200, 1000, 10);
    first(&doc, &[("columns.count", json!(1))]);
    block(&doc, "0");
    let mut last = None;
    for n in 1..33 {
        let node = block(&doc, &n.to_string());
        continuous(&doc, node, &[("columns.count", json!(1))]);
        last = Some(node);
    }
    let snapshot = engine.layout(&doc);
    let pages = &snapshot.sections.as_ref().unwrap().pages;
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].bands.len(), 32);
    assert_eq!(pages[1].bands.len(), 1);
    assert_eq!(pages[1].section, SectionId::Block(last.unwrap()));
    assert_eq!(snapshot.diagnostics_with("section.band-limit").count(), 1);
    assert!(!pages[0].bands[31].balanced);
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn oversized_cell_reference_keeps_output_and_diagnostics_without_balancing() {
    let (engine, doc) = setup();
    page(&doc, 200, 80, 10);
    first(&doc, &[("columns.count", json!(2))]);
    doc.define_style(
        "tall",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(10))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(90))),
            ..Default::default()
        },
    )
    .unwrap();
    let table = doc
        .append_table(reprise_doc::TableColumns {
            columns: vec![reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Proportional(1),
            }],
        })
        .unwrap();
    let row = doc.append_table_row(table, false).unwrap();
    let cell = doc.append_table_cell(row, 0).unwrap();
    let content = doc
        .append_cell_block(cell, BlockKind::Paragraph, "tall", "oversized")
        .unwrap();
    let b = block(&doc, "b");
    continuous(&doc, b, &[("columns.count", json!(1))]);
    let snapshot = engine.layout(&doc);
    let pages = &snapshot.sections.as_ref().unwrap().pages;
    assert_eq!(pages.len(), 2, "{:?}", snapshot.diagnostics);
    assert!(!pages[0].bands[0].balanced);
    assert_eq!(pages[0].bands[0].depth, Length::from_pt(60));
    assert!(snapshot.block(content).is_some());
    assert!(
        snapshot
            .diagnostics_with("layout.frame-overflow")
            .next()
            .is_some()
    );
    assert_eq!(
        snapshot
            .diagnostics_with("section.continuous-new-page")
            .count(),
        1
    );
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn huge_template_depth_retains_a_verified_fit_after_twenty_four_trials() {
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("huge", Dim::pt(200), Dim::Pt(Length(40_000_000))).with_frame(
            FrameTemplate::new(
                "body",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(200), Dim::Pt(Length(40_000_000))),
            ),
        ),
    )
    .unwrap();
    first(&doc, &[("columns.count", json!(2))]);
    let a = block(&doc, "a");
    let b = block(&doc, "b");
    continuous(&doc, b, &[("columns.count", json!(1))]);
    let snapshot = engine.layout(&doc);
    assert_eq!(snapshot.pages.len(), 1, "{:?}", snapshot.diagnostics);
    let band = &snapshot.sections.as_ref().unwrap().pages[0].bands[0];
    assert!(band.balanced);
    assert!(band.depth >= Length::from_pt(12) && band.depth < Length(40_000_000));
    assert_eq!(
        snapshot.diagnostics_with("section.balance-approx").count(),
        1
    );
    assert!(
        snapshot
            .block(a)
            .unwrap()
            .lines
            .iter()
            .all(|l| l.rect.origin.y + l.rect.height <= band.depth)
    );
}

#[test]
fn notes_reflow_an_earlier_band_and_share_the_carried_pages_owner() {
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
    first(
        &doc,
        &[("columns.count", json!(2)), ("columns.gap", json!(8192))],
    );
    let a = block(&doc, &["a"; 8].join("\n"));
    let b = block(&doc, "b");
    continuous(&doc, b, &[("columns.count", json!(1))]);
    let mut owners = Vec::new();
    for (anchor, count) in [(a, 20), (b, 8)] {
        let note = doc
            .append_block(BlockKind::Annotation, "s", &vec!["note"; count].join("\n"))
            .unwrap();
        owners.push(note);
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
    let snapshot = engine.layout(&doc);
    let metadata = snapshot.sections.as_ref().unwrap();
    assert!(metadata.pages.len() > 2, "{:?}", snapshot.diagnostics);
    assert!(metadata.pages.iter().all(|p| p.section == SectionId::First));
    assert!(
        snapshot
            .block(a)
            .unwrap()
            .lines
            .iter()
            .any(|l| snapshot.frames[l.frame].page == 1),
        "the occupied notes area must invalidate the earlier 48pt band"
    );
    let shared = metadata
        .pages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.kind == PageKind::NoteContinuation)
        .any(|(page, _)| {
            owners.iter().all(|node| {
                snapshot
                    .block(*node)
                    .unwrap()
                    .lines
                    .iter()
                    .any(|l| snapshot.frames[l.frame].page == page)
            })
        });
    assert!(
        shared,
        "notes from both anchor sections share a continuation page"
    );
    assert!(
        snapshot
            .diagnostics_with("layout.region-cycle")
            .next()
            .is_none()
    );
    for body in snapshot
        .blocks
        .iter()
        .filter(|block| [a, b].contains(&block.node))
    {
        for line in &body.lines {
            let rect = snapshot.frames[line.frame].to_page.apply(line.rect.origin);
            assert!(rect.y + line.rect.height <= Length::from_pt(36));
        }
    }
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn paragraph_continuation_balances_only_the_last_page_suffix() {
    let (engine, doc) = setup();
    page(&doc, 200, 100, 10);
    first(
        &doc,
        &[
            ("columns.widths", json!("1,3")),
            ("columns.gap", json!(10240)),
        ],
    );
    let a = block(&doc, &vec!["a"; 20].join("\n"));
    let b = block(&doc, "b");
    continuous(&doc, b, &[("columns.count", json!(1))]);
    let snapshot = engine.layout(&doc);
    assert_eq!(snapshot.pages.len(), 2, "{:?}", snapshot.diagnostics);
    let pages = &snapshot.sections.as_ref().unwrap().pages;
    assert!(!pages[0].bands[0].balanced);
    assert_eq!(pages[0].bands[0].depth, Length::from_pt(80));
    assert!(pages[1].bands[0].balanced);
    assert_eq!(pages[1].bands.len(), 2);
    let lines = &snapshot.block(a).unwrap().lines;
    assert_eq!(lines.len(), 20);
    assert_eq!(
        lines.last().unwrap().text.end,
        snapshot.block(a).unwrap().text.len()
    );
    assert!(
        lines
            .iter()
            .filter(|l| snapshot.frames[l.frame].page == 0)
            .all(|l| snapshot.frames[l.frame].rect.height == Length::from_pt(80))
    );
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn fragmented_row_group_checkpoint_preserves_every_earlier_cell_line() {
    let (engine, doc) = setup();
    page(&doc, 200, 100, 10);
    first(
        &doc,
        &[
            ("columns.widths", json!("1,3")),
            ("columns.gap", json!(10240)),
        ],
    );
    let table = doc
        .append_table(reprise_doc::TableColumns {
            columns: vec![
                reprise_doc::Column {
                    width: reprise_doc::ColumnWidth::Proportional(1)
                };
                2
            ],
        })
        .unwrap();
    let row = doc.append_table_row(table, false).unwrap();
    let mut cells = Vec::new();
    for column in 0..2 {
        let cell = doc.append_table_cell(row, column).unwrap();
        cells.push(
            doc.append_cell_block(cell, BlockKind::Paragraph, "s", &vec!["a"; 18].join("\n"))
                .unwrap(),
        );
    }
    let b = block(&doc, "b");
    continuous(&doc, b, &[("columns.count", json!(1))]);
    let snapshot = engine.layout(&doc);
    let pages = &snapshot.sections.as_ref().unwrap().pages;
    assert!(pages.len() >= 2, "{:?}", snapshot.diagnostics);
    assert!(!pages[0].bands[0].balanced);
    assert!(
        pages.last().unwrap().bands[0].balanced,
        "{:?}",
        snapshot.diagnostics
    );
    for node in cells {
        let cell = snapshot.block(node).unwrap();
        assert_eq!(cell.lines.len(), 18);
        assert_eq!(cell.lines.last().unwrap().text.end, cell.text.len());
        assert!(
            cell.lines
                .iter()
                .all(|l| l.rect.origin.y + l.rect.height <= snapshot.frames[l.frame].rect.height)
        );
    }
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn a_manual_column_break_is_forced_in_every_balance_trial() {
    let (engine, doc) = setup();
    page(&doc, 200, 200, 10);
    first(&doc, &[("columns.count", json!(2))]);
    let a = block(&doc, "a");
    let b = block(&doc, "b");
    boundary(&doc, b, BreakKind::Column, &[]);
    let c = block(&doc, "c");
    continuous(&doc, c, &[("columns.count", json!(1))]);
    let snapshot = engine.layout(&doc);
    let band = &snapshot.sections.as_ref().unwrap().pages[0].bands[0];
    assert!(band.balanced);
    assert_eq!(band.depth, Length::from_pt(12));
    assert_eq!(snapshot.block(a).unwrap().lines[0].frame, band.frames[0]);
    assert_eq!(snapshot.block(b).unwrap().lines[0].frame, band.frames[1]);
    assert_eq!(snapshot.pages.len(), 1);
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn float_source_geometry_stays_valid_while_main_trial_depth_changes() {
    use reprise_doc::relation::builtin;
    use reprise_doc::text::RangePolicy;
    use reprise_doc::{Param, Relation, Target};
    let (engine, doc) = setup();
    page(&doc, 220, 200, 10);
    first(
        &doc,
        &[
            ("columns.widths", json!("1,2")),
            ("columns.gap", json!(8192)),
        ],
    );
    let a = block(&doc, "text text text text text text text text text text");
    let b = block(&doc, "following band");
    continuous(&doc, b, &[("columns.count", json!(1))]);
    let owner = doc
        .append_block(BlockKind::Annotation, "s", "float\nfloat\nfloat")
        .unwrap();
    let anchor = doc.add_range(a, 0..1, RangePolicy::FIXED).unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::FLOAT)
            .owned_by(owner)
            .target("anchor", Target::Range(anchor))
            .param("side", Param::Text("right".into()))
            .param("width", Param::Length(LengthExpr::Pt(Length::from_pt(28))))
            .param("margin", Param::Length(LengthExpr::Pt(Length::from_pt(2)))),
    )
    .unwrap();
    let snapshot = engine.layout(&doc);
    assert!(
        snapshot.block(owner).is_some(),
        "{:?}",
        snapshot.diagnostics
    );
    assert!(snapshot.sections.as_ref().unwrap().pages[0].bands[0].balanced);
    assert!(
        snapshot.frames.iter().any(
            |f| matches!(&f.role, FrameRole::Flow(name) if name.starts_with("reprise.float."))
        )
    );
    assert!(
        snapshot
            .diagnostics_with("layout.region-cycle")
            .next()
            .is_none()
    );
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn continuous_vertical_bands_advance_on_the_owners_physical_block_axis() {
    for (mode, first_x, second_x) in [("vertical-rl", 178, 154), ("vertical-lr", 10, 22)] {
        let (engine, doc) = setup();
        page(&doc, 200, 200, 10);
        first(
            &doc,
            &[("columns.count", json!(1)), ("writing-mode", json!(mode))],
        );
        block(&doc, "a");
        let b = block(&doc, "b\nb\nb\nb");
        continuous(
            &doc,
            b,
            &[
                ("columns.count", json!(2)),
                ("columns.gap", json!(10240)),
                ("writing-mode", json!(mode)),
                ("direction", json!("rtl")),
                ("top", json!(30720)),
                ("bottom", json!(40960)),
                ("left", json!(61440)),
                ("right", json!(61440)),
            ],
        );
        let c = block(&doc, "c");
        continuous(
            &doc,
            c,
            &[("columns.count", json!(1)), ("writing-mode", json!(mode))],
        );
        let snapshot = engine.layout(&doc);
        assert_eq!(
            snapshot.pages.len(),
            1,
            "{mode}: {:?}",
            snapshot.diagnostics
        );
        let bands = &snapshot.sections.as_ref().unwrap().pages[0].bands;
        assert_eq!(bands.len(), 3);
        assert_eq!(bands[0].depth, Length::from_pt(12));
        assert_eq!(bands[1].depth, Length::from_pt(24));
        assert_eq!(bands[0].body.origin.x, Length::from_pt(first_x));
        assert_eq!(bands[1].body.origin.x, Length::from_pt(second_x));
        assert_eq!(bands[1].body.origin.y, Length::from_pt(30));
        assert_eq!(bands[1].body.width, Length::from_pt(24));
        assert_eq!(bands[1].body.height, Length::from_pt(130));
        assert_eq!(snapshot, complete_by_units(&engine, &doc));
    }
}

#[test]
fn a_new_unsatisfied_keep_rejects_the_reference_without_discarding_its_warning() {
    let (engine, doc) = setup();
    page(&doc, 200, 200, 10);
    first(&doc, &[("columns.count", json!(2))]);
    doc.define_style(
        "keep",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(10))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(12))),
            keep_with_next: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let a = doc.append_block(BlockKind::Paragraph, "keep", "a").unwrap();
    let b = block(&doc, "b");
    boundary(&doc, b, BreakKind::Column, &[]);
    let c = block(&doc, "c");
    continuous(&doc, c, &[("columns.count", json!(1))]);
    let snapshot = engine.layout(&doc);
    let band = &snapshot.sections.as_ref().unwrap().pages[0].bands[0];
    assert!(!band.balanced);
    assert_eq!(band.depth, Length::from_pt(180));
    assert_eq!(snapshot.pages.len(), 2);
    assert_eq!(
        snapshot
            .diagnostics_with("layout.keep-unsatisfied")
            .filter(|d| d.subject == Subject::Node(a))
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .diagnostics_with("section.continuous-new-page")
            .count(),
        1
    );
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

/// A pure composer with a hole in its fitting depths. Its output still obeys
/// the composer contract: in the hole all text remains for the next frame.
struct NonMonotone;
impl reprise_compose::Composer for NonMonotone {
    fn name(&self) -> &'static str {
        "non-monotone-depth-test"
    }
    fn compose(
        &self,
        request: &reprise_compose::ComposeRequest<'_>,
    ) -> reprise_compose::Composition {
        use reprise_compose::{Available, GeometryProvider, LineQuery};
        struct End;
        impl GeometryProvider for End {
            fn available(&self, _: &LineQuery<'_>) -> Available {
                Available::End
            }
        }
        let probe = |pt| {
            request.geometry.available(&LineQuery {
                line: 0,
                block_offset: Length::from_pt(pt),
                line_height: Length::from_pt(12),
                previous: &[],
            })
        };
        if matches!(probe(52), Available::Room(_)) && matches!(probe(88), Available::End) {
            reprise_compose::Greedy.compose(&reprise_compose::ComposeRequest {
                text: request.text,
                shaped: request.shaped,
                reshape: request.reshape,
                breaks: request.breaks,
                line_height: request.line_height,
                geometry: &End,
                start: request.start,
                block_start: request.block_start,
            })
        } else {
            reprise_compose::Greedy.compose(request)
        }
    }
}

#[test]
fn non_monotone_trials_commit_the_retained_fit_without_claiming_a_minimum() {
    let (mut engine, doc) = setup();
    engine.composer = Box::new(NonMonotone);
    page(&doc, 200, 200, 10);
    first(&doc, &[("columns.count", json!(2))]);
    let a = block(&doc, "a");
    let b = block(&doc, "b");
    continuous(&doc, b, &[("columns.count", json!(1))]);
    let snapshot = engine.layout(&doc);
    let band = &snapshot.sections.as_ref().unwrap().pages[0].bands[0];
    assert!(band.balanced);
    assert_eq!(
        band.depth,
        Length::from_pt(100),
        "12pt fits too, but the bounded bisection never tested it below the hole"
    );
    assert_eq!(snapshot.block(a).unwrap().lines.len(), 1);
    assert!(
        snapshot
            .diagnostics_with("section.balance-approx")
            .next()
            .is_none()
    );
    assert!(
        snapshot
            .diagnostics_with("layout.frame-overflow")
            .next()
            .is_none()
    );
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn suspended_trials_keep_owned_state_and_reject_stale_or_cancelled_publication() {
    use crate::incremental::JobError;
    let (engine, doc) = setup();
    page(&doc, 200, 200, 10);
    first(&doc, &[("columns.count", json!(2))]);
    block(&doc, &["a"; 12].join("\n"));
    let b = block(&doc, "b");
    continuous(&doc, b, &[("columns.count", json!(1))]);
    let expected = engine.layout(&doc);
    let mut session = LayoutSession::new(&engine);
    let mut state = session.start(&doc, Viewport::Pages(0..1000)).suspend();
    let mut complete = None;
    for n in 0..10000 {
        let mut job = session.resume(&doc, state).unwrap();
        if n % 7 == 0 {
            let before = job.partial().unwrap().snapshot().to_json();
            assert_eq!(job.step(0).unwrap().used, 0);
            assert_eq!(before, job.partial().unwrap().snapshot().to_json());
        }
        if job.step(1 + n % 3).unwrap().complete {
            complete = job.complete().unwrap();
            break;
        }
        state = job.suspend();
    }
    assert_eq!(complete.unwrap(), expected);
    let state = session.start(&doc, Viewport::Pages(0..1000)).suspend();
    doc.block(b).unwrap().text.insert(0, "changed ").unwrap();
    assert!(matches!(session.resume(&doc, state), Err(JobError::Stale)));
    let mut job = session.start(&doc, Viewport::Pages(0..1000));
    job.step(1).unwrap();
    job.cancel();
    assert!(matches!(job.partial(), Err(JobError::Cancelled)));
    assert!(matches!(job.complete(), Err(JobError::Cancelled)));
}

#[test]
fn unequal_columns_balance_with_widows_and_an_unbroken_keep_chain() {
    let (engine, doc) = setup();
    page(&doc, 200, 200, 10);
    first(
        &doc,
        &[
            ("columns.widths", json!("1,3")),
            ("columns.gap", json!(10240)),
        ],
    );
    let a = block(&doc, &["a"; 8].join("\n"));
    doc.set_overrides(
        a,
        &reprise_doc::Style {
            widows: Some(3),
            orphans: Some(3),
            ..Default::default()
        },
    )
    .unwrap();
    let heading = block(&doc, "heading");
    doc.set_overrides(
        heading,
        &reprise_doc::Style {
            keep_with_next: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let tail = block(&doc, "b\nb\nb\nb");
    doc.set_overrides(
        tail,
        &reprise_doc::Style {
            keep_together: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let endpoint = block(&doc, "next section");
    continuous(&doc, endpoint, &[("columns.count", json!(1))]);
    let snapshot = engine.layout(&doc);
    let band = &snapshot.sections.as_ref().unwrap().pages[0].bands[0];
    assert!(band.balanced, "{:?}", snapshot.diagnostics);
    assert_eq!(band.depth, Length::from_pt(96));
    assert_eq!(
        snapshot.block(heading).unwrap().lines[0].frame,
        snapshot.block(tail).unwrap().lines[0].frame
    );
    assert!(
        snapshot
            .block(tail)
            .unwrap()
            .lines
            .iter()
            .all(|l| l.frame == snapshot.block(heading).unwrap().lines[0].frame)
    );
    assert!(
        snapshot
            .diagnostics_with("layout.keep-unsatisfied")
            .next()
            .is_none()
    );
    assert_eq!(snapshot.block(a).unwrap().lines.len(), 8);
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}

#[test]
fn continuous_spatial_templates_preserve_authored_depths_and_overflow_detection() {
    for oversized in [false, true] {
        let (engine, doc) = setup();
        doc.set_page_template(
            &PageTemplate::new("spatial", Dim::pt(200), Dim::pt(200))
                .with_frame(FrameTemplate::new(
                    "short",
                    FrameRole::Flow("main".into()),
                    (Dim::pt(0), Dim::pt(0)),
                    (Dim::pt(100), Dim::pt(80)),
                ))
                .with_frame(FrameTemplate::new(
                    "tall",
                    FrameRole::Flow("main".into()),
                    (Dim::pt(100), Dim::pt(0)),
                    (Dim::pt(100), Dim::pt(120)),
                )),
        )
        .unwrap();
        first(&doc, &[("mode", json!("paged")), ("bottom", json!(0))]);
        block(&doc, "a\na\na\na");
        let b = block(&doc, "b");
        continuous(&doc, b, &[]);
        if oversized {
            doc.set_overrides(
                b,
                &reprise_doc::Style {
                    line_height: Some(LengthExpr::Pt(Length::from_pt(150))),
                    ..Default::default()
                },
            )
            .unwrap();
            let c = block(&doc, "c");
            continuous(&doc, c, &[]);
        }
        let snapshot = engine.layout(&doc);
        let bands = &snapshot.sections.as_ref().unwrap().pages[0].bands;
        assert_eq!(bands[0].depth, Length::from_pt(24));
        assert_eq!(bands[1].offset, Length::from_pt(24));
        assert_eq!(
            snapshot.frames[bands[1].frames[0]].rect.height,
            Length::from_pt(80)
        );
        assert_eq!(
            snapshot.frames[bands[1].frames[1]].rect.height,
            Length::from_pt(120)
        );
        assert_eq!(
            snapshot.frames[bands[1].frames[0]].to_page.matrix().ty,
            Length::from_pt(24)
        );
        assert!(!bands[1].balanced);
        if oversized {
            assert_eq!(snapshot.pages.len(), 2, "{:?}", snapshot.diagnostics);
            assert!(
                snapshot
                    .diagnostics_with("layout.frame-overflow")
                    .any(|d| d.subject == Subject::Node(b))
            );
            assert_eq!(
                snapshot
                    .diagnostics_with("section.continuous-new-page")
                    .count(),
                1
            );
        } else {
            assert!(
                snapshot.diagnostics.is_empty(),
                "{:?}",
                snapshot.diagnostics
            );
        }
        assert_eq!(snapshot, complete_by_units(&engine, &doc));
    }
}

fn page_rects(
    snapshot: &crate::LayoutSnapshot,
    node: NodeId,
) -> Vec<(usize, reprise_geom::Rect<reprise_geom::PageSpace>)> {
    snapshot
        .block(node)
        .map(|b| {
            b.lines
                .iter()
                .map(|l| {
                    let f = &snapshot.frames[l.frame];
                    (f.page, f.to_page.bounds(&l.rect))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn overlaps(
    a: &reprise_geom::Rect<reprise_geom::PageSpace>,
    b: &reprise_geom::Rect<reprise_geom::PageSpace>,
) -> bool {
    a.origin.x < b.max_x()
        && b.origin.x < a.max_x()
        && a.origin.y < b.max_y()
        && b.origin.y < a.max_y()
}

/// Review of S1-S5: balancing compared each frame's local line bottoms against
/// one shared depth, so a main frame that starts lower than the band did was
/// certified at a depth its content physically exceeds, and the next band
/// started on top of it.
#[test]
fn staggered_spatial_frames_balance_in_band_coordinates_and_never_overlap_the_next_band() {
    for (lines, expected) in [
        (4, Length::from_pt(48)),
        (10, Length::from_pt(86)),
        (1, Length::from_pt(12)),
    ] {
        let (engine, doc) = setup();
        doc.set_page_template(
            &PageTemplate::new("spatial", Dim::pt(200), Dim::pt(200))
                .with_frame(FrameTemplate::new(
                    "one",
                    FrameRole::Flow("main".into()),
                    (Dim::pt(0), Dim::pt(0)),
                    (Dim::pt(100), Dim::pt(100)),
                ))
                .with_frame(FrameTemplate::new(
                    "two",
                    FrameRole::Flow("main".into()),
                    (Dim::pt(100), Dim::pt(50)),
                    (Dim::pt(100), Dim::pt(100)),
                )),
        )
        .unwrap();
        first(&doc, &[("mode", json!("paged"))]);
        let a = block(&doc, &vec!["a"; lines].join("\n"));
        let b = block(&doc, "b");
        continuous(&doc, b, &[("columns.count", json!(1)), ("right", json!(0))]);
        let snapshot = engine.layout(&doc);
        let page = &snapshot.sections.as_ref().unwrap().pages[0];
        assert_eq!(page.bands.len(), 2, "{lines}: {:?}", snapshot.diagnostics);
        assert_eq!(page.bands[0].depth, expected, "{lines}");
        assert_eq!(page.bands[1].offset, expected, "{lines}");
        assert!(page.bands[0].balanced);
        // No two placed lines intersect, whichever band and frame made them.
        let all: Vec<_> = [a, b]
            .iter()
            .flat_map(|n| page_rects(&snapshot, *n).into_iter().map(move |r| (*n, r)))
            .collect();
        assert_eq!(all.len(), lines + 1, "{lines}: nothing is left out");
        for (i, (_, (pa, ra))) in all.iter().enumerate() {
            for (_, (pb, rb)) in &all[i + 1..] {
                assert!(pa != pb || !overlaps(ra, rb), "{lines}: {ra:?} / {rb:?}");
            }
        }
        // The outgoing band's text ends inside its depth, the incoming starts at it.
        let first_page = |n| {
            page_rects(&snapshot, n)
                .into_iter()
                .filter(|(p, _)| *p == 0)
                .map(|(_, r)| r)
                .collect::<Vec<_>>()
        };
        let out = first_page(a).iter().map(|r| r.max_y()).max().unwrap();
        let incoming = first_page(b).iter().map(|r| r.origin.y).min().unwrap();
        assert!(out <= expected && expected <= incoming, "{lines}");
        assert_eq!(snapshot, complete_by_units(&engine, &doc), "{lines}");
    }
}

/// Frames the band can't measure along its own block axis (a rotated frame)
/// are not balanced: the next section starts a new page, with a diagnostic.
#[test]
fn spatial_frames_off_the_block_axis_are_not_balanced() {
    let (engine, doc) = setup();
    let mut turned = FrameTemplate::new(
        "turned",
        FrameRole::Flow("main".into()),
        (Dim::pt(100), Dim::pt(0)),
        (Dim::pt(100), Dim::pt(100)),
    );
    turned.transform.rotation = reprise_doc::Rotation::Quarter(1);
    doc.set_page_template(
        &PageTemplate::new("spatial", Dim::pt(200), Dim::pt(200))
            .with_frame(FrameTemplate::new(
                "one",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(100), Dim::pt(100)),
            ))
            .with_frame(turned),
    )
    .unwrap();
    first(&doc, &[("mode", json!("paged"))]);
    let a = block(&doc, "a\na\na\na");
    let b = block(&doc, "b");
    continuous(&doc, b, &[("columns.count", json!(1))]);
    let snapshot = engine.layout(&doc);
    assert_eq!(snapshot.pages.len(), 2, "{:?}", snapshot.diagnostics);
    let metadata = snapshot.sections.as_ref().unwrap();
    assert!(metadata.pages[0].bands.iter().all(|b| !b.balanced));
    assert_eq!(metadata.pages[0].bands.len(), 1);
    assert_eq!(
        snapshot
            .diagnostics_with("section.continuous-new-page")
            .count(),
        1,
        "{:?}",
        snapshot.diagnostics
    );
    assert!(snapshot.block(a).is_some() && snapshot.block(b).is_some());
    assert_eq!(snapshot, complete_by_units(&engine, &doc));
}
