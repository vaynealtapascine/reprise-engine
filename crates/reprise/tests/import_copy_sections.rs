//! Import as a copy keeps the sections of a document: its boundaries and their
//! bindings, every story (orphans included), `first`, the geometry mode and what
//! the stories hold (sections §14, S9).
use reprise::*;
use reprise_doc::sections::{BreakKind, SectionId};
use reprise_doc::{BlockKind, Document, Domain, NewBlock};
use serde_json::json;

const COPY: &str = "ffeeddccbbaa99887766554433221100";

fn workspace() -> Workspace {
    Workspace::new()
}

fn import(bytes: &[u8]) -> DocumentSession {
    workspace()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "2".into(),
            }),
            bytes,
        )
        .unwrap()
}

/// What a package holds, read back as a document.
fn reopened(bytes: &[u8]) -> (reprise_format::Package, Document) {
    let opened = reprise_format::Package::open(
        bytes,
        9,
        reprise_format::Limits::default(),
        &reprise_format::MigrationRegistry::builtin(),
    )
    .unwrap();
    let package = opened.package().clone();
    (package, opened.into_document().unwrap())
}

struct Source {
    bytes: Vec<u8>,
    header: reprise_doc::NodeId,
    footer: reprise_doc::NodeId,
    orphan: reprise_doc::NodeId,
    image: String,
}

/// A document with a header (holding an image) bound by `first`, a footer bound
/// by a section, an orphan story, an inactive page break, a start on a deleted
/// block and a pageless geometry mode.
fn source() -> Source {
    let mut store = reprise_display::AssetStore::default();
    let image = store.insert(b"logo bytes".as_slice()).unwrap();
    let doc = Document::new(1).unwrap();
    let header = doc
        .create_story(&[
            NewBlock::new(BlockKind::Paragraph, "", "Running head"),
            NewBlock::new(BlockKind::Image, "", "logo"),
        ])
        .unwrap();
    let logo = doc.top(Domain::Story(header))[1];
    doc.set_image(logo, &reprise_doc::image::ImageData::new(&image))
        .unwrap();
    let footer = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "", "Footer")])
        .unwrap();
    let orphan = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "", "Nobody binds me")])
        .unwrap();
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[
            ("region.header.default".into(), json!(header.to_string())),
            ("columns.count".into(), json!(2)),
            ("unknown.future".into(), json!("retained")),
        ]
        .into(),
        &[],
    )
    .unwrap();
    let p: Vec<_> = (0..4)
        .map(|i| {
            doc.append_block(BlockKind::Paragraph, "", &format!("p{i}"))
                .unwrap()
        })
        .collect();
    let start = |node, kind, entries: &[(&str, serde_json::Value)]| {
        let life = doc.mint_lifetime(node).unwrap();
        let props = entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        doc.activate_start(node, &life, kind, &props).unwrap();
    };
    start(
        p[1],
        BreakKind::Section,
        &[("region.footer.default", json!(footer.to_string()))],
    );
    start(p[2], BreakKind::Page, &[("future.key", json!("old"))]);
    doc.deactivate_start(p[2]).unwrap();
    let dead = doc.append_block(BlockKind::Paragraph, "", "gone").unwrap();
    start(dead, BreakKind::Section, &[]);
    doc.delete_block(dead).unwrap();
    doc.set_geometry_mode("pageless").unwrap();
    doc.commit();
    let mut package = reprise_format::Package::new(
        &doc,
        reprise_format::DocumentId([1; 16]),
        reprise_doc::PersistenceMode::History,
    )
    .unwrap();
    assert!(
        package
            .embed_document_images(&doc, &store)
            .unwrap()
            .is_empty()
    );
    Source {
        bytes: package.save().unwrap(),
        header,
        footer,
        orphan,
        image,
    }
}

#[test]
fn a_copy_keeps_sections_stories_orphans_first_and_the_geometry_mode() {
    let source = source();
    let mut copy = import(&source.bytes);
    let state = copy.state().unwrap().data;
    // Every story, the orphan too, which is still bound by nothing.
    assert_eq!(state.stories.len(), 3);
    let text = |story: &StoryState| story.blocks.first().map(|b| b.text.clone());
    let by_text = |wanted: &str| {
        state
            .stories
            .iter()
            .find(|s| text(s).as_deref() == Some(wanted))
            .unwrap()
    };
    assert!(
        [source.header, source.footer, source.orphan]
            .iter()
            .all(|id| !state.stories.iter().any(|s| s.id == id.to_string())),
        "new stories, new identities"
    );
    assert_eq!(by_text("Running head").bound_by.len(), 2);
    assert_eq!(by_text("Footer").bound_by.len(), 1);
    assert!(by_text("Nobody binds me").bound_by.is_empty());
    // `first` keeps its keys, unknown ones included; the boundary keeps its own.
    assert_eq!(state.sections.len(), 2);
    assert_eq!(state.sections[0].own["raw"]["columns.count"], "2");
    assert_eq!(state.sections[0].unknown_keys, ["unknown.future"]);
    assert!(state.blocks[1].starts.is_some());

    // The rest is read from the saved copy: the inactive record is inactive,
    // the record on the dead block is gone, the mode is kept, and the image in
    // the header travelled with its bytes.
    let bytes = copy.save().unwrap().data.bytes;
    let (package, doc) = reopened(&bytes);
    let records = doc.start_records();
    let body = doc.top(Domain::Body);
    let inactive = records
        .iter()
        .find(|r| r.id == SectionId::Block(body[2]))
        .unwrap();
    assert!(!inactive.active);
    assert_eq!(inactive.own["future.key"], json!("old"));
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r.id, SectionId::Block(_)))
            .count(),
        2,
        "the record on a deleted block was not carried"
    );
    assert_eq!(
        doc.geometry_mode_record().unwrap()["mode"],
        json!("pageless")
    );
    let header = by_text("Running head");
    let logo = doc.top(Domain::Story(
        reprise_doc::NodeId::parse(&header.id).unwrap(),
    ))[1];
    let asset = doc.image(logo).unwrap().asset;
    assert_eq!(asset, source.image);
    let assets = package.assets().unwrap();
    assert!(
        assets
            .bundled
            .values()
            .any(|b| b.as_slice() == b"logo bytes"),
        "the header image is packaged"
    );
    assert!(assets.missing.is_empty());
}

#[test]
fn a_copy_is_independent_of_the_source_stories() {
    let source = source();
    let mut copy = import(&source.bytes);
    let before = copy.state().unwrap().data;
    let header = before
        .stories
        .iter()
        .find(|s| s.blocks.first().map(|b| b.text.as_str()) == Some("Running head"))
        .unwrap();
    // Editing the copy's header edits the copy only, and keeps the binding.
    let node = header.blocks[0].id.clone();
    copy.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertText {
            node,
            at: 0,
            text: ">> ".into(),
        }],
    }))
    .unwrap();
    let after = copy.state().unwrap().data;
    let edited = after.stories.iter().find(|s| s.id == header.id).unwrap();
    assert_eq!(edited.blocks[0].text, ">> Running head");
    assert_eq!(edited.bound_by.len(), 2);
}
