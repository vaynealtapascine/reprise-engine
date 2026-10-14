//! Inline objects through the facade: insertion, updates, batches, stale
//! revisions, undo, copies, field targets and hostile input.
use reprise::*;

const DOC: &str = "00112233445566778899aabbccddeeff";
const COPY: &str = "ffeeddccbbaa99887766554433221100";
const ASSET: &str = "ab5f7c0d3e1a9b2c4d6e8f0a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3e4";
const FFFC: char = '\u{FFFC}';

fn session(peer: &str) -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: peer.into(),
        }))
        .unwrap()
}
fn finish(s: &mut DocumentSession) {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    while !job.step(s, 1000).unwrap().data.complete {}
}
fn tx(s: &mut DocumentSession, commands: Vec<Command>) -> Payload<Applied> {
    s.apply(&Payload::new(Transaction { commands })).unwrap()
}
fn block(text: &str, index: u32) -> Command {
    Command::InsertBlock {
        parent: None,
        index,
        block_kind: BlockKind::Paragraph,
        text: text.into(),
        style: Style::default(),
    }
}
fn batch(s: &mut DocumentSession, commands: Vec<Command>) -> Result<Payload<BatchReport>> {
    s.batch(&Payload::new(Batch {
        commands,
        expected_revision: None,
        selection: None,
    }))
}
fn state(s: &DocumentSession) -> State {
    s.state().unwrap().data
}
fn revision(s: &DocumentSession) -> Vec<Clock> {
    state(s).revision
}
fn texts(s: &DocumentSession) -> Vec<String> {
    state(s).blocks.into_iter().map(|b| b.text).collect()
}
fn field(function: &str) -> InlineObject {
    InlineObject {
        kind: "field".into(),
        params: [("field".to_string(), function.to_string())].into(),
        ..InlineObject::default()
    }
}
fn image(alt: &str) -> InlineObject {
    InlineObject {
        kind: "image".into(),
        params: [
            ("asset".to_string(), ASSET.to_string()),
            ("alt".to_string(), alt.to_string()),
            ("width".to_string(), "pt:20480".to_string()),
        ]
        .into(),
        ..InlineObject::default()
    }
}
fn insert(node: &str, at: u32, object: InlineObject) -> Command {
    Command::InsertInline {
        node: node.into(),
        at,
        object,
    }
}
fn update(node: &str, at: u32, patch: InlinePatch) -> Command {
    Command::UpdateInline {
        node: node.into(),
        at,
        patch,
    }
}
/// A paragraph "see  for" (two spaces) with an object between them at byte 4.
fn session_with_object() -> (DocumentSession, String) {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("see  for", 0)]).data.blocks[0].clone();
    tx(&mut s, vec![insert(&node, 4, image("a cat"))]);
    (s, node)
}

#[test]
fn an_inserted_object_is_listed_with_its_offset_in_the_text() {
    let (s, node) = session_with_object();
    let block = &state(&s).blocks[0];
    assert_eq!(block.id, node);
    assert_eq!(block.text, format!("see {FFFC} for"));
    assert_eq!(block.inline.len(), 1);
    let entry = &block.inline[0];
    assert_eq!(entry.at, 4, "the byte offset of its U+FFFC");
    assert!(entry.id.contains('/'), "host/char identity: {}", entry.id);
    assert_eq!(entry.object.kind, "image");
    assert_eq!(entry.object.align, InlineAlign::Baseline);
    assert_eq!(entry.object.params["alt"], "a cat");
}

#[test]
fn a_paragraph_without_objects_has_no_inline_key_on_the_wire() {
    let mut s = session("1");
    tx(&mut s, vec![block("plain", 0)]);
    let value = serde_json::to_value(&state(&s).blocks[0]).unwrap();
    assert!(value.get("inline").is_none(), "existing listings unchanged");
    let mut s = session("1");
    let node = tx(&mut s, vec![block("a", 0)]).data.blocks[0].clone();
    tx(&mut s, vec![insert(&node, 1, field("page-number"))]);
    let value = serde_json::to_value(&state(&s).blocks[0]).unwrap();
    assert_eq!(value["inline"][0]["object"]["align"], "baseline");
    assert_eq!(value["inline"][0]["at"], 1);
}

