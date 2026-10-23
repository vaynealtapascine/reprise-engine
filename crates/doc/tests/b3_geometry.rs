//! B3 fixes (the Astra B3 review), finding 6: a complete copy carries the raw
//! geometry record exactly, or is refused. It never drops a key, or the record.
use reprise_doc::fragment::FragmentError;
use reprise_doc::{BlockKind, Document, Domain, PersistenceMode, SchemaRegistry};

/// A document with a pageless geometry record, then `edit` applied to the raw
/// `geometry1` root by another peer, and read back.
fn with_geometry(edit: impl FnOnce(&loro::LoroDoc)) -> Document {
    let base = Document::new(1).unwrap();
    base.append_block(BlockKind::Paragraph, "", "body").unwrap();
    base.set_geometry_mode("pageless").unwrap();
    base.commit();
    let raw = loro::LoroDoc::new();
    raw.import(&base.export(PersistenceMode::History)).unwrap();
    raw.set_peer_id(99).unwrap();
    edit(&raw);
    raw.commit();
    Document::import(&raw.export(loro::ExportMode::Snapshot).unwrap(), 2).unwrap()
}

#[test]
fn every_plain_key_of_the_record_travels_exactly_and_is_pasted_back() {
    let d = with_geometry(|raw| {
        let map = raw.get_map("geometry1");
        map.insert("future-text", "kept").unwrap();
        map.insert("future-flag", true).unwrap();
        map.insert("future-count", 7i64).unwrap();
    });
    let before = d.geometry_mode_record().unwrap();
    assert_eq!(before.len(), 5);
    let schemas = SchemaRegistry::builtin();
    for fragment in [
        d.copy_document_fragment("source", &schemas).unwrap(),
        d.copy_all_fragment("source", &schemas).unwrap(),
    ] {
        let envelope = fragment.envelope().unwrap().unwrap();
        assert_eq!(envelope.geometry.as_ref(), Some(&before));
    }
    // And an empty destination takes it as it was: the paste writes the
    // envelope's record with `write_geometry_record`.
    let fragment = d.copy_document_fragment("source", &schemas).unwrap();
    let dst = Document::new(3).unwrap();
    dst.write_geometry_record(&fragment.envelope().unwrap().unwrap().geometry.unwrap())
        .unwrap();
    dst.commit();
    assert_eq!(dst.geometry_mode_record().as_ref(), Some(&before));
}

#[test]
fn a_value_the_copy_cannot_carry_refuses_the_complete_copy() {
    let schemas = SchemaRegistry::builtin();
    let cases: Vec<(&str, Document)> = vec![
        (
            "a container",
            with_geometry(|raw| {
                raw.get_map("geometry1")
                    .insert_container("future", loro::LoroMap::new())
                    .unwrap();
            }),
        ),
        (
            "a number that is not an integer",
            with_geometry(|raw| {
                raw.get_map("geometry1").insert("ratio", 1.5f64).unwrap();
            }),
        ),
        (
            "a list",
            with_geometry(|raw| {
                raw.get_map("geometry1")
                    .insert("many", vec![1i64, 2])
                    .unwrap();
            }),
        ),
    ];
    for (what, d) in cases {
        for result in [
            d.copy_document_fragment("source", &schemas),
            d.copy_all_fragment("source", &schemas),
        ] {
            assert!(
                matches!(result, Err(FragmentError::Invalid(_))),
                "{what}: {result:?}"
            );
        }
        // A copy of part of the body carries no geometry, so it has nothing
        // to refuse.
        let first = d.top(Domain::Body)[0];
        let part = d
            .copy_fragment(
                "source",
                &[reprise_doc::fragment::CopyBlock {
                    node: first,
                    bytes: Some(0..2),
                }],
                &schemas,
            )
            .unwrap();
        assert!(
            part.envelope()
                .unwrap()
                .is_none_or(|e| e.geometry.is_none()),
            "{what}"
        );
    }
}

#[test]
fn a_geometry_root_that_is_not_a_record_refuses_rather_than_vanishes() {
    let d = with_geometry(|_| {});
    // Another peer's replacement of the root by a list: the reader sees a
    // record that isn't one.
    let raw = loro::LoroDoc::new();
    raw.import(&d.export(PersistenceMode::History)).unwrap();
    raw.set_peer_id(98).unwrap();
    raw.get_map("geometry1").delete("mode").unwrap();
    raw.get_map("geometry1")
        .insert_container("v", loro::LoroList::new())
        .unwrap();
    raw.commit();
    let d = Document::import(&raw.export(loro::ExportMode::Snapshot).unwrap(), 4).unwrap();
    let record = d.geometry_mode_record().unwrap();
    assert!(record["v"].is_null());
    assert!(matches!(
        d.copy_document_fragment("source", &SchemaRegistry::builtin()),
        Err(FragmentError::Invalid(_))
    ));
}
