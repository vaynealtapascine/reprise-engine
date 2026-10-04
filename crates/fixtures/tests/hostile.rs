//! Runs every hostile fixture through layout and every backend, and checks
//! the invariants that no workstream may break.

use reprise_diag::Severity;
use reprise_doc::context::{Extent, ResolutionContext};
use reprise_doc::text::segment;
use reprise_doc::{BlockKind, FrameRole, MAIN_FLOW};
use reprise_doc::{Document, NodeId};
use reprise_fixtures::hostile::{self, Fixture};
use reprise_geom::Length;
use reprise_layout::{DisplayOptions, LayoutSnapshot, LineRef, Subject};

fn check(fixture: Fixture) {
    let name = fixture.name;
    let snapshot = fixture.engine.layout(&fixture.doc);

    // Deterministic within a run, and identical on every platform (the
    // snapshot below is checked on Linux, Windows and macOS).
    let again = fixture.engine.layout(&fixture.doc);
    assert_eq!(snapshot.to_json(), again.to_json(), "{name}: repeatable");
    if let Some(replica) = &fixture.replica {
        let theirs = fixture.engine.layout(replica);
        assert_eq!(
            snapshot.revision, theirs.revision,
            "{name}: replicas converge"
        );
        assert_eq!(snapshot.to_json(), theirs.to_json(), "{name}: same layout");
    }

    check_diagnostics(name, &fixture, &snapshot);
    check_lines(name, &snapshot);
    check_flow(name, &snapshot);
    check_queries(name, &snapshot);
    check_backends(name, &fixture, &snapshot);

    insta::assert_snapshot!(name, snapshot.to_json());
}

fn check_diagnostics(name: &str, fixture: &Fixture, snapshot: &LayoutSnapshot) {
    for code in fixture.expect {
        assert!(
            snapshot.diagnostics_with(code).next().is_some(),
            "{name}: expected a {code} diagnostic, got {:#?}",
            snapshot.diagnostics
        );
    }
    for d in &snapshot.diagnostics {
        assert!(
            d.severity == Severity::Info || fixture.expect.contains(&d.code.as_str()),
            "{name}: unexpected {:?} {}: {}",
            d.severity,
            d.code,
            d.message
        );
    }
}

/// Every laid-out block has lines covering its text, contiguously, breaking
/// only between grapheme clusters, with glyphs only from their own line.
fn check_lines(name: &str, snapshot: &LayoutSnapshot) {
    for block in &snapshot.blocks {
        let node = block.node;
        let text = &block.text;
        assert!(!block.lines.is_empty(), "{name}: {node} has a line");
        let unplaced = snapshot
            .diagnostics_with("layout.text-unplaced")
            .any(|d| d.subject == reprise_layout::Subject::Node(node));
        let mut at = 0;
        for line in &block.lines {
            assert_eq!(line.text.start, at, "{name}: {node} lines are contiguous");
            assert!(
                segment::is_grapheme_boundary(text, line.text.start),
                "{name}: {node} line starts inside a grapheme at {}",
                line.text.start
            );
            assert!(line.rect.width.0 >= 0 && line.rect.height.0 >= 0);
            assert!(
                line.rect.origin.y.0 >= 0,
                "{name}: {node} line at {:?} is above its frame",
                line.rect.origin.y
            );
            assert!(snapshot.frame(line.frame).is_some());
            for run in &line.runs {
                assert!(run.range.start >= line.text.start && run.range.end <= line.text.end);
                for g in &run.glyphs {
                    let c = g.cluster as usize;
                    assert!(
                        run.range.contains(&c),
                        "{name}: {node} glyph cluster {c} outside its run {:?}",
                        run.range
                    );
                }
            }
            at = line.text.end;
        }
        if !unplaced {
            assert_eq!(at, text.len(), "{name}: {node} lines cover the text");
        }
    }
}