#[test]
fn the_command_wire_shape_is_kebab_case_with_the_object_inside() {
    let command = insert("1@1", 4, image("a cat"));
    let value = serde_json::to_value(&command).unwrap();
    assert_eq!(value["kind"], "insert-inline");
    assert_eq!(value["object"]["kind"], "image");
    let patch = update(
        "1@1",
        4,
        InlinePatch {
            align: Some(InlineAlign::TextBottom),
            ..InlinePatch::default()
        },
    );
    let value = serde_json::to_value(&patch).unwrap();
    assert_eq!(value["kind"], "update-inline");
    assert_eq!(value["patch"]["align"], "text-bottom");
    let round: Command = serde_json::from_value(value).unwrap();
    assert_eq!(round, patch);
}

#[test]
fn update_changes_parameters_and_keeps_the_identity() {
    let (mut s, node) = session_with_object();
    let before = state(&s).blocks[0].inline[0].clone();
    let patch = InlinePatch {
        align: Some(InlineAlign::Middle),
        params: [
            ("width".to_string(), Some("em:2000".to_string())),
            ("alt".to_string(), None),
        ]
        .into(),
    };
    tx(&mut s, vec![update(&node, 4, patch)]);
    let after = state(&s).blocks[0].inline[0].clone();
    assert_eq!(after.id, before.id, "an update is not a new object");
    assert_eq!(after.at, 4);
    assert_eq!(after.object.align, InlineAlign::Middle);
    assert_eq!(after.object.params["width"], "em:2000");
    assert!(!after.object.params.contains_key("alt"), "null removes it");
    assert_eq!(
        after.object.params["asset"], ASSET,
        "untouched parameters stay"
    );
}

#[test]
fn a_batch_inserts_and_updates_in_one_undo_step_and_respects_revisions() {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("ab", 0)]).data.blocks[0].clone();
    batch(
        &mut s,
        vec![
            insert(&node, 1, field("page-number")),
            // The second command sees the object the first one put in.
            update(
                &node,
                1,
                InlinePatch {
                    align: Some(InlineAlign::TextTop),
                    ..InlinePatch::default()
                },
            ),
        ],
    )
    .unwrap();
    let entry = state(&s).blocks[0].inline[0].clone();
    assert_eq!(entry.object.align, InlineAlign::TextTop);
    assert!(s.undo().unwrap().data);
    assert_eq!(texts(&s), ["ab"], "one undo undoes the whole batch");
    assert!(state(&s).blocks[0].inline.is_empty());
}

#[test]
fn a_failing_command_in_a_batch_writes_nothing() {
    let (mut s, node) = session_with_object();
    let before = (state(&s), s.sync_info().data);
    let error = batch(
        &mut s,
        vec![
            insert(&node, 0, field("page-number")),
            // Offset 1 holds no object, so the second command is refused.
            update(&node, 1, InlinePatch::default()),
        ],
    )
    .unwrap_err();
    assert_eq!(error.payload().data.command, Some(1));
    assert_eq!(state(&s), before.0);
    assert_eq!(
        s.sync_info().data,
        before.1,
        "not even a tombstone was written"
    );
}

#[test]
fn a_stale_revision_refuses_an_insertion_and_writes_nothing() {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("hello", 0)]).data.blocks[0].clone();
    let stale = revision(&s);
    tx(&mut s, vec![block("second", 1)]);
    let before = state(&s);
    let error = s
        .batch(&Payload::new(Batch {
            commands: vec![insert(&node, 5, field("page-number"))],
            expected_revision: Some(stale),
            selection: None,
        }))
        .unwrap_err();
    assert_eq!(error.code(), "bindings.stale");
    assert_eq!(state(&s), before, "no object, no text change");
    // The current revision is accepted.
    s.batch(&Payload::new(Batch {
        commands: vec![insert(&node, 5, field("page-number"))],
        expected_revision: Some(revision(&s)),
        selection: None,
    }))
    .unwrap();
    assert_eq!(state(&s).blocks[0].inline.len(), 1);
}

