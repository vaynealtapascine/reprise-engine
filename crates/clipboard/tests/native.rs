use reprise_clipboard::{CopyBlock, NativeFragment, copy_all, copy_blocks, copy_selection};
use reprise_doc::fragment::{FragmentError, MAX_FRAGMENT_DEPTH};
use reprise_doc::relation::{CopyCrossing, CopyInside};
use reprise_doc::text::RangePolicy;
use reprise_doc::{BlockKind, Document, LengthExpr, Relation, SchemaRegistry, Style, Target};
use reprise_edit::{Caret, Command, Editor, Selection, Transaction};
use reprise_geom::Length;

fn doc(peer: u64, text: &str) -> Document {
    let doc = Document::new(peer).unwrap();
    doc.append_block(BlockKind::Paragraph, "", text).unwrap();
    doc.commit();
    doc
}
fn copy(doc: &Document) -> NativeFragment {
    copy_all(doc, "source", &SchemaRegistry::builtin(), None, None).unwrap()
}

#[test]
fn partial_unicode_ranges_and_deterministic_encoding() {
    let doc = doc(1, "office e\u{301} אבג end");
    let node = doc.blocks()[0];
    let start = "off".len();
    let end = "office e\u{301} אב".len();
    let range = doc.add_range(node, 1..end, RangePolicy::EXPANDING).unwrap();
    let selection = [CopyBlock {
        node,
        bytes: Some(start..end),
    }];
    let a = copy_blocks(
        &doc,
        "source",
        &selection,
        &SchemaRegistry::builtin(),
        None,
        None,
    )
    .unwrap();
    let b = copy_blocks(
        &doc,
        "source",
        &selection,
        &SchemaRegistry::builtin(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(a.encode().unwrap(), b.encode().unwrap());
    assert_eq!(a.fragment.blocks[0].text, "ice e\u{301} אב");
    assert_eq!(a.fragment.ranges[0].id, range);
    assert_eq!(a.fragment.ranges[0].bytes, 0..end - start);
    assert_eq!(a.fragment.ranges[0].policy, RangePolicy::EXPANDING);
    assert!(
        copy_blocks(
            &doc,
            "source",
            &[CopyBlock {
                node,
                bytes: Some(end - 1..end)
            }],
            &SchemaRegistry::builtin(),
            None,
            None
        )
        .is_err()
    );
    assert_eq!(
        NativeFragment::decode(&a.encode().unwrap())
            .unwrap()
            .encode()
            .unwrap(),
        a.encode().unwrap()
    );
    // Explicit byte ranges can cut inside a combining cluster and a ligature.
    let a = copy_blocks(
        &doc,
        "source",
        &[CopyBlock {
            node,
            bytes: Some(8..10),
        }],
        &SchemaRegistry::builtin(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(a.fragment.blocks[0].text, "\u{301}");
}

#[test]
fn selection_uses_kernel_reading_order_and_rejects_stale_layout() {
    let doc = doc(1, "office אבג e\u{301}");
    let node = doc.blocks()[0];
    let engine = reprise_fixtures::engine();
    let snapshot = engine.layout(&doc);
    let selection = Selection {
        anchor: Caret::new(node, 3),
        focus: Caret::new(node, 6),
    };
    let copied =
        copy_selection(&doc, "source", &selection, &snapshot, &engine.schemas, None).unwrap();
    assert_eq!(copied.fragment.blocks[0].text, "ice");
    doc.block(node).unwrap().text.insert(0, "x").unwrap();
    doc.commit();
    assert!(copy_selection(&doc, "source", &selection, &snapshot, &engine.schemas, None).is_err());
}

#[test]
fn paste_twice_has_new_ids_single_undo_and_stable_redo() {
    let source = doc(1, "one");
    let first = source.blocks()[0];
    let second = source
        .append_block(BlockKind::Paragraph, "", "two")
        .unwrap();
    source.add_range(first, 0..3, RangePolicy::FIXED).unwrap();
    source
        .add_relation(
            &SchemaRegistry::builtin(),
            &Relation::new(reprise_doc::relation::builtin::REFERENCE)
                .owned_by(first)
                .target("to", Target::Node(second)),
        )
        .unwrap();
    let fragment = copy(&source);
    let target = doc(2, "ABCD");
    let host = target.blocks()[0];
    let mut editor = Editor::new(target, SchemaRegistry::builtin());
    let a = editor
        .paste(&fragment.fragment, Some((host, 2)), "target")
        .unwrap();
    let texts: Vec<_> = editor
        .document()
        .blocks()
        .into_iter()
        .map(|n| editor.document().block(n).unwrap().text.to_string())
        .collect();
    assert_eq!(texts, ["ABone", "twoCD"]);
    assert_eq!(editor.undo_count(), 1);
    let range = *a.ids.ranges.values().next().unwrap();
    assert!(
        matches!(editor.document().resolve_range(range), reprise_doc::RangeState::Valid { bytes, .. } if bytes == (2..5))
    );
    assert!(editor.undo().unwrap());
    assert_eq!(editor.document().blocks(), [host]);
    assert!(matches!(
        editor.document().resolve_range(range),
        reprise_doc::RangeState::Missing { .. }
    ));
    assert!(editor.document().relations().is_empty());
    assert!(editor.redo().unwrap());
    assert_eq!(editor.document().blocks(), a.applied.blocks);
    let b = editor.paste(&fragment.fragment, None, "target").unwrap();
    assert!(
        a.ids
            .nodes
            .values()
            .all(|n| !b.ids.nodes.values().any(|b| b == n))
    );
    assert!(
        a.ids
            .ranges
            .values()
            .all(|r| !b.ids.ranges.values().any(|b| b == r))
    );
    assert!(
        a.relations
            .values()
            .all(|r| !b.relations.values().any(|b| b == r))
    );
    assert_eq!(editor.undo_count(), 2);
}

#[test]
fn cross_document_same_peer_never_reuses_source_ids_or_external_targets() {
    let source = doc(1, "owner");
    let node = source.blocks()[0];
    let outside = source
        .append_block(BlockKind::Paragraph, "", "outside")
        .unwrap();
    source.add_range(node, 0..5, RangePolicy::POINT).unwrap();
    source
        .add_relation(
            &SchemaRegistry::builtin(),
            &Relation::new(reprise_doc::relation::builtin::REFERENCE)
                .owned_by(node)
                .target("to", Target::Node(outside)),
        )
        .unwrap();
    let partial = copy_blocks(
        &source,
        "source",
        &[CopyBlock { node, bytes: None }],
        &SchemaRegistry::builtin(),
        None,
        None,
    )
    .unwrap();
    let target = Document::new(1).unwrap();
    // Colliding source labels exist in an unrelated target document.
    let mut editor = Editor::new(target, SchemaRegistry::builtin());
    let a = editor.paste(&partial.fragment, None, "target").unwrap();
    assert_ne!(a.ids.nodes[&node], node);
    for (old, new) in &a.ids.ranges {
        assert_ne!(old, new);
    }
    assert!(a.relations.is_empty());
    assert_eq!(a.notes[0].code, "clipboard.relation-dropped");
    let all = copy(&source);
    let mut empty = Editor::new(Document::new(1).unwrap(), SchemaRegistry::builtin());
    let pasted = empty.paste(&all.fragment, None, "target").unwrap();
    for (old, new) in &pasted.ids.nodes {
        assert_ne!(old, new);
    }
    for (old, new) in &pasted.ids.ranges {
        assert_ne!(old, new);
    }
    for (old, new) in &pasted.relations {
        assert_ne!(old, new);
    }
}

#[test]
fn copy_policies_inside_crossing_and_drop_are_rechecked_at_paste() {
    let source = doc(1, "owner");
    let owner = source.blocks()[0];
    let other = source
        .append_block(BlockKind::Paragraph, "", "other")
        .unwrap();
    let schemas = SchemaRegistry::builtin();
    source
        .add_relation(
            &schemas,
            &Relation::new(reprise_doc::relation::builtin::REFERENCE)
                .owned_by(owner)
                .target("to", Target::Node(other)),
        )
        .unwrap();
    let partial = copy_blocks(
        &source,
        "source",
        &[CopyBlock {
            node: owner,
            bytes: None,
        }],
        &schemas,
        None,
        None,
    )
    .unwrap();
    let mut editor = Editor::new(source.fork(2).unwrap(), schemas);
    let kept = editor.paste(&partial.fragment, None, "source").unwrap();
    assert_eq!(
        editor
            .document()
            .relations()
            .last()
            .unwrap()
            .1
            .as_ref()
            .unwrap()
            .first("to"),
        Some(&Target::Node(other))
    );
    assert_eq!(kept.relations.len(), 1);
    let mut drop_schema = reprise_doc::relation::builtin::reference();
    drop_schema.on_copy.inside = CopyInside::Drop;
    drop_schema.on_copy.crossing = CopyCrossing::Drop;
    let mut schemas = SchemaRegistry::default();
    schemas.register(drop_schema).unwrap();
    let dropped = copy_blocks(
        editor.document(),
        "source",
        &[CopyBlock {
            node: owner,
            bytes: None,
        }],
        &schemas,
        None,
        None,
    )
    .unwrap();
    assert!(dropped.fragment.relations.is_empty());
    assert_eq!(dropped.notes[0].code, "clipboard.relation-dropped");
    let mut target = Editor::new(Document::new(3).unwrap(), schemas);
    let malicious = copy(&source);
    let result = target.paste(&malicious.fragment, None, "target").unwrap();
    assert!(result.relations.is_empty());
}

#[test]
fn styles_keep_inheritance_and_undo_as_one_step() {
    let source = Document::new(1).unwrap();
    source
        .append_block(BlockKind::Paragraph, "child", "content")
        .unwrap();
    source
        .define_style(
            "parent",
            &Style {
                size: Some(LengthExpr::Pt(Length::MIN)),
                ..Style::default()
            },
        )
        .unwrap();
    source
        .define_style(
            "child",
            &Style {
                parent: Some("parent".into()),
                line_height: Some(LengthExpr::Pt(Length::MAX)),
                ..Style::default()
            },
        )
        .unwrap();
    let fragment = copy(&source);
    let target = Document::new(2).unwrap();
    target
        .define_style(
            "parent",
            &Style {
                size: Some(LengthExpr::Pt(Length::ZERO)),
                ..Style::default()
            },
        )
        .unwrap();
    target.commit();
    let mut editor = Editor::new(target, SchemaRegistry::builtin());
    let result = editor.paste(&fragment.fragment, None, "target").unwrap();
    assert_eq!(result.notes[0].code, "clipboard.style-clash");
    let node = result.applied.blocks[0];
    assert_eq!(
        editor.document().computed_style(node).unwrap().line_height,
        Length::MAX
    );
    assert_eq!(
        editor.document().style("parent").unwrap().size,
        Some(LengthExpr::Pt(Length::ZERO))
    );
    editor.undo().unwrap();
    assert!(editor.document().style("child (paste 1)").is_none());
    editor.redo().unwrap();
    assert!(editor.document().style("child (paste 1)").is_some());
}

#[test]
fn concurrent_pastes_and_undo_merge_converge() {
    let source = doc(10, "paste");
    let fragment = copy(&source);
    let target = doc(1, "base");
    let peer = target.fork(2).unwrap();
    let mut a = Editor::new(target, SchemaRegistry::builtin());
    let mut b = Editor::new(peer, SchemaRegistry::builtin());
    let a_ids = a.paste(&fragment.fragment, None, "target").unwrap();
    let b_ids = b.paste(&fragment.fragment, None, "target").unwrap();
    assert_ne!(a_ids.applied.blocks, b_ids.applied.blocks);
    a.merge(b.document()).unwrap();
    b.merge(a.document()).unwrap();
    assert_eq!(a.document().blocks(), b.document().blocks());
    assert_eq!(a.document().revision(), b.document().revision());
    a.undo().unwrap();
    b.merge(a.document()).unwrap();
    a.merge(b.document()).unwrap();
    assert!(a.document().is_live(b_ids.applied.blocks[0]));
    assert!(!a.document().is_live(a_ids.applied.blocks[0]));
    a.redo().unwrap();
    b.merge(a.document()).unwrap();
    a.merge(b.document()).unwrap();
    assert_eq!(a.document().blocks(), b.document().blocks());
}

#[test]
fn invalid_future_deep_deleted_and_mixed_paste_leave_revision_unchanged() {
    let source = doc(1, "x");
    let mut fragment = copy(&source).fragment;
    let target = doc(2, "y");
    let node = target.blocks()[0];
    target.delete_block(node).unwrap();
    target.commit();
    let mut editor = Editor::new(target, SchemaRegistry::builtin());
    let before = editor.document().revision();
    assert!(editor.paste(&fragment, Some((node, 0)), "target").is_err());
    fragment.version = 99;
    assert!(matches!(
        editor.paste(&fragment, None, "target").unwrap_err().reason,
        reprise_edit::Reason::Fragment(FragmentError::Version(99))
    ));
    assert_eq!(before, editor.document().revision());
    fragment.version = 1;
    let mut parent = fragment.blocks[0].id;
    for i in 1..MAX_FRAGMENT_DEPTH + 3 {
        let mut block = fragment.blocks[0].clone();
        block.id = reprise_doc::NodeId::parse(&format!("{i}@55")).unwrap();
        block.parent = Some(parent);
        parent = block.id;
        fragment.blocks.push(block);
    }
    assert!(matches!(
        fragment.validate(),
        Err(FragmentError::Limit("tree depth"))
    ));
    let error = editor
        .apply(
            &Transaction::new()
                .with(Command::Paste {
                    fragment: Box::new(copy(&source).fragment),
                    at: None,
                    target_namespace: "target".into(),
                })
                .with(Command::DeleteBlock { node }),
        )
        .unwrap_err();
    assert_eq!(error.reason, reprise_edit::Reason::MixedPaste);
    assert_eq!(before, editor.document().revision());
    let mut fragment = copy(&source).fragment;
    fragment.blocks[0].text = "x".repeat(17 << 20);
    assert!(matches!(fragment.validate(), Err(FragmentError::Limit(_))));
}
