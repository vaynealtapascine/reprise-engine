//! The concurrency matrix (docs/collaboration.md): every pair of conflicting
//! kernel commands that matters, from a common base, on two peers that sync
//! through delta packets. Every pair is seeded and run in both merge orders.
//! Each must converge, lay out and display identically on every replica, save
//! and reopen to the same layout, and report only documented diagnostic codes.

use std::collections::BTreeSet;

use reprise_doc::image::ImageData;
use reprise_doc::relation::builtin::FOLLOW;
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, Column, ColumnWidth, Document, LayoutQuery, NewBlock, NodeId, PersistenceMode,
    RangeState, Relation, Style, TableColumns, Target,
};
use reprise_edit::{Command, Editor};
use reprise_fixtures::{OTHER_PEER, PEER, engine};
use reprise_layout::DisplayOptions;

const SEEDS: u64 = 6;
const THIRD_PEER: u64 = 3;

/// The codes in the table of `docs/contracts.md`, plus this workstream's.
fn documented_codes() -> BTreeSet<String> {
    let contracts = include_str!("../../../docs/contracts.md");
    let start = contracts.find("Codes in use:").unwrap_or(0);
    contracts[start..]
        .lines()
        .take_while(|l| l.starts_with('|') || l.trim().is_empty() || l.starts_with("Codes"))
        .flat_map(|l| l.split('`').skip(1).step_by(2).map(str::to_owned))
        .collect()
}

fn text(seed: u64, i: u64) -> String {
    [
        "x",
        "\u{5d0}\u{5d1}",
        "e\u{301}",
        "ffi ",
        "\u{1f469}\u{200d}\u{1f467}",
        " ",
    ][((seed + i) % 6) as usize]
        .repeat(1 + (seed % 3) as usize)
}

fn dump(doc: &Document) -> Vec<(NodeId, Option<NodeId>, String)> {
    doc.document_order()
        .into_iter()
        .map(|id| {
            let text = doc
                .block(id)
                .map(|b| b.text.to_string())
                .unwrap_or_default();
            (id, doc.parent_of(id).flatten(), text)
        })
        .collect()
}

fn sync(from: &Editor, to: &mut Editor) {
    let packet = from
        .document()
        .export_delta(&to.document().version_vector())
        .unwrap();
    to.import_packet(&packet).unwrap();
}

/// Lays out, displays and audits a converged document, returning what must
/// agree across replicas.
fn observe(doc: &Document, codes: &BTreeSet<String>, case: &str) -> String {
    let snapshot = engine().layout(doc);
    let mut seen = snapshot.to_json();
    for list in snapshot.to_display_lists(DisplayOptions::default()) {
        seen.push_str(&list.to_json());
    }
    for d in &snapshot.diagnostics {
        assert!(
            codes.contains(d.code.as_str()),
            "{case}: undocumented code {}",
            d.code.as_str()
        );
    }
    for finding in doc.audit() {
        let code = finding.note.code.as_str().to_owned();
        assert!(codes.contains(&code), "{case}: undocumented code {code}");
        seen.push_str(&format!("{} {code}\n", finding.node));
    }
    seen
}

/// Runs one pair. `setup` builds the base; `left` and `right` act on two
/// peers; the returned editors have converged.
fn pair<T, S, L, R>(case: &str, seed: u64, setup: S, left: L, right: R) -> (Editor, Editor, T)
where
    S: Fn(&Document) -> T,
    L: Fn(&mut Editor, &T),
    R: Fn(&mut Editor, &T),
{
    let codes = documented_codes();
    let base = Document::new(PEER).unwrap();
    reprise_fixtures::spike::define_styles(&base).unwrap();
    let ids = setup(&base);
    base.commit();
    let fork = |peer| Editor::new(base.fork(peer).unwrap(), engine().schemas.clone());
    let (mut a, mut b, mut c) = (fork(PEER), fork(OTHER_PEER), fork(THIRD_PEER));
    let since = a.document().version_vector();
    left(&mut a, &ids);
    right(&mut b, &ids);
    let pa = a.document().export_delta(&since).unwrap();
    let pb = b.document().export_delta(&since).unwrap();
    // Two orders: a and b each receive the other's; c receives b's first.
    sync(&b, &mut a);
    sync(&a, &mut b);
    c.import_packet(&pb).unwrap();
    c.import_packet(&pa).unwrap();
    let case = format!("{case} seed {seed}");
    assert_eq!(dump(a.document()), dump(b.document()), "{case}");
    assert_eq!(dump(a.document()), dump(c.document()), "{case}");
    let seen = observe(a.document(), &codes, &case);
    assert_eq!(seen, observe(b.document(), &codes, &case), "{case}");
    assert_eq!(seen, observe(c.document(), &codes, &case), "{case}");
    let saved = a.document().try_export(PersistenceMode::History).unwrap();
    let reopened = Document::import(&saved, 9).unwrap();
    assert_eq!(observe(&reopened, &codes, &case), seen, "{case}: reopen");
    (a, b, ids)
}

