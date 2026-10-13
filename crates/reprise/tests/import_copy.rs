//! Import as a copy: a new identity and a fresh history that never syncs back.
use reprise::*;

const SOURCE: &str = "00112233445566778899aabbccddeeff";
const COPY: &str = "ffeeddccbbaa99887766554433221100";

fn unmarked_package(id: u8, peer: u64, text: &str) -> Vec<u8> {
    let doc = reprise_doc::Document::new(peer).unwrap();
    doc.define_style("", &reprise_doc::Style::default())
        .unwrap();
    doc.append_block(reprise_doc::BlockKind::Paragraph, "", text)
        .unwrap();
    doc.commit();
    reprise_format::Package::new(
        &doc,
        reprise_format::DocumentId([id; 16]),
        reprise_doc::PersistenceMode::History,
    )
    .unwrap()
    .save()
    .unwrap()
}

fn historical_copy_refusal(legacy: bool) {
    let original = workspace()
        .open(
            &Payload::new(Open {
                peer_id: "1".into(),
            }),
            &unmarked_package(1, 1, "source"),
        )
        .unwrap();
    // Models a copy saved before durable metadata was introduced: its package
    // identity and history are independent, but neither document has a marker.
    let mut copy = workspace()
        .open(
            &Payload::new(Open {
                peer_id: "2".into(),
            }),
            &unmarked_package(2, 2, "copy"),
        )
        .unwrap();
    // Prime the query roots before capturing the history used for refusal checks.
    copy.state().unwrap();
    let before = (
        copy.export_updates().unwrap().data,
        copy.state().unwrap().data,
    );
    let id = before.1.document_id.clone();
    if legacy {
        let mut packet = original.export_updates().unwrap();
        packet.data.document_id = id;
        assert!(
            copy.import_updates(&packet).is_err(),
            "unbound history merged"
        );
    } else {
        let mut packet = original
            .sync_export(&Payload::new(SyncRequest { since: None }))
            .unwrap();
        packet.data.document_id = id;
        assert!(
            copy.sync_import(&packet).is_err(),
            "foreign snapshot merged"
        );
    }
    assert!(
        before
            == (
                copy.export_updates().unwrap().data,
                copy.state().unwrap().data
            )
    );
}

#[test]
fn historical_unmarked_copy_refuses_rewritten_snapshot() {
    historical_copy_refusal(false);
}

#[test]
fn historical_unmarked_copy_refuses_unverifiable_legacy_history() {
    historical_copy_refusal(true);
}

#[test]
fn unmarked_packages_scope_frames_without_authoring_identity() {
    let bytes = unmarked_package(1, 1, "source");
    let original = workspace()
        .open(
            &Payload::new(Open {
                peer_id: "1".into(),
            }),
            &bytes,
        )
        .unwrap();
    let mut peer = workspace()
        .open(
            &Payload::new(Open {
                peer_id: "3".into(),
            }),
            &bytes,
        )
        .unwrap();
    let before = original.export_updates().unwrap().data;
    let packet = original
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    assert!(
        packet
            .data
            .features
            .iter()
            .any(|f| f == "document-lineage1")
    );
    peer.sync_import(&packet).unwrap();
    assert_eq!(before, original.export_updates().unwrap().data);
    let persisted = reprise_doc::Document::import(&before.content.bytes, 7).unwrap();
    assert_eq!(persisted.document_lineage().unwrap(), None);
    assert_eq!(
        original.state().unwrap().data.blocks,
        peer.state().unwrap().data.blocks
    );
}

#[test]
fn rewritten_source_envelope_cannot_contaminate_reopened_or_transitive_copy() {
    let (bytes, mut original) = source();
    let mut copy = workspace()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    let saved = copy.save().unwrap().data.bytes;
    let reopened = workspace()
        .open(
            &Payload::new(Open {
                peer_id: "3".into(),
            }),
            &saved,
        )
        .unwrap();
    let transitive = workspace()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: "00000000000000000000000000000009".into(),
                peer_id: "4".into(),
            }),
            &saved,
        )
        .unwrap();
    let since = original.sync_info().data.vector;
    let n = original.state().unwrap().data.blocks[0].id.clone();
    tx(
        &mut original,
        vec![Command::InsertText {
            node: n,
            at: 0,
            text: "foreign ".into(),
        }],
    );
    let packet = original
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    let delta = original
        .sync_export(&Payload::new(SyncRequest { since: Some(since) }))
        .unwrap();
    let legacy = original.export_updates().unwrap();
    for mut receiver in [copy, reopened, transitive] {
        let before = (
            receiver.save().unwrap().data.bytes,
            receiver.state().unwrap().data,
            receiver.sync_info().data,
        );
        for mut forged in [packet.clone(), delta.clone()] {
            forged.data.document_id = receiver.state().unwrap().data.document_id;
            assert!(
                receiver.sync_import(&forged).is_err(),
                "rewritten envelope merged source history"
            );
        }
        let mut forged = legacy.clone();
        forged.data.document_id = receiver.state().unwrap().data.document_id;
        assert!(receiver.import_updates(&forged).is_err());
        assert_eq!(
            (
                receiver.save().unwrap().data.bytes,
                receiver.state().unwrap().data,
                receiver.sync_info().data
            ),
            before
        );
    }
}

