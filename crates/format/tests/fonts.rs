use reprise_doc::{BlockKind, Document, PersistenceMode, Style};
use reprise_fixtures::{fonts::declaration, hostile};
use reprise_font::{FontStore, GenericFamily};
use reprise_format::{
    Asset, AssetKind, AssetSource, DocumentId, Limits, MigrationRegistry, Package, Section,
    content_hash, ids,
};
use reprise_layout::Engine;
use std::collections::BTreeMap;
fn open(bytes: &[u8], peer: u64) -> reprise_format::OpenedFile {
    Package::open(
        bytes,
        peer,
        Limits::default(),
        &MigrationRegistry::builtin(),
    )
    .unwrap()
}
#[test]
fn used_fonts_embed_restore_and_preserve_layout_on_both_peers() {
    let fixture = hostile::font_three_faces().unwrap();
    let layout = fixture.engine.layout(&fixture.doc);
    let package = Package::new_with_layout(
        &fixture.doc,
        DocumentId([21; 16]),
        PersistenceMode::History,
        &layout,
        &fixture.engine.fonts,
    )
    .unwrap();
    assert_eq!(package.assets().unwrap().fonts.len(), 3);
    let bytes = package.save().unwrap();
    for peer in [1, 2] {
        let mut opened = open(&bytes, peer);
        let mut fonts = FontStore::default();
        assert!(opened.missing_fonts().is_empty());
        assert_eq!(opened.restore_fonts(&mut fonts).len(), 3);
        assert!(opened.restore_fonts(&mut fonts).is_empty());
        let engine = Engine::new(fonts);
        let restored = engine.layout(opened.editable_document().unwrap());
        assert_eq!(restored.to_json(), layout.to_json());
        assert_eq!(opened.package().save().unwrap(), bytes);
        let saved = opened
            .save_with_layout(PersistenceMode::History, &restored, &engine.fonts)
            .unwrap();
        assert_eq!(saved, bytes);
    }
}
#[test]
fn removed_bundle_reports_missing_pin_then_substitutes() {
    let fixture = hostile::font_three_faces().unwrap();
    let layout = fixture.engine.layout(&fixture.doc);
    let package = Package::new_with_layout(
        &fixture.doc,
        DocumentId([22; 16]),
        PersistenceMode::History,
        &layout,
        &fixture.engine.fonts,
    )
    .unwrap();
    let mut container = package.container().clone();
    let asset = package
        .assets()
        .unwrap()
        .needed
        .into_iter()
        .find(|need| need.font.as_ref().is_some_and(|p| p.face.family == "One"))
        .unwrap();
    let assets: Vec<Asset> =
        serde_json::from_slice(&container.sections[&ids::ASSETS].bytes).unwrap();
    let section = assets
        .iter()
        .find_map(|a| {
            if a.id == asset.id {
                match a.source {
                    AssetSource::Bundled { section } => Some(section),
                    _ => None,
                }
            } else {
                None
            }
        })
        .unwrap();
    container.sections.remove(&section);
    let mut damaged = package.clone();
    damaged
        .set_assets(
            assets,
            container
                .sections
                .iter()
                .filter(|(id, _)| (ids::FIRST_ASSET..=ids::LAST_ASSET).contains(id))
                .map(|(id, section)| (*id, section.bytes.clone()))
                .collect(),
        )
        .unwrap();
    let mut opened = open(&damaged.save().unwrap(), 2);
    assert_eq!(opened.missing_fonts().len(), 1);
    assert_eq!(opened.missing_fonts()[0].face.family, "One");
    assert!(
        opened
            .notes
            .iter()
            .any(|n| n.code.as_str() == "format.font-missing"
                && n.severity == reprise_diag::Severity::Warning)
    );
    let mut fonts = FontStore::default();
    opened.restore_fonts(&mut fonts);
    let layout = Engine::new(fonts).layout(opened.editable_document().unwrap());
    assert!(layout.diagnostics_with("font.fallback").next().is_some());
    assert!(!layout.blocks[0].lines[0].runs.is_empty());
}
#[test]
fn collection_face_alias_descriptors_and_index_survive_embedding() {
    let mut fonts = FontStore::default();
    let collection = reprise_fixtures::fonts::collection();
    let mut declared = declaration("Collection", 1);
    declared.descriptors.weight = 500;
    let id = fonts
        .register(collection.clone(), declared.clone())
        .unwrap();
    let zero = fonts
        .register(collection, declaration("Collection", 0))
        .unwrap();
    assert_ne!(id.hash, zero.hash);
    let doc = Document::new(1).unwrap();
    doc.define_style("body", &Style::default()).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "body", "").unwrap();
    doc.block(node)
        .unwrap()
        .text
        .insert(0, "collection office")
        .unwrap();
    doc.set_overrides(
        node,
        &Style {
            families: Some(vec!["Collection".into(), "sans-serif".into()]),
            ..Style::default()
        },
    )
    .unwrap();
    doc.commit();
    // Register only index one in the layout store so the normal-weight request
    // must use the nearest face (weight 500) and report it.
    let mut selected = FontStore::default();
    selected
        .register(reprise_fixtures::fonts::collection(), declared.clone())
        .unwrap();
    let engine = Engine::new(selected);
    let layout = engine.layout(&doc);
    assert!(layout.diagnostics_with("font.nearest").next().is_some());
    let package = Package::new_with_layout(
        &doc,
        DocumentId([23; 16]),
        PersistenceMode::History,
        &layout,
        &engine.fonts,
    )
    .unwrap();
    let mut opened = open(&package.save().unwrap(), 2);
    let mut restored = FontStore::default();
    assert_eq!(opened.restore_fonts(&mut restored), vec![id.clone()]);
    assert_eq!(restored.get(&id).unwrap().declaration(), &declared);
    assert_eq!(
        Engine::new(restored)
            .layout(opened.editable_document().unwrap())
            .to_json(),
        layout.to_json()
    );
}
#[test]
fn embedding_is_idempotent_preserves_other_assets_and_refuses_stale_layout() {
    let fixture = hostile::font_generics().unwrap();
    let layout = fixture.engine.layout(&fixture.doc);
    let mut package =
        Package::new(&fixture.doc, DocumentId([24; 16]), PersistenceMode::History).unwrap();
    let image = b"opaque image".to_vec();
    package
        .set_assets(
            vec![Asset {
                id: "image".into(),
                kind: AssetKind::Image,
                hash: content_hash(&image),
                source: AssetSource::Bundled {
                    section: ids::FIRST_ASSET,
                },
                extra: BTreeMap::from([("future".into(), serde_json::json!({"raw":1}))]),
            }],
            BTreeMap::from([(ids::FIRST_ASSET, image.clone())]),
        )
        .unwrap();
    package
        .set_unknown_section(55, Section::raw(vec![9, 8, 7]))
        .unwrap();
    package
        .embed_layout_fonts(&layout, &fixture.engine.fonts)
        .unwrap();
    let before = package.save().unwrap();
    package
        .embed_layout_fonts(&layout, &fixture.engine.fonts)
        .unwrap();
    assert_eq!(package.save().unwrap(), before);
    assert_eq!(package.assets().unwrap().bundled["image"], image);
    assert_eq!(package.container().sections[&55].bytes, [9, 8, 7]);
    let mut stale = layout.clone();
    stale.revision.0.push((99, 1));
    assert!(
        package
            .embed_layout_fonts(&stale, &fixture.engine.fonts)
            .is_err()
    );
    assert_eq!(package.save().unwrap(), before);
    let mut missing = layout.clone();
    missing.blocks[0].lines[0].runs[0].face.family = "absent".into();
    assert!(
        package
            .embed_layout_fonts(&missing, &fixture.engine.fonts)
            .is_err()
    );
    assert_eq!(package.save().unwrap(), before);
}
#[test]
fn empty_runs_and_unused_style_names_are_not_embedded() {
    let doc = Document::new(1).unwrap();
    doc.define_style("body", &Style::default()).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "body", "").unwrap();
    doc.set_overrides(
        node,
        &Style {
            families: Some(vec!["missing".into(), "script".into()]),
            ..Style::default()
        },
    )
    .unwrap();
    doc.commit();
    let fonts = FontStore::default();
    let engine = Engine::new(fonts);
    let layout = engine.layout(&doc);
    let package = Package::new_with_layout(
        &doc,
        DocumentId([25; 16]),
        PersistenceMode::History,
        &layout,
        &engine.fonts,
    )
    .unwrap();
    assert!(package.assets().unwrap().fonts.is_empty());
    doc.block(node)
        .unwrap()
        .text
        .insert(0, "\u{10ffff}")
        .unwrap();
    doc.commit();
    let layout = engine.layout(&doc);
    let package = Package::new_with_layout(
        &doc,
        DocumentId([25; 16]),
        PersistenceMode::History,
        &layout,
        &engine.fonts,
    )
    .unwrap();
    assert_eq!(
        package.assets().unwrap().fonts[0].id(),
        engine.fonts.generic(GenericFamily::Script).id()
    );
}

