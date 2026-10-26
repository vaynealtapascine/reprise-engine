//! The native fragment carries what the stories of a sections envelope use: an
//! image in a header travels with its bytes, and is laid out in the header it
//! is pasted into (sections §14, S9).
use reprise_clipboard::{NativeFragment, copy_all};
use reprise_doc::image::ImageData;
use reprise_doc::sections::SectionId;
use reprise_doc::{BlockKind, Document, Domain, NewBlock, SchemaRegistry};
use reprise_edit::Editor;

const RED: &[u8] = include_bytes!("../../../fixtures/images/red-1x1.png");

#[test]
fn an_image_in_a_header_story_travels_with_its_bytes() {
    let mut engine = reprise_fixtures::engine();
    let hash = engine.assets.insert(RED).unwrap();
    let doc = Document::new(1).unwrap();
    doc.append_block(BlockKind::Paragraph, "", "Body").unwrap();
    let header = doc
        .create_story(&[NewBlock::new(BlockKind::Image, "", "logo")])
        .unwrap();
    let logo = doc.top(Domain::Story(header))[0];
    doc.set_image(logo, &ImageData::new(&hash)).unwrap();
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[(
            "region.header.default".to_owned(),
            serde_json::json!(header.to_string()),
        )]
        .into(),
        &[],
    )
    .unwrap();
    doc.commit();
    let layout = engine.layout(&doc);
    let mut fragment = copy_all(
        &doc,
        "source",
        &engine.schemas,
        Some(&layout),
        Some(&engine.fonts),
    )
    .unwrap();
    assert!(
        fragment.fragment.blocks.iter().all(|b| b.image.is_none()),
        "the image is in the story, not the body"
    );
    fragment.attach_images(&engine.assets).unwrap();
    assert!(
        fragment.resources.values().any(|r| r.bytes == RED),
        "the story's image bytes are bundled"
    );
    // The fonts the header's running instance was set in are bundled too: a
    // fragment of a document with a text header names its faces.
    let fragment = NativeFragment::decode(&fragment.encode().unwrap()).unwrap();

    let mut target = reprise_fixtures::engine();
    fragment.install_assets(&mut target.assets).unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    let pasted = fragment.paste(&mut editor, None, "target").unwrap();
    assert_eq!(pasted.stories.len(), 1);
    let placed = target.layout(editor.document());
    let running = &placed.sections.as_ref().unwrap().running;
    let image = running
        .iter()
        .find(|r| r.slot == "header")
        .and_then(|r| r.blocks.first())
        .and_then(|b| b.image.as_ref())
        .expect("the header shows its image");
    assert!(!image.placeholder);
    assert_eq!(image.alt, "logo");
}

#[test]
fn the_fonts_of_a_header_story_are_named_by_the_fragment() {
    use reprise_font::{Face, FontStore};
    // A header in a family the body doesn't use: only the story needs its face.
    let sans = Face::from_bytes(
        include_bytes!("../../../fixtures/fonts/SourceSans3-Regular.otf").as_slice(),
    )
    .unwrap();
    let family = sans.id().family.clone();
    let mut fonts = FontStore::default();
    fonts.add(
        Face::from_bytes(
            include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
        )
        .unwrap(),
    );
    fonts.add(sans);
    let engine = reprise_layout::Engine::new(fonts);
    let doc = Document::new(1).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    doc.define_style(
        "head",
        &reprise_doc::Style {
            family: Some(family.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    doc.append_block(BlockKind::Paragraph, "body", "Body")
        .unwrap();
    let header = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "head", "Running head")])
        .unwrap();
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[(
            "region.header.default".to_owned(),
            serde_json::json!(header.to_string()),
        )]
        .into(),
        &[],
    )
    .unwrap();
    doc.commit();
    let layout = engine.layout(&doc);
    let in_header: std::collections::BTreeSet<_> = layout
        .sections
        .iter()
        .flat_map(|s| &s.running)
        .flat_map(|r| r.blocks.iter())
        .flat_map(|b| b.lines.iter().flat_map(|l| &l.runs))
        .map(|run| run.face.clone())
        .collect();
    assert_eq!(in_header.len(), 1);
    assert!(
        in_header.iter().all(|f| f.family == family),
        "{in_header:?}"
    );
    // With a layout the running instance names the face; without one the
    // story's own styles do.
    for layout in [Some(&layout), None] {
        let fragment =
            copy_all(&doc, "source", &engine.schemas, layout, Some(&engine.fonts)).unwrap();
        let bundled: std::collections::BTreeSet<_> = fragment
            .resources
            .values()
            .filter_map(|r| match &r.kind {
                reprise_clipboard::ResourceKind::Font { pin, .. } => Some(pin.clone()),
                _ => None,
            })
            .collect();
        for face in &in_header {
            assert!(
                bundled.contains(face),
                "{face:?} is not bundled (layout given: {})",
                layout.is_some()
            );
        }
    }
}

#[test]
fn a_malformed_envelope_is_refused_when_the_bytes_are_decoded() {
    let engine = reprise_fixtures::engine();
    let doc = Document::new(1).unwrap();
    doc.append_block(BlockKind::Paragraph, "", "Body").unwrap();
    let header = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "", "Head")])
        .unwrap();
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[(
            "region.header.default".to_owned(),
            serde_json::json!(header.to_string()),
        )]
        .into(),
        &[],
    )
    .unwrap();
    doc.commit();
    let good = copy_all(&doc, "source", &engine.schemas, None, None).unwrap();
    assert!(good.fragment.sections.is_some());
    NativeFragment::decode(&good.encode().unwrap()).unwrap();
    for (envelope, code) in [
        (r#"{"v":2,"mode":"partial"}"#, "clipboard.version"),
        (
            r#"{"v":1,"mode":"partial","surprise":1}"#,
            "clipboard.invalid",
        ),
        (
            r#"{"v":1,"mode":"complete","stories":[{"label":"a","source":"1@1","blocks":[]},{"label":"a","source":"2@1","blocks":[]}]}"#,
            "clipboard.invalid",
        ),
        ("{", "clipboard.invalid"),
    ] {
        let mut bad = good.clone();
        bad.fragment.sections = Some(envelope.to_owned());
        let bytes = serde_json::to_vec(&bad).unwrap();
        let error = NativeFragment::decode(&bytes).err().expect(envelope);
        assert_eq!(error.note().code.as_str(), code, "{envelope}");
    }
}