fn workspace() -> Workspace {
    Workspace::new()
}
fn session(id: &str, peer: &str) -> DocumentSession {
    workspace()
        .create(&Payload::new(Create {
            document_id: id.into(),
            peer_id: peer.into(),
        }))
        .unwrap()
}
fn tx(s: &mut DocumentSession, commands: Vec<Command>) -> Payload<Applied> {
    s.apply(&Payload::new(Transaction { commands })).unwrap()
}
fn finish(s: &mut DocumentSession) {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    while !job.step(s, 1000).unwrap().data.complete {}
}
fn texts(s: &DocumentSession) -> Vec<String> {
    s.state()
        .unwrap()
        .data
        .blocks
        .into_iter()
        .map(|b| b.text)
        .collect()
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
/// A source with text, character formatting, a page setup, a pin and an asset.
fn source() -> (Vec<u8>, DocumentSession) {
    let mut s = session(SOURCE, "1");
    let ids = tx(
        &mut s,
        vec![block("first paragraph", 0), block("second", 1)],
    )
    .data
    .blocks;
    tx(
        &mut s,
        vec![
            Command::FormatText {
                node: ids[0].clone(),
                start: 0,
                end: 5,
                style: TextStyle {
                    weight: Some(700),
                    ..TextStyle::default()
                },
            },
            Command::InsertTab {
                node: ids[1].clone(),
                at: 3,
            },
            Command::AddAnchor {
                node: ids[1].clone(),
                at: 0,
                edge: LineEdge::Start,
                target: ids[0].clone(),
                target_at: 0,
                target_edge: AnchorEdge::Position,
            },
        ],
    );
    s.set_page_setup(&Payload::new(PageSetupPatch {
        width: Some(626_688),
        height: Some(811_008),
        ..PageSetupPatch::default()
    }))
    .unwrap();
    s.register_asset(
        &Payload::new(AssetDeclaration {
            id: "note.txt".into(),
            kind: AssetKind::Other,
        }),
        b"bundled bytes",
    )
    .unwrap();
    let bytes = s.save().unwrap().data.bytes;
    (bytes, s)
}

#[test]
fn a_copy_keeps_content_and_drops_history() {
    let (bytes, original) = source();
    let mut copy = workspace()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    let (a, b) = (original.state().unwrap().data, copy.state().unwrap().data);
    assert_eq!(b.document_id, COPY);
    assert_eq!(b.peer_id, "2");
    assert_eq!(texts(&copy), texts(&original));
    assert_eq!(b.page_setup, a.page_setup);
    assert_eq!(
        b.blocks[0].formatting, a.blocks[0].formatting,
        "character formatting is kept"
    );
    assert!(!b.can_undo, "no undo history");
    // No operation of the source survives: its peer is gone from the log.
    let peers: Vec<_> = copy
        .sync_info()
        .data
        .vector
        .into_iter()
        .map(|c| c.peer)
        .collect();
    assert_eq!(peers, ["2"]);
    // The pin, tab and the bundled asset are kept.
    finish(&mut copy);
    assert!(
        copy.marks(0)
            .unwrap()
            .data
            .marks
            .iter()
            .any(|m| m.kind == MarkKind::Anchor && m.applied)
    );
    let resource = copy
        .resources()
        .unwrap()
        .data
        .into_iter()
        .find(|r| r.id == "note.txt")
        .unwrap();
    assert!(resource.available);
    assert_eq!(
        copy.resource_bytes(&Payload::new("note.txt".into()))
            .unwrap()
            .data
            .bytes,
        b"bundled bytes"
    );
}

#[test]
fn a_copy_saves_and_reopens_under_its_own_identity() {
    let (bytes, _) = source();
    let mut copy = workspace()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "1".into(),
            }),
            &bytes,
        )
        .unwrap();
    let saved = copy.save().unwrap().data.bytes;
    let reopened = workspace()
        .open(
            &Payload::new(Open {
                peer_id: "3".into(),
            }),
            &saved,
        )
        .unwrap();
    assert_eq!(reopened.state().unwrap().data.document_id, COPY);
    assert_eq!(texts(&reopened), texts(&copy));
}

