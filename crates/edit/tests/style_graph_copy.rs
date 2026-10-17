use reprise_doc::{BlockKind, Document, PersistenceMode, SchemaRegistry, Style};
use reprise_edit::Editor;

#[test]
fn document_copy_refusals_preserve_history_revision_and_undo() {
    let source = Document::new(1).unwrap();
    source
        .define_named_style("Parent", &Style::default())
        .unwrap();
    source
        .append_block(BlockKind::Paragraph, "Parent", "source")
        .unwrap();
    source.commit();
    let schemas = SchemaRegistry::builtin();
    let fragment = source.copy_document_fragment("source", &schemas).unwrap();
    let graph = source.authored_style_graph().unwrap();
    for case in 0..3 {
        let target = Document::new(2).unwrap();
        match case {
            0 => {}
            1 => {
                target.define_style("existing", &Style::default()).unwrap();
            }
            _ => {
                target
                    .append_block(BlockKind::Paragraph, "", "existing")
                    .unwrap();
            }
        }
        target.commit();
        let mut editor = Editor::new(target, schemas.clone());
        let mut bad = fragment.clone();
        if case == 0 {
            bad.source.clear();
        }
        let frozen = |editor: &Editor| {
            (
                editor
                    .document()
                    .try_export(PersistenceMode::History)
                    .unwrap(),
                editor.document().revision(),
                editor.undo_count(),
                editor.redo_count(),
            )
        };
        let before = frozen(&editor);
        assert!(
            editor
                .paste_document_fragment(&bad, "target", &graph)
                .is_err()
        );
        assert_eq!(before, frozen(&editor));
    }
}
