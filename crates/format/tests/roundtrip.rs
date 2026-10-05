use std::collections::BTreeMap;

use reprise_doc::{BlockKind, Document, PersistenceMode};
use reprise_fixtures::{engine, hostile, spike};
use reprise_font::Face;
use reprise_format::{
    Asset, AssetKind, AssetSource, CacheTags, DerivedCache, DocumentId, ExternalLocation, FontPin,
    Limits, MigrationRegistry, Package, Section, content_hash, ids,
};
use reprise_shape::AdapterInfo;

fn open(bytes: &[u8], peer: u64) -> reprise_format::OpenedFile {
    Package::open(
        bytes,
        peer,
        Limits::default(),
        &MigrationRegistry::builtin(),
    )
    .unwrap()
}

fn check(doc: &Document, engine: &reprise_fixtures::hostile::Fixture) {
    let before = engine.engine.layout(doc).to_json();
    for peer in [1, 2] {
        let package = Package::new(doc, DocumentId([42; 16]), PersistenceMode::History).unwrap();
        let bytes = package.save().unwrap();
        assert_eq!(
            bytes,
            package.save().unwrap(),
            "{} repeat save",
            engine.name
        );
        assert_eq!(
            bytes,
            Package::new(doc, DocumentId([42; 16]), PersistenceMode::History)
                .unwrap()
                .save()
                .unwrap(),
            "{} fresh snapshot save",
            engine.name
        );
        let opened = open(&bytes, peer);
        let reopened = opened.editable_document().unwrap();
        assert_eq!(
            before,
            engine.engine.layout(reopened).to_json(),
            "{} peer {peer}",
            engine.name
        );
        assert_eq!(doc.revision(), reopened.revision());
        assert_eq!(
            bytes,
            opened.package().save().unwrap(),
            "opaque document retained"
        );
        doc.merge(reopened).unwrap();
        reopened.merge(doc).unwrap();
        assert_eq!(doc.revision(), reopened.revision());
        assert_eq!(before, engine.engine.layout(reopened).to_json());
    }
}

#[test]
fn every_hostile_fixture_preserves_layout_and_merge_on_both_peers() {
    for fixture in hostile::all().unwrap() {
        check(&fixture.doc, &fixture);
    }
}

#[test]
fn spike_preserves_layout_history_and_concurrent_merge() {
    let spike = spike::document().unwrap();
    let engine = engine();
    for peer in [1, 2] {
        let before = engine.layout(&spike.doc).to_json();
        let package =
            Package::new(&spike.doc, DocumentId([1; 16]), PersistenceMode::History).unwrap();
        let bytes = package.save().unwrap();
        let reopened = open(&bytes, peer);
        assert_eq!(
            before,
            engine
                .layout(reopened.editable_document().unwrap())
                .to_json()
        );
    }
    let before = spike.doc.revision();
    let bytes = Package::new(&spike.doc, DocumentId([1; 16]), PersistenceMode::History)
        .unwrap()
        .save()
        .unwrap();
    let reopened = open(&bytes, 2);
    spike.edit().unwrap();
    let other = reopened.editable_document().unwrap();
    other
        .block(spike.opening)
        .unwrap()
        .text
        .insert(0, "Peer two. ")
        .unwrap();
    spike.doc.merge(other).unwrap();
    other.merge(&spike.doc).unwrap();
    assert_eq!(
        engine.layout(&spike.doc).to_json(),
        engine.layout(other).to_json()
    );
    assert!(
        other.at(&before).is_ok(),
        "old revisions survived history export"
    );
    let edited = reopened.save(PersistenceMode::History).unwrap();
    let again = open(&edited, 2);
    assert_eq!(
        engine.layout(other).to_json(),
        engine.layout(again.editable_document().unwrap()).to_json()
    );
}