/// Frames, pages and flow order (24): every frame is on a page that exists,
/// every line is inside its frame unless it carries a diagnostic, and text
/// follows the order frames thread in.
fn check_flow(name: &str, snapshot: &LayoutSnapshot) {
    assert!(!snapshot.pages.is_empty(), "{name}: at least one page");
    let mut page = 0;
    for (i, frame) in snapshot.frames.iter().enumerate() {
        assert!(
            frame.page < snapshot.pages.len(),
            "{name}: frame {i} is on page {}, which doesn't exist",
            frame.page
        );
        assert!(frame.page >= page, "{name}: frames are listed page by page");
        page = frame.page;
        assert!(frame.rect.width.0 >= 0 && frame.rect.height.0 >= 0);
    }

    let mut previous = 0;
    for block in &snapshot.blocks {
        let node = block.node;
        // Lines that overflow their frame say so with a frame-overflow
        // diagnostic on their block.
        let overflows = snapshot
            .diagnostics_with("layout.frame-overflow")
            .any(|d| d.subject == Subject::Node(node));
        for line in &block.lines {
            let frame = snapshot.frame(line.frame).expect("the frame exists");
            if !overflows {
                let bottom = line.rect.origin.y.0.saturating_add(line.rect.height.0);
                assert!(
                    bottom <= frame.rect.height.0,
                    "{name}: {node} has a line ending at {bottom}, below its frame's {}",
                    frame.rect.height.0
                );
            }
            match block.kind {
                BlockKind::Annotation => assert_eq!(
                    frame.role,
                    FrameRole::Margin,
                    "{name}: {node} is placed by a relation, in a margin frame"
                ),
                BlockKind::Paragraph => {
                    assert_eq!(frame.role, FrameRole::Flow(MAIN_FLOW.into()));
                    assert!(
                        line.frame >= previous,
                        "{name}: {node} goes back to frame {} after frame {previous}",
                        line.frame
                    );
                    previous = line.frame;
                }
            }
        }
    }
}

/// The query API answers for every position and agrees with itself.
fn check_queries(name: &str, snapshot: &LayoutSnapshot) {
    for block in &snapshot.blocks {
        let node = block.node;
        // Text that was left out (reported with `layout.text-unplaced`) has
        // no line, so no query can find it.
        let placed = block.lines.last().map_or(0, |l| l.text.end);
        for at in (0..=placed).filter(|&i| block.text.is_char_boundary(i)) {
            let line = snapshot
                .line_containing(node, at)
                .unwrap_or_else(|| panic!("{name}: no line contains byte {at} of {node}"));
            let l = snapshot.line(line).expect("the line exists");
            assert!(
                l.text.contains(&at) || at == l.text.end,
                "{name}: line_containing({at}) gave {:?}",
                l.text
            );
            assert!(snapshot.line_bounds(line).is_some());
            assert!(snapshot.line_to_page(line).is_some());
        }
        for line in 0..block.lines.len() {
            let r = LineRef { node, line };
            if let Some(next) = snapshot.next_line(r) {
                assert_eq!(snapshot.previous_line(next), Some(r));
            }
        }
        assert_eq!(
            snapshot.lines_in(node, 0..placed).len(),
            if block.text.is_empty() {
                1
            } else {
                block.lines.len()
            },
            "{name}: lines_in covers the block"
        );
    }
}

fn check_backends(name: &str, fixture: &Fixture, snapshot: &LayoutSnapshot) {
    let fonts = &fixture.engine.fonts;
    let pages = snapshot.to_display_lists(DisplayOptions {
        debug: true,
        ..DisplayOptions::default()
    });
    assert_eq!(pages.len(), snapshot.pages.len());
    for page in &pages {
        reprise_display::svg::render(page, fonts).unwrap_or_else(|e| panic!("{name}: svg: {e}"));
        reprise_display::png::render(page, fonts, 1.0)
            .unwrap_or_else(|e| panic!("{name}: png: {e}"));
    }
    let content: Vec<_> = pages.iter().map(|p| p.content_only()).collect();
    reprise_display::pdf::render(&content, fonts).unwrap_or_else(|e| panic!("{name}: pdf: {e}"));
}

macro_rules! hostile_tests {
    ($($name:ident),* $(,)?) => {$(
        #[test]
        fn $name() {
            check(hostile::$name().expect("the fixture builds"));
        }
    )*};
}

hostile_tests!(
    empty_text,
    combining_marks,
    emoji_zwj,
    rtl_mixed,
    overlong_word,
    zero_width_measure,
    deleted_targets,
    concurrent_edits,
    extreme_lengths,
    structural_matches,
    snapshot_targets,
    snapshot_compacted,
    concurrent_policy_deletion,
    self_reference,
    optimal_paragraph,
    verse_turnover,
    optimal_extreme_lengths,
    style_expressions,
    style_cycles,
    style_bases,
);