#[test]
fn undo_removes_an_insertion_and_redo_restores_its_identity() {
    let (mut s, node) = session_with_object();
    let id = state(&s).blocks[0].inline[0].id.clone();
    tx(
        &mut s,
        vec![update(
            &node,
            4,
            InlinePatch {
                params: [("alt".to_string(), Some("a dog".to_string()))].into(),
                ..InlinePatch::default()
            },
        )],
    );
    assert!(s.undo().unwrap().data, "undo the update");
    assert_eq!(state(&s).blocks[0].inline[0].object.params["alt"], "a cat");
    assert!(s.undo().unwrap().data, "undo the insertion");
    assert!(state(&s).blocks[0].inline.is_empty());
    assert_eq!(texts(&s), ["see  for"], "the object's text is gone");
    assert!(s.redo().unwrap().data);
    assert!(s.redo().unwrap().data);
    let back = state(&s).blocks[0].inline[0].clone();
    assert_eq!(back.id, id, "redo keeps the object's identity");
    assert_eq!(back.object.params["alt"], "a dog");
    assert_eq!(texts(&s), [format!("see {FFFC} for")]);
}

#[test]
fn deleting_the_object_character_removes_it_from_the_listing() {
    let (mut s, node) = session_with_object();
    tx(
        &mut s,
        vec![Command::DeleteText {
            node: node.clone(),
            start: 4,
            end: 7,
        }],
    );
    assert_eq!(texts(&s), ["see  for"]);
    assert!(state(&s).blocks[0].inline.is_empty());
    assert!(s.undo().unwrap().data);
    assert_eq!(state(&s).blocks[0].inline.len(), 1, "undo brings it back");
}

#[test]
fn a_copy_gets_fresh_identities_and_the_same_objects() {
    let (mut s, _) = session_with_object();
    let source = state(&s).blocks[0].inline[0].clone();
    let bytes = s.save().unwrap().data.bytes;
    let copy = Workspace::new()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    let copied = state(&copy).blocks[0].inline[0].clone();
    assert_eq!(copied.at, source.at);
    assert_eq!(copied.object, source.object, "parameters are carried");
    assert_ne!(copied.id, source.id, "the copy's object is new");
    assert_eq!(
        state(&s).blocks[0].inline[0].id,
        source.id,
        "the source is untouched"
    );
}

#[test]
fn a_copy_keeps_field_parameters() {
    let mut s = session("1");
    let ids = tx(&mut s, vec![block("Intro", 0), block("see  for", 1)])
        .data
        .blocks;
    let target = s
        .field_target(&Payload::new(FieldTarget {
            node: ids[0].clone(),
            at: 0,
        }))
        .unwrap()
        .data;
    tx(
        &mut s,
        vec![insert(
            &ids[1],
            4,
            InlineObject {
                kind: "field".into(),
                params: [
                    ("field".to_string(), "page-of".to_string()),
                    ("target".to_string(), target.clone()),
                ]
                .into(),
                ..InlineObject::default()
            },
        )],
    );
    let bytes = s.save().unwrap().data.bytes;
    let copy = Workspace::new()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    let copied = &state(&copy).blocks[1].inline[0].object;
    assert_eq!(copied.params["field"], "page-of");
    assert!(
        copied.params.contains_key("target"),
        "the target string is carried"
    );
    assert!(target.starts_with(&format!("{} ", ids[0])));
}

#[test]
fn field_target_names_a_position_and_refuses_bad_ones() {
    let mut s = session("1");
    let ids = tx(&mut s, vec![block("héllo", 0)]).data.blocks;
    let ok = s
        .field_target(&Payload::new(FieldTarget {
            node: ids[0].clone(),
            at: 3,
        }))
        .unwrap()
        .data;
    assert!(ok.starts_with(&format!("{} ", ids[0])), "{ok}");
    let inside_a_character = s.field_target(&Payload::new(FieldTarget {
        node: ids[0].clone(),
        at: 2,
    }));
    assert!(inside_a_character.is_err(), "é is two bytes");
    let past_the_end = s.field_target(&Payload::new(FieldTarget {
        node: ids[0].clone(),
        at: 99,
    }));
    assert!(past_the_end.is_err());
    let unknown = s.field_target(&Payload::new(FieldTarget {
        node: "not a node".into(),
        at: 0,
    }));
    assert!(unknown.is_err());
}

