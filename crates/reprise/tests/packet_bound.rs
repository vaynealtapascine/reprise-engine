use reprise::*;

#[test]
fn legacy_and_framed_import_refuse_wire_limit_before_parsing() {
    let mut s = Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: "1".into(),
        }))
        .unwrap();
    let before = s.state().unwrap().data;
    let mut legacy = s.export_updates().unwrap();
    legacy.data.from_peer = "2".into();
    legacy.data.content.bytes = vec![0; MAX_PACKET_BYTES + 1];
    assert_eq!(
        s.import_updates(&legacy).unwrap_err().code(),
        "bindings.limit"
    );
    let mut framed = s
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    framed.data.from_peer = "2".into();
    framed.data.content.bytes = vec![0; MAX_PACKET_BYTES + 1];
    assert_eq!(s.sync_import(&framed).unwrap_err().code(), "bindings.limit");
    assert_eq!(s.state().unwrap().data, before);
}
