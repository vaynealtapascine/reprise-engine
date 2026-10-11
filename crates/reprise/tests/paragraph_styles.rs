//! Named paragraph styles, per-property clearing and resolved styles through
//! the facade (`docs/paragraph-styles.md`).
use reprise::*;

fn session(peer: &str) -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: peer.into(),
        }))
        .unwrap()
}
fn apply(s: &mut DocumentSession, commands: Vec<Command>) -> Applied {
    s.apply(&Payload::new(Transaction { commands }))
        .unwrap()
        .data
}
fn refuse(s: &mut DocumentSession, commands: Vec<Command>) {
    assert!(s.apply(&Payload::new(Transaction { commands })).is_err());
}
fn sync(a: &mut DocumentSession, b: &mut DocumentSession) {
    let from_a = a
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    let from_b = b
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    a.sync_import(&from_b).unwrap();
    b.sync_import(&from_a).unwrap();
}
fn paragraph(s: &mut DocumentSession, text: &str) -> String {
    let index = s.state().unwrap().data.blocks.len() as u32;
    apply(
        s,
        vec![Command::InsertBlock {
            parent: None,
            index,
            block_kind: BlockKind::Paragraph,
            text: text.into(),
            style: Style::default(),
        }],
    )
    .blocks[0]
        .clone()
}
fn resolved(s: &DocumentSession, node: &str) -> (BlockStyle, ResolvedStyle) {
    let state = s.state().unwrap().data;
    let block = state.blocks.iter().find(|b| b.id == node).unwrap().clone();
    let r = state.resolved[block.style.resolved as usize].clone();
    (block.style, r)
}
const PT: i32 = 1024;

#[test]
fn style_lifecycle_state_and_undo() {
    let mut s = session("1");
    let a = paragraph(&mut s, "first");
    let b = paragraph(&mut s, "second");
    let state = s.state().unwrap().data;
    let names: Vec<_> = state.styles.iter().map(|n| n.name.as_str()).collect();
    for builtin in ["", "Normal", "Title", "Heading 1", "Heading 6"] {
        assert!(names.contains(&builtin), "{names:?}");
    }
    assert!(
        state
            .styles
            .iter()
            .find(|n| n.name == "Heading 1")
            .unwrap()
            .builtin
    );
    // Untouched blocks share one resolved entry.
    assert_eq!(state.resolved.len(), 1);
    assert_eq!(state.blocks[0].style.name, "");
    assert_eq!(state.resolved[0].size, 10 * PT);
    assert_eq!(state.resolved[0].family, "serif");

    apply(
        &mut s,
        vec![
            Command::DefineStyle {
                name: "Quote".into(),
                parent: Some("".into()),
                style: Style {
                    indent_start: Some("24pt".into()),
                    space_before: Some("1lh".into()),
                    keep_together: Some(true),
                    alignment: Some(Alignment::Justify),
                    ..Style::default()
                },
            },
            Command::ApplyStyle {
                node: a.clone(),
                style: Some("Quote".into()),
            },
            Command::ApplyStyle {
                node: b.clone(),
                style: Some("Heading 2".into()),
            },
        ],
    );
    let (style, r) = resolved(&s, &a);
    assert_eq!(style.name, "Quote");
    assert_eq!(r.indent_start, 24 * PT);
    assert_eq!(r.space_before, r.line_height);
    assert!(r.keep_together);
    assert_eq!(r.alignment, Some(Alignment::Justify));
    let (_, h) = resolved(&s, &b);
    assert_eq!(h.size, 16 * PT);
    assert_eq!(h.weight, Some(700));
    assert!(h.keep_with_next);
    // Update one property of a built-in and clear another of Quote.
    apply(
        &mut s,
        vec![
            Command::UpdateStyle {
                name: "Heading 2".into(),
                parent: None,
                style: Style {
                    color: Some([0, 0, 255, 255]),
                    ..Style::default()
                },
                clear: Vec::new(),
            },
            Command::UpdateStyle {
                name: "Quote".into(),
                parent: None,
                style: Style::default(),
                clear: vec![StyleProperty::IndentStart],
            },
        ],
    );
    let (_, h) = resolved(&s, &b);
    assert_eq!((h.size, h.color), (16 * PT, Some([0, 0, 255, 255])));
    assert_eq!(resolved(&s, &a).1.indent_start, 0);
    // Rename forwards; delete falls back to the parent.
    apply(
        &mut s,
        vec![Command::RenameStyle {
            from: "Quote".into(),
            to: "Block quote".into(),
        }],
    );
    let (style, r) = resolved(&s, &a);
    assert_eq!(style.name, "Block quote");
    assert!(r.keep_together);
    let names: Vec<_> = s
        .state()
        .unwrap()
        .data
        .styles
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert!(names.contains(&"Block quote".into()) && !names.contains(&"Quote".into()));
    apply(
        &mut s,
        vec![Command::DeleteStyle {
            name: "Block quote".into(),
        }],
    );
    assert!(!resolved(&s, &a).1.keep_together);
    // Each step undoes; the definition comes back exactly.
    for _ in 0..3 {
        assert!(s.undo().unwrap().data);
    }
    let (style, r) = resolved(&s, &a);
    assert_eq!(style.name, "Quote");
    assert_eq!(r.indent_start, 24 * PT);
    // Save and reopen keep everything.
    let bytes = s.save().unwrap().data.bytes;
    let opened = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "9".into(),
            }),
            &bytes,
        )
        .unwrap();
    assert_eq!(
        opened.state().unwrap().data.styles,
        s.state().unwrap().data.styles
    );
    assert_eq!(
        opened.state().unwrap().data.resolved,
        s.state().unwrap().data.resolved
    );
}

