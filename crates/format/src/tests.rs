use std::collections::BTreeMap;

use reprise_doc::{BlockKind, Document, PersistenceMode};

use crate::{Container, DocumentId, FormatError, Limits, MigrationRegistry, Package, Section, ids};

fn small() -> Package {
    let doc = Document::new(1).unwrap();
    doc.append_block(BlockKind::Paragraph, "body", "file")
        .unwrap();
    Package::new(&doc, DocumentId([7; 16]), PersistenceMode::History).unwrap()
}

fn open(bytes: &[u8]) -> Result<crate::OpenedFile, FormatError> {
    Package::open(bytes, 2, Limits::default(), &MigrationRegistry::builtin())
}

fn encoded(container: &Container) -> Vec<u8> {
    container.encode(Limits::default()).unwrap()
}

#[test]
fn golden_current_and_version_zero_migration() {
    let package = small();
    let bytes = package.save().unwrap();
    let mut old = package.container().clone();
    old.header.version = 0;
    old.sections.remove(&ids::FONTS);
    old.sections.remove(&ids::ASSETS);
    let old_bytes = encoded(&old);
    // Explicit fixture authoring only; subsequent runs compare checked-in bytes.
    if std::env::var("FORMAT_WRITE_FIXTURES").as_deref() == Ok("1") {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
        std::fs::write(data.join("v1.reprise"), &bytes).unwrap();
        std::fs::write(data.join("v0.reprise"), &old_bytes).unwrap();
        return;
    }
    assert_eq!(bytes, include_bytes!("../tests/data/v1.reprise"));
    assert_eq!(old_bytes, include_bytes!("../tests/data/v0.reprise"));
    let opened = open(include_bytes!("../tests/data/v0.reprise")).unwrap();
    assert_eq!(
        opened.migrations,
        vec![crate::MigrationStep { from: 0, to: 1 }]
    );
    assert_eq!(opened.save(PersistenceMode::History).unwrap(), bytes);
    assert_eq!(open(&bytes).unwrap().migrations.len(), 0);
    assert!(matches!(
        Package::open(
            &old_bytes,
            2,
            Limits {
                sections: 2,
                ..Limits::default()
            },
            &MigrationRegistry::builtin()
        ),
        Err(FormatError::Limit("sections"))
    ));
}

#[test]
fn every_truncation_and_bit_flip_terminates_without_panicking() {
    let bytes = small().save().unwrap();
    for end in 0..bytes.len() {
        assert!(
            std::panic::catch_unwind(|| open(&bytes[..end]))
                .unwrap()
                .is_err(),
            "truncation {end}"
        );
    }
    for index in 0..bytes.len() {
        for bit in 0..8 {
            let mut hostile = bytes.clone();
            hostile[index] ^= 1 << bit;
            assert!(
                std::panic::catch_unwind(|| open(&hostile))
                    .unwrap()
                    .is_err(),
                "flip {index}/{bit}"
            );
        }
    }
}

#[test]
fn lengths_counts_duplicates_and_trailing_data_are_typed_failures() {
    let bytes = small().save().unwrap();
    for length in [0_u64, 1, u64::MAX, u32::MAX.into(), 1024 * 1024 * 1024] {
        let mut hostile = bytes.clone();
        hostile[92..100].copy_from_slice(&length.to_le_bytes());
        assert!(
            std::panic::catch_unwind(|| open(&hostile))
                .unwrap()
                .is_err()
        );
    }
    let mut hostile = bytes.clone();
    hostile[44..48].copy_from_slice(&u32::MAX.to_le_bytes());
    let hash = crate::container::digest(&hostile[..48]);
    hostile[48..80].copy_from_slice(&hash);
    assert!(matches!(
        open(&hostile),
        Err(FormatError::Limit("sections"))
    ));
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(matches!(open(&trailing), Err(FormatError::Trailing)));
    // Duplicate the first complete record with an honest header checksum.
    let length = u64::from_le_bytes(bytes[92..100].try_into().unwrap()) as usize;
    let record = &bytes[80..132 + length];
    let mut duplicate = bytes.clone();
    duplicate.extend_from_slice(record);
    duplicate[44..48].copy_from_slice(&5_u32.to_le_bytes());
    let hash = crate::container::digest(&duplicate[..48]);
    duplicate[48..80].copy_from_slice(&hash);
    assert!(matches!(
        open(&duplicate),
        Err(FormatError::Duplicate(ids::DOCUMENT))
    ));
}

