use reprise_doc::sections::{BreakKind, SectionId, SectionProperties};
use reprise_doc::{BlockKind, Document, Domain, NewBlock, NodeId, SchemaRegistry};
use reprise_edit::{BreakAt, Command, Editor, Reason, RegionBinding, StoryFrom, Transaction};
use serde_json::json;
fn para(t: &str) -> NewBlock {
    NewBlock {
        kind: BlockKind::Paragraph,
        style: "body".into(),
        text: t.into(),
        overrides: Default::default(),
    }
}
fn setup() -> (Editor, NodeId) {
    let d = Document::new(1).unwrap();
    let n = d
        .append_block(BlockKind::Paragraph, "body", "aé🙂z")
        .unwrap();
    d.commit();
    (Editor::new(d, SchemaRegistry::builtin()), n)
}
fn patch(k: &str, v: serde_json::Value) -> SectionProperties {
    [(k.into(), v)].into()
}
fn before(n: NodeId, k: BreakKind, p: SectionProperties) -> Command {
    Command::InsertBreak {
        at: BreakAt::Before(n),
        kind: k,
        patch: p,
    }
}
fn record(e: &Editor, n: NodeId) -> reprise_doc::sections::StartRecord {
    e.document()
        .effective_starts()
        .into_iter()
        .find(|r| r.id == SectionId::Block(n))
        .unwrap()
}
fn reopen(d: &Document) -> Document {
    Document::import(
        &d.try_export(reprise_doc::PersistenceMode::History).unwrap(),
        99,
    )
    .unwrap()
}
fn dump(
    d: &Document,
) -> (
    Vec<(NodeId, String)>,
    Vec<reprise_doc::sections::StartRecord>,
) {
    (
        d.authored_order()
            .into_iter()
            .map(|n| (n, d.block(n).unwrap().text.to_string()))
            .collect(),
        d.effective_starts(),
    )
}
#[test]
fn breaks_split_utf8_and_redo_the_exact_address_and_lifetime() {
    for kind in [BreakKind::Section, BreakKind::Page, BreakKind::Column] {
        for offset in [0, 1, 3, 7, 8] {
            let (mut e, n) = setup();
            let a = e
                .apply_command(Command::InsertBreak {
                    at: BreakAt::Position { node: n, offset },
                    kind,
                    patch: Default::default(),
                })
                .unwrap();
            let address = if offset == 0 { n } else { a.blocks[0] };
            let life = record(&e, address).life;
            assert_eq!(record(&e, address).kind, Some(kind));
            assert_eq!(
                e.document().top(Domain::Body).len(),
                if offset == 0 { 1 } else { 2 }
            );
            assert!(e.undo().unwrap());
            assert!(e.document().effective_starts().is_empty());
            assert_eq!(e.document().block(n).unwrap().text.to_string(), "aé🙂z");
            assert!(e.redo().unwrap());
            assert_eq!(record(&e, address).life, life);
            assert_eq!(dump(e.document()), dump(&reopen(e.document())));
        }
    }
}
#[test]
fn failed_batches_and_empty_first_clear_do_not_stage_roots_or_undo() {
    let (mut e, n) = setup();
    let bytes = e
        .document()
        .try_export(reprise_doc::PersistenceMode::History)
        .unwrap();
    let count = e.undo_count();
    let tx = Transaction::new()
        .with(before(n, BreakKind::Section, Default::default()))
        .with(Command::InsertText {
            node: n,
            at: 2,
            text: "x".into(),
        });
    assert!(matches!(
        e.apply(&tx).unwrap_err().reason,
        Reason::BadOffset { .. }
    ));
    assert!(!e.document().has_sections());
    assert_eq!(e.undo_count(), count);
    assert_eq!(
        e.document()
            .try_export(reprise_doc::PersistenceMode::History)
            .unwrap(),
        bytes
    );
    e.apply_command(Command::SetSection {
        section: SectionId::First,
        patch: Default::default(),
        clear: vec!["width".into()],
    })
    .unwrap();
    assert!(!e.document().has_sections());
    assert_eq!(e.undo_count(), count);
}
#[test]
fn reset_is_atomic_and_copies_only_the_designs_exact_geometry_set() {
    let (mut e, n) = setup();
    let first = [
        ("width".into(), json!(400000)),
        ("columns.future".into(), json!(3)),
        ("region.header.default".into(), json!("")),
        ("numbering.restart".into(), json!("42")),
        ("unrecognized".into(), json!("keep")),
    ]
    .into();
    e.apply_command(Command::SetSection {
        section: SectionId::First,
        patch: first,
        clear: vec![],
    })
    .unwrap();
    e.apply_command(before(
        n,
        BreakKind::Page,
        patch("start", json!("odd-page")),
    ))
    .unwrap();
    let old = record(&e, n).life;
    assert!(
        matches!(e.apply_command(before(n,BreakKind::Section,Default::default())).unwrap_err().reason,Reason::BreakExists(x) if x==n)
    );
    e.apply(
        &Transaction::new()
            .with(Command::RemoveBreak { block: n })
            .with(before(
                n,
                BreakKind::Section,
                patch("height", json!(500000)),
            ))
            .with(Command::SetSection {
                section: SectionId::Block(n),
                patch: patch("left", json!(2000)),
                clear: vec!["width".into()],
            }),
    )
    .unwrap();
    let r = record(&e, n);
    assert_ne!(r.life, old);
    assert_eq!(r.own.get("columns.future"), Some(&json!(3)));
    assert!(!r.own.contains_key("width"));
    assert!(!r.own.contains_key("start"));
    assert!(!r.own.contains_key("region.header.default"));
    assert!(!r.own.contains_key("numbering.restart"));
    assert!(!r.own.contains_key("unrecognized"));
    assert!(e.undo().unwrap());
    assert_eq!(record(&e, n).life, old);
    assert_eq!(record(&e, n).kind, Some(BreakKind::Page));
    assert!(e.redo().unwrap());
    assert_eq!(record(&e, n), r);
}
#[test]
fn two_peers_converge_for_mid_insert_same_address_and_page_vs_section() {
    for mid in [false, true] {
        for (ka, kb) in [
            (BreakKind::Section, BreakKind::Section),
            (BreakKind::Page, BreakKind::Section),
        ] {
            let (mut a, n) = setup();
            let mut b = Editor::new(a.document().fork(2).unwrap(), SchemaRegistry::builtin());
            let at = if mid {
                BreakAt::Position { node: n, offset: 3 }
            } else {
                BreakAt::Before(n)
            };
            a.apply_command(Command::InsertBreak {
                at,
                kind: ka,
                patch: patch("width", json!(111000)),
            })
            .unwrap();
            b.apply_command(Command::InsertBreak {
                at,
                kind: kb,
                patch: patch("height", json!(222000)),
            })
            .unwrap();
            let ab = a.document().fork(3).unwrap();
            ab.merge(b.document()).unwrap();
            let ba = b.document().fork(4).unwrap();
            ba.merge(a.document()).unwrap();
            assert_eq!(dump(&ab), dump(&ba));
            assert_eq!(dump(&ab), dump(&reopen(&ab)));
            assert_eq!(ab.effective_starts().len(), if mid { 2 } else { 1 });
            if !mid {
                let own = &ab.effective_starts()[0].own;
                assert_ne!(own.contains_key("width"), own.contains_key("height"));
            }
        }
    }
}
#[test]
fn remove_edit_divergent_resets_and_undo_reset_keep_old_properties_inert() {
    for action in 0..4 {
        let (mut a, n) = setup();
        a.apply_command(before(n, BreakKind::Section, patch("width", json!(111000))))
            .unwrap();
        let old = record(&a, n).life;
        let mut b = Editor::new(a.document().fork(2).unwrap(), SchemaRegistry::builtin());
        a.apply_command(Command::RemoveBreak { block: n }).unwrap();
        if action != 0 {
            a.apply_command(before(
                n,
                BreakKind::Section,
                patch("height", json!(222000)),
            ))
            .unwrap();
        }
        if action == 2 {
            b.apply_command(Command::RemoveBreak { block: n }).unwrap();
            b.apply_command(before(n, BreakKind::Section, patch("left", json!(333))))
                .unwrap();
        } else {
            b.apply_command(Command::SetSection {
                section: SectionId::Block(n),
                patch: patch("right", json!(444)),
                clear: vec![],
            })
            .unwrap();
        }
        if action == 3 {
            assert!(a.undo().unwrap());
            assert!(a.undo().unwrap());
            assert_eq!(record(&a, n).life, old);
        }
        let ab = a.document().fork(3).unwrap();
        ab.merge(b.document()).unwrap();
        let ba = b.document().fork(4).unwrap();
        ba.merge(a.document()).unwrap();
        assert_eq!(dump(&ab), dump(&ba));
        assert_eq!(dump(&ab), dump(&reopen(&ab)));
        if action == 0 {
            assert!(ab.effective_starts().is_empty());
        } else if action == 1 {
            assert!(!ab.effective_starts()[0].own.contains_key("right"));
        }
    }
}
#[test]
fn clone_uses_sequential_story_edits_keeps_formatting_and_redo_ids() {
    let (mut e, _) = setup();
    let story = e
        .apply_command(Command::CreateStory {
            blocks: vec![para("hello"), para("world")],
        })
        .unwrap()
        .stories[0];
    let node = e.document().top(Domain::Story(story))[0];
    let t = Transaction::new()
        .with(Command::InsertText {
            node,
            at: 5,
            text: "!".into(),
        })
        .with(Command::FormatText {
            node,
            range: 0..5,
            style: reprise_doc::formatting::TextStyle {
                weight: Some(700),
                ..Default::default()
            },
        })
        .with(Command::CreateRegion {
            section: SectionId::First,
            slot: "header".into(),
            variant: "default".into(),
            from: StoryFrom::CopyOf(story),
        });
    let copy = e.apply(&t).unwrap().stories[0];
    let ids = e.document().top(Domain::Story(copy));
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], node);
    assert_eq!(
        e.document().block(ids[0]).unwrap().text.to_string(),
        "hello!"
    );
    assert_eq!(
        e.document().text_formats(ids[0]).unwrap().runs,
        e.document().text_formats(node).unwrap().runs
    );
    assert!(e.undo().unwrap());
    assert!(!e.document().is_live(copy));
    assert_eq!(e.document().block(node).unwrap().text.to_string(), "hello");
    assert!(e.redo().unwrap());
    assert_eq!(e.document().top(Domain::Story(copy)), ids);
    e.apply_command(Command::SetRegion {
        section: SectionId::First,
        slot: "header".into(),
        variant: "default".into(),
        binding: RegionBinding::Inherit,
    })
    .unwrap();
    assert!(e.document().is_live(copy));
    assert!(e.document().is_live(story));
    e.apply_command(Command::DeleteStory { story: copy })
        .unwrap();
    assert!(!e.document().is_live(ids[0]));
    assert!(e.undo().unwrap());
    assert!(e.document().is_live(ids[0]));
}
#[test]
fn stories_stay_editable_after_link_away_and_cross_domain_joins_are_refused() {
    let (mut a, n) = setup();
    let story = a
        .apply_command(Command::CreateRegion {
            section: SectionId::First,
            slot: "header".into(),
            variant: "default".into(),
            from: StoryFrom::Empty,
        })
        .unwrap()
        .stories[0];
    let p = a
        .apply_command(Command::InsertBlock {
            parent: Some(story),
            index: 0,
            block: para("story"),
        })
        .unwrap()
        .blocks[0];
    let mut b = Editor::new(a.document().fork(2).unwrap(), SchemaRegistry::builtin());
    a.apply_command(Command::SetRegion {
        section: SectionId::First,
        slot: "header".into(),
        variant: "default".into(),
        binding: RegionBinding::None,
    })
    .unwrap();
    b.apply_command(Command::InsertText {
        node: p,
        at: 5,
        text: "!".into(),
    })
    .unwrap();
    a.merge(b.document()).unwrap();
    assert!(a.document().is_live(story));
    assert_eq!(a.document().block(p).unwrap().text.to_string(), "story!");
    let bytes = a
        .document()
        .try_export(reprise_doc::PersistenceMode::History)
        .unwrap();
    assert!(matches!(
        a.apply_command(Command::JoinBlocks {
            first: n,
            second: p
        })
        .unwrap_err()
        .reason,
        Reason::CrossDomain { .. }
    ));
    assert_eq!(
        a.document()
            .try_export(reprise_doc::PersistenceMode::History)
            .unwrap(),
        bytes
    );
    assert!(
        a.apply_command(Command::InsertBreak {
            at: BreakAt::Position { node: p, offset: 0 },
            kind: BreakKind::Section,
            patch: Default::default()
        })
        .is_err()
    );
}
#[test]
fn shared_list_definitions_number_independently_in_every_domain_and_cache_all_items() {
    let (mut e, n) = setup();
    let list = e
        .apply_command(Command::CreateList {
            definition: reprise_doc::ListDefinition::numbered(),
        })
        .unwrap()
        .lists[0];
    e.apply_command(Command::SetListItem {
        node: n,
        list: reprise_edit::ListRef::Existing(list),
        level: 0,
    })
    .unwrap();
    let mut all = vec![n];
    for _ in 0..2 {
        let story = e
            .apply_command(Command::CreateStory {
                blocks: vec![para("a"), para("b")],
            })
            .unwrap()
            .stories[0];
        let ids = e.document().top(Domain::Story(story));
        for &node in &ids {
            e.apply_command(Command::SetListItem {
                node,
                list: reprise_edit::ListRef::Existing(list),
                level: 0,
            })
            .unwrap();
        }
        all.extend(ids);
    }
    for _ in 0..3 {
        let numbering = e.document().list_numbering();
        assert_eq!(numbering.items.len(), 5);
        let values: Vec<_> = all.iter().map(|n| numbering.items[n].value).collect();
        assert_eq!(values, [1, 1, 2, 1, 2]);
    }
    e.apply_command(Command::RestartList {
        node: all[1],
        value: Some(20),
    })
    .unwrap();
    assert_eq!(e.document().list_item(all[2]).unwrap().value, 21);
    assert_eq!(e.document().list_item(all[3]).unwrap().value, 1);
}
#[test]
fn inherited_head_split_uses_the_returned_copy_and_lineage_id() {
    let d = Document::new(1).unwrap();
    let head = d
        .append_block(BlockKind::Paragraph, "body", "one two")
        .unwrap();
    let surviving = d.split_block(head, 4).unwrap();
    d.commit();
    let remote = d.fork(2).unwrap();
    d.delete_block(head).unwrap();
    d.commit();
    remote.block(head).unwrap().text.insert(0, "zero ").unwrap();
    d.merge(&remote).unwrap();
    let original = d.block(surviving).unwrap().text.to_string();
    let mut e = Editor::new(d, SchemaRegistry::builtin());
    let a = e
        .apply_command(Command::InsertBreak {
            at: BreakAt::Position {
                node: surviving,
                offset: 2,
            },
            kind: BreakKind::Section,
            patch: Default::default(),
        })
        .unwrap();
    let tail = a.blocks[0];
    assert_eq!(record(&e, tail).id, SectionId::Block(tail));
    assert_eq!(
        e.document().block(surviving).unwrap().text.to_string(),
        &original[..2]
    );
    assert_eq!(
        e.document().block(tail).unwrap().text.to_string(),
        &original[2..]
    );
    let life = record(&e, tail).life;
    assert!(e.undo().unwrap());
    assert_eq!(
        e.document().block(surviving).unwrap().text.to_string(),
        original
    );
    assert!(e.redo().unwrap());
    assert_eq!(record(&e, tail).life, life);
}
#[test]
fn simultaneous_unlinks_keep_both_orphans_and_bind_whole_stories() {
    let (mut a, _) = setup();
    let story = a
        .apply_command(Command::CreateStory {
            blocks: vec![para("source")],
        })
        .unwrap()
        .stories[0];
    a.apply_command(Command::SetRegion {
        section: SectionId::First,
        slot: "header".into(),
        variant: "default".into(),
        binding: RegionBinding::Story(story),
    })
    .unwrap();
    let mut b = Editor::new(a.document().fork(2).unwrap(), SchemaRegistry::builtin());
    let command = || Command::CreateRegion {
        section: SectionId::First,
        slot: "header".into(),
        variant: "default".into(),
        from: StoryFrom::CopyOf(story),
    };
    let ac = a.apply_command(command()).unwrap().stories[0];
    let bc = b.apply_command(command()).unwrap().stories[0];
    let ab = a.document().fork(3).unwrap();
    ab.merge(b.document()).unwrap();
    let ba = b.document().fork(4).unwrap();
    ba.merge(a.document()).unwrap();
    assert_eq!(dump(&ab), dump(&ba));
    assert!(ab.stories().contains(&story));
    assert!(ab.stories().contains(&ac));
    assert!(ab.stories().contains(&bc));
    let mut recovery = Editor::new(ab, SchemaRegistry::builtin());
    recovery
        .apply_command(Command::SetRegion {
            section: SectionId::First,
            slot: "footer".into(),
            variant: "default".into(),
            binding: RegionBinding::Story(ac),
        })
        .unwrap();
    assert!(recovery.document().is_live(ac));
    assert_eq!(recovery.document().stories().len(), 3);
}
#[test]
fn cross_domain_ranges_are_refused_at_anchor_and_at_replacement_without_writes() {
    use reprise_edit::{Affinity, Caret, Selection, StableCaret, StableError, StableSelection};
    let (mut e, n) = setup();
    let story = e
        .apply_command(Command::CreateStory {
            blocks: vec![para("story")],
        })
        .unwrap()
        .stories[0];
    let p = e.document().top(Domain::Story(story))[0];
    let caret = |node| Caret {
        node,
        offset: 0,
        affinity: Affinity::Downstream,
    };
    assert!(matches!(
        StableSelection::anchor(
            e.document(),
            &Selection {
                anchor: caret(n),
                focus: caret(p)
            }
        ),
        Err(StableError::CrossDomain { .. })
    ));
    let hostile = StableSelection {
        anchor: StableCaret::anchor(e.document(), caret(n)).unwrap(),
        focus: StableCaret::anchor(e.document(), caret(p)).unwrap(),
    };
    let fragment = e
        .document()
        .copy_fragment("empty", &[], e.schemas())
        .unwrap();
    let staged = e.stage_replacement(&fragment, "empty").unwrap();
    let bytes = e
        .document()
        .try_export(reprise_doc::PersistenceMode::History)
        .unwrap();
    let undo = e.undo_count();
    assert!(matches!(
        e.replace_selection(&hostile, &staged).unwrap_err().reason,
        Reason::CrossDomain { .. }
    ));
    assert_eq!(e.undo_count(), undo);
    assert_eq!(
        e.document()
            .try_export(reprise_doc::PersistenceMode::History)
            .unwrap(),
        bytes
    );
}
#[test]
fn body_top_level_tables_accept_before_breaks_and_cell_positions_refuse_them() {
    let d = Document::new(1).unwrap();
    let t = d
        .append_table(reprise_doc::TableColumns { columns: vec![] })
        .unwrap();
    let row = d.append_table_row(t, false).unwrap();
    let cell = d.append_table_cell(row, 0).unwrap();
    let p = d
        .append_cell_block(cell, BlockKind::Paragraph, "body", "cell")
        .unwrap();
    d.commit();
    let mut e = Editor::new(d, SchemaRegistry::builtin());
    e.apply_command(before(t, BreakKind::Section, Default::default()))
        .unwrap();
    assert_eq!(record(&e, t).id, SectionId::Block(t));
    assert!(
        e.apply_command(Command::InsertBreak {
            at: BreakAt::Position { node: p, offset: 0 },
            kind: BreakKind::Section,
            patch: Default::default()
        })
        .is_err()
    );
    assert!(e.undo().unwrap());
    assert!(e.document().is_live(t));
    assert!(e.document().effective_starts().is_empty());
}
#[test]
fn the_empty_first_section_participates_in_bindings_after_prefix_deletion() {
    let (mut e, n) = setup();
    let second = e
        .apply_command(Command::SplitBlock { node: n, at: 3 })
        .unwrap()
        .blocks[0];
    let story = e
        .apply_command(Command::CreateStory {
            blocks: vec![para("header")],
        })
        .unwrap()
        .stories[0];
    e.apply_command(Command::SetRegion {
        section: SectionId::First,
        slot: "header".into(),
        variant: "default".into(),
        binding: RegionBinding::Story(story),
    })
    .unwrap();
    e.apply_command(before(n, BreakKind::Section, Default::default()))
        .unwrap();
    e.apply_command(before(second, BreakKind::Section, Default::default()))
        .unwrap();
    let table = e.document().section_table();
    assert_eq!(table.len(), 3);
    assert_eq!(table[0].first_block, None);
    assert_eq!(table[2].bindings[0].resolved, Some(story));
    assert_eq!(table[2].bindings[0].from_section, SectionId::First);
    e.apply_command(Command::SetRegion {
        section: SectionId::Block(n),
        slot: "header".into(),
        variant: "default".into(),
        binding: RegionBinding::None,
    })
    .unwrap();
    assert_eq!(e.document().section_table()[2].bindings[0].resolved, None);
    e.apply_command(Command::DeleteBlock { node: n }).unwrap();
    let table = e.document().section_table();
    assert_eq!(table.len(), 2);
    assert_eq!(table[0].first_block, None);
    assert_eq!(table[1].bindings[0].resolved, Some(story));
    assert_eq!(table[1].bindings[0].from_section, SectionId::First);
}