#[test]
fn shallow_is_an_explicit_history_retention_choice() {
    let spike = spike::document().unwrap();
    let before = spike.doc.revision();
    spike.edit().unwrap();
    let bytes = Package::new(&spike.doc, DocumentId([2; 16]), PersistenceMode::Shallow)
        .unwrap()
        .save()
        .unwrap();
    let reopened = open(&bytes, 2);
    assert_eq!(
        engine().layout(&spike.doc).to_json(),
        engine()
            .layout(reopened.editable_document().unwrap())
            .to_json()
    );
    assert!(reopened.editable_document().unwrap().at(&before).is_err());
}

#[test]
fn optional_flags_sections_extensions_and_metadata_survive_verbatim() {
    let doc = Document::new(1).unwrap();
    let mut package = Package::new(&doc, DocumentId([3; 16]), PersistenceMode::History).unwrap();
    package.set_optional_features(1 << 63).unwrap();
    let section = Section {
        flags: 0xffff_fffe,
        codec: 990,
        bytes: vec![0, 255, 0, 12],
    };
    package.set_unknown_section(99, section.clone()).unwrap();
    package.set_extensions(vec![255, 0, 254]).unwrap();
    package
        .set_settings(&BTreeMap::from([(
            "future-setting".into(),
            serde_json::json!({"x":[1,2,3]}),
        )]))
        .unwrap();
    let bytes = package.save().unwrap();
    let reopened = open(&bytes, 2);
    assert_eq!(
        reopened.package().container().sections.get(&99),
        Some(&section)
    );
    assert_eq!(
        reopened.package().extensions(),
        Some([255, 0, 254].as_slice())
    );
    assert_eq!(bytes, reopened.save(PersistenceMode::History).unwrap());
}

#[test]
fn font_bundles_are_pinned_and_corrupt_fonts_are_never_used() {
    let doc = Document::new(1).unwrap();
    let face = Face::from_bytes(reprise_fixtures::SERIF).unwrap();
    let mut package = Package::new(&doc, DocumentId([4; 16]), PersistenceMode::History).unwrap();
    let mut pin = FontPin {
        face: face.id().clone(),
        version: "fixture-1".into(),
        asset: "serif".into(),
        extra: BTreeMap::new(),
    };
    package.set_fonts(vec![pin.clone()]).unwrap();
    let mut duplicate = pin.clone();
    duplicate.version = "fixture-2".into();
    assert!(
        package.set_fonts(vec![pin.clone(), duplicate]).is_err(),
        "one font pin per bundled resource bounds font allocations"
    );
    let font = Asset {
        id: "serif".into(),
        kind: AssetKind::Font,
        hash: content_hash(face.data()),
        source: AssetSource::Bundled {
            section: ids::FIRST_ASSET,
        },
        extra: BTreeMap::new(),
    };
    let external = Asset {
        id: "photo".into(),
        kind: AssetKind::Image,
        hash: content_hash(b"unavailable"),
        source: AssetSource::External {
            location: ExternalLocation::Url("https://invalid.example/never-fetch".into()),
        },
        extra: BTreeMap::new(),
    };
    let missing = Asset {
        id: "missing".into(),
        kind: AssetKind::Other,
        hash: content_hash(b"missing"),
        source: AssetSource::Bundled {
            section: ids::FIRST_ASSET + 1,
        },
        extra: BTreeMap::new(),
    };
    package
        .set_assets(
            vec![font.clone(), external.clone(), missing.clone()],
            BTreeMap::from([(ids::FIRST_ASSET, face.data().to_vec())]),
        )
        .unwrap();
    let opened = open(&package.save().unwrap(), 2);
    assert_eq!(opened.assets.fonts.len(), 1);
    assert_eq!(opened.assets.needed.len(), 3);
    assert_eq!(opened.assets.missing.len(), 2);
    assert_eq!(opened.assets.fonts[0].id(), face.id());
    pin.face.hash = "0".repeat(32);
    package.set_fonts(vec![pin]).unwrap();
    let opened = open(&package.save().unwrap(), 2);
    assert!(opened.assets.fonts.is_empty());
    assert!(!opened.assets.bundled.contains_key("serif"));
    assert!(opened.notes.iter().any(|n| n.code == "format.font-hash"));
    package
        .set_assets(
            vec![font, external, missing],
            BTreeMap::from([(ids::FIRST_ASSET, b"bad font".to_vec())]),
        )
        .unwrap();
    let opened = open(&package.save().unwrap(), 2);
    assert!(opened.assets.fonts.is_empty());
    assert!(opened.notes.iter().any(|n| n.code == "format.font-hash"));
}