#[test]
fn a_copy_and_its_source_never_cross_sync() {
    let (bytes, mut original) = source();
    let mut copy = workspace()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                // The same peer as the source on purpose: histories are
                // independent, so their operation ids may coincide.
                peer_id: "1".into(),
            }),
            &bytes,
        )
        .unwrap();
    let source_node = original.state().unwrap().data.blocks[0].id.clone();
    let copy_node = copy.state().unwrap().data.blocks[0].id.clone();
    tx(
        &mut original,
        vec![Command::InsertText {
            node: source_node,
            at: 0,
            text: "SOURCE ".into(),
        }],
    );
    tx(
        &mut copy,
        vec![Command::InsertText {
            node: copy_node,
            at: 0,
            text: "COPY ".into(),
        }],
    );
    let from_source = original
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    let from_copy = copy
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    let before = (texts(&original), texts(&copy));
    // Honest packets are refused with stable codes, in both directions.
    let error = copy.sync_import(&from_source).unwrap_err();
    assert_eq!(error.code(), "bindings.copy-source");
    assert_eq!(
        original.sync_import(&from_copy).unwrap_err().code(),
        "bindings.id"
    );
    let legacy = original.export_updates().unwrap();
    assert_eq!(
        copy.import_updates(&legacy).unwrap_err().code(),
        "bindings.copy-source"
    );
    // A relay that rewrites the document id cannot make a source delta apply:
    // the copy has none of the operations it depends on. (A forged *snapshot*
    // would merge: packets are not authenticated, so the relay must keep rooms
    // per document id. See docs/plumbing.md.)
    let delta = original
        .sync_export(&Payload::new(SyncRequest {
            since: Some(copy.sync_info().data.vector),
        }))
        .unwrap();
    let mut forged = delta.clone();
    forged.data.document_id = COPY.into();
    copy.sync_import(&forged).unwrap_err();
    assert_eq!(texts(&copy), before.1);
    assert_eq!(texts(&original), before.0);
}

#[test]
fn identity_must_change_and_input_is_bounded() {
    let (bytes, _) = source();
    let w = workspace();
    let request = |id: &str| {
        Payload::new(ImportCopy {
            document_id: id.into(),
            peer_id: "2".into(),
        })
    };
    assert_eq!(
        w.import_copy(&request(SOURCE), &bytes)
            .err()
            .unwrap()
            .code(),
        "bindings.id",
        "a copy needs a new identity"
    );
    assert_eq!(
        w.import_copy(&request("not-hex"), &bytes)
            .err()
            .unwrap()
            .code(),
        "bindings.id"
    );
    assert!(w.import_copy(&request(COPY), b"garbage").is_err());
    assert!(w.import_copy(&request(COPY), &[]).is_err());
    let bad = Payload {
        version: 3,
        data: request(COPY).data,
    };
    assert_eq!(
        w.import_copy(&bad, &bytes).err().unwrap().code(),
        "bindings.version"
    );
    // The source bytes are untouched and still open as the source.
    let reopened = w
        .open(
            &Payload::new(Open {
                peer_id: "9".into(),
            }),
            &bytes,
        )
        .unwrap();
    assert_eq!(reopened.state().unwrap().data.document_id, SOURCE);
}

