//! Roles/lists through app-owned DTOs, batch, copy lineage and reading ranges.
use reprise::*;
const DOC: &str = "00112233445566778899aabbccddeeff";
fn session() -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: "1".into(),
        }))
        .unwrap()
}
fn apply(s: &mut DocumentSession, commands: Vec<Command>) -> Applied {
    s.apply(&Payload::new(Transaction { commands }))
        .unwrap()
        .data
}
fn paragraphs(s: &mut DocumentSession, texts: &[&str]) -> Vec<String> {
    apply(
        s,
        texts
            .iter()
            .enumerate()
            .map(|(index, text)| Command::InsertBlock {
                parent: None,
                index: index as u32,
                block_kind: BlockKind::Paragraph,
                text: (*text).into(),
                style: Style::default(),
            })
            .collect(),
    )
    .blocks
}
fn definition(format: &str, start: &str) -> ListDefinition {
    ListDefinition {
        base: None,
        levels: vec![ListLevel {
            level: 0,
            format: Some(format.into()),
            start: Some(start.into()),
            ..Default::default()
        }],
    }
}
fn listify(s: &mut DocumentSession, ids: &[String], d: ListDefinition) -> String {
    let mut commands = vec![Command::CreateList { definition: d }];
    commands.extend(ids.iter().map(|node| Command::SetListItem {
        node: node.clone(),
        list: ListRef::New { index: 0 },
        level: 0,
    }));
    apply(s, commands).lists[0].clone()
}
fn markers(s: &DocumentSession) -> Vec<Option<String>> {
    s.state()
        .unwrap()
        .data
        .blocks
        .into_iter()
        .map(|b| b.list.map(|i| i.marker))
        .collect()
}
fn finish(s: &mut DocumentSession) {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    while !job.step(s, 1000).unwrap().data.complete {}
}
#[test]
fn role_and_named_style_share_one_checked_undo_step_and_outline_has_pages() {
    let mut s = session();
    let ids = paragraphs(&mut s, &["First heading", "body", "Third heading"]);
    let revision = s.state().unwrap().data.revision;
    let batch = Batch {
        commands: vec![
            Command::SetRole {
                node: ids[0].clone(),
                role: BlockRole::Heading2,
            },
            Command::ApplyStyle {
                node: ids[0].clone(),
                style: Some("Heading 2".into()),
            },
            Command::SetRole {
                node: ids[2].clone(),
                role: BlockRole::Heading6,
            },
        ],
        expected_revision: Some(revision),
        selection: None,
    };
    s.batch(&Payload::new(batch.clone())).unwrap();
    assert!(s.batch(&Payload::new(batch)).is_err());
    let outline = s.outline().unwrap().data;
    assert_eq!(
        outline
            .iter()
            .map(|h| (h.level, h.text.as_str(), h.page))
            .collect::<Vec<_>>(),
        [(2, "First heading", None), (6, "Third heading", None)]
    );
    insta::assert_json_snapshot!("outline_before_layout", outline);
    finish(&mut s);
    assert!(s.outline().unwrap().data.iter().all(|h| h.page == Some(0)));
    assert_eq!(s.state().unwrap().data.blocks[0].style.name, "Heading 2");
    s.undo().unwrap();
    assert!(s.outline().unwrap().data.is_empty());
    assert_eq!(s.state().unwrap().data.blocks[0].style.name, "");
    s.redo().unwrap();
    assert_eq!(s.state().unwrap().data.blocks[0].role, BlockRole::Heading2);
}
#[test]
fn all_list_primitives_work_in_batches_and_refusals_are_atomic() {
    let mut s = session();
    let ids = paragraphs(&mut s, &["one", "two", "body", "three", "four"]);
    let list = listify(&mut s, &ids[..2], definition("decimal", "5"));
    let other = listify(&mut s, &ids[3..], definition("decimal-leading-zero", "1"));
    assert_eq!(
        markers(&s),
        [
            Some("5.".into()),
            Some("6.".into()),
            None,
            Some("01.".into()),
            Some("02.".into())
        ]
    );
    let commands = vec![
        Command::ContinueList {
            node: ids[3].clone(),
        },
        Command::RestartList {
            node: ids[3].clone(),
            value: Some("10".into()),
        },
        Command::ContinueList {
            node: ids[3].clone(),
        },
    ];
    s.batch(&Payload::new(Batch {
        commands,
        expected_revision: Some(s.state().unwrap().data.revision),
        selection: None,
    }))
    .unwrap();
    assert_eq!(markers(&s)[3..], [Some("7.".into()), Some("8.".into())]);
    apply(
        &mut s,
        vec![
            Command::SetListLevel {
                node: ids[1].clone(),
                level: 1,
            },
            Command::IndentListItem {
                node: ids[1].clone(),
                by: -1,
            },
            Command::UpdateList {
                list: list.clone(),
                definition: definition("upper-alpha", "1"),
            },
        ],
    );
    assert_eq!(markers(&s)[..2], [Some("A.".into()), Some("B.".into())]);
    let before = s.state().unwrap().data;
    for command in [
        Command::SetListItem {
            node: ids[2].clone(),
            list: ListRef::New { index: 99 },
            level: 0,
        },
        Command::SetListLevel {
            node: ids[0].clone(),
            level: 9,
        },
        Command::RestartList {
            node: ids[0].clone(),
            value: Some("01".into()),
        },
        Command::RestartList {
            node: ids[0].clone(),
            value: Some("9223372036854775808".into()),
        },
    ] {
        assert!(
            s.batch(&Payload::new(Batch {
                commands: vec![
                    Command::SetRole {
                        node: ids[2].clone(),
                        role: BlockRole::Title
                    },
                    command
                ],
                expected_revision: Some(before.revision.clone()),
                selection: None
            }))
            .is_err()
        );
        assert_eq!(s.state().unwrap().data, before);
    }
    apply(
        &mut s,
        vec![
            Command::DeleteList {
                list: ListRef::Existing { id: other },
            },
            Command::RemoveListItem {
                node: ids[1].clone(),
            },
            Command::DeleteList {
                list: ListRef::Existing { id: list },
            },
        ],
    );
    assert!(markers(&s).iter().all(Option::is_none));
    finish(&mut s);
    let notes = s
        .diagnostics_for(&Payload::new(DiagnosticFilter {
            code_prefix: Some("list.definition-missing".into()),
            subject: Some(ids[0].clone()),
            min_severity: Some(Severity::Warning),
            limit: Some(5),
        }))
        .unwrap()
        .data;
    assert_eq!(notes.diagnostics.len(), 1);
    assert_eq!(notes.diagnostics[0].code, "list.definition-missing");
    s.undo().unwrap();
    assert!(markers(&s)[0].is_some());
}
#[test]
fn app_key_commands_and_reading_ranges_keep_authored_offsets() {
    let mut s = session();
    let ids = paragraphs(&mut s, &["café wraps across lines ", ""]);
    listify(&mut s, &ids, definition("decimal", "1"));
    let tail = apply(
        &mut s,
        vec![Command::Enter {
            node: ids[0].clone(),
            at: 5,
        }],
    )
    .blocks[0]
        .clone();
    assert!(
        s.state()
            .unwrap()
            .data
            .blocks
            .iter()
            .find(|b| b.id == tail)
            .unwrap()
            .list
            .is_some()
    );
    apply(
        &mut s,
        vec![Command::BackspaceAtStart { node: tail.clone() }],
    );
    assert!(
        s.state()
            .unwrap()
            .data
            .blocks
            .iter()
            .find(|b| b.id == tail)
            .unwrap()
            .list
            .is_none()
    );
    apply(
        &mut s,
        vec![
            Command::BackspaceAtStart { node: tail },
            Command::IndentListItem {
                node: ids[1].clone(),
                by: 1,
            },
            Command::Enter {
                node: ids[1].clone(),
                at: 0,
            },
        ],
    );
    assert_eq!(
        s.state().unwrap().data.blocks[1]
            .list
            .as_ref()
            .unwrap()
            .level,
        0
    );
    apply(
        &mut s,
        vec![Command::Enter {
            node: ids[1].clone(),
            at: 0,
        }],
    );
    assert!(s.state().unwrap().data.blocks[1].list.is_none());
    let old = s.state().unwrap().data;
    assert!(
        s.apply(&Payload::new(Transaction {
            commands: vec![Command::Enter {
                node: ids[1].clone(),
                at: 1
            }]
        }))
        .is_err()
    );
    assert_eq!(s.state().unwrap().data, old);
    finish(&mut s);
    for step in s.reading_order().unwrap().data {
        let state = s.state().unwrap().data;
        let b = state.blocks.iter().find(|b| b.id == step.node).unwrap();
        assert!(b.text.get(step.start as usize..step.end as usize).is_some());
        if step.line == 0 && b.list.is_some() {
            assert_eq!(step.marker, b.list.as_ref().map(|i| i.marker.clone()));
        } else {
            assert!(step.marker.is_none());
        }
    }
}
#[test]
fn import_copy_remaps_roles_membership_and_inherited_and_unreferenced_definitions() {
    let mut s = session();
    let ids = paragraphs(&mut s, &["Heading", "item"]);
    let base = apply(
        &mut s,
        vec![Command::CreateList {
            definition: definition("decimal-leading-zero", "7"),
        }],
    )
    .lists[0]
        .clone();
    let child = apply(
        &mut s,
        vec![Command::CreateList {
            definition: ListDefinition {
                base: Some(base.clone()),
                levels: vec![],
            },
        }],
    )
    .lists[0]
        .clone();
    let unused = apply(
        &mut s,
        vec![Command::CreateList {
            definition: definition("upper-roman", "2"),
        }],
    )
    .lists[0]
        .clone();
    apply(
        &mut s,
        vec![
            Command::SetRole {
                node: ids[0].clone(),
                role: BlockRole::Heading1,
            },
            Command::SetListItem {
                node: ids[1].clone(),
                list: ListRef::Existing { id: child.clone() },
                level: 0,
            },
        ],
    );
    let bytes = s.save().unwrap().data.bytes;
    let copy = Workspace::new()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: "ffeeddccbbaa99887766554433221100".into(),
                peer_id: "3".into(),
            }),
            &bytes,
        )
        .unwrap();
    let state = copy.state().unwrap().data;
    assert!(!state.can_undo);
    assert_eq!(state.blocks[0].role, BlockRole::Heading1);
    assert_eq!(state.blocks[1].list.as_ref().unwrap().marker, "07.");
    assert_eq!(state.lists.len(), 3);
    assert!(
        state
            .lists
            .iter()
            .all(|l| l.id != base && l.id != child && l.id != unused)
    );
    assert!(state.blocks.iter().all(|b| !ids.contains(&b.id)));
    for list in &state.lists {
        if let Some(base) = &list.definition.as_ref().unwrap().base {
            assert!(state.lists.iter().any(|l| &l.id == base));
        }
    }
}
#[test]
fn read_only_semantic_queries_do_not_enable_the_package_feature() {
    let mut s = session();
    paragraphs(&mut s, &["plain"]);
    let bytes = s.save().unwrap().data.bytes;
    assert!(s.outline().unwrap().data.is_empty());
    assert!(s.state().unwrap().data.lists.is_empty());
    let after = s.save().unwrap().data.bytes;
    let container =
        reprise_format::Container::decode(&after, reprise_format::Limits::default()).unwrap();
    let before =
        reprise_format::Container::decode(&bytes, reprise_format::Limits::default()).unwrap();
    assert_eq!(container.0.header.features, before.0.header.features);
    assert_eq!(
        container.0.header.features.required & reprise_format::features::REQUIRED_BLOCK_SEMANTICS,
        0
    );
}
#[test]
fn outline_reports_each_headings_page_after_a_multi_page_layout() {
    let mut s = session();
    let mut texts = Vec::new();
    for chapter in 0..3 {
        texts.push(format!("Chapter {}", chapter + 1));
        for line in 0..30 {
            texts.push(format!("Body line {line} of chapter {}.", chapter + 1));
        }
    }
    let ids = paragraphs(
        &mut s,
        &texts.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let commands = (0..3)
        .map(|chapter| Command::SetRole {
            node: ids[chapter * 31].clone(),
            role: if chapter == 1 {
                BlockRole::Heading2
            } else {
                BlockRole::Heading1
            },
        })
        .collect();
    apply(&mut s, commands);
    finish(&mut s);
    let outline = s.outline().unwrap().data;
    let steps = s.reading_order().unwrap().data;
    for entry in &outline {
        let first = steps.iter().find(|step| step.node == entry.node).unwrap();
        assert_eq!(entry.page, Some(first.page), "{}", entry.text);
    }
    let pages: Vec<_> = outline.iter().map(|h| h.page.unwrap()).collect();
    assert_eq!(pages[0], 0);
    assert!(pages.windows(2).all(|w| w[0] < w[1]), "{pages:?}");
    insta::assert_json_snapshot!("outline_after_layout", outline);
}
#[test]
fn state_cuts_an_oversized_list_registry_and_items_still_number() {
    let mut s = session();
    let ids = paragraphs(&mut s, &["only item"]);
    let mut commands: Vec<Command> = (0..1025)
        .map(|_| Command::CreateList {
            definition: definition("decimal", "1"),
        })
        .collect();
    commands.push(Command::SetListItem {
        node: ids[0].clone(),
        list: ListRef::New { index: 1024 },
        level: 0,
    });
    apply(&mut s, commands);
    let state = s.state().unwrap().data;
    assert_eq!(state.lists.len(), 1024);
    assert!(
        state
            .diagnostics
            .iter()
            .any(|d| d.code == "bindings.limit" && d.message.contains("list definitions"))
    );
    // The last definition is the one cut from the listing; its item still has
    // a marker, and the registry is not a precondition of reading the state.
    assert_eq!(state.blocks[0].list.as_ref().unwrap().marker, "1.");
}
#[test]
fn roles_without_a_built_in_style_need_it_defined_first() {
    let mut s = session();
    let ids = paragraphs(&mut s, &["quoted words"]);
    let pair = |role, style: &str| Batch {
        commands: vec![
            Command::SetRole {
                node: ids[0].clone(),
                role,
            },
            Command::ApplyStyle {
                node: ids[0].clone(),
                style: Some(style.into()),
            },
        ],
        expected_revision: None,
        selection: None,
    };
    // `Quote` is an application style name, not a built-in: the pair is
    // refused as a whole, and the role is not left half applied.
    let before = s.state().unwrap().data;
    assert!(
        s.batch(&Payload::new(pair(BlockRole::Quote, "Quote")))
            .is_err()
    );
    assert_eq!(s.state().unwrap().data, before);
    // Defined first (the app's own definition), the same pair is one undo step.
    apply(
        &mut s,
        vec![Command::DefineStyle {
            name: "Quote".into(),
            parent: Some("Normal".into()),
            style: Style::default(),
        }],
    );
    s.batch(&Payload::new(pair(BlockRole::Quote, "Quote")))
        .unwrap();
    let block = &s.state().unwrap().data.blocks[0];
    assert_eq!(
        (block.role, block.style.name.as_str()),
        (BlockRole::Quote, "Quote")
    );
    s.undo().unwrap();
    let block = &s.state().unwrap().data.blocks[0];
    assert_eq!(
        (block.role, block.style.name.as_str()),
        (BlockRole::Body, "")
    );
}