#[test]
fn malformed_huge_nested_manifests_and_unknown_required_data_fail() {
    let package = small();
    for manifest in [
        vec![255],
        b"{unbalanced".to_vec(),
        [b"[".repeat(100_000), b"]".repeat(100_000)].concat(),
        vec![b' '; 1024 * 1024 + 1],
        format!("{{\"many\":[{}]}}", vec!["0"; 5000].join(",")).into_bytes(),
    ] {
        let mut container = package.container().clone();
        container
            .sections
            .insert(ids::SETTINGS, Section::raw(manifest));
        assert!(
            std::panic::catch_unwind(|| open(&encoded(&container)))
                .unwrap()
                .is_err()
        );
    }
    let mut container = package.container().clone();
    container.header.features.required = 1;
    assert!(matches!(
        open(&encoded(&container)),
        Err(FormatError::RequiredFeatures(1))
    ));
    container.header.features.required = 0;
    container.sections.insert(
        99,
        Section {
            flags: 1,
            codec: 0,
            bytes: Vec::new(),
        },
    );
    assert!(matches!(
        open(&encoded(&container)),
        Err(FormatError::RequiredSection(99))
    ));
}

#[test]
fn compressed_bomb_is_never_decompressed() {
    let mut container = small().container().clone();
    // A tiny LZ4-frame-like body declaring 2^64-1 expanded bytes. Container v1
    // has no compression codec, so this fails before any decompressor executes.
    let bomb = [
        vec![4, 34, 77, 24, 0x68, 0x70],
        u64::MAX.to_le_bytes().to_vec(),
        vec![0, 0, 0, 0],
    ]
    .concat();
    container.sections.insert(
        ids::SETTINGS,
        Section {
            flags: 0,
            codec: 1,
            bytes: bomb,
        },
    );
    assert!(matches!(
        open(&encoded(&container)),
        Err(FormatError::Encoding(ids::SETTINGS))
    ));
}

#[test]
fn newer_files_are_read_only_and_never_downgraded() {
    let mut container = small().container().clone();
    container.header.version = 99;
    container.header.features.optional = 1 << 63;
    container
        .sections
        .insert(99, Section::raw(b"future".to_vec()));
    let opened = open(&encoded(&container)).unwrap();
    assert!(opened.is_read_only());
    assert_eq!(opened.view().unwrap().blocks().len(), 1);
    assert!(matches!(
        opened.editable_document(),
        Err(FormatError::ReadOnly)
    ));
    assert!(matches!(
        opened.save(PersistenceMode::History),
        Err(FormatError::ReadOnly)
    ));
    assert!(matches!(
        opened.package().save(),
        Err(FormatError::ReadOnly)
    ));
    container.header.features.required = 1;
    assert!(matches!(
        open(&encoded(&container)),
        Err(FormatError::RequiredFeatures(1))
    ));
}

#[test]
fn migration_chain_preserves_opaque_data_and_rejects_bad_registry_steps() {
    let mut container = small().container().clone();
    container.header.version = 0;
    container
        .sections
        .insert(99, Section::raw(b"opaque".to_vec()));
    let bytes = encoded(&container);
    let opened = open(&bytes).unwrap();
    assert_eq!(
        opened.package().container().sections.get(&99),
        container.sections.get(&99)
    );
    assert!(matches!(
        Package::open(&bytes, 2, Limits::default(), &MigrationRegistry::default()),
        Err(FormatError::MissingMigration(0))
    ));
    let mut bad = MigrationRegistry::default();
    bad.register(0, Ok).unwrap();
    assert!(matches!(
        Package::open(&bytes, 2, Limits::default(), &bad),
        Err(FormatError::BadMigration)
    ));
    assert!(bad.register(0, Ok).is_err());
    let mut drops = MigrationRegistry::default();
    drops
        .register(0, |mut c| {
            c.header.version = 1;
            c.sections.remove(&99);
            Ok(c)
        })
        .unwrap();
    assert!(matches!(
        Package::open(&bytes, 2, Limits::default(), &drops),
        Err(FormatError::BadMigration)
    ));
}