#[test]
fn invalid_style_commands_are_refused_atomically() {
    let mut s = session("1");
    let a = paragraph(&mut s, "x");
    let before = s.state().unwrap().data;
    for commands in [
        vec![Command::DeleteStyle { name: "".into() }],
        vec![Command::RenameStyle {
            from: "".into(),
            to: "Base".into(),
        }],
        vec![Command::DeleteStyle {
            name: "Nowhere".into(),
        }],
        vec![Command::ApplyStyle {
            node: a.clone(),
            style: Some("Nowhere".into()),
        }],
        vec![Command::RenameStyle {
            from: "Title".into(),
            to: "Normal".into(),
        }],
        vec![Command::DefineStyle {
            name: "x".repeat(300),
            parent: None,
            style: Style::default(),
        }],
        vec![Command::DefineStyle {
            name: "Loop".into(),
            parent: Some("Loop".into()),
            style: Style::default(),
        }],
        vec![Command::DefineStyle {
            name: "Bad".into(),
            parent: None,
            style: Style {
                widows: Some(0),
                ..Style::default()
            },
        }],
        // A valid command followed by an invalid one changes nothing.
        vec![
            Command::DefineStyle {
                name: "Fine".into(),
                parent: None,
                style: Style::default(),
            },
            Command::UpdateStyle {
                name: "Fine".into(),
                parent: None,
                style: Style::default(),
                clear: vec![StyleProperty::Language],
            },
        ],
        vec![Command::FormatText {
            node: a.clone(),
            start: 0,
            end: 1,
            style: TextStyle {
                clear: vec![StyleProperty::IndentStart],
                ..TextStyle::default()
            },
        }],
        vec![Command::SetStyle {
            node: a.clone(),
            style: Style {
                weight: Some(700),
                ..Style::default()
            },
            clear: Some(vec![StyleProperty::Weight]),
        }],
    ] {
        refuse(&mut s, commands);
    }
    let after = s.state().unwrap().data;
    assert_eq!(after.styles, before.styles);
    assert_eq!(after.blocks, before.blocks);
    assert!(!after.can_undo || before.can_undo);
}

#[test]
fn set_style_patches_with_clear_and_replaces_without() {
    let mut s = session("1");
    let a = paragraph(&mut s, "x");
    let set = |clear: Option<Vec<StyleProperty>>, style: Style| Command::SetStyle {
        node: a.clone(),
        style,
        clear,
    };
    apply(
        &mut s,
        vec![set(
            None,
            Style {
                weight: Some(700),
                indent_first: Some("2em".into()),
                ..Style::default()
            },
        )],
    );
    apply(
        &mut s,
        vec![set(
            Some(vec![StyleProperty::Weight]),
            Style {
                space_after: Some("6pt".into()),
                ..Style::default()
            },
        )],
    );
    let (style, r) = resolved(&s, &a);
    assert_eq!(style.direct.weight, None);
    assert_eq!(style.direct.indent_first.as_deref(), Some("2em"));
    assert_eq!(r.indent_first, 20 * PT);
    assert_eq!(r.space_after, 6 * PT);
    // Replacement drops every patch.
    apply(&mut s, vec![set(None, Style::default())]);
    let (style, r) = resolved(&s, &a);
    assert_eq!(style.direct, Style::default());
    assert_eq!((r.indent_first, r.space_after), (0, 0));
}

