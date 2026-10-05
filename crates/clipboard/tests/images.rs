use reprise_clipboard::{
    Disposition, ExportOptions, Exporter, Feature, NativeFragment, NativeWithAssets, PdfWithAssets,
    PlainText, copy_all,
};
use reprise_display::AssetStore;
use reprise_doc::image::ImageData;
use reprise_doc::{BlockKind, Document, PersistenceMode, SchemaRegistry};
use reprise_edit::Editor;
use reprise_format::{DocumentId, Limits, MigrationRegistry, Package};
use reprise_layout::Engine;

const RED: &[u8] = include_bytes!("../../../fixtures/images/red-1x1.png");

fn source() -> (Document, Engine) {
    let mut engine = reprise_fixtures::engine();
    let hash = engine.assets.insert(RED).unwrap();
    let doc = Document::new(1).unwrap();
    doc.append_block(BlockKind::Paragraph, "", "Before")
        .unwrap();
    doc.append_image("", &ImageData::new(hash), "A red café image")
        .unwrap();
    doc.append_block(BlockKind::Paragraph, "", "After").unwrap();
    doc.commit();
    (doc, engine)
}

#[test]
fn package_embeds_used_images_only_and_open_restores_identical_layout() {
    let (doc, mut engine) = source();
    let unused = engine.assets.insert(b"unused bytes".as_slice()).unwrap();
    let layout = engine.layout(&doc);
    let mut package = Package::new_with_resources(
        &doc,
        DocumentId([32; 16]),
        PersistenceMode::History,
        &layout,
        &engine.fonts,
        &engine.assets,
    )
    .unwrap();
    let before = package.save().unwrap();
    package.embed_document_images(&doc, &engine.assets).unwrap();
    assert_eq!(before, package.save().unwrap(), "embedding is idempotent");
    assert!(
        !package
            .assets()
            .unwrap()
            .needed
            .iter()
            .any(|a| a.hash == unused)
    );
    for peer in [1, 2] {
        let mut opened_engine = reprise_fixtures::engine();
        let opened = Package::open_with_resources(
            &before,
            peer,
            Limits::default(),
            &MigrationRegistry::builtin(),
            &mut opened_engine.fonts,
            &mut opened_engine.assets,
        )
        .unwrap();
        let restored = opened.editable_document().unwrap();
        assert_eq!(opened_engine.layout(restored), layout);
        assert_eq!(
            opened
                .save_with_resources(
                    PersistenceMode::History,
                    &layout,
                    &opened_engine.fonts,
                    &opened_engine.assets
                )
                .unwrap(),
            before
        );
        for node in restored
            .blocks()
            .into_iter()
            .filter(|&n| restored.kind_of(n) == Some(BlockKind::Image))
        {
            assert_eq!(
                opened_engine
                    .assets
                    .get(&restored.image(node).unwrap().asset),
                Some(RED)
            );
        }
    }
}

#[test]
fn clipboard_transports_assets_and_raw_records_through_twenty_undo_redo_steps() {
    let (doc, engine) = source();
    let layout = engine.layout(&doc);
    let mut fragment = copy_all(
        &doc,
        "source",
        &engine.schemas,
        Some(&layout),
        Some(&engine.fonts),
    )
    .unwrap();
    fragment.attach_images(&engine.assets).unwrap();
    let fragment = NativeFragment::decode(&fragment.encode().unwrap()).unwrap();
    let mut target_engine = reprise_fixtures::engine();
    fragment.install_assets(&mut target_engine.assets).unwrap();
    fragment.install_fonts(&mut target_engine.fonts).unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    let mut ids = Vec::new();
    for _ in 0..20 {
        let pasted = fragment.paste(&mut editor, None, "target").unwrap();
        ids.push(pasted.ids.nodes.clone());
        let placed = target_engine.layout(editor.document());
        let images: Vec<_> = placed
            .blocks
            .iter()
            .filter_map(|b| b.image.as_ref())
            .collect();
        assert!(
            images
                .iter()
                .all(|i| !i.placeholder && i.alt == "A red café image")
        );
    }
    let complete = editor.document().document_order();
    for _ in 0..20 {
        editor.undo().unwrap();
    }
    assert!(editor.document().blocks().is_empty());
    for _ in 0..20 {
        editor.redo().unwrap();
    }
    assert_eq!(editor.document().document_order(), complete);
    assert_eq!(
        target_engine
            .layout(editor.document())
            .blocks
            .iter()
            .filter(|b| b.image.is_some())
            .count(),
        20
    );
    for mappings in ids {
        for (old, new) in mappings {
            if doc.kind_of(old) == Some(BlockKind::Image) {
                assert_eq!(
                    editor.document().image_record(new).unwrap(),
                    doc.image_record(old).unwrap()
                );
            }
        }
    }
}