fn paragraphs(doc: &Document, texts: &[&str]) -> Vec<NodeId> {
    texts
        .iter()
        .map(|t| doc.append_block(BlockKind::Paragraph, "body", t).unwrap())
        .collect()
}

fn apply(editor: &mut Editor, command: Command) {
    editor.apply_command(command).unwrap();
}

#[test]
fn delete_block_against_edit_inside_it() {
    for seed in 0..SEEDS {
        let (mut a, b, ids) = pair(
            "delete/edit",
            seed,
            |d| paragraphs(d, &["one two", "three"]),
            |e, ids| apply(e, Command::DeleteBlock { node: ids[0] }),
            |e, ids| {
                apply(
                    e,
                    Command::InsertText {
                        node: ids[0],
                        at: 3,
                        text: text(seed, 0),
                    },
                );
            },
        );
        assert!(!a.document().is_live(ids[0]));
        // Undoing the deletion brings the block back with the concurrent edit.
        assert!(a.undo().unwrap());
        let restored = a.document().block(ids[0]).unwrap().text.to_string();
        assert!(restored.contains(&text(seed, 0)), "{restored}");
        drop(b);
    }
}

#[test]
fn split_against_join_of_the_same_paragraph() {
    for seed in 0..SEEDS {
        pair(
            "split/join",
            seed,
            |d| paragraphs(d, &["hello world", "second"]),
            |e, ids| {
                apply(
                    e,
                    Command::SplitBlock {
                        node: ids[0],
                        at: 5,
                    },
                )
            },
            |e, ids| {
                apply(
                    e,
                    Command::JoinBlocks {
                        first: ids[0],
                        second: ids[1],
                    },
                )
            },
        );
    }
}

#[test]
fn table_row_delete_against_cell_edit() {
    for seed in 0..SEEDS {
        pair(
            "row delete/cell edit",
            seed,
            |d| {
                let table = d
                    .append_table(TableColumns {
                        columns: vec![
                            Column {
                                width: ColumnWidth::Proportional(1)
                            };
                            2
                        ],
                    })
                    .unwrap();
                let row = d.append_table_row(table, false).unwrap();
                let cell = d.append_table_cell(row, 0).unwrap();
                let text = d
                    .append_cell_block(cell, BlockKind::Paragraph, "body", "cell")
                    .unwrap();
                vec![row, text]
            },
            |e, ids| apply(e, Command::DeleteBlock { node: ids[0] }),
            |e, ids| {
                apply(
                    e,
                    Command::InsertText {
                        node: ids[1],
                        at: 4,
                        text: text(seed, 1),
                    },
                )
            },
        );
    }
}

#[test]
fn concurrent_moves_that_would_form_a_cycle() {
    for seed in 0..SEEDS {
        let (a, _, ids) = pair(
            "move cycle",
            seed,
            |d| paragraphs(d, &["p", "q", "r"]),
            |e, ids| {
                apply(
                    e,
                    Command::MoveBlock {
                        node: ids[0],
                        parent: Some(ids[1]),
                        index: 0,
                    },
                )
            },
            |e, ids| {
                apply(
                    e,
                    Command::MoveBlock {
                        node: ids[1],
                        parent: Some(ids[0]),
                        index: 0,
                    },
                )
            },
        );
        let order = a.document().document_order();
        let unique: BTreeSet<_> = order.iter().collect();
        assert_eq!(unique.len(), order.len(), "no node is visited twice");
        assert!(order.contains(&ids[2]));
    }
}

#[test]
fn relation_against_its_owner_or_target_deleted() {
    for seed in 0..SEEDS {
        pair(
            "relation/delete",
            seed,
            |d| {
                let p = d
                    .append_block(BlockKind::Paragraph, "body", "anchor text")
                    .unwrap();
                let n = d
                    .append_block(BlockKind::Annotation, "note", "note")
                    .unwrap();
                let r = d.add_range(p, 0..6, RangePolicy::FIXED).unwrap();
                (p, n, r)
            },
            |e, &(_, n, r)| {
                let relation = Relation::new(FOLLOW).owned_by(n).target(
                    "line",
                    Target::Layout(LayoutQuery::LineContaining { range: r }),
                );
                apply(e, Command::AddRelation { relation });
            },
            |e, &(p, n, _)| {
                let node = if seed % 2 == 0 { p } else { n };
                apply(e, Command::DeleteBlock { node });
            },
        );
    }
}