#[test]
fn repair_preserves_nonfont_assets_and_unknown_declaration_fields() {
    let fixture = hostile::font_generics().unwrap();
    let mut layout = fixture.engine.layout(&fixture.doc);
    layout.blocks.truncate(1);
    let face = fixture.engine.fonts.generic(GenericFamily::Serif);
    let mut package =
        Package::new(&fixture.doc, DocumentId([26; 16]), PersistenceMode::History).unwrap();
    let image = b"retained image".to_vec();
    // A malformed legacy pin claims the image; refresh must allocate its own asset.
    package.set_fonts(vec![reprise_format::FontPin { face: face.id().clone(), version: "legacy".into(), asset: "image".into(), extra: BTreeMap::from([
        ("future-pin".into(),serde_json::json!([1,2,3])),
        ("font-declaration1".into(),serde_json::json!({"future-declaration": "retained", "descriptors": {"future-descriptor":42}})),
    ]) }]).unwrap();
    package
        .set_assets(
            vec![Asset {
                id: "image".into(),
                kind: AssetKind::Image,
                hash: content_hash(&image),
                source: AssetSource::Bundled {
                    section: ids::FIRST_ASSET,
                },
                extra: BTreeMap::new(),
            }],
            BTreeMap::from([(ids::FIRST_ASSET, image.clone())]),
        )
        .unwrap();
    package
        .embed_layout_fonts(&layout, &fixture.engine.fonts)
        .unwrap();
    assert_eq!(package.assets().unwrap().bundled["image"], image);
    let pins: Vec<reprise_format::FontPin> =
        serde_json::from_slice(&package.container().sections[&ids::FONTS].bytes).unwrap();
    assert_ne!(pins[0].asset, "image");
    assert_eq!(pins[0].extra["future-pin"], serde_json::json!([1, 2, 3]));
    assert_eq!(
        pins[0].extra["font-declaration1"]["future-declaration"],
        "retained"
    );
    assert_eq!(
        pins[0].extra["font-declaration1"]["descriptors"]["future-descriptor"],
        42
    );
    let before = package.save().unwrap();
    package
        .embed_layout_fonts(&layout, &fixture.engine.fonts)
        .unwrap();
    assert_eq!(package.save().unwrap(), before);
    let mut fonts = FontStore::default();
    let opened = Package::open_with_fonts(
        &before,
        2,
        Limits::default(),
        &MigrationRegistry::builtin(),
        &mut fonts,
    )
    .unwrap();
    assert!(opened.missing_fonts().is_empty());
    assert!(fonts.by_family(&face.id().family).is_some());
}

