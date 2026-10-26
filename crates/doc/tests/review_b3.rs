use reprise_doc::{BlockKind, Document, PersistenceMode, SchemaRegistry};
#[test]
fn unreadable_geometry_is_preserved_or_refused_never_silently_dropped() {
    let base = Document::new(1).unwrap();
    base.append_block(BlockKind::Paragraph, "", "body").unwrap();
    base.set_geometry_mode("pageless").unwrap();
    base.commit();
    let raw = loro::LoroDoc::new();
    raw.import(&base.export(PersistenceMode::History)).unwrap();
    raw.set_peer_id(99).unwrap();
    raw.get_map("geometry1")
        .insert_container("future", loro::LoroMap::new())
        .unwrap();
    raw.commit();
    let d = Document::import(&raw.export(loro::ExportMode::Snapshot).unwrap(), 2).unwrap();
    let before = d.geometry_mode_record().unwrap();
    assert_eq!(before["mode"], "pageless");
    assert!(before["future"].is_null());
    let f = d.copy_document_fragment("source", &SchemaRegistry::builtin());
    match f {
        Err(_) => {}
        Ok(f) => {
            eprintln!(
                "original geometry={before:?}, copied envelope={:?}",
                f.sections
            );
            assert!(
                f.envelope().unwrap().and_then(|e| e.geometry).is_some(),
                "unreadable geometry silently dropped by complete copy"
            );
        }
    }
}
