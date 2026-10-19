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
        // A pasted object is a new object: it gets a new identity.
        for (new, old) in normal.lines.iter_mut().zip(&block.lines) {
            for (new, old) in new.atoms.iter_mut().zip(&old.atoms) {
                new.id = old.id;
            }
        }
        assert_eq!(*block, normal, "{name}: text/glyph/line/style geometry");
    }
}

/// What a sections envelope says, with IDs replaced by places so two copies of
/// one document, made under different IDs, can be compared: a boundary's block
/// by its position among the copied blocks, a story by its label and the kind,
/// style and text of its blocks.
fn envelope_shape(fragment: &reprise_doc::fragment::Fragment) -> Option<serde_json::Value> {
    let mut value: serde_json::Value = serde_json::from_str(fragment.sections.as_deref()?).unwrap();
    let places: std::collections::BTreeMap<String, usize> = fragment
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id.to_string(), i))
        .collect();
    for boundary in value["boundaries"].as_array_mut().into_iter().flatten() {
        let id = boundary["block"].as_str().unwrap().to_owned();
        boundary["block"] = serde_json::json!(places[&id]);
    }
    for story in value["stories"].as_array_mut().into_iter().flatten() {
        let object = story.as_object_mut().unwrap();
        object.remove("source");
        object.remove("ranges");
        for block in object["blocks"].as_array_mut().into_iter().flatten() {
            *block = serde_json::json!({
                "kind": block["kind"], "style": block["style"], "text": block["text"],
            });
        }
    }
    Some(value)
}

/// The sections a layout reports, apart from the identities of the blocks and
/// stories under them: each page's kind, counter and label, and each running
/// instance's place, variant, clipping and text.
fn sections_shape(snapshot: &LayoutSnapshot) -> Option<String> {
    let sections = snapshot.sections.as_ref()?;
    let pages: Vec<_> = sections
        .pages
        .iter()
        .map(|p| (p.kind, p.counter, p.label.clone(), p.bands.len()))
        .collect();
    let running: Vec<_> = sections
        .running
        .iter()
        .map(|r| {
            let text: Vec<_> = r
                .blocks
                .iter()
                .flat_map(|b| {
                    b.lines
                        .iter()
                        .map(move |l| b.text[l.text.clone()].to_owned())
                })
                .collect();
            (r.page, r.slot.clone(), r.variant.clone(), r.clipped, text)
        })
        .collect();
    Some(format!("{pages:?} {running:?} {}", sections.rules.len()))
}

#[test]
fn every_hostile_fixture_roundtrips_live_structure_and_rendered_layout() {
    let mut with_sections = Vec::new();
    for fixture in reprise_fixtures::hostile::all().unwrap() {
        let before = fixture.engine.layout(&fixture.doc);
        // A block or an object a hostile peer left unreadable is refused whole.
        if fixture.name == "collab_hostile_peer" || fixture.name == "inline_storm" {
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
        assert_eq!(
            sections_shape(&before),
            sections_shape(&after),
            "{}: the sections a layout reports",
            fixture.name
        );
        if fragment.fragment.sections.is_some() {
            with_sections.push(fixture.name);
        }
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
            assert_eq!(old.inline, new.inline, "{}: inline objects", fixture.name);
            assert_eq!(old.style, new.style);
            assert_eq!(old.overrides, new.overrides);
            assert_eq!(old.table, new.table);
        }
        assert_eq!(
            envelope_shape(&fragment.fragment),
            envelope_shape(&again),
            "{}: sections envelope",
            fixture.name
        );
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
            assert!(editor.document().top(reprise_doc::Domain::Body).is_empty());
            editor.redo().unwrap();
            assert_eq!(
                editor.document().top(reprise_doc::Domain::Body),
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
    // The envelope was really exercised: the fixtures that bind stories, hold
    // boundaries or a geometry mode all carry one.
    for name in [
        "sections_geometry_hostile",
        "sections_bands_hostile",
        "sections_regions_hostile",
        "sections_retained_hostile",
    ] {
        assert!(with_sections.contains(&name), "{name}: {with_sections:?}");
    }
}