#[test]
fn a_page_of_field_lays_out_and_settles_on_a_long_document() {
    let mut s = session("1");
    s.declare_font(
        &Payload::new(FontDeclaration {
            family: "Source Serif Pro".into(),
            weight: 400,
            style: FontStyle::Normal,
            stretch: 1000,
            face_index: 0,
        }),
        include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf"),
    )
    .unwrap();
    let mut blocks = vec![block("Intro", 0)];
    for i in 1..=120 {
        blocks.push(block(&format!("paragraph {i}"), i));
    }
    let ids = tx(&mut s, blocks).data.blocks;
    let target = s
        .field_target(&Payload::new(FieldTarget {
            node: ids[0].clone(),
            at: 0,
        }))
        .unwrap()
        .data;
    let last = ids.last().unwrap().clone();
    tx(
        &mut s,
        vec![insert(
            &last,
            0,
            InlineObject {
                kind: "field".into(),
                params: [
                    ("field".to_string(), "page-of".to_string()),
                    ("target".to_string(), target),
                ]
                .into(),
                ..InlineObject::default()
            },
        )],
    );
    finish(&mut s);
    // Layout reads the target and settles: no feedback or orphan diagnostics.
    // The resolved page number is not asserted here: field text is measured
    // but not painted on this branch yet (see the report on `docs/inline.md`).
    let pages = (0..).map_while(|page| s.display_page(page).ok()).count();
    assert!(pages > 1, "the document runs to a second page");
    let codes: Vec<_> = s.diagnostics().data.into_iter().map(|d| d.code).collect();
    assert!(
        !codes.iter().any(|c| c == "layout.field-unsettled"),
        "{codes:?}"
    );
    let last = state(&s).blocks.last().unwrap().inline.clone();
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].object.params["field"], "page-of");
    assert!(last[0].object.params["target"].starts_with(&format!("{} ", ids[0])));
}

#[test]
fn hostile_inserts_are_refused_without_writing() {
    let (mut s, node) = session_with_object();
    let before = (state(&s), s.sync_info().data);
    let refused = [
        insert(&node, 0, InlineObject::default()),
        insert(
            &node,
            0,
            InlineObject {
                kind: "Bad Kind".into(),
                ..InlineObject::default()
            },
        ),
        insert(
            &node,
            0,
            InlineObject {
                kind: "image".into(),
                params: [("asset".to_string(), "nothex".to_string())].into(),
                ..InlineObject::default()
            },
        ),
        insert(
            &node,
            0,
            InlineObject {
                kind: "placeholder".into(),
                params: [("text".to_string(), "x".repeat(16 * 1024 + 1))].into(),
                ..InlineObject::default()
            },
        ),
        insert(&node, 99, field("page-number")),
        insert("not a node", 0, field("page-number")),
    ];
    for command in refused {
        assert!(
            tx_result(&mut s, vec![command]).is_err(),
            "every one of these is refused"
        );
        assert_eq!(state(&s), before.0);
        assert_eq!(s.sync_info().data, before.1);
    }
}

fn tx_result(s: &mut DocumentSession, commands: Vec<Command>) -> Result<Payload<Applied>> {
    s.apply(&Payload::new(Transaction { commands }))
}

#[test]
fn an_unknown_kind_is_kept_verbatim() {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("a", 0)]).data.blocks[0].clone();
    tx(
        &mut s,
        vec![insert(
            &node,
            1,
            InlineObject {
                kind: "equation".into(),
                params: [("tex".to_string(), "x^2".to_string())].into(),
                ..InlineObject::default()
            },
        )],
    );
    let entry = &state(&s).blocks[0].inline[0];
    assert_eq!(entry.object.kind, "equation");
    assert_eq!(entry.object.params["tex"], "x^2");
}

