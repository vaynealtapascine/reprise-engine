//! Role and list editing through the kernel: Enter, Backspace and Tab on
//! items, split and join across roles, continue/restart, and undo.

use reprise_doc::{BlockKind, Document, ListDefinition, NodeId, Role, SchemaRegistry};
use reprise_edit::{Command, Editor, ListRef, Reason, Transaction};

fn editor(texts: &[&str]) -> (Editor, Vec<NodeId>) {
    let doc = Document::new(1).unwrap();
    let ids = texts
        .iter()
        .map(|t| doc.append_block(BlockKind::Paragraph, "body", t).unwrap())
        .collect();
    doc.commit();
    (Editor::new(doc, SchemaRegistry::builtin()), ids)
}

fn markers(ed: &Editor) -> Vec<String> {
    let doc = ed.document();
    doc.document_order()
        .into_iter()
        .map(|n| {
            let text = doc.block(n).unwrap().text.to_string();
            match doc.list_item(n) {
                Some(i) => format!("{}{} {text}", "  ".repeat(i.level.into()), i.marker),
                None => text,
            }
        })
        .collect()
}

fn listify(ed: &mut Editor, ids: &[NodeId]) {
    let mut tx = Transaction::new().with(Command::CreateList {
        definition: ListDefinition::numbered(),
    });
    for &node in ids {
        tx.push(Command::SetListItem {
            node,
            list: ListRef::New(0),
            level: 0,
        });
    }
    let applied = ed.apply(&tx).unwrap();
    assert_eq!(applied.lists.len(), 1);
}

#[test]
fn create_and_fill_a_list_in_one_undoable_step() {
    let (mut ed, ids) = editor(&["a", "b", "c"]);
    listify(&mut ed, &ids);
    assert_eq!(markers(&ed), ["1. a", "2. b", "3. c"]);
    let list = ed.document().list_item(ids[0]).unwrap().list;
    assert!(ed.undo().unwrap());
    assert_eq!(markers(&ed), ["a", "b", "c"]);
    assert!(ed.redo().unwrap());
    assert_eq!(ed.document().list_item(ids[0]).unwrap().list, list);
}

#[test]
fn enter_splits_items_and_leaves_or_outdents_empty_ones() {
    let (mut ed, ids) = editor(&["first"]);
    listify(&mut ed, &ids);
    // Enter at the end: a new, empty item.
    let applied = ed
        .apply_command(Command::Enter {
            node: ids[0],
            at: 5,
        })
        .unwrap();
    let new = applied.blocks[0];
    assert_eq!(markers(&ed), ["1. first", "2. "]);
    // Tab on it, then Enter on the empty nested item outdents.
    ed.apply_command(Command::IndentListItem { node: new, by: 1 })
        .unwrap();
    assert_eq!(markers(&ed), ["1. first", "  a. "]);
    ed.apply_command(Command::Enter { node: new, at: 0 })
        .unwrap();
    assert_eq!(markers(&ed), ["1. first", "2. "]);
    // Enter on an empty level-0 item leaves the list.
    ed.apply_command(Command::Enter { node: new, at: 0 })
        .unwrap();
    assert_eq!(markers(&ed), ["1. first", ""]);
    assert_eq!(ed.document().blocks().len(), 2, "no block was added");
    // Undo walks back one key at a time.
    assert!(ed.undo().unwrap());
    assert_eq!(markers(&ed), ["1. first", "2. "]);
    assert!(ed.undo().unwrap());
    assert_eq!(markers(&ed), ["1. first", "  a. "]);
}

#[test]
fn shift_tab_and_tab_clamp_and_ignore_plain_paragraphs() {
    let (mut ed, ids) = editor(&["a", "plain"]);
    listify(&mut ed, &ids[..1]);
    ed.apply_command(Command::IndentListItem {
        node: ids[0],
        by: -1,
    })
    .unwrap();
    assert_eq!(markers(&ed), ["1. a", "plain"]);
    for _ in 0..12 {
        ed.apply_command(Command::IndentListItem {
            node: ids[0],
            by: 1,
        })
        .unwrap();
    }
    assert_eq!(ed.document().list_item(ids[0]).unwrap().level, 8);
    let before = ed.undo_count();
    let applied = ed
        .apply_command(Command::IndentListItem {
            node: ids[1],
            by: 1,
        })
        .unwrap();
    assert!(applied.effects.is_empty());
    assert_eq!(ed.undo_count(), before, "a no-op is not an undo step");
}

#[test]
fn backspace_removes_the_marker_then_joins() {
    let (mut ed, ids) = editor(&["one", "two"]);
    listify(&mut ed, &ids);
    ed.apply_command(Command::BackspaceAtStart { node: ids[1] })
        .unwrap();
    assert_eq!(markers(&ed), ["1. one", "two"]);
    ed.apply_command(Command::BackspaceAtStart { node: ids[1] })
        .unwrap();
    assert_eq!(markers(&ed), ["1. onetwo"]);
    // At the very first block there is nothing to join.
    ed.apply_command(Command::RemoveListItem { node: ids[0] })
        .unwrap();
    let applied = ed
        .apply_command(Command::BackspaceAtStart { node: ids[0] })
        .unwrap();
    assert!(applied.effects.is_empty());
    assert_eq!(markers(&ed), ["onetwo"]);
}

