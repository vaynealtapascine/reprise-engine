use reprise_clipboard::{ExportOptions, Exporter, Html, ImportLimits, import_html};
use reprise_doc::{BlockKind, Document, NewBlock, SchemaRegistry};
use reprise_edit::Editor;
#[test]
fn orphan_authored_text_survives_html_roundtrip() {
    let d = Document::new(1).unwrap();
    d.append_block(BlockKind::Paragraph, "", "body").unwrap();
    d.create_story(&[NewBlock::new(
        BlockKind::Paragraph,
        "",
        "ORPHAN AUTHORED TEXT",
    )])
    .unwrap();
    d.commit();
    let schemas = SchemaRegistry::builtin();
    let out = Html
        .export(
            &d,
            None,
            &ExportOptions {
                source_namespace: "src",
                schemas: &schemas,
                fonts: None,
            },
        )
        .unwrap();
    let html = String::from_utf8(out.bytes).unwrap();
    let imported = import_html(&html, ImportLimits::default()).unwrap();
    let mut e = Editor::new(Document::new(2).unwrap(), schemas);
    imported.fragment.paste(&mut e, None, "dst").unwrap();
    eprintln!(
        "HTML contains orphan={}, stories after import={}, losses={:?}",
        html.contains("ORPHAN AUTHORED TEXT"),
        e.document().stories().len(),
        out.losses.notes
    );
    assert_eq!(
        e.document().stories().len(),
        1,
        "HTML lost authored orphan story"
    );
}
#[test]
fn orphan_template_is_retained_when_importing_html() {
    let imported=import_html("<html><body><p>body</p><template data-reprise-story=\"s0\"><p>ORPHAN AUTHORED TEXT</p></template></body></html>",ImportLimits::default()).unwrap();
    let mut e = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    imported.fragment.paste(&mut e, None, "dst").unwrap();
    assert_eq!(
        e.document().stories().len(),
        1,
        "HTML importer discarded an explicitly encoded unbound story"
    );
}
