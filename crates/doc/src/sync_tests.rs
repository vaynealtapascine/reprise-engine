//! Sync packet tests: delivery orders, duplicates, refusals and change reports.

use loro::JsonSchema;

use crate::sync::*;
use crate::{BlockKind, Document, NodeId, Style};

fn dump(doc: &Document) -> Vec<(NodeId, String)> {
    doc.document_order()
        .into_iter()
        .filter_map(|id| Some((id, doc.block(id).ok()?.text.to_string())))
        .collect()
}

fn pair() -> (Document, Document, NodeId) {
    let a = Document::new(1).unwrap();
    let p = a.append_block(BlockKind::Paragraph, "", "hello").unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    (a, b, p)
}

fn type_at(doc: &Document, node: NodeId, at: usize, s: &str) {
    doc.block(node).unwrap().text.insert(at, s).unwrap();
    doc.commit();
}

/// Rebuilds a packet with a new header and body, for tampering.
fn repack(header: &PacketHeader, body: &[u8]) -> Vec<u8> {
    super::sync::tests_support::write(header, body)
}

#[test]
fn delta_converges_and_is_idempotent() {
    let (a, b, p) = pair();
    type_at(&b, p, 5, " world");
    let packet = b.export_delta(&a.version_vector()).unwrap();
    let report = a.import_packet(&packet).unwrap();
    assert_eq!(dump(&a), dump(&b));
    assert_eq!(a.revision(), b.revision());
    assert!(report.blocks.contains(&p));
    assert!(!report.structure);
    let before = a.revision();
    let again = a.import_packet(&packet).unwrap();
    assert!(again.is_empty(), "a replay reports nothing: {again:?}");
    assert_eq!(a.revision(), before);
}

#[test]
fn one_keystroke_delta_stays_small_in_a_large_document() {
    let (a, b, p) = pair();
    for i in 0..200 {
        let n = a
            .append_block(BlockKind::Paragraph, "", &"x".repeat(500))
            .unwrap();
        if i % 50 == 0 {
            type_at(&a, n, 0, "y");
        }
    }
    a.commit();
    b.import_packet(&a.export_delta(&b.version_vector()).unwrap())
        .unwrap();
    type_at(&b, p, 0, "k");
    let packet = b.export_delta(&a.version_vector()).unwrap();
    assert!(packet.len() < 1024, "{} bytes", packet.len());
}

#[test]
fn out_of_order_delta_is_refused_then_applies() {
    let (a, b, p) = pair();
    let v0 = a.version_vector();
    type_at(&b, p, 0, "one ");
    let first = b.export_delta(&v0).unwrap();
    let v1 = b.version_vector();
    type_at(&b, p, 0, "two ");
    let second = b.export_delta(&v1).unwrap();
    let before = a.revision();
    match a.import_packet(&second) {
        Err(SyncError::Missing { have }) => assert_eq!(have, v0),
        other => panic!("expected missing, got {other:?}"),
    }
    assert_eq!(a.revision(), before, "nothing applied");
    a.import_packet(&first).unwrap();
    a.import_packet(&second).unwrap();
    assert_eq!(dump(&a), dump(&b));
    // In order on a third replica.
    let c = Document::import(&b.export(crate::PersistenceMode::History), 3).unwrap();
    assert_eq!(dump(&c), dump(&b));
}