#[test]
fn split_and_join_across_roles() {
    let (mut ed, ids) = editor(&["Heading body"]);
    ed.apply_command(Command::SetRole {
        node: ids[0],
        role: Role::Heading(1),
    })
    .unwrap();
    let applied = ed
        .apply_command(Command::SplitBlock {
            node: ids[0],
            at: 7,
        })
        .unwrap();
    let tail = applied.blocks[0];
    let doc = ed.document();
    assert_eq!(
        doc.role(tail).unwrap(),
        Role::Heading(1),
        "Enter keeps the role"
    );
    // The app's "Enter at the end of a heading" policy, in one step.
    ed.apply(&Transaction::new().with(Command::SetRole {
        node: tail,
        role: Role::Body,
    }))
    .unwrap();
    ed.apply_command(Command::JoinBlocks {
        first: ids[0],
        second: tail,
    })
    .unwrap();
    assert_eq!(ed.document().role(ids[0]).unwrap(), Role::Heading(1));
    assert!(ed.undo().unwrap());
    assert_eq!(ed.document().role(tail).unwrap(), Role::Body);
    assert!(
        ed.apply_command(Command::SetRole {
            node: ids[0],
            role: Role::Heading(9)
        })
        .is_err_and(|e| e.reason == Reason::BadRole)
    );
}

#[test]
fn restart_and_continue() {
    let (mut ed, ids) = editor(&["a", "b", "body", "c", "d"]);
    listify(&mut ed, &[ids[0], ids[1]]);
    listify(&mut ed, &[ids[3], ids[4]]);
    assert_eq!(markers(&ed), ["1. a", "2. b", "body", "1. c", "2. d"]);
    ed.apply_command(Command::ContinueList { node: ids[3] })
        .unwrap();
    assert_eq!(markers(&ed), ["1. a", "2. b", "body", "3. c", "4. d"]);
    ed.apply_command(Command::RestartList {
        node: ids[3],
        value: Some(10),
    })
    .unwrap();
    assert_eq!(markers(&ed), ["1. a", "2. b", "body", "10. c", "11. d"]);
    ed.apply_command(Command::ContinueList { node: ids[3] })
        .unwrap();
    assert_eq!(markers(&ed), ["1. a", "2. b", "body", "3. c", "4. d"]);
    assert!(
        ed.apply_command(Command::RestartList {
            node: ids[2],
            value: None
        })
        .is_err_and(|e| e.reason == Reason::NotAListItem(ids[2]))
    );
}

#[test]
fn refusals_leave_the_document_untouched() {
    let (mut ed, ids) = editor(&["a"]);
    listify(&mut ed, &ids);
    let list = ed.document().list_item(ids[0]).unwrap().list;
    let before = markers(&ed);
    for (tx, reason) in [
        (
            Transaction::new()
                .with(Command::DeleteList {
                    list: ListRef::Existing(list),
                })
                .with(Command::SetListItem {
                    node: ids[0],
                    list: ListRef::Existing(list),
                    level: 0,
                }),
            Reason::NoSuchList(ListRef::Existing(list)),
        ),
        (
            Transaction::new().with(Command::SetListItem {
                node: ids[0],
                list: ListRef::New(0),
                level: 0,
            }),
            Reason::NoSuchList(ListRef::New(0)),
        ),
        (
            Transaction::new().with(Command::SetListLevel {
                node: ids[0],
                level: 9,
            }),
            Reason::BadLevel(9),
        ),
    ] {
        let err = ed.apply(&tx).unwrap_err();
        assert_eq!(err.reason, reason);
        assert_eq!(markers(&ed), before);
    }
    let bad = ListDefinition {
        base: None,
        levels: [(
            0,
            reprise_doc::LevelDef {
                text: Some("x".repeat(100)),
                ..Default::default()
            },
        )]
        .into(),
    };
    assert!(matches!(
        ed.apply_command(Command::CreateList { definition: bad })
            .unwrap_err()
            .reason,
        Reason::InvalidList(_)
    ));
}

#[test]
fn deleting_a_list_and_undoing_it() {
    let (mut ed, ids) = editor(&["a", "b"]);
    listify(&mut ed, &ids);
    let list = ed.document().list_item(ids[0]).unwrap().list;
    ed.apply_command(Command::DeleteList {
        list: ListRef::Existing(list),
    })
    .unwrap();
    assert_eq!(markers(&ed), ["a", "b"]);
    assert!(ed.undo().unwrap());
    assert_eq!(markers(&ed), ["1. a", "2. b"]);
}

#[test]
fn two_peers_indent_different_items() {
    let (mut a, ids) = editor(&["x", "y", "z"]);
    listify(&mut a, &ids);
    let mut b = Editor::new(a.document().fork(2).unwrap(), SchemaRegistry::builtin());
    a.apply_command(Command::IndentListItem {
        node: ids[1],
        by: 1,
    })
    .unwrap();
    b.apply_command(Command::IndentListItem {
        node: ids[2],
        by: 1,
    })
    .unwrap();
    let (da, db) = (a.document().fork(3).unwrap(), b.document().fork(4).unwrap());
    a.merge(&db).unwrap();
    b.merge(&da).unwrap();
    assert_eq!(markers(&a), ["1. x", "  a. y", "  b. z"]);
    assert_eq!(markers(&a), markers(&b));
    // Each peer's undo removes only its own indent.
    assert!(a.undo().unwrap());
    assert_eq!(markers(&a), ["1. x", "2. y", "  a. z"]);
}