#[test]
fn caches_require_every_input_tag_and_are_dropped_after_document_edits() {
    let doc = Document::new(1).unwrap();
    let node = doc
        .append_block(BlockKind::Paragraph, "body", "cache")
        .unwrap();
    let id = DocumentId([8; 16]);
    let mut package = Package::new(&doc, id, PersistenceMode::History).unwrap();
    let tags = CacheTags {
        document_id: id,
        revision: doc.revision(),
        engine_version: "1".into(),
        adapter: AdapterInfo {
            name: "test".into(),
            version: "1".into(),
            platform_independent: true,
        },
        configuration_hash: content_hash(b"config"),
    };
    package
        .set_cache(Some(DerivedCache {
            tags: tags.clone(),
            bytes: b"opaque-layout".to_vec(),
        }))
        .unwrap();
    let bytes = package.save().unwrap();
    let opened = open(&bytes, 2);
    assert_eq!(
        opened.package().usable_cache(&tags).0,
        Some(b"opaque-layout".to_vec())
    );
    for field in 0..7 {
        let mut wrong = tags.clone();
        match field {
            0 => wrong.document_id = DocumentId([9; 16]),
            1 => wrong.revision.0.clear(),
            2 => wrong.engine_version = "2".into(),
            3 => wrong.adapter.name = "other".into(),
            4 => wrong.adapter.version = "other".into(),
            5 => wrong.adapter.platform_independent = false,
            _ => wrong.configuration_hash = content_hash(b"other"),
        }
        let (value, notes) = opened.package().usable_cache(&wrong);
        assert!(value.is_none());
        assert_eq!(notes[0].code, "format.cache-ignored");
    }
    opened
        .editable_document()
        .unwrap()
        .block(node)
        .unwrap()
        .text
        .insert(0, "edit ")
        .unwrap();
    let edited = open(&opened.save(PersistenceMode::History).unwrap(), 2);
    assert!(
        !edited
            .package()
            .container()
            .sections
            .contains_key(&ids::CACHE)
    );
}

#[test]
fn package_roundtrip_retains_all_authored_endpoint_policies() {
    use reprise_doc::text::{Affinity, Empty, RangePolicy};
    let doc = Document::new(1).unwrap();
    let node = doc
        .append_block(BlockKind::Paragraph, "", "\u{e9}\u{5d0}\u{5d1}")
        .unwrap();
    let mut ranges = Vec::new();
    for start in [Affinity::Before, Affinity::After] {
        for end in [Affinity::Before, Affinity::After] {
            for empty in [Empty::Keep, Empty::Missing] {
                let policy = RangePolicy { start, end, empty };
                ranges.push((doc.add_range(node, 0..6, policy).unwrap(), policy));
            }
        }
    }
    for mode in [PersistenceMode::History, PersistenceMode::Shallow] {
        let bytes = Package::new(&doc, DocumentId([55; 16]), mode)
            .unwrap()
            .save()
            .unwrap();
        let opened = open(&bytes, 2);
        for &(id, policy) in &ranges {
            assert_eq!(
                opened
                    .editable_document()
                    .unwrap()
                    .range_policy(id)
                    .unwrap(),
                Some(policy)
            );
        }
    }
}