#[test]
fn a_transaction_with_oversized_inline_text_is_refused_up_front() {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("a", 0)]).data.blocks[0].clone();
    let huge = InlineObject {
        kind: "field".into(),
        params: [("text".to_string(), "x".repeat(17 << 20))].into(),
        ..InlineObject::default()
    };
    let error = tx_result(&mut s, vec![insert(&node, 1, huge)]).unwrap_err();
    assert_eq!(error.code(), "bindings.limit");
    assert!(state(&s).blocks[0].inline.is_empty());
}

#[test]
fn too_many_commands_are_refused_by_the_facade_limit() {
    let (mut s, node) = session_with_object();
    let commands = (0..reprise_edit::MAX_COMMANDS + 1)
        .map(|_| update(&node, 4, InlinePatch::default()))
        .collect();
    let error = batch(&mut s, commands).unwrap_err();
    assert_eq!(error.code(), "bindings.limit");
}

#[test]
fn many_objects_in_one_paragraph_keep_their_order() {
    // One command per object is quadratic in the objects of one paragraph (each
    // staging commit makes the host's flow be read again): about 4 s at 200 and
    // 8 minutes at 2000 in a debug build. A paste or an HTML import of many
    // objects goes through `insert_with_inline` instead and is linear
    // (`doc`'s fifty-thousand-object test).
    const COUNT: u32 = 200;
    let mut s = session("1");
    let node = tx(&mut s, vec![block("", 0)]).data.blocks[0].clone();
    let commands = (0..COUNT)
        .map(|i| insert(&node, i * 3, field("page-number")))
        .collect();
    batch(&mut s, commands).unwrap();
    let block = &state(&s).blocks[0];
    assert_eq!(block.inline.len(), COUNT as usize);
    assert!(
        block
            .inline
            .iter()
            .enumerate()
            .all(|(i, e)| e.at == i as u32 * 3),
        "in text order"
    );
    assert_eq!(block.text.matches(FFFC).count(), COUNT as usize);
}

#[test]
fn the_declarations_describe_the_new_types() {
    let text = reprise::typescript::declarations();
    for name in [
        "InlineAlign",
        "InlineObject",
        "InlinePatch",
        "InlineEntry",
        "FieldTarget",
    ] {
        assert!(
            text.contains(&format!("export type {name} =")),
            "{name} is declared"
        );
    }
    assert!(text.contains("\"text-top\""), "kebab-case alignment");
    assert!(text.contains("inline?:"), "an empty listing is optional");
}

fn selection(node: &str, from: u32, to: u32) -> Selection {
    let caret = |offset| Caret {
        node: node.into(),
        offset,
        affinity: Affinity::Downstream,
    };
    Selection {
        anchor: caret(from),
        focus: caret(to),
    }
}

#[test]
fn layout_problems_with_an_object_show_in_diagnostics_for_with_their_bytes() {
    let mut s = session("1");
    let node = tx(&mut s, vec![block("see  for", 0)]).data.blocks[0].clone();
    let equation = InlineObject {
        kind: "equation".into(),
        ..InlineObject::default()
    };
    // `image` names an asset the session does not hold.
    tx(
        &mut s,
        vec![
            insert(&node, 4, image("missing")),
            insert(&node, 4, equation),
        ],
    );
    finish(&mut s);
    let found = s
        .diagnostics_for(&Payload::new(DiagnosticFilter {
            code_prefix: Some("layout.".into()),
            min_severity: Some(Severity::Warning),
            subject: Some(node.clone()),
            limit: None,
        }))
        .unwrap()
        .data;
    let codes: Vec<_> = found.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"layout.inline-kind"), "{codes:?}");
    assert!(codes.contains(&"layout.image-missing"), "{codes:?}");
    for d in &found.diagnostics {
        // Each names the object's three bytes.
        assert_eq!(d.end, d.start.map(|s| s + 3), "{d:?}");
    }
    // And a document without objects reports none of them.
    let mut plain = session("1");
    tx(&mut plain, vec![block("plain", 0)]);
    finish(&mut plain);
    assert!(
        plain
            .diagnostics()
            .data
            .iter()
            .all(|d| !d.code.starts_with("layout.inline") && d.code != "layout.image-missing")
    );
}