#[test]
fn every_fixture_has_a_test() {
    assert_eq!(hostile::all().expect("fixtures build").len(), 60);
}

hostile_tests!(
    frame_shorter_than_a_line,
    no_main_flow,
    negative_page_size,
    zero_sized_frames,
    page_limit,
    unreadable_template,
    column_storm,
    concurrent_templates,
    no_margin_frame,
);

hostile_tests!(
    bidi_stray_controls,
    bidi_override_ligature,
    scripts_common_inherited
);

#[test]
fn display_text_clusters() {
    check(hostile::display_text_clusters().expect("the fixture builds"));
}

#[test]
fn debug_families_preserve_content_for_every_fixture() {
    for fixture in hostile::all().unwrap() {
        let snapshot = fixture.engine.layout(&fixture.doc);
        let content: Vec<_> = snapshot
            .to_display_lists(DisplayOptions::default())
            .iter()
            .map(|page| page.content_only())
            .collect();
        insta::assert_snapshot!(
            format!("content_{}", fixture.name),
            serde_json::to_string_pretty(&content).unwrap()
        );
        let all = DisplayOptions {
            debug: true,
            ..Default::default()
        };
        let none = DisplayOptions {
            debug: true,
            boxes: false,
            baselines: false,
            intervals: false,
            run_boundaries: false,
            break_reasons: false,
            reshaped_lines: false,
            relations: false,
            diagnostics: false,
        };
        for options in [
            all,
            none,
            DisplayOptions {
                boxes: false,
                ..all
            },
            DisplayOptions {
                baselines: false,
                ..all
            },
            DisplayOptions {
                intervals: false,
                ..all
            },
            DisplayOptions {
                run_boundaries: false,
                ..all
            },
            DisplayOptions {
                break_reasons: false,
                ..all
            },
            DisplayOptions {
                reshaped_lines: false,
                ..all
            },
            DisplayOptions {
                relations: false,
                ..all
            },
            DisplayOptions {
                diagnostics: false,
                ..all
            },
        ] {
            let stripped: Vec<_> = snapshot
                .to_display_lists(options)
                .iter()
                .map(|p| p.content_only())
                .collect();
            assert_eq!(
                stripped, content,
                "{}: overlay affects content",
                fixture.name
            );
        }
    }
}

