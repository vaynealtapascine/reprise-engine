//! Import as a copy: a new identity and a fresh history that never syncs back.
use reprise::*;

const SOURCE: &str = "00112233445566778899aabbccddeeff";
const COPY: &str = "ffeeddccbbaa99887766554433221100";

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
    s.state().unwrap().data.blocks.into_iter().map(|b| b.text).collect()
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
    let ids = tx(&mut s, vec![block("first paragraph", 0), block("second", 1)])
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
    let peers: Vec<_> = copy.sync_info().data.vector.into_iter().map(|c| c.peer).collect();
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
        .open(&Payload::new(Open { peer_id: "3".into() }), &saved)
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
    assert_eq!(original.sync_import(&from_copy).unwrap_err().code(), "bindings.id");
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
        w.import_copy(&request(SOURCE), &bytes).err().unwrap().code(),
        "bindings.id",
        "a copy needs a new identity"
    );
    assert_eq!(
        w.import_copy(&request("not-hex"), &bytes).err().unwrap().code(),
        "bindings.id"
    );
    assert!(w.import_copy(&request(COPY), b"garbage").is_err());
    assert!(w.import_copy(&request(COPY), &[]).is_err());
    let bad = Payload {
        version: 3,
        data: request(COPY).data,
    };
    assert_eq!(w.import_copy(&bad, &bytes).err().unwrap().code(), "bindings.version");
    // The source bytes are untouched and still open as the source.
    let reopened = w
        .open(&Payload::new(Open { peer_id: "9".into() }), &bytes)
        .unwrap();
    assert_eq!(reopened.state().unwrap().data.document_id, SOURCE);
}