#[test]
fn reading_order_ranges_include_the_object_bytes() {
    let mut s = session("1");
    let ids = tx(&mut s, vec![block("one  two", 0), block("three", 1)])
        .data
        .blocks;
    tx(
        &mut s,
        vec![
            insert(&ids[0], 4, field("page-number")),
            insert(&ids[0], 4, image("pic")),
        ],
    );
    finish(&mut s);
    let steps = s.reading_order().unwrap().data;
    let first: Vec<_> = steps.iter().filter(|st| st.node == ids[0]).collect();
    let text = &state(&s).blocks[0].text;
    assert_eq!(text.len(), "one  two".len() + 6);
    // The lines of the paragraph tile its whole text, objects included, and
    // every object's three bytes are inside one step.
    assert_eq!(first.first().unwrap().start, 0);
    assert_eq!(first.last().unwrap().end as usize, text.len());
    for pair in first.windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
    }
    for entry in &state(&s).blocks[0].inline {
        assert!(
            first
                .iter()
                .any(|st| st.start <= entry.at && entry.at + 3 <= st.end),
            "{entry:?} is inside a step"
        );
    }
}

#[test]
fn an_objects_rectangle_is_exposed_for_selection_handles() {
    let (mut s, node) = session_with_object();
    finish(&mut s);
    let rect = s
        .atom_rect(&Payload::new(FieldTarget {
            node: node.clone(),
            at: 4,
        }))
        .unwrap()
        .data;
    assert_eq!(rect.page, 0);
    assert!(rect.rect.width > 0 && rect.rect.height > 0, "{rect:?}");
    // Offsets that are not the start of an object are refused, not guessed.
    for at in [0, 3, 5, 6, 4000] {
        assert!(
            s.atom_rect(&Payload::new(FieldTarget {
                node: node.clone(),
                at,
            }))
            .is_err(),
            "offset {at}"
        );
    }
}

#[test]
fn copy_and_paste_carry_an_object_with_a_new_identity() {
    let (mut s, node) = session_with_object();
    finish(&mut s);
    let source = state(&s).blocks[0].inline[0].clone();
    // Copy "e  f" (around the object) and paste it at the end of a new line.
    let bytes = s
        .copy(&Payload::new(selection(&node, 2, 10)))
        .unwrap()
        .data
        .bytes;
    let second = tx(&mut s, vec![block("end:", 1)]).data.blocks[0].clone();
    finish(&mut s);
    s.paste(
        &Payload::new(Paste {
            at: Some(Caret {
                node: second.clone(),
                offset: 4,
                affinity: Affinity::Downstream,
            }),
        }),
        &bytes,
    )
    .unwrap();
    let blocks = state(&s).blocks;
    assert_eq!(blocks[0].inline.len(), 1, "the source keeps its object");
    assert_eq!(blocks[0].inline[0], source);
    let pasted = &blocks[1].inline;
    assert_eq!(pasted.len(), 1);
    assert_eq!(pasted[0].object, source.object);
    assert_ne!(pasted[0].id, source.id);
    assert_eq!(pasted[0].at, 4 + 2, "after \"end:\" and two letters");
    // One undo takes the paste back whole.
    assert!(s.undo().unwrap().data);
    assert!(state(&s).blocks[1].inline.is_empty());
    assert_eq!(state(&s).blocks[0].inline.len(), 1);
}

#[test]
fn html_copy_of_a_selection_with_an_object_names_the_object() {
    let (mut s, node) = session_with_object();
    finish(&mut s);
    let html = s
        .copy_as(&Payload::new(CopyAs {
            selection: selection(&node, 0, 11),
            format: CopyFormat::Html,
        }))
        .unwrap()
        .data;
    let text = String::from_utf8(html.content.bytes).unwrap();
    assert!(text.contains("<img "), "{text}");
    assert!(text.contains("alt=\"a cat\""), "{text}");
    let plain = s
        .copy_as(&Payload::new(CopyAs {
            selection: selection(&node, 0, 11),
            format: CopyFormat::PlainText,
        }))
        .unwrap()
        .data;
    assert_eq!(
        String::from_utf8(plain.content.bytes).unwrap(),
        "see a cat for"
    );
}