/// Snapshot the added overlay geometry alone, with all statuses, severities,
/// break reasons and a reshaped line represented regardless of what the
/// current composer/relation pass happens to produce.
#[test]
fn debug_explainability() {
    use reprise_display::{DisplayList, Item, Layer};
    use reprise_layout::{Diagnostic, RelationStatus, Subject};
    let fixture = hostile::combining_marks().unwrap();
    let mut snapshot = fixture.engine.layout(&fixture.doc);
    let reasons = ["opportunity", "forced", "end", "overflow"];
    for (index, line) in snapshot
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.lines)
        .enumerate()
    {
        line.explanation.reason =
            serde_json::from_value(serde_json::json!(reasons[index % reasons.len()])).unwrap();
        line.explanation.reshaped = index == 0;
    }
    for (index, severity) in [Severity::Info, Severity::Warning, Severity::Error]
        .into_iter()
        .enumerate()
    {
        let mut diagnostic = Diagnostic::new(
            severity,
            reprise_diag::Code::new("test.display"),
            Subject::Node(snapshot.blocks[0].node),
            "test marker",
        );
        diagnostic.bytes = Some([7..16, 16..24, 0..0][index].clone());
        snapshot.diagnostics.push(diagnostic);
    }
    let relation = snapshot.relations[0].clone();
    for status in [
        RelationStatus::Rebound,
        RelationStatus::Ambiguous,
        RelationStatus::Missing,
    ] {
        let mut relation = relation.clone();
        relation.status = status;
        relation.applied = false;
        for target in &mut relation.targets {
            target.status = status;
            if status == RelationStatus::Missing {
                target.resolved = None;
            }
        }
        snapshot.relations.push(relation);
    }
    let lists = snapshot.to_display_lists(DisplayOptions {
        debug: true,
        ..Default::default()
    });
    fn debug(item: &Item) -> Option<Item> {
        match item {
            Item::Path {
                layer: Layer::Debug,
                ..
            } => Some(item.clone()),
            Item::Group {
                transform,
                clip,
                items,
            } => Some(Item::Group {
                transform: *transform,
                clip: clip.clone(),
                items: items.iter().filter_map(debug).collect(),
            }),
            _ => None,
        }
    }
    let overlays: Vec<_> = lists
        .iter()
        .map(|list| DisplayList {
            width: list.width,
            height: list.height,
            items: list.items.iter().filter_map(debug).collect(),
        })
        .collect();
    fn path_count(item: &Item) -> usize {
        match item {
            Item::Path {
                layer: Layer::Debug,
                ..
            } => 1,
            Item::Group { items, .. } => items.iter().map(path_count).sum(),
            _ => 0,
        }
    }
    let none = DisplayOptions {
        debug: true,
        boxes: false,
        baselines: false,
        intervals: false,
        run_boundaries: false,
        break_reasons: false,
        reshaped_lines: false,
        relations: false,
        diagnostics: false,
    };
    let families = [
        DisplayOptions {
            boxes: true,
            ..none
        },
        DisplayOptions {
            baselines: true,
            ..none
        },
        DisplayOptions {
            intervals: true,
            ..none
        },
        DisplayOptions {
            run_boundaries: true,
            ..none
        },
        DisplayOptions {
            break_reasons: true,
            ..none
        },
        DisplayOptions {
            reshaped_lines: true,
            ..none
        },
        DisplayOptions {
            relations: true,
            ..none
        },
        DisplayOptions {
            diagnostics: true,
            ..none
        },
    ];
    let counts: Vec<usize> = families
        .iter()
        .map(|options| {
            snapshot
                .to_display_lists(*options)
                .iter()
                .flat_map(|page| &page.items)
                .map(path_count)
                .sum()
        })
        .collect();
    assert!(
        counts.iter().all(|&count| count > 0),
        "every selected family draws geometry"
    );
    assert_eq!(
        counts.iter().sum::<usize>(),
        lists
            .iter()
            .flat_map(|page| &page.items)
            .map(path_count)
            .sum::<usize>(),
        "families are independent and disabling them removes their geometry"
    );
    insta::assert_snapshot!(
        "debug_explainability",
        serde_json::to_string_pretty(&overlays).unwrap()
    );
    for list in &lists {
        reprise_display::svg::render(list, &fixture.engine.fonts).unwrap();
        reprise_display::png::render(list, &fixture.engine.fonts, 1.0).unwrap();
    }
    reprise_display::pdf::render(&lists, &fixture.engine.fonts).unwrap();
}

// --- style notes (17, 18) ---------------------------------------------------
//
// `Document::computed_style` reports what it could not honour as `style.*`
// notes. Layout doesn't publish them yet (the orchestrator wires that into
// `flow.rs`), so these tests check the notes where they are made, and that
// layout still lays out every block.

/// The `style.*` codes of every block, in document order, in a context.
fn style_codes(doc: &Document, context: &ResolutionContext) -> Vec<Vec<String>> {
    doc.blocks()
        .into_iter()
        .map(|node| {
            doc.computed_style_in(node, context)
                .expect("the block exists")
                .notes
                .iter()
                .map(|n| n.code.as_str().to_string())
                .collect()
        })
        .collect()
}

fn size_of(doc: &Document, node: NodeId, context: &ResolutionContext) -> Length {
    doc.computed_style_in(node, context)
        .expect("the block exists")
        .size
}

fn every_block_is_laid_out(fixture: &Fixture) {
    let snapshot = fixture.engine.layout(&fixture.doc);
    assert_eq!(
        snapshot.blocks.len(),
        fixture.doc.blocks().len(),
        "{}: every block is laid out",
        fixture.name
    );
}

fn per_block(expected: &[&[&str]]) -> Vec<Vec<String>> {
    expected
        .iter()
        .map(|codes| codes.iter().map(|c| c.to_string()).collect())
        .collect()
}

#[test]
fn style_expressions_report_and_fall_back() {
    let fixture = hostile::style_expressions().expect("the fixture builds");
    let default = ResolutionContext::default();
    assert_eq!(
        style_codes(&fixture.doc, &default),
        per_block(&[
            &["style.expr-limit"],
            &["style.expr-limit"],
            &["style.type-error"],
            &["style.type-error"],
            &["style.unknown-function"],
            &["style.function-failed"],
            &["style.unparsed"],
            &["style.unparsed"],
        ])
    );
    for node in fixture.doc.blocks() {
        assert_eq!(
            size_of(&fixture.doc, node, &default),
            Length::from_pt(10),
            "a value that can't be used leaves the default size"
        );
    }
    every_block_is_laid_out(&fixture);
}