#[test]
fn corrupt_cache_is_recoverable_but_document_corruption_is_not() {
    let mut container = small().container().clone();
    container
        .sections
        .insert(ids::CACHE, Section::raw(b"not a cache".to_vec()));
    let bytes = encoded(&container);
    let opened = open(&bytes).unwrap();
    assert!(
        opened
            .notes
            .iter()
            .any(|n| n.code == "format.cache-dropped")
    );
    assert!(
        !opened
            .package()
            .container()
            .sections
            .contains_key(&ids::CACHE)
    );
    let mut corrupt = bytes.clone();
    let end = corrupt.len() - 1;
    corrupt[end] ^= 1;
    assert!(
        open(&corrupt)
            .unwrap()
            .notes
            .iter()
            .any(|n| n.code == "format.cache-dropped")
    );
    assert!(
        open(&bytes[..bytes.len() - 1])
            .unwrap()
            .notes
            .iter()
            .any(|n| n.code == "format.cache-dropped")
    );
    let mut corrupt = bytes;
    corrupt[132] ^= 1;
    assert!(matches!(
        open(&corrupt),
        Err(FormatError::Integrity(Some(ids::DOCUMENT)))
    ));
    container
        .sections
        .insert(ids::DOCUMENT, Section::raw(vec![0; 30]));
    assert!(matches!(
        open(&encoded(&container)),
        Err(FormatError::Document(_))
    ));
}

#[test]
fn limits_apply_to_caller_data_and_cannot_be_relaxed_past_hard_limits() {
    let bytes = small().save().unwrap();
    assert!(matches!(
        Package::open(
            &bytes,
            2,
            Limits {
                file_bytes: 1,
                ..Limits::default()
            },
            &MigrationRegistry::builtin()
        ),
        Err(FormatError::Limit("file bytes"))
    ));
    let mut package = small();
    let before = package.save().unwrap();
    let settings = BTreeMap::from([(
        "large".into(),
        serde_json::Value::String(" ".repeat(1024 * 1024)),
    )]);
    assert!(matches!(
        package.set_settings(&settings),
        Err(FormatError::Limit("manifest bytes"))
    ));
    assert_eq!(before, package.save().unwrap(), "failed setter is atomic");
    let mut nested = serde_json::json!(0);
    for _ in 0..64 {
        nested = serde_json::Value::Array(vec![nested]);
    }
    let settings = BTreeMap::from([("deep".into(), nested)]);
    assert!(matches!(
        package.set_settings(&settings),
        Err(FormatError::Limit("manifest depth"))
    ));
    assert_eq!(before, package.save().unwrap());
}