#[test]
fn format_text_clear_reveals_the_paragraph_and_alignment_null_follows_it() {
    let mut s = session("1");
    let a = paragraph(&mut s, "colour me");
    apply(
        &mut s,
        vec![
            Command::FormatText {
                node: a.clone(),
                start: 0,
                end: 9,
                style: TextStyle {
                    color: Some([255, 0, 0, 255]),
                    weight: Some(700),
                    ..TextStyle::default()
                },
            },
            Command::FormatText {
                node: a.clone(),
                start: 0,
                end: 6,
                style: TextStyle {
                    clear: vec![StyleProperty::Color],
                    ..TextStyle::default()
                },
            },
        ],
    );
    let state = s.state().unwrap().data;
    let runs = state.blocks[0].formatting.clone().unwrap();
    assert_eq!(runs[0].style.color, None, "cleared: the paragraph's colour");
    assert_eq!(runs[0].style.weight, Some(700), "other fields kept");
    assert_eq!(runs.last().unwrap().style.color, Some([255, 0, 0, 255]));
    // Paragraph alignment, then a line default, each cleared with null.
    apply(
        &mut s,
        vec![
            Command::SetAlignment {
                node: a.clone(),
                at: None,
                alignment: Some(Alignment::End),
            },
            Command::SetAlignment {
                node: a.clone(),
                at: Some(2),
                alignment: Some(Alignment::Centre),
            },
        ],
    );
    assert_eq!(resolved(&s, &a).1.alignment, Some(Alignment::End));
    apply(
        &mut s,
        vec![
            Command::SetAlignment {
                node: a.clone(),
                at: None,
                alignment: None,
            },
            Command::SetAlignment {
                node: a.clone(),
                at: Some(4),
                alignment: None,
            },
        ],
    );
    assert_eq!(resolved(&s, &a).1.alignment, None);
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    while !job.step(&mut s, 1000).unwrap().data.complete {}
    let marks = s.marks(0).unwrap().data.marks;
    assert!(
        !marks.iter().any(|m| m.kind == MarkKind::Alignment),
        "the line default is gone: {marks:?}"
    );
}

#[test]
fn two_peers_edit_one_style_and_delete_while_applying() {
    let mut a = session("1");
    let mut b = session("2");
    let node = paragraph(&mut a, "shared");
    apply(
        &mut a,
        vec![Command::DefineStyle {
            name: "Card".into(),
            parent: Some("".into()),
            style: Style::default(),
        }],
    );
    sync(&mut a, &mut b);
    apply(
        &mut a,
        vec![Command::UpdateStyle {
            name: "Card".into(),
            parent: None,
            style: Style {
                indent_start: Some("12pt".into()),
                ..Style::default()
            },
            clear: Vec::new(),
        }],
    );
    apply(
        &mut b,
        vec![Command::UpdateStyle {
            name: "Card".into(),
            parent: None,
            style: Style {
                background: Some([240, 240, 240, 255]),
                ..Style::default()
            },
            clear: Vec::new(),
        }],
    );
    sync(&mut a, &mut b);
    let card = |s: &DocumentSession| {
        s.state()
            .unwrap()
            .data
            .styles
            .into_iter()
            .find(|n| n.name == "Card")
            .unwrap()
    };
    assert_eq!(card(&a), card(&b));
    assert_eq!(card(&a).style.indent_start.as_deref(), Some("12pt"));
    assert_eq!(card(&a).style.background, Some([240, 240, 240, 255]));
    // a deletes Card while b applies it: both fall back to the base.
    apply(
        &mut a,
        vec![Command::DeleteStyle {
            name: "Card".into(),
        }],
    );
    apply(
        &mut b,
        vec![Command::ApplyStyle {
            node: node.clone(),
            style: Some("Card".into()),
        }],
    );
    sync(&mut a, &mut b);
    for s in [&a, &b] {
        let (style, r) = resolved(s, &node);
        assert_eq!(style.name, "Card");
        assert_eq!(r.indent_start, 0);
        assert_eq!(r.size, 10 * PT);
    }
    assert_eq!(
        a.state().unwrap().data.resolved,
        b.state().unwrap().data.resolved
    );
    // Undo after remote edits: b's apply undoes, a's delete stays.
    assert!(b.undo().unwrap().data);
    assert_eq!(resolved(&b, &node).0.name, "");
    sync(&mut a, &mut b);
    assert!(
        !a.state()
            .unwrap()
            .data
            .styles
            .iter()
            .any(|n| n.name == "Card")
    );
    assert!(a.undo().unwrap().data);
    sync(&mut a, &mut b);
    assert!(
        b.state()
            .unwrap()
            .data
            .styles
            .iter()
            .any(|n| n.name == "Card")
    );
}