#[test]
fn declaration_version_index_family_and_content_corruption_are_recoverable() {
    let fixture = hostile::font_three_faces().unwrap();
    let layout = fixture.engine.layout(&fixture.doc);
    let package = Package::new_with_layout(
        &fixture.doc,
        DocumentId([27; 16]),
        PersistenceMode::History,
        &layout,
        &fixture.engine.fonts,
    )
    .unwrap();
    let pins: Vec<reprise_format::FontPin> =
        serde_json::from_slice(&package.container().sections[&ids::FONTS].bytes).unwrap();
    for mutation in 0..5 {
        let mut bad_pins = pins.clone();
        match mutation {
            0 => bad_pins[0].version = "forged version".into(),
            1 => {
                bad_pins[0].extra.get_mut("font-declaration1").unwrap()["face_index"] =
                    serde_json::json!(u32::MAX)
            }
            2 => {
                bad_pins[0].extra.get_mut("font-declaration1").unwrap()["family"] =
                    serde_json::json!("forged alias")
            }
            3 => {
                bad_pins[0].extra.get_mut("font-declaration1").unwrap()["descriptors"]["weight"] =
                    serde_json::json!(0)
            }
            _ => bad_pins[0].face.hash = "0".repeat(32),
        }
        let mut damaged = package.clone();
        damaged.set_fonts(bad_pins).unwrap();
        let mut fonts = FontStore::default();
        let opened = Package::open_with_fonts(
            &damaged.save().unwrap(),
            2,
            Limits::default(),
            &MigrationRegistry::builtin(),
            &mut fonts,
        )
        .unwrap();
        assert_eq!(opened.missing_fonts().len(), 1);
        assert!(
            opened
                .notes
                .iter()
                .any(|n| n.code.as_str() == "format.font-missing")
        );
        assert!(opened.notes.iter().any(|n| n.code.as_str()
            == if mutation == 4 {
                "format.font-hash"
            } else {
                "format.font-unreadable"
            }));
        assert!(fonts.by_family(&pins[0].face.family).is_none());
        assert!(
            !Engine::new(fonts)
                .layout(opened.editable_document().unwrap())
                .blocks
                .is_empty()
        );
    }
}
