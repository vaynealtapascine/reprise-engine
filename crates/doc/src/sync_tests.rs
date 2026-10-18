//! Sync packet tests: delivery orders, duplicates, refusals and change reports.

use loro::JsonSchema;

use crate::sync::*;
use crate::{BlockKind, Document, NodeId, Style};

fn dump(doc: &Document) -> Vec<(NodeId, String)> {
    doc.order(crate::Domain::Body)
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
fn maximum_vectors_use_the_total_wire_bound_at_exact_boundary() {
    let vector: Vector = (1..=MAX_VECTOR_ENTRIES as u64).map(|p| (p, 1)).collect();
    let header = PacketHeader {
        format: SYNC_FORMAT,
        features: Features::DELTA_JSON,
        kind: PacketKind::Delta,
        since: vector.clone(),
        until: vector,
    };
    let overhead = 23 + 12 * (header.since.len() + header.until.len());
    let mut body = vec![b' '; MAX_PACKET_BYTES - overhead];
    let packet = super::sync::tests_support::try_write(&header, &body).unwrap();
    assert_eq!(packet.len(), MAX_PACKET_BYTES);
    assert_eq!(read_header(&packet).unwrap().0, header);
    body.push(b' ');
    assert!(matches!(
        super::sync::tests_support::try_write(&header, &body),
        Err(SyncError::Limit("packet bytes"))
    ));
    let mut oversized = packet;
    oversized.push(0);
    assert!(matches!(
        read_header(&oversized),
        Err(SyncError::Limit("packet bytes"))
    ));
}

#[test]
fn valid_bound_join_at_exact_wire_limit_imports() {
    let source = Document::new(1).unwrap();
    let id = "00112233445566778899aabbccddeeff";
    source.bind_document_lineage(id).unwrap();
    let node = source
        .append_block(BlockKind::Paragraph, "", "boundary")
        .unwrap();
    source.commit();
    let small = source.export_snapshot_packet().unwrap();
    let (header, body) = read_header(&small).unwrap();
    let overhead = small.len() - body.len();
    let mut padded = body.to_vec();
    padded.resize(MAX_PACKET_BYTES - overhead, b' ');
    let packet = super::sync::tests_support::try_write(&header, &padded).unwrap();
    assert_eq!(packet.len(), MAX_PACKET_BYTES);
    let receiver = Document::new(2).unwrap();
    receiver.bind_document_lineage(id).unwrap();
    receiver.commit();
    receiver.import_packet(&packet).unwrap();
    assert_eq!(receiver.block(node).unwrap().text.to_string(), "boundary");
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

/// A delta that moves `q` before `p`, with its tree position rewritten.
fn move_with_position(position: &str) -> (Document, Document, Vec<u8>, Vec<u8>) {
    let (a, b, _) = pair();
    let q = b.append_block(BlockKind::Paragraph, "", "second").unwrap();
    b.commit();
    a.import_packet(&b.export_delta(&a.version_vector()).unwrap())
        .unwrap();
    b.move_block(q, None, 0).unwrap();
    b.commit();
    let packet = b.export_delta(&a.version_vector()).unwrap();
    let (header, body) = read_header(&packet).unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(body).unwrap();
    let mut moved = 0;
    for change in json["changes"].as_array_mut().unwrap() {
        for op in change["ops"].as_array_mut().unwrap() {
            if op["content"]["type"] == "move" {
                op["content"]["fractional_index"] = position.into();
                moved += 1;
            }
        }
    }
    assert_eq!(moved, 1);
    let tampered = repack(&header, &serde_json::to_vec(&json).unwrap());
    (a, b, packet, tampered)
}

#[test]
fn tree_positions_loro_cannot_insert_beside_are_refused() {
    // Each of these imported, then aborted the receiver's next local block
    // insertion beside it inside Loro and poisoned its store.
    let long = format!("{}80", "7F".repeat(MAX_TREE_POSITION_BYTES));
    for position in ["", "00", "FF", "0000", "FFFF", "8000", long.as_str()] {
        let (a, b, packet, tampered) = move_with_position(position);
        let before = a.revision();
        assert!(
            matches!(a.import_packet(&tampered), Err(SyncError::Invalid(_))),
            "position {position:?} was accepted"
        );
        assert_eq!(a.revision(), before);
        a.import_packet(&packet).unwrap();
        assert_eq!(dump(&a), dump(&b));
    }
}

#[test]
fn unusual_but_valid_tree_positions_keep_local_insertion_working() {
    for position in ["80", "7F80", "0080", "FF80", "00000080", "FFFFFF80"] {
        let (a, _, _, tampered) = move_with_position(position);
        a.import_packet(&tampered).unwrap();
        for index in 0..=3 {
            a.insert_block_at(
                None,
                index,
                &crate::NewBlock::new(BlockKind::Paragraph, "", "local"),
            )
            .unwrap();
            a.commit();
        }
        assert_eq!(a.top(crate::Domain::Body).len(), 6, "position {position:?}");
    }
}

#[test]
fn non_ascii_tree_positions_are_refused_before_loro_reads_them() {
    // Loro's reader slices position strings by byte offset: "€a" splits the
    // euro sign and panicked inside serde_json, before any other check.
    for position in ["€a", "a€", "éé"] {
        let (a, _, _, tampered) = move_with_position(position);
        assert!(matches!(
            a.import_packet(&tampered),
            Err(SyncError::Invalid(_))
        ));
    }
    // The key is matched after JSON unescaping, as serde reads it.
    let (a, _, _, tampered) = move_with_position("€a");
    let (header, body) = read_header(&tampered).unwrap();
    let escaped = String::from_utf8(body.to_vec())
        .unwrap()
        .replace("\"fractional_index\"", r#""fractional\u005findex""#);
    assert!(escaped.contains(r"fractional\u005findex"));
    assert!(matches!(
        a.import_packet(&repack(&header, escaped.as_bytes())),
        Err(SyncError::Invalid(_))
    ));
}

#[test]
fn saved_documents_with_invalid_tree_positions_are_refused() {
    let (a, _, _, tampered) = move_with_position("00");
    let (_, body) = read_header(&tampered).unwrap();
    let json: loro::JsonSchema = serde_json::from_slice(body).unwrap();
    // Bypass the packet boundary, as a crafted file would.
    a.doc.import_json_updates(json).unwrap();
    let bytes = a.try_export(crate::PersistenceMode::History).unwrap();
    assert!(Document::import(&bytes, 9).is_err());
}

#[test]
fn identity_feature_transfers_empty_root_and_rejects_downgraded_references() {
    let (a, b, node) = pair();
    a.set_style_name(node, "Heading 1").unwrap();
    a.commit();
    let packet = a.export_delta(&b.version_vector()).unwrap();
    let (header, body) = read_header(&packet).unwrap();
    assert_ne!(header.features.0 & Features::STYLE_IDENTITIES.0, 0);
    let before = b.try_export(crate::PersistenceMode::History).unwrap();
    let mut downgraded = header.clone();
    downgraded.features.0 &= !Features::STYLE_IDENTITIES.0;
    assert!(b.import_packet(&repack(&downgraded, body)).is_err());
    assert_eq!(
        before,
        b.try_export(crate::PersistenceMode::History).unwrap()
    );
    b.import_packet(&packet).unwrap();
    assert!(b.has_style_identities());
    assert_eq!(
        a.computed_style(node).unwrap(),
        b.computed_style(node).unwrap()
    );
}

#[test]
fn downgraded_flat_builtin_parent_is_refused_without_merging() {
    let (a, b, _) = pair();
    a.define_named_style(
        "Child",
        &Style {
            parent: Some("Heading 1".into()),
            ..Default::default()
        },
    )
    .unwrap();
    a.commit();
    let packet = a.export_delta(&b.version_vector()).unwrap();
    let (mut header, body) = read_header(&packet).unwrap();
    header.features.0 &= !Features::STYLE_IDENTITIES.0;
    let before = b.try_export(crate::PersistenceMode::History).unwrap();
    assert!(b.import_packet(&repack(&header, body)).is_err());
    assert_eq!(
        before,
        b.try_export(crate::PersistenceMode::History).unwrap()
    );
}

#[test]
fn historical_builtin_context_survives_join_even_with_empty_legacy_root() {
    for (empty, native) in [(false, false), (true, false), (false, true), (true, true)] {
        let raw = Document::new(1).unwrap();
        let node = raw.append_block(BlockKind::Paragraph, "", "old").unwrap();
        raw.meta_of(node)
            .unwrap()
            .insert("style", "Heading 1")
            .unwrap();
        let legacy = raw.doc.get_map("styles1");
        if !empty {
            legacy.insert("weight/Unused", "400").unwrap();
        }
        raw.commit();
        let source =
            Document::import(&raw.try_export(crate::PersistenceMode::History).unwrap(), 3).unwrap();
        let before = source.try_export(crate::PersistenceMode::History).unwrap();
        let packet = source.export_snapshot_packet().unwrap();
        assert_eq!(
            before,
            source.try_export(crate::PersistenceMode::History).unwrap()
        );
        let receiver = Document::new(2).unwrap();
        let mut undo = receiver.undo_stack();
        let old_revision = receiver.revision();
        if native {
            receiver.merge(&source).unwrap();
        } else {
            let report = receiver.import_packet(&packet).unwrap();
            assert!(report.styles);
        }
        assert_ne!(receiver.revision(), old_revision);
        assert!(receiver.has_style_identities());
        assert_eq!(
            receiver.computed_style(node).unwrap().size,
            source.computed_style(node).unwrap().size
        );
        let reopened = Document::import(
            &receiver
                .try_export(crate::PersistenceMode::History)
                .unwrap(),
            4,
        )
        .unwrap();
        assert_eq!(
            reopened.computed_style(node).unwrap().size,
            source.computed_style(node).unwrap().size
        );
        assert!(!undo.undo().unwrap());
    }
}

#[test]
fn range_endpoint_packets_require_their_feature_before_merging() {
    let (a, b, first) = pair();
    let last = a.append_block(BlockKind::Paragraph, "", "last").unwrap();
    let policy = crate::text::RangePolicy::FIXED;
    let range = a.add_range(first, 1..3, policy).unwrap();
    a.stage_fragment_range_end(range, last, 2, policy).unwrap();
    a.commit();
    let packet = a.export_snapshot_packet().unwrap();
    let (mut header, body) = read_header(&packet).unwrap();
    assert_ne!(header.features.0 & Features::RANGE_ENDPOINTS.0, 0);
    header.features.0 &= !Features::RANGE_ENDPOINTS.0;
    let before = b.try_export(crate::PersistenceMode::History).unwrap();
    assert!(b.import_packet(&repack(&header, body)).is_err());
    assert_eq!(
        before,
        b.try_export(crate::PersistenceMode::History).unwrap()
    );
    b.import_packet(&packet).unwrap();
    assert_eq!(b.range_extent(range), a.range_extent(range));
    assert!(b.has_cross_paragraph_ranges());
}

#[test]
fn explicit_style_context_packets_refuse_downgrading_before_import() {
    let legacy = Document::import(include_bytes!("../tests/data/review5-legacy.bin"), 11).unwrap();
    let a = Document::new(2).unwrap();
    a.define_named_style("NewPeerStyle", &Style::default())
        .unwrap();
    a.commit();
    a.merge(&legacy).unwrap();
    assert!(a.has_style_reference_contexts());
    let packet = a.export_snapshot_packet().unwrap();
    let (mut header, body) = read_header(&packet).unwrap();
    assert_ne!(header.features.0 & Features::STYLE_REFERENCE_CONTEXTS.0, 0);
    header.features.0 &= !Features::STYLE_REFERENCE_CONTEXTS.0;
    let b = Document::new(3).unwrap();
    let before = b.try_export(crate::PersistenceMode::History).unwrap();
    assert!(b.import_packet(&repack(&header, body)).is_err());
    assert_eq!(
        before,
        b.try_export(crate::PersistenceMode::History).unwrap()
    );
    b.import_packet(&packet).unwrap();
    assert!(b.has_style_reference_contexts());
    assert_eq!(
        b.computed_style(NodeId::parse("2@1").unwrap())
            .unwrap()
            .size,
        reprise_geom::Length::from_pt(20)
    );
}

#[test]
fn deleted_version_markers_keep_their_declarations_and_round_trip() {
    let legacy = Document::import(include_bytes!("../tests/data/review5-legacy.bin"), 11).unwrap();
    let a = Document::new(2).unwrap();
    a.define_named_style("NewPeerStyle", &Style::default())
        .unwrap();
    a.commit();
    a.merge(&legacy).unwrap();
    a.doc
        .get_map(crate::style_identity::ROOT)
        .delete("reference-contexts2")
        .unwrap();
    let first = a.append_block(BlockKind::Paragraph, "", "first").unwrap();
    let last = a.append_block(BlockKind::Paragraph, "", "last").unwrap();
    let policy = crate::text::RangePolicy::FIXED;
    let range = a.add_range(first, 1..3, policy).unwrap();
    a.stage_fragment_range_end(range, last, 2, policy).unwrap();
    a.tree("ranges")
        .get_meta(range.0)
        .unwrap()
        .delete("end-node1")
        .unwrap();
    a.commit();
    for mode in [
        crate::PersistenceMode::History,
        crate::PersistenceMode::Shallow,
    ] {
        let reopened = Document::import(&a.try_export(mode).unwrap(), 5).unwrap();
        assert!(reopened.has_style_reference_contexts());
        assert!(reopened.has_cross_paragraph_ranges());
    }
    let packet = a.export_snapshot_packet().unwrap();
    let (header, _) = read_header(&packet).unwrap();
    assert_ne!(header.features.0 & Features::RANGE_ENDPOINTS.0, 0);
    assert_ne!(header.features.0 & Features::STYLE_REFERENCE_CONTEXTS.0, 0);
    let b = Document::new(3).unwrap();
    b.import_packet(&packet).unwrap();
    let graph = a.authored_style_graph().unwrap();
    let copy = Document::new(4).unwrap();
    copy.restore_authored_style_graph(&graph).unwrap();
    assert!(copy.has_style_reference_contexts());
}