#[test]
fn style_cycles_are_cut_and_reported() {
    let fixture = hostile::style_cycles().expect("the fixture builds");
    assert_eq!(
        style_codes(&fixture.doc, &ResolutionContext::default()),
        per_block(&[
            &["style.cycle"],
            &["style.cycle"],
            &["style.parent-cycle"],
            &["style.parent-cycle"],
            &["style.parent-missing"],
            &["style.parent-missing"],
        ])
    );
    every_block_is_laid_out(&fixture);
}

#[test]
fn style_bases_that_are_missing_or_indefinite_are_reported() {
    let fixture = hostile::style_bases().expect("the fixture builds");
    // Nothing is known in the default context.
    assert_eq!(
        style_codes(&fixture.doc, &ResolutionContext::default()),
        per_block(&[
            &["style.basis-unresolved"],
            &["style.basis-unresolved"],
            &["style.basis-unresolved"],
            &[],
            &[],
        ])
    );
    // In a frame whose height depends on its content, a percentage of that
    // height is indefinite; a named frame that exists is fine.
    let context = ResolutionContext::default()
        .with_current_frame("main", Extent::auto_height(Length::from_pt(220)))
        .with_named_frame(
            "margin",
            Extent::definite(Length::from_pt(110), Length::from_pt(228)),
        );
    assert_eq!(
        style_codes(&fixture.doc, &context),
        per_block(&[
            &["style.basis-indefinite"],
            &[],
            &["style.basis-unresolved"],
            &[],
            &[],
        ])
    );
    let blocks = fixture.doc.blocks();
    assert_eq!(
        size_of(&fixture.doc, blocks[1], &context),
        Length::from_pt(11)
    );
    // The percentage that needs no frame resolves the same everywhere.
    for context in [&context, &ResolutionContext::default()] {
        assert_eq!(
            size_of(&fixture.doc, blocks[3], context),
            Length::from_pt(12)
        );
    }
    every_block_is_laid_out(&fixture);
}

hostile_tests!(bidi_line_override);

#[test]
fn line_bidi_resets_trailing_spaces_and_preserves_advances() {
    let fixture = hostile::bidi_line_override().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    for (i, block) in snapshot.blocks.iter().enumerate() {
        assert!(block.lines.len() > 1);
        for line in &block.lines {
            let end = line.text.start + block.text[line.text.clone()].trim_end().len();
            for run in &line.runs {
                assert_eq!(
                    run.width,
                    run.glyphs.iter().fold(Length::ZERO, |w, g| w + g.advance)
                );
                for glyph in &run.glyphs {
                    if glyph.cluster as usize >= end {
                        assert_eq!(
                            run.level, i as u8,
                            "trailing whitespace uses paragraph level"
                        );
                    }
                }
            }
            for pair in line.runs.windows(2) {
                assert_eq!(pair[1].x, pair[0].x + pair[0].width);
            }
        }
    }
}

hostile_tests!(follow_lines_across_frames);

hostile_tests!(style_fragment_basis);

#[test]
fn style_uses_the_actual_starting_frame_once_per_block() {
    let fixture = hostile::style_fragment_basis().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    let first = &snapshot.blocks[0];
    assert_eq!(first.style.size, Length::from_pt(10));
    assert!(
        first
            .lines
            .iter()
            .any(|l| snapshot.frame(l.frame).unwrap().name == "narrow")
    );
    for line in &first.lines {
        for run in &line.runs {
            assert_eq!(run.size, Length::from_pt(10));
        }
    }
    for block in &snapshot.blocks {
        for note in &block.style.notes {
            assert!(snapshot.diagnostics.iter().any(|d| d.code == note.code
                && d.severity == note.severity
                && d.subject == Subject::Node(block.node)));
        }
    }
    assert_eq!(snapshot.blocks[1].style.size, Length::from_pt(10));
    let bases = hostile::style_bases().unwrap();
    assert_eq!(
        bases.engine.layout(&bases.doc).blocks[0].style.size,
        Length::from_pt(12)
    );
}