#[test]
fn exports_follow_image_reading_order_and_report_asset_fidelity() {
    let (doc, engine) = source();
    let layout = engine.layout(&doc);
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &engine.schemas,
        fonts: Some(&engine.fonts),
    };
    let plain = PlainText.export(&doc, Some(&layout), &options).unwrap();
    assert_eq!(
        String::from_utf8(plain.bytes).unwrap(),
        "Before\n\nA red café image\n\nAfter"
    );
    assert_eq!(
        plain
            .losses
            .features
            .iter()
            .find(|l| l.feature == Feature::Assets)
            .unwrap()
            .disposition,
        Disposition::Dropped
    );
    let native = NativeWithAssets {
        assets: &engine.assets,
    }
    .export(&doc, Some(&layout), &options)
    .unwrap();
    assert_eq!(
        native
            .losses
            .features
            .iter()
            .find(|l| l.feature == Feature::Assets)
            .unwrap()
            .disposition,
        Disposition::Preserved
    );
    let decoded = NativeFragment::decode(&native.bytes).unwrap();
    assert!(
        decoded
            .resources
            .values()
            .any(|r| r.kind == reprise_clipboard::ResourceKind::Asset && r.bytes == RED)
    );
    let pdf = PdfWithAssets {
        assets: &engine.assets,
    }
    .export(&doc, Some(&layout), &options)
    .unwrap();
    assert!(pdf.bytes.starts_with(b"%PDF"));
    assert_eq!(
        pdf.losses
            .features
            .iter()
            .find(|l| l.feature == Feature::Assets)
            .unwrap()
            .disposition,
        Disposition::Preserved
    );
    let empty = AssetStore::default();
    let pdf = PdfWithAssets { assets: &empty }
        .export(&doc, Some(&layout), &options)
        .unwrap();
    assert_eq!(
        pdf.losses
            .features
            .iter()
            .find(|l| l.feature == Feature::Assets)
            .unwrap()
            .disposition,
        Disposition::Approximated
    );
    let native = NativeWithAssets { assets: &empty }
        .export(&doc, Some(&layout), &options)
        .unwrap();
    assert!(
        native
            .losses
            .notes
            .iter()
            .any(|n| n.code.as_str() == "clipboard.resource-missing")
    );
}

#[test]
fn missing_and_unreadable_images_still_survive_packages_and_clipboard() {
    for fixture in [
        reprise_fixtures::hostile::image_missing().unwrap(),
        reprise_fixtures::hostile::image_unreadable().unwrap(),
    ] {
        let doc = fixture.doc;
        let layout = fixture.engine.layout(&doc);
        let package = Package::new_with_resources(
            &doc,
            DocumentId([34; 16]),
            PersistenceMode::History,
            &layout,
            &fixture.engine.fonts,
            &fixture.engine.assets,
        )
        .unwrap();
        let opened = Package::open(
            &package.save().unwrap(),
            2,
            Limits::default(),
            &MigrationRegistry::builtin(),
        )
        .unwrap();
        assert_eq!(
            fixture.engine.layout(opened.editable_document().unwrap()),
            layout
        );
        let source_image = doc.blocks()[0];
        let fragment = copy_all(
            &doc,
            "source",
            &fixture.engine.schemas,
            Some(&layout),
            Some(&fixture.engine.fonts),
        )
        .unwrap();
        let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
        let pasted = fragment.paste(&mut editor, None, "target").unwrap();
        assert_eq!(
            editor
                .document()
                .image_record(pasted.ids.nodes[&source_image])
                .unwrap(),
            doc.image_record(source_image).unwrap()
        );
    }
}