/// Paragraph styles (`docs/paragraph-styles.md`) survive a copy as the
/// source resolves them: a defined style with paragraph properties, a
/// patched built-in, a deleted built-in and an unreferenced style.
#[test]
fn a_copy_keeps_paragraph_styles() {
    let mut s = session(SOURCE, "1");
    let ids = tx(&mut s, vec![block("quoted", 0), block("heading", 1)])
        .data
        .blocks;
    tx(
        &mut s,
        vec![
            Command::DefineStyle {
                name: "Quote".into(),
                parent: Some("".into()),
                style: Style {
                    indent_start: Some("24pt".into()),
                    space_before: Some("1lh".into()),
                    keep_together: Some(true),
                    ..Style::default()
                },
            },
            Command::DefineStyle {
                name: "Unused".into(),
                parent: Some("".into()),
                style: Style {
                    indent_end: Some("3pt".into()),
                    ..Style::default()
                },
            },
            Command::ApplyStyle {
                node: ids[0].clone(),
                style: Some("Quote".into()),
            },
            Command::ApplyStyle {
                node: ids[1].clone(),
                style: Some("Heading 2".into()),
            },
            Command::UpdateStyle {
                name: "Heading 2".into(),
                parent: None,
                style: Style {
                    color: Some([0, 0, 255, 255]),
                    ..Style::default()
                },
                clear: Vec::new(),
            },
            Command::DeleteStyle {
                name: "Subtitle".into(),
            },
        ],
    );
    let bytes = s.save().unwrap().data.bytes;
    let copy = workspace()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    let (a, b) = (s.state().unwrap().data, copy.state().unwrap().data);
    assert_eq!(b.styles, a.styles);
    assert!(b.styles.iter().any(|n| n.name == "Unused"));
    assert!(!b.styles.iter().any(|n| n.name == "Subtitle"));
    let style_of = |st: &State, i: usize| {
        let block = &st.blocks[i];
        (
            block.style.name.clone(),
            st.resolved[block.style.resolved as usize].clone(),
        )
    };
    for i in 0..2 {
        assert_eq!(style_of(&b, i), style_of(&a, i));
    }
}

fn renamed_parent_copy(populated: bool) {
    let mut source = session(SOURCE, "1");
    tx(
        &mut source,
        vec![
            Command::DefineStyle {
                name: "Parent".into(),
                parent: Some("".into()),
                style: Style {
                    size: Some("40pt".into()),
                    ..Default::default()
                },
            },
            Command::DefineStyle {
                name: "Child".into(),
                parent: Some("Parent".into()),
                style: Style::default(),
            },
            Command::RenameStyle {
                from: "Parent".into(),
                to: "Renamed".into(),
            },
        ],
    );
    if populated {
        let node = tx(&mut source, vec![block("child", 0)]).data.blocks[0].clone();
        tx(
            &mut source,
            vec![Command::ApplyStyle {
                node,
                style: Some("Child".into()),
            }],
        );
    }
    let mut copy = workspace()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "2".into(),
            }),
            &source.save().unwrap().data.bytes,
        )
        .unwrap();
    for document in [&mut source, &mut copy] {
        if !populated {
            let node = tx(document, vec![block("child", 0)]).data.blocks[0].clone();
            tx(
                document,
                vec![Command::ApplyStyle {
                    node,
                    style: Some("Child".into()),
                }],
            );
        }
        let state = document.state().unwrap().data;
        assert_eq!(
            state.resolved[state.blocks[0].style.resolved as usize].size,
            40 * 1024,
            "copy detached an unused parent alias"
        );
        tx(
            document,
            vec![Command::UpdateStyle {
                name: "Renamed".into(),
                parent: None,
                style: Style {
                    size: Some("50pt".into()),
                    ..Default::default()
                },
                clear: Vec::new(),
            }],
        );
        let state = document.state().unwrap().data;
        assert_eq!(
            state.resolved[state.blocks[0].style.resolved as usize].size,
            50 * 1024,
            "copy materialised the alias as a detached definition"
        );
    }
}

#[test]
fn empty_copy_retains_unused_parent_aliases() {
    renamed_parent_copy(false);
}

#[test]
fn populated_copy_retains_parent_aliases_after_later_edits() {
    renamed_parent_copy(true);
}

#[test]
fn copy_retains_referenced_tombstones_and_builtin_forwarding() {
    let mut source = session(SOURCE, "1");
    let node = tx(&mut source, vec![block("deleted quote", 0)]).data.blocks[0].clone();
    tx(
        &mut source,
        vec![
            Command::DefineStyle {
                name: "Gone".into(),
                parent: Some("".into()),
                style: Style {
                    size: Some("40pt".into()),
                    ..Default::default()
                },
            },
            Command::ApplyStyle {
                node,
                style: Some("Gone".into()),
            },
            Command::DeleteStyle {
                name: "Gone".into(),
            },
            Command::RenameStyle {
                from: "Subtitle".into(),
                to: "Subheading".into(),
            },
        ],
    );
    let copy = workspace()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "2".into(),
            }),
            &source.save().unwrap().data.bytes,
        )
        .unwrap();
    let native = |session: &DocumentSession| {
        reprise_doc::Document::import(&session.export_updates().unwrap().data.content.bytes, 9)
            .unwrap()
    };
    let (a, b) = (native(&source), native(&copy));
    for name in ["Gone", "Subtitle", "Subheading"] {
        assert_eq!(
            a.style_record(name),
            b.style_record(name),
            "lost authored record {name}"
        );
    }
}