#[test]
fn a_block_that_cannot_start_in_the_current_frame_uses_the_next_frame_basis() {
    use reprise_doc::{Authored, Dim, Expr, PageTemplate, Property, Style};
    use reprise_fixtures::templates::flow_frame;
    let doc = Document::new(reprise_fixtures::PEER).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    doc.set_page_template(
        &PageTemplate::new("start-after-full-frame", Dim::pt(400), Dim::pt(100))
            .with_frame(flow_frame(
                "wide",
                Dim::pt(0),
                Dim::pt(0),
                Dim::pt(200),
                Dim::pt(20),
            ))
            .with_frame(flow_frame(
                "narrow",
                Dim::pt(210),
                Dim::pt(0),
                Dim::pt(100),
                Dim::pt(40),
            )),
    )
    .unwrap();
    doc.append_block(BlockKind::Paragraph, "body", "Fills the first frame.")
        .unwrap();
    let mut style = Style::default();
    style.set(
        Property::Size,
        Authored::Expr(Expr::parse("5% * frame-width").unwrap()),
    );
    doc.define_style("frame-sized", &style).unwrap();
    let p = doc
        .append_block(BlockKind::Paragraph, "frame-sized", "Starts here.")
        .unwrap();
    let snapshot = reprise_fixtures::engine().layout(&doc);
    let block = snapshot.block(p).unwrap();
    assert_eq!(snapshot.frame(block.lines[0].frame).unwrap().name, "narrow");
    assert_eq!(block.style.size, Length::from_pt(5));
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
}

hostile_tests!(justified_bidi);

#[test]
fn justified_bidi_fills_intervals_and_adjusts_only_content_word_spaces() {
    use reprise_compose::{BreakReason, is_word_space};
    use reprise_compose::{ComposeRequest, Composer, Composition, LineFragment, Optimal};
    use std::sync::{Arc, Mutex};
    type Recorded = Arc<Mutex<Vec<(String, LineFragment)>>>;
    struct Recording(Recorded);
    impl Composer for Recording {
        fn name(&self) -> &'static str {
            "optimal-justified"
        }
        fn compose(&self, request: &ComposeRequest<'_>) -> Composition {
            let composed = Optimal::justified().compose(request);
            self.0.lock().unwrap().extend(
                composed
                    .lines
                    .iter()
                    .cloned()
                    .map(|l| (request.text.to_string(), l)),
            );
            composed
        }
    }
    let mut fixture = hostile::justified_bidi().unwrap();
    let recorded = Recorded::default();
    fixture.engine.composer = Box::new(Recording(recorded.clone()));
    let snapshot = fixture.engine.layout(&fixture.doc);
    let mut adjusted_lines = 0;
    let mut rtl_hanging = false;
    for block in &snapshot.blocks {
        for line in &block.lines {
            let source = &block.text[line.text.clone()];
            let end = line.text.start + source.trim_end().len();
            let spaces = source
                .trim_end()
                .chars()
                .filter(|&c| is_word_space(c))
                .count() as i64;
            let all = recorded.lock().unwrap();
            let (_, natural) = all
                .iter()
                .find(|(text, l)| {
                    text == &block.text
                        && l.text == line.text
                        && l.block_offset == line.rect.origin.y
                        && l.available.width() == line.rect.width
                })
                .unwrap();
            let ws = line.explanation.adjustment.word_spacing;
            let mut before = std::collections::BTreeMap::<(u32, u32), Vec<Length>>::new();
            for glyph in natural.runs.iter().flat_map(|r| &r.glyphs) {
                before
                    .entry((glyph.cluster, glyph.id))
                    .or_default()
                    .push(glyph.advance);
            }
            let mut after = std::collections::BTreeMap::<(u32, u32), Vec<Length>>::new();
            for run in &line.runs {
                assert_eq!(
                    run.width,
                    run.glyphs.iter().fold(Length::ZERO, |w, g| w + g.advance)
                );
                for glyph in &run.glyphs {
                    let c = glyph.cluster as usize;
                    let space =
                        c < end && block.text[c..].chars().next().is_some_and(is_word_space);
                    let advance = glyph.advance - if space { ws } else { Length::ZERO };
                    after
                        .entry((glyph.cluster, glyph.id))
                        .or_default()
                        .push(advance);
                }
            }
            for advances in before.values_mut() {
                advances.sort();
            }
            for advances in after.values_mut() {
                advances.sort();
            }
            assert_eq!(
                before, after,
                "only content word-space glyphs are adjusted: {source:?}"
            );
            let used = line
                .runs
                .iter()
                .flat_map(|r| &r.glyphs)
                .filter(|g| (g.cluster as usize) < end)
                .fold(Length::ZERO, |w, g| w + g.advance);
            assert_eq!(line.width, used);
            for pair in line.runs.windows(2) {
                assert_eq!(pair[1].x, pair[0].x + pair[0].width);
            }
            if line.explanation.reason == BreakReason::Opportunity
                && line.explanation.score.is_some()
                && spaces > 0
            {
                adjusted_lines += 1;
                assert!(
                    (line.width.0 as i64 - line.rect.width.0 as i64).abs() <= spaces,
                    "{source:?}"
                );
                let mut actual_end = Length::MIN;
                for run in &line.runs {
                    let mut pen = run.x;
                    for g in &run.glyphs {
                        pen += g.advance;
                        if (g.cluster as usize) < end {
                            actual_end = actual_end.max(pen);
                        }
                    }
                }
                assert!(
                    (actual_end.0 as i64 - line.rect.max_x().0 as i64).abs() <= spaces,
                    "rendered end for {source:?}"
                );
                if block.text.starts_with("אבג")
                    && line.runs.first().is_some_and(|r| r.x < line.rect.origin.x)
                {
                    rtl_hanging = true;
                }
            }
            if matches!(
                line.explanation.reason,
                BreakReason::Forced | BreakReason::End
            ) {
                assert!(ws <= Length::ZERO, "forced and final lines never stretch");
            }
            if spaces == 0 {
                assert_eq!(ws, Length::ZERO);
            }
        }
    }
    assert!(adjusted_lines > 10);
    assert!(rtl_hanging, "RTL trailing spaces hang to the visual left");
}

