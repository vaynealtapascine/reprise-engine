//! Runs every hostile fixture through layout and every backend, and checks
//! the invariants that no workstream may break.

use reprise_diag::Severity;
use reprise_doc::text::segment;
use reprise_fixtures::hostile::{self, Fixture};
use reprise_layout::{DisplayOptions, LayoutSnapshot, LineRef};

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

/// The query API answers for every position and agrees with itself.
fn check_queries(name: &str, snapshot: &LayoutSnapshot) {
    for block in &snapshot.blocks {
        let node = block.node;
        for at in (0..=block.text.len()).filter(|&i| block.text.is_char_boundary(i)) {
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
            snapshot.lines_in(node, 0..block.text.len()).len(),
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
);

#[test]
fn every_fixture_has_a_test() {
    assert_eq!(hostile::all().expect("fixtures build").len(), 21);
}

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