#[test]
fn style_cycles_and_missing_styles_from_two_peers() {
    for seed in 0..SEEDS {
        pair(
            "style cycle/missing",
            seed,
            |d| paragraphs(d, &["styled"]),
            |e, ids| {
                let style = Style {
                    parent: Some("right".into()),
                    ..Style::default()
                };
                e.document().define_style("left", &style).unwrap();
                e.document().commit();
                apply(
                    e,
                    Command::InsertBlock {
                        parent: None,
                        index: 1,
                        block: NewBlock::new(BlockKind::Paragraph, "left", "uses left"),
                    },
                );
                let _ = ids;
            },
            |e, _| {
                let style = Style {
                    parent: Some("left".into()),
                    ..Style::default()
                };
                e.document().define_style("right", &style).unwrap();
                e.document().commit();
                apply(
                    e,
                    Command::InsertBlock {
                        parent: None,
                        index: 0,
                        block: NewBlock::new(BlockKind::Paragraph, "never-defined", "ghost"),
                    },
                );
            },
        );
    }
}

#[test]
fn image_deleted_against_alt_text_edit() {
    for seed in 0..SEEDS {
        let (mut a, _, img) = pair(
            "image delete/alt",
            seed,
            |d| {
                d.append_image("body", &ImageData::new("ab".repeat(32)), "alt")
                    .unwrap()
            },
            |e, &img| apply(e, Command::DeleteBlock { node: img }),
            |e, &img| {
                apply(
                    e,
                    Command::InsertText {
                        node: img,
                        at: 3,
                        text: text(seed, 2),
                    },
                )
            },
        );
        assert!(a.undo().unwrap());
        let alt = a.document().block(img).unwrap().text.to_string();
        assert!(alt.ends_with(&text(seed, 2)), "{alt}");
    }
}

#[test]
fn range_endpoint_text_deleted_against_insertion_at_it() {
    for seed in 0..SEEDS {
        let (a, b, (_, r)) = pair(
            "range endpoint",
            seed,
            |d| {
                let p = d
                    .append_block(BlockKind::Paragraph, "body", "one two three")
                    .unwrap();
                let r = d.add_range(p, 4..7, RangePolicy::FIXED).unwrap();
                (p, r)
            },
            |e, &(p, _)| {
                apply(
                    e,
                    Command::DeleteText {
                        node: p,
                        range: 4..7,
                    },
                )
            },
            |e, &(p, _)| {
                apply(
                    e,
                    Command::InsertText {
                        node: p,
                        at: 7,
                        text: text(seed, 3),
                    },
                )
            },
        );
        let state = a.document().resolve_range(r);
        assert_eq!(state, b.document().resolve_range(r));
        assert!(!matches!(state, RangeState::Valid { .. }), "{state:?}");
    }
}

#[test]
fn undo_on_one_peer_against_edit_of_the_same_text() {
    for seed in 0..SEEDS {
        let (mut a, mut b, ids) = pair(
            "undo/edit",
            seed,
            |d| paragraphs(d, &["shared"]),
            |e, ids| {
                apply(
                    e,
                    Command::InsertText {
                        node: ids[0],
                        at: 0,
                        text: "AAA".into(),
                    },
                )
            },
            |e, ids| {
                apply(
                    e,
                    Command::InsertText {
                        node: ids[0],
                        at: 6,
                        text: text(seed, 4),
                    },
                )
            },
        );
        assert!(a.undo().unwrap());
        sync(&a, &mut b);
        let (sa, sb) = (
            a.document().block(ids[0]).unwrap().text.to_string(),
            b.document().block(ids[0]).unwrap().text.to_string(),
        );
        assert_eq!(sa, sb);
        assert!(!sa.contains("AAA") && sa.contains(&text(seed, 4)), "{sa}");
    }
}

#[test]
fn child_inserted_into_a_block_joined_concurrently_is_hidden_and_reported() {
    for seed in 0..SEEDS {
        let (a, _, ids) = pair(
            "child/join",
            seed,
            |d| paragraphs(d, &["first", "second"]),
            |e, ids| {
                apply(
                    e,
                    Command::InsertBlock {
                        parent: Some(ids[1]),
                        index: 0,
                        block: NewBlock::new(BlockKind::Paragraph, "body", &text(seed, 5)),
                    },
                );
            },
            |e, ids| {
                apply(
                    e,
                    Command::JoinBlocks {
                        first: ids[0],
                        second: ids[1],
                    },
                )
            },
        );
        let hidden: Vec<_> = a
            .document()
            .audit()
            .into_iter()
            .filter(|f| f.note.code == reprise_doc::invariants::HIDDEN_CONTENT)
            .collect();
        assert_eq!(hidden.len(), 1, "{hidden:?}");
        assert!(!a.document().is_live(hidden[0].node));
        let _ = ids;
    }
}

#[test]
fn two_joins_forming_a_succession_cycle() {
    for seed in 0..SEEDS {
        let (a, _, ids) = pair(
            "join cycle",
            seed,
            |d| paragraphs(d, &["left", "right"]),
            |e, ids| {
                apply(
                    e,
                    Command::JoinBlocks {
                        first: ids[0],
                        second: ids[1],
                    },
                )
            },
            |e, ids| {
                apply(
                    e,
                    Command::JoinBlocks {
                        first: ids[1],
                        second: ids[0],
                    },
                )
            },
        );
        let _ = a.document().succession(ids[0]);
        let _ = a.document().succession(ids[1]);
    }
}