/// Orchestrator review: after line reordering (L1/L2) and justification
/// rewrite a line's runs, every line of every fixture still has runs that
/// cover disjoint bytes inside the line, sit edge to edge along the inline
/// axis in visual order, and whose glyph advances add up to their width.
#[test]
fn positioned_runs_tile_their_lines() {
    for fixture in hostile::all().expect("fixtures build") {
        let name = fixture.name;
        let snapshot = fixture.engine.layout(&fixture.doc);
        for block in &snapshot.blocks {
            for (i, line) in block.lines.iter().enumerate() {
                let at = format!("{name}: {} line {i}", block.node);
                let mut ranges: Vec<_> = line.runs.iter().map(|r| r.range.clone()).collect();
                ranges.sort_by_key(|r| r.start);
                for r in &ranges {
                    assert!(
                        line.text.start <= r.start && r.end <= line.text.end,
                        "{at}: run {r:?} outside line {:?}",
                        line.text
                    );
                }
                for pair in ranges.windows(2) {
                    assert!(pair[0].end <= pair[1].start, "{at}: runs overlap");
                }
                for pair in line.runs.windows(2) {
                    assert_eq!(
                        pair[0].x + pair[0].width,
                        pair[1].x,
                        "{at}: runs are not edge to edge"
                    );
                }
                for run in &line.runs {
                    let sum: Length = run.glyphs.iter().map(|g| g.advance).sum();
                    assert_eq!(sum, run.width, "{at}: glyph advances vs run width");
                }
            }
        }
    }
}
hostile_tests!(persistence_tombstones);

hostile_tests!(
    transformed_rtl,
    vertical_rl,
    spiral_text,
    reading_cycle,
    degenerate_transform,
    rational_rotation_extreme
);

