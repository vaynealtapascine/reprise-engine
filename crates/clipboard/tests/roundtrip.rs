use reprise_clipboard::copy_all;
use reprise_doc::{Document, NodeId};
use reprise_edit::Editor;
use reprise_layout::LayoutSnapshot;

/// Rendered layout equality: all page/frame geometry, text, styles, bidi levels,
/// glyphs, line widths/baselines and composition explanations; identities and
/// history-based relation resolutions/diagnostics are deliberately separate.
fn equal_geometry(
    a: &LayoutSnapshot,
    b: &LayoutSnapshot,
    nodes: &std::collections::BTreeMap<NodeId, NodeId>,
    name: &str,
) {
    assert_eq!(a.pages, b.pages, "{name}: pages");
    assert_eq!(a.frames, b.frames, "{name}: frames");
    assert_eq!(a.blocks.len(), b.blocks.len(), "{name}: block count");
    for block in &a.blocks {
        let new = nodes.get(&block.node).unwrap();
        let pasted = b.blocks.iter().find(|b| b.node == *new).unwrap();
        let mut normal = pasted.clone();
        normal.node = block.node;
        assert_eq!(*block, normal, "{name}: text/glyph/line/style geometry");
    }
}

#[test]
fn every_hostile_fixture_roundtrips_live_structure_and_rendered_layout() {
    for fixture in reprise_fixtures::hostile::all().unwrap() {
        let before = fixture.engine.layout(&fixture.doc);
        if fixture.name == "collab_hostile_peer" {
            let error = copy_all(
                &fixture.doc,
                "source",
                &fixture.engine.schemas,
                Some(&before),
                Some(&fixture.engine.fonts),
            )
            .err()
            .expect("unreadable peer block must be refused");
            assert_eq!(error.note().code.as_str(), "clipboard.invalid");
            assert!(matches!(
                error,
                reprise_clipboard::ClipboardError::Fragment(
                    reprise_clipboard::FragmentError::Invalid(_)
                )
            ));
            continue;
        }
        let fragment = copy_all(
            &fixture.doc,
            "source",
            &fixture.engine.schemas,
            Some(&before),
            Some(&fixture.engine.fonts),
        )
        .unwrap();
        assert_eq!(
            fragment.encode().unwrap(),
            copy_all(
                &fixture.doc,
                "source",
                &fixture.engine.schemas,
                Some(&before),
                Some(&fixture.engine.fonts)
            )
            .unwrap()
            .encode()
            .unwrap(),
            "{}: deterministic bytes",
            fixture.name
        );
        let mut editor = Editor::new(Document::new(1001).unwrap(), fixture.engine.schemas.clone());
        let pasted = editor.paste(&fragment.fragment, None, "target").unwrap();
        let after = fixture.engine.layout(editor.document());
        equal_geometry(&before, &after, &pasted.ids.nodes, fixture.name);
        let again = editor
            .document()
            .copy_all_fragment("source", &fixture.engine.schemas)
            .unwrap();
        assert_eq!(
            fragment.fragment.blocks.len(),
            again.blocks.len(),
            "{}: topology",
            fixture.name
        );
        for (old, new) in fragment.fragment.blocks.iter().zip(&again.blocks) {
            assert_eq!(new.id, pasted.ids.nodes[&old.id]);
            assert_eq!(new.parent, old.parent.map(|p| pasted.ids.nodes[&p]));
            assert_eq!(old.kind, new.kind);
            assert_eq!(old.text, new.text);
            assert_eq!(old.style, new.style);
            assert_eq!(old.overrides, new.overrides);
            assert_eq!(old.table, new.table);
        }
        assert_eq!(fragment.fragment.styles, again.styles);
        assert_eq!(fragment.fragment.templates, again.templates);
        assert_eq!(fragment.fragment.template_choice, again.template_choice);
        for old in &fragment.fragment.ranges {
            let new = again
                .ranges
                .iter()
                .find(|r| r.id == pasted.ids.ranges[&old.id])
                .unwrap();
            assert_eq!(new.bytes, old.bytes);
            assert_eq!(new.policy, old.policy);
        }
        for old in &fragment.fragment.relations {
            if let Some(new_id) = pasted.relations.get(&old.id) {
                let new = again.relations.iter().find(|r| r.id == *new_id).unwrap();
                assert_eq!(
                    new.relation,
                    old.relation.remapped(&pasted.ids),
                    "{}: relation graph",
                    fixture.name
                );
            } else {
                assert!(
                    pasted
                        .notes
                        .iter()
                        .any(|n| n.code == "clipboard.relation-dropped"),
                    "{}: unreported edge loss",
                    fixture.name
                );
            }
        }
        if !fragment.fragment.blocks.is_empty() {
            assert_eq!(editor.undo_count(), 1, "{}: one undo", fixture.name);
            editor.undo().unwrap();
            assert!(editor.document().blocks().is_empty());
            editor.redo().unwrap();
            assert_eq!(
                editor.document().blocks(),
                fragment
                    .fragment
                    .blocks
                    .iter()
                    .filter(|b| b.parent.is_none())
                    .map(|b| pasted.ids.nodes[&b.id])
                    .collect::<Vec<_>>()
            );
        }
    }
}
