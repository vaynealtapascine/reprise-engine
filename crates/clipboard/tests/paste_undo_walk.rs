//! Orchestrator review: many pastes, at the end and into the middle of
//! paragraphs (which splits and joins), of a fragment carrying relations
//! and ranges, then every one undone and redone. Each undo must restore
//! exactly the document before that paste, and each redo exactly the state
//! after it, with the same block IDs. The fragment also survives an encode
//! and decode round trip first, as it would through the system clipboard.

use reprise_clipboard::{NativeFragment, copy_all};
use reprise_doc::{Document, NodeId, SchemaRegistry};
use reprise_edit::Editor;
use reprise_fixtures::{OTHER_PEER, engine, spike};

fn state(doc: &Document) -> Vec<(NodeId, String)> {
    doc.blocks()
        .into_iter()
        .map(|n| (n, doc.block(n).unwrap().text.to_string()))
        .collect()
}

#[test]
fn twenty_pastes_undo_and_redo_exactly() {
    let source = spike::document().unwrap().doc;
    let schemas = SchemaRegistry::builtin();
    let engine = engine();
    let layout = engine.layout(&source);
    let copied = copy_all(&source, "source", &schemas, Some(&layout), None).unwrap();
    let fragment = NativeFragment::decode(&copied.encode().unwrap()).unwrap();
    assert_eq!(
        fragment.encode().unwrap(),
        copied.encode().unwrap(),
        "stable bytes"
    );

    let target = Document::new(OTHER_PEER).unwrap();
    spike::define_styles(&target).unwrap();
    target.commit();
    let mut editor = Editor::new(target, schemas);
    let mut states = vec![state(editor.document())];
    for i in 0..20 {
        let doc = editor.document();
        let at = if i % 2 == 1 {
            doc.blocks().first().map(|&n| {
                let text = doc.block(n).unwrap().text.to_string();
                let mid = text
                    .char_indices()
                    .map(|(b, _)| b)
                    .nth(text.chars().count() / 2)
                    .unwrap_or(text.len());
                (n, mid)
            })
        } else {
            None
        };
        fragment.paste(&mut editor, at, "target").unwrap();
        editor.document().commit();
        states.push(state(editor.document()));
        // Layout stays total on the growing document.
        let _ = engine.layout(editor.document());
    }
    for i in (0..20).rev() {
        assert!(editor.undo().unwrap(), "undo {i}");
        assert_eq!(
            state(editor.document()),
            states[i],
            "after undoing paste {i}"
        );
    }
    for i in 1..=20 {
        assert!(editor.redo().unwrap(), "redo {i}");
        assert_eq!(
            state(editor.document()),
            states[i],
            "after redoing paste {i}"
        );
    }
}