// The frozen original fixture checks require annotations to be margin notes.
// Regions add other authored roles; retain the original checks and separately
// exercise the same text/query/backend invariants with their declared roles.
fn check_region(fixture: Fixture) {
    let snapshot = fixture.engine.layout(&fixture.doc);
    assert_eq!(
        snapshot,
        fixture.engine.layout(&fixture.doc),
        "repeatable region layout"
    );
    check_diagnostics(fixture.name, &fixture, &snapshot);
    check_lines(fixture.name, &snapshot);
    check_queries(fixture.name, &snapshot);
    check_backends(fixture.name, &fixture, &snapshot);
    for block in &snapshot.blocks {
        for line in &block.lines {
            let frame = snapshot.frame(line.frame).unwrap();
            assert!(frame.page < snapshot.pages.len());
            assert!(
                line.rect.origin.y + line.rect.height <= frame.rect.height,
                "{}: line outside allocated frame",
                fixture.name
            );
            match block.kind {
                BlockKind::Paragraph => assert_eq!(frame.role, FrameRole::Flow(MAIN_FLOW.into())),
                BlockKind::Annotation => {
                    assert!(matches!(frame.role, FrameRole::Notes | FrameRole::Flow(_)))
                }
            }
        }
    }
    insta::assert_snapshot!(fixture.name, snapshot.to_json());
}

macro_rules! region_hostile_tests {
    ($($name:ident),* $(,)?) => {$(
        #[test]
        fn $name() { check_region(hostile::$name().unwrap()); }
    )*};
}

region_hostile_tests!(
    float_wider_than_frame,
    float_moves_its_anchor,
    note_taller_than_page,
    notes_nested_three_deep,
    note_on_last_line,
    notes_take_over_page,
    table_zero_columns,
    table_conflicting_widths,
    table_row_taller_than_page,
    infeasible_solver_domain,
);

#[test]
fn incremental_page_seam() {
    check(hostile::incremental_page_seam().unwrap());
}

#[test]
fn editing_concurrent_delete_undo() {
    let fixture = hostile::editing_concurrent_delete_undo().unwrap();
    let node = fixture.doc.blocks()[0];
    assert_eq!(
        fixture.doc.block(node).unwrap().text.to_string(),
        "anchor office e\u{301} \u{5D0}\u{5D1}"
    );
    assert_eq!(
        fixture.doc.blocks(),
        fixture.replica.as_ref().unwrap().blocks()
    );
    check(fixture);
}

#[test]
fn editing_half_invalid() {
    use reprise_edit::{Command, Editor, Reason, Transaction};
    let fixture = hostile::editing_half_invalid().unwrap();
    let node = fixture.doc.blocks()[0];
    let keep = fixture.doc.blocks()[1];
    let before = fixture.doc.revision();
    let mut editor = Editor::new(fixture.doc, reprise_doc::SchemaRegistry::builtin());
    let error = editor
        .apply(
            &Transaction::new()
                .with(Command::DeleteBlock { node: keep })
                .with(Command::InsertText {
                    node,
                    at: 2,
                    text: "invalid".into(),
                }),
        )
        .unwrap_err();
    assert_eq!(error.command, Some(1));
    assert_eq!(error.reason, Reason::BadOffset { node, offset: 2 });
    assert_eq!(before, editor.document().revision());
    assert!(editor.document().is_live(keep));
    assert!(!editor.can_undo());
    check(Fixture {
        doc: editor.into_document(),
        ..fixture
    });
}

#[test]
fn editing_empty_document() {
    let fixture = hostile::editing_empty_document().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    let nav = reprise_edit::Navigator::semantic(&snapshot, &fixture.doc);
    assert!(
        nav.hit(0, reprise_geom::Point::new(Length::MIN, Length::MAX))
            .is_none()
    );
    assert!(nav.select_all().is_none());
    check(fixture);
}

#[test]
fn editing_empty_block() {
    use reprise_edit::{Caret, Movement, Navigator};
    let fixture = hostile::editing_empty_block().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    let nav = Navigator::semantic(&snapshot, &fixture.doc);
    let caret = Caret::new(fixture.doc.blocks()[0], 0);
    for movement in [
        Movement::VisualLeft,
        Movement::VisualRight,
        Movement::NextGrapheme,
        Movement::PreviousGrapheme,
        Movement::LineUp,
        Movement::LineDown,
    ] {
        assert!(nav.move_caret(caret, movement).is_some());
    }
    for x in [Length::MIN, Length::ZERO, Length::MAX] {
        for y in [Length::MIN, Length::ZERO, Length::MAX] {
            let hit = nav.hit(0, reprise_geom::Point::new(x, y)).unwrap();
            assert!(nav.caret_rect(hit.caret).is_some());
        }
    }
    check(fixture);
}

#[test]
fn clipboard_unicode_seams() {
    check(hostile::clipboard_unicode_seams().unwrap());
}