#[test]
fn unrecognized_manifest_fields_and_escaped_strings_remain_exact() {
    let mut container = small().container().clone();
    let settings = br#"{ "future" : {"escaped":"[\"{\\}]"}, "value":-12 }"#.to_vec();
    container
        .sections
        .insert(ids::SETTINGS, Section::raw(settings.clone()));
    container.sections.insert(ids::FONTS, Section::raw(br#"[ {"face":{"family":"Future","hash":"00000000000000000000000000000000"},"version":"1","asset":"missing","future-field":{"raw":"yes"}} ]"#.to_vec()));
    container.sections.insert(ids::ASSETS, Section::raw(br#"[ {"id":"external","kind":"other","hash":"0000000000000000000000000000000000000000000000000000000000000000","source":{"kind":"external","location":{"kind":"path","value":"C:/not-opened"}},"future-field":{"other":"yes"}} ]"#.to_vec()));
    let bytes = encoded(&container);
    let opened = open(&bytes).unwrap();
    assert_eq!(
        opened.package().container().sections[&ids::SETTINGS].bytes,
        settings
    );
    assert_eq!(bytes, opened.save(PersistenceMode::History).unwrap());
}

#[test]
fn fixed_seed_framing_mutations_terminate() {
    let original = small().save().unwrap();
    let mut seed = 0x7265_7072_6973_6501_u64;
    for _ in 0..256 {
        let mut hostile = original.clone();
        for _ in 0..8 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let index = (seed as usize) % hostile.len();
            hostile[index] ^= (seed >> 32) as u8;
        }
        let _ = std::panic::catch_unwind(|| open(&hostile)).unwrap();
    }
}

fn table_doc(header: bool, spans: bool) -> Document {
    use reprise_doc::{Column, ColumnWidth, TableColumns};
    let doc = Document::new(1).unwrap();
    let table = doc
        .append_table(TableColumns {
            columns: vec![
                Column {
                    width: ColumnWidth::Proportional(1)
                };
                2
            ],
        })
        .unwrap();
    let row = doc.append_table_row(table, header).unwrap();
    let cell = doc.append_table_cell(row, 0).unwrap();
    if spans {
        doc.set_table_cell_span(cell, 2, 1).unwrap();
    }
    doc.append_cell_block(cell, BlockKind::Paragraph, "body", "x")
        .unwrap();
    doc
}

#[test]
fn table_features_are_declared_from_content_and_refused_by_old_readers() {
    use crate::features::{OPTIONAL_TABLE_HEADERS, REQUIRED_TABLE_SPANS};
    let flags = |doc: &Document| {
        Package::new(doc, DocumentId([1; 16]), PersistenceMode::History)
            .unwrap()
            .container()
            .header
            .features
    };
    // Spanless, headerless tables write no feature bits: bytes are unchanged.
    let plain = flags(&table_doc(false, false));
    assert_eq!((plain.required, plain.optional), (0, 0));
    let headers = flags(&table_doc(true, false));
    assert_eq!(
        (headers.required, headers.optional),
        (0, OPTIONAL_TABLE_HEADERS)
    );
    let spans = flags(&table_doc(false, true));
    assert_eq!((spans.required, spans.optional), (REQUIRED_TABLE_SPANS, 0));

    // This reader opens its own spans file; a reader that knows no required
    // bit refuses it with the typed error (checked against the raw mask).
    let doc = table_doc(true, true);
    let package = Package::new(&doc, DocumentId([1; 16]), PersistenceMode::History).unwrap();
    let bytes = package.save().unwrap();
    assert!(open(&bytes).is_ok());
    let mut unknown = package.container().clone();
    unknown.header.features.required |= 1 << 11;
    assert!(matches!(
        open(&encoded(&unknown)),
        Err(FormatError::RequiredFeatures(m)) if m == 1 << 11
    ));

    // Saving after the last span is removed clears the required bit and keeps
    // unknown optional bits.
    let opened = open(&bytes).unwrap();
    let cell = {
        let d = opened.editable_document().unwrap();
        let table = d.blocks()[0];
        let row = d.children(Some(table))[0];
        d.children(Some(row))[0]
    };
    opened
        .editable_document()
        .unwrap()
        .set_table_cell_span(cell, 1, 1)
        .unwrap();
    let mut package = opened.package().clone();
    package
        .set_optional_features(1 << 63 | OPTIONAL_TABLE_HEADERS)
        .unwrap();
    let again = package
        .with_document(
            opened.editable_document().unwrap(),
            PersistenceMode::History,
        )
        .unwrap();
    let f = again.container().header.features;
    assert_eq!(f.required, 0);
    assert_eq!(f.optional, 1 << 63 | OPTIONAL_TABLE_HEADERS);
}
