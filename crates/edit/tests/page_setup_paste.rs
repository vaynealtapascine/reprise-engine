use reprise_doc::fragment::{CopyBlock, Fragment};
use reprise_doc::page_setup::PageSetupPatch;
use reprise_doc::{BlockKind, Document, SchemaRegistry, Style};
use reprise_edit::Editor;
use reprise_geom::Length;

fn paper() -> (Document, Fragment) {
    let doc = Document::new(1).unwrap();
    doc.define_style("body", &Style::default()).unwrap();
    doc.append_block(BlockKind::Paragraph, "body", "Native paper setup.")
        .unwrap();
    doc.set_page_setup_patch(PageSetupPatch {
        width: Some(Length::from_pt(612)),
        height: Some(Length::from_pt(792)),
        top: Some(Length::from_pt(36)),
        right: Some(Length::from_pt(48)),
        bottom: Some(Length::from_pt(54)),
        left: Some(Length::from_pt(72)),
    })
    .unwrap();
    doc.commit();
    let fragment = doc
        .copy_all_fragment("source", &SchemaRegistry::builtin())
        .unwrap();
    (doc, fragment)
}

#[test]
fn copy_all_preserves_page_setup_and_paste_undo_restores_the_empty_targets_page() {
    let (source, fragment) = paper();
    let target = Document::new(2).unwrap();
    target
        .set_page_setup_patch(PageSetupPatch {
            width: Some(Length::from_pt(500)),
            ..Default::default()
        })
        .unwrap();
    target.commit();
    let original = target.page_setup_patch().unwrap();
    let mut editor = Editor::new(target, SchemaRegistry::builtin());
    editor.paste(&fragment, None, "target").unwrap();
    assert_eq!(
        editor.document().page_setup_patch().unwrap(),
        source.page_setup_patch().unwrap()
    );
    assert_eq!(
        editor.document().raw_page_setup().unwrap(),
        fragment.page_setup_patch
    );
    assert_eq!(editor.undo_count(), 1);
    editor.undo().unwrap();
    assert!(editor.document().blocks().is_empty());
    assert_eq!(editor.document().page_setup_patch().unwrap(), original);
    editor.redo().unwrap();
    assert_eq!(
        editor.document().raw_page_setup().unwrap(),
        fragment.page_setup_patch
    );
    let copied = editor
        .document()
        .copy_all_fragment("source", &SchemaRegistry::builtin())
        .unwrap();
    assert_eq!(copied.page_setup_patch, fragment.page_setup_patch);
}

#[test]
fn partial_copies_omit_page_setup_and_nonempty_targets_keep_their_page() {
    let (source, fragment) = paper();
    let partial = source
        .copy_fragment(
            "source",
            &[CopyBlock {
                node: source.blocks()[0],
                bytes: None,
            }],
            &SchemaRegistry::builtin(),
        )
        .unwrap();
    assert!(partial.page_setup_patch.is_none());
    let encoded = serde_json::to_value(&partial).unwrap();
    assert!(encoded.get("page_setup_patch").is_none());
    let decoded: Fragment = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.page_setup_patch, None);

    let target = Document::new(2).unwrap();
    target.define_style("body", &Style::default()).unwrap();
    target
        .append_block(BlockKind::Paragraph, "body", "Existing content.")
        .unwrap();
    target
        .set_page_setup_patch(PageSetupPatch {
            width: Some(Length::from_pt(700)),
            ..Default::default()
        })
        .unwrap();
    target.commit();
    let original = target.raw_page_setup().unwrap();
    let mut editor = Editor::new(target, SchemaRegistry::builtin());
    editor.paste(&fragment, None, "target").unwrap();
    assert_eq!(editor.document().raw_page_setup().unwrap(), original);
}

#[test]
fn hostile_fragment_page_metadata_is_refused_without_staging_or_undo() {
    let (_, mut fragment) = paper();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    let revision = editor.document().revision();
    for raw in [
        "{",
        r#"{"version":2,"properties":{}}"#,
        r#"{"version":1,"properties":{"width":[]}}"#,
    ] {
        fragment.page_setup_patch = Some(raw.into());
        let error = editor.paste(&fragment, None, "target").unwrap_err();
        assert_eq!(error.note().code.as_str(), "clipboard.invalid");
        assert_eq!(error.note().severity, reprise_diag::Severity::Error);
        assert_eq!(editor.document().revision(), revision);
        assert!(editor.document().blocks().is_empty());
        assert!(!editor.document().has_page_setup());
        assert_eq!(editor.undo_count(), 0);
    }
}

#[test]
fn conflicting_raw_margins_and_an_emptied_history_root_survive_copy_all() {
    let source = Document::new(1).unwrap();
    source
        .set_page_setup_patch(PageSetupPatch {
            top: Some(Length::from_pt(200)),
            bottom: Some(Length::from_pt(150)),
            ..Default::default()
        })
        .unwrap();
    source.commit();
    let fragment = source
        .copy_all_fragment("source", &SchemaRegistry::builtin())
        .unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    editor.paste(&fragment, None, "target").unwrap();
    assert_eq!(
        editor.document().raw_page_setup().unwrap(),
        fragment.page_setup_patch
    );
    assert_eq!(editor.undo_count(), 1);
    editor.undo().unwrap();
    assert_eq!(
        editor.document().page_setup_patch().unwrap(),
        PageSetupPatch::default()
    );
    let empty = editor
        .document()
        .copy_all_fragment("source", &SchemaRegistry::builtin())
        .unwrap();
    assert!(empty.page_setup_patch.is_some());
    let mut again = Editor::new(Document::new(3).unwrap(), SchemaRegistry::builtin());
    again.paste(&empty, None, "target").unwrap();
    assert!(again.document().has_page_setup());
    assert_eq!(
        again.document().raw_page_setup().unwrap(),
        empty.page_setup_patch
    );
}