#[test]
fn dependency_on_a_third_peer_is_missing() {
    let (a, b, p) = pair();
    let c = a.fork(3).unwrap();
    type_at(&c, p, 0, "c");
    b.import_packet(&c.export_delta(&b.version_vector()).unwrap())
        .unwrap();
    type_at(&b, p, 0, "b");
    // b's change depends on c's, but the delta only carries b's.
    let mut since = a.version_vector();
    since.push((3, 1));
    since.sort_unstable();
    let only_b = b.export_delta(&since).unwrap();
    assert!(matches!(
        a.import_packet(&only_b),
        Err(SyncError::Missing { .. })
    ));
    a.import_packet(&b.export_delta(&a.version_vector()).unwrap())
        .unwrap();
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn unknown_format_and_features_are_refused_before_the_body() {
    let (a, b, p) = pair();
    type_at(&b, p, 0, "x");
    let packet = b.export_delta(&a.version_vector()).unwrap();
    let mut newer = packet.clone();
    newer[4] = 3;
    assert_eq!(a.import_packet(&newer), Err(SyncError::Format(3)));
    let mut feature = packet.clone();
    feature[6 + 7] = 0x80;
    assert!(matches!(
        a.import_packet(&feature),
        Err(SyncError::Feature(f)) if f == 1 << 63
    ));
    let (header, _) = read_header(&packet).unwrap();
    let garbage = repack(&header, b"not json at all");
    assert!(matches!(
        a.import_packet(&garbage),
        Err(SyncError::Invalid(_))
    ));
    let before = a.revision();
    for end in 0..packet.len() {
        assert!(a.import_packet(&packet[..end]).is_err(), "prefix {end}");
    }
    let mut trailing = packet.clone();
    trailing.push(0);
    assert!(a.import_packet(&trailing).is_err());
    assert_eq!(a.revision(), before);
    assert_eq!(
        Features::from_names(&Features::SUPPORTED.names()),
        Ok(Features::SUPPORTED)
    );
    assert_eq!(Features(1 << 40).names(), vec!["bit-40".to_owned()]);
    assert!(Features::from_names(&["bit-064".into()]).is_err());
}

#[test]
fn vectors_that_misdescribe_the_content_are_refused() {
    let (a, b, p) = pair();
    type_at(&b, p, 0, "abc");
    let packet = b.export_delta(&a.version_vector()).unwrap();
    let (header, body) = read_header(&packet).unwrap();
    let mut short = header.clone();
    for entry in &mut short.until {
        if entry.0 == 2 {
            entry.1 -= 1;
        }
    }
    let mut extra = header.clone();
    extra.until.push((9, 4));
    let mut late = header.clone();
    for entry in &mut late.since {
        if entry.0 == 1 {
            entry.1 = 0;
        }
    }
    for tampered in [short, extra, late] {
        assert!(
            matches!(
                a.import_packet(&repack(&tampered, body)),
                Err(SyncError::Invalid(_))
            ),
            "{tampered:?}"
        );
    }
    let mut unsorted = header.clone();
    unsorted.until.reverse();
    assert!(a.import_packet(&repack(&unsorted, body)).is_err());
}

#[test]
fn operations_under_the_receivers_peer_are_refused() {
    let (a, _b, p) = pair();
    let impostor = a.fork(1).unwrap();
    type_at(&impostor, p, 0, "spoof");
    let packet = impostor.export_delta(&a.version_vector()).unwrap();
    assert_eq!(a.import_packet(&packet), Err(SyncError::LocalPeer));
    // The receiver's own history, sent back, is a harmless duplicate.
    let echo = a.export_delta(&[]).unwrap();
    assert!(a.import_packet(&echo).unwrap().is_empty());
}

#[test]
fn hostile_counters_and_lamports_are_bounded() {
    let (a, b, p) = pair();
    type_at(&b, p, 0, "x");
    let packet = b.export_delta(&a.version_vector()).unwrap();
    let (header, body) = read_header(&packet).unwrap();
    let json: JsonSchema = serde_json::from_slice(body).unwrap();
    let mut lamport = json.clone();
    lamport.changes[0].lamport = u32::MAX - 1;
    let tampered = repack(&header, &serde_json::to_vec(&lamport).unwrap());
    assert_eq!(
        a.import_packet(&tampered),
        Err(SyncError::Limit("Lamport timestamps"))
    );
    let mut empty = json.clone();
    empty.changes[0].ops.clear();
    let tampered = repack(&header, &serde_json::to_vec(&empty).unwrap());
    assert!(matches!(
        a.import_packet(&tampered),
        Err(SyncError::Invalid(_))
    ));
    let mut peers = json.clone();
    peers.peers = Some(vec![u64::MAX]);
    let tampered = repack(&header, &serde_json::to_vec(&peers).unwrap());
    assert!(a.import_packet(&tampered).is_err());
}

#[test]
fn snapshot_packets_join_and_merge() {
    let (a, b, p) = pair();
    type_at(&b, p, 0, "b ");
    type_at(&a, p, 5, " a");
    let snapshot = b.export_snapshot_packet().unwrap();
    let fresh = Document::new(7).unwrap();
    fresh.import_packet(&snapshot).unwrap();
    assert_eq!(dump(&fresh), dump(&b));
    a.import_packet(&snapshot).unwrap();
    b.import_packet(&a.export_delta(&b.version_vector()).unwrap())
        .unwrap();
    assert_eq!(dump(&a), dump(&b));
    let (header, body) = read_header(&snapshot).unwrap();
    let mut wrong = header.clone();
    wrong.until.push((99, 1));
    assert!(matches!(
        fresh.import_packet(&repack(&wrong, body)),
        Err(SyncError::Invalid(_))
    ));
    let shallow = b.try_export(crate::PersistenceMode::Shallow).unwrap();
    assert!(fresh.import_packet(&repack(&header, &shallow)).is_err());
    let impostor = Document::new(2).unwrap();
    assert_eq!(
        b.import_packet(&impostor.export_snapshot_packet().unwrap())
            .map(|r| r.is_empty()),
        Ok(true),
        "an empty history under the receiver's peer is nothing new"
    );
    type_at(
        &impostor,
        impostor
            .append_block(BlockKind::Paragraph, "", "x")
            .unwrap(),
        0,
        "y",
    );
    assert_eq!(
        b.import_packet(&impostor.export_snapshot_packet().unwrap()),
        Err(SyncError::LocalPeer)
    );
}

#[test]
fn change_reports_name_blocks_structure_and_styles() {
    let (a, b, p) = pair();
    let parent = b.append_block(BlockKind::Paragraph, "", "parent").unwrap();
    b.commit();
    let child = b
        .insert_block_at(
            Some(parent),
            0,
            &crate::NewBlock::new(BlockKind::Paragraph, "", "c"),
        )
        .unwrap();
    b.commit();
    let report = a
        .import_packet(&b.export_delta(&a.version_vector()).unwrap())
        .unwrap();
    assert!(report.structure);
    assert!(report.blocks.contains(&parent) && report.blocks.contains(&child));
    assert!(!report.blocks.contains(&p));

    b.delete_block(parent).unwrap();
    b.commit();
    let report = a
        .import_packet(&b.export_delta(&a.version_vector()).unwrap())
        .unwrap();
    assert!(
        report.blocks.contains(&child),
        "a deleted parent reports its subtree: {report:?}"
    );
    assert!(!a.is_live(child));

    b.define_style("body", &Style::default()).unwrap();
    b.commit();
    let report = a
        .import_packet(&b.export_delta(&a.version_vector()).unwrap())
        .unwrap();
    assert!(report.styles && report.blocks.is_empty());
}

#[test]
fn merge_sends_only_what_is_missing() {
    let (a, b, p) = pair();
    for i in 0..50 {
        type_at(&a, p, 0, &i.to_string());
    }
    b.merge(&a).unwrap();
    type_at(&b, p, 0, "z");
    a.merge(&b).unwrap();
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn full_join_uses_json_and_refuses_legacy_binary_before_decoding() {
    let (a, b, _) = pair();
    let packet = a.export_snapshot_packet().unwrap();
    let (header, body) = read_header(&packet).unwrap();
    assert_eq!(header.features, Features::SNAPSHOT_JSON);
    assert_eq!(header.kind, PacketKind::Snapshot);
    assert!(serde_json::from_slice::<JsonSchema>(body).is_ok());
    let mut legacy = header.clone();
    legacy.features = Features::SNAPSHOT_LORO_1_16;
    let before = b.revision();
    // Even an invalid body never reaches the binary decoder.
    assert_eq!(
        b.import_packet(&repack(&legacy, b"hostile binary")),
        Err(SyncError::Feature(Features::SNAPSHOT_LORO_1_16.0))
    );
    assert_eq!(b.revision(), before);
}

#[test]
fn compacted_history_refuses_requests_before_its_retained_base() {
    let (a, _, p) = pair();
    let bytes = a.try_export(crate::PersistenceMode::Shallow).unwrap();
    let compact = Document::import(&bytes, 9).unwrap();
    assert!(matches!(
        compact.export_snapshot_packet(),
        Err(SyncError::Invalid(_))
    ));
    let since = compact.version_vector();
    type_at(&compact, p, 0, "new");
    let packet = compact.export_delta(&since).unwrap();
    a.import_packet(&packet).unwrap();
    assert_eq!(dump(&compact), dump(&a));
}

#[test]
fn a_tree_parent_created_later_in_the_packet_is_refused_before_import() {
    use loro::{JsonOpContent, JsonTreeOp};
    let (a, b, _) = pair();
    b.append_block(BlockKind::Paragraph, "", "first").unwrap();
    b.commit();
    b.append_block(BlockKind::Paragraph, "", "later").unwrap();
    b.commit();
    let packet = b.export_delta(&a.version_vector()).unwrap();
    let (header, body) = read_header(&packet).unwrap();
    let mut json: JsonSchema = serde_json::from_slice(body).unwrap();
    let targets: Vec<_> = json
        .changes
        .iter()
        .flat_map(|c| &c.ops)
        .filter_map(|op| {
            if let JsonOpContent::Tree(JsonTreeOp::Create { target, .. }) = &op.content {
                Some(*target)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(targets.len(), 2);
    for op in json.changes.iter_mut().flat_map(|c| &mut c.ops) {
        if let JsonOpContent::Tree(JsonTreeOp::Create { target, parent, .. }) = &mut op.content
            && *target == targets[0]
        {
            *parent = Some(targets[1]);
        }
    }
    let before = a.revision();
    let tampered = repack(&header, &serde_json::to_vec(&json).unwrap());
    assert!(matches!(
        a.import_packet(&tampered),
        Err(SyncError::Invalid(_))
    ));
    assert_eq!(a.revision(), before);
    a.import_packet(&packet).unwrap();
    assert_eq!(dump(&a), dump(&b));
}
