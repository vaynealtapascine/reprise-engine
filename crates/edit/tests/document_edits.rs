//! General document edit primitives used by kernel commands.

use reprise_doc::{BlockKind, Document, NewBlock, NodeId};

fn para(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "body", text)
        .unwrap()
}
fn texts(doc: &Document) -> Vec<String> {
    doc.blocks()
        .into_iter()
        .map(|n| doc.block(n).unwrap().text.to_string())
        .collect()
}
fn exchange(a: &Document, b: &Document) {
    a.merge(b).unwrap();
    b.merge(a).unwrap();
}

#[test]
fn moving_blocks_counts_live_siblings_and_refuses_cycles() {
    let doc = Document::new(1).unwrap();
    let (a, b, c) = (para(&doc, "a"), para(&doc, "b"), para(&doc, "c"));
    let gone = para(&doc, "gone");
    doc.delete_block(gone).unwrap();
    doc.move_block(a, None, 2).unwrap();
    assert_eq!(doc.blocks(), [b, c, a]);
    doc.move_block(a, None, 0).unwrap();
    assert_eq!(doc.blocks(), [a, b, c]);
    assert!(doc.move_block(a, None, 3).is_err(), "past the end");
    doc.move_block(c, Some(a), 0).unwrap();
    assert_eq!(doc.blocks(), [a, b]);
    assert_eq!(doc.children(Some(a)), [c]);
    assert!(doc.move_block(a, Some(a), 0).is_err());
    assert!(
        doc.move_block(a, Some(c), 0).is_err(),
        "into its own subtree"
    );
    assert!(
        doc.move_block(a, Some(gone), 0).is_err(),
        "under a deleted block"
    );
    assert!(doc.move_block(gone, None, 0).is_err(), "a deleted block");
    assert_eq!(doc.blocks(), [a, b]);
}

#[test]
fn inserting_checks_its_parent_and_index() {
    let doc = Document::new(1).unwrap();
    let a = para(&doc, "a");
    let new = NewBlock::new(BlockKind::Paragraph, "body", "x");
    assert!(doc.insert_block_at(None, 2, &new).is_err());
    let x = doc.insert_block_at(None, 0, &new).unwrap();
    let y = doc.insert_block_at(None, 2, &new).unwrap();
    assert_eq!(doc.blocks(), [x, a, y]);
    doc.delete_block(a).unwrap();
    assert!(doc.insert_block_at(Some(a), 0, &new).is_err());
    assert_eq!(doc.blocks(), [x, y]);
}

#[test]
fn splitting_and_joining_keep_ids_text_and_succession() {
    let doc = Document::new(1).unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "body", "héllo world")
        .unwrap();
    doc.set_overrides(
        a,
        &reprise_doc::Style {
            family: Some("serif".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let after = para(&doc, "after");
    assert!(doc.split_block(a, 2).is_err(), "inside the é");
    let b = doc.split_block(a, "héllo".len()).unwrap();
    assert_eq!(doc.blocks(), [a, b, after]);
    assert_eq!(texts(&doc), ["héllo", " world", "after"]);
    let moved = doc.block(b).unwrap();
    assert_eq!(
        (moved.kind, moved.style.as_deref()),
        (BlockKind::Paragraph, Some("body"))
    );
    assert_eq!(moved.overrides.family.as_deref(), Some("serif"));

    // Joining `b` back: relations that named it follow the text.
    doc.join_blocks(a, b).unwrap();
    assert_eq!(doc.blocks(), [a, after]);
    assert_eq!(texts(&doc), ["héllo world", "after"]);
    assert_eq!(doc.successors(b), [a]);
    assert!(doc.join_blocks(a, a).is_err());
    assert!(doc.join_blocks(a, b).is_err(), "b is deleted");
    let note = doc
        .append_block(BlockKind::Annotation, "note", "n")
        .unwrap();
    assert!(doc.join_blocks(a, note).is_err(), "different kinds");
    assert!(doc.split_block(a, 999).is_err());
    assert_eq!(doc.blocks(), [a, after, note], "refusals change nothing");
}

#[test]
fn two_peers_edit_and_each_undoes_only_their_own_text() {
    let one = Document::new(1).unwrap();
    let p = para(&one, "abc");
    let two = one.fork(2).unwrap();
    let mut undo = one.undo_stack();
    // I type at the start, you type at the end, I undo.
    one.block(p).unwrap().text.insert(0, "ME ").unwrap();
    one.commit_step();
    two.block(p).unwrap().text.insert(3, " YOU").unwrap();
    exchange(&one, &two);
    assert_eq!(texts(&one), ["ME abc YOU"]);
    assert!(undo.undo().unwrap());
    assert_eq!(texts(&one), ["abc YOU"], "only my text went");
    exchange(&one, &two);
    assert_eq!(texts(&two), ["abc YOU"]);
    assert!(undo.redo().unwrap());
    assert_eq!(texts(&one), ["ME abc YOU"]);
}

#[test]
fn deleting_table_row_or_cell_hides_the_subtree_and_undo_restores_all_ids() {
    use reprise_doc::text::RangePolicy;
    use reprise_doc::{
        Column, ColumnWidth, RangeState, SchemaRegistry, StructuralQuery, TableColumns,
    };
    use reprise_edit::{Command, Editor};
    use reprise_fixtures::{engine, spike::define_styles};
    use reprise_geom::Length;

    for level in 0..3 {
        let doc = Document::new(1).unwrap();
        define_styles(&doc).unwrap();
        let before = para(&doc, "before");
        let table = doc
            .append_table(TableColumns {
                columns: (0..2)
                    .map(|_| Column {
                        width: ColumnWidth::Fixed(Length::from_pt(80)),
                    })
                    .collect(),
            })
            .unwrap();
        let mut rows = Vec::new();
        let mut cells = Vec::new();
        let mut paragraphs = Vec::new();
        for r in 0..2 {
            let row = doc.append_table_row(table, r == 0).unwrap();
            rows.push(row);
            for c in 0..2 {
                let cell = doc.append_table_cell(row, c).unwrap();
                cells.push(cell);
                paragraphs.push(
                    doc.append_cell_block(cell, BlockKind::Paragraph, "body", "cell text")
                        .unwrap(),
                );
            }
        }
        let after = para(&doc, "after");
        let range = doc
            .add_range(paragraphs[0], 0..4, RangePolicy::FIXED)
            .unwrap();
        let topology: Vec<_> = doc
            .document_order()
            .into_iter()
            .map(|id| (id, doc.parent_of(id), doc.table_role(id).unwrap()))
            .collect();
        let target = [table, rows[0], cells[0]][level];
        let mut hidden = vec![target];
        let mut index = 0;
        while index < hidden.len() {
            hidden.extend(doc.children(Some(hidden[index])));
            index += 1;
        }
        let layout = engine();
        let baseline = layout.layout(&doc);
        assert!(paragraphs.iter().all(|id| baseline.block(*id).is_some()));
        let mut editor = Editor::new(doc, SchemaRegistry::builtin());
        editor
            .apply_command(Command::DeleteBlock { node: target })
            .unwrap();
        assert_eq!(editor.undo_count(), 1);
        let doc = editor.document();
        assert_eq!(
            doc.blocks(),
            if level == 0 {
                vec![before, after]
            } else {
                vec![before, table, after]
            }
        );
        for id in &hidden {
            assert!(!doc.is_live(*id));
            assert!(doc.block(*id).is_err());
            assert!(doc.table_role(*id).is_err());
            assert_eq!(doc.parent_of(*id), None);
            assert!(doc.children(Some(*id)).is_empty());
            assert!(
                doc.evaluate(&StructuralQuery::Children {
                    of: Some(*id),
                    kind: None
                })
                .is_empty()
            );
            assert!(
                doc.evaluate(&StructuralQuery::Parent { of: *id })
                    .is_empty()
            );
            assert!(!doc.document_order().contains(id));
        }
        assert!(matches!(
            doc.resolve_range(range),
            RangeState::Missing { .. }
        ));
        let deleted = layout.layout(doc);
        assert!(hidden.iter().all(|id| deleted.block(*id).is_none()));
        assert!(deleted.block(before).is_some() && deleted.block(after).is_some());
        for id in paragraphs.iter().filter(|id| !hidden.contains(id)) {
            assert!(
                deleted.block(*id).is_some(),
                "live sibling cell still lays out"
            );
        }

        assert!(editor.undo().unwrap());
        let doc = editor.document();
        assert_eq!(
            doc.document_order(),
            topology.iter().map(|(id, ..)| *id).collect::<Vec<_>>()
        );
        for (id, parent, role) in &topology {
            assert!(doc.is_live(*id));
            assert_eq!(doc.parent_of(*id), *parent);
            assert_eq!(doc.table_role(*id).unwrap(), *role);
        }
        assert!(
            matches!(doc.resolve_range(range), RangeState::Valid { node, bytes } if node == paragraphs[0] && bytes == (0..4))
        );
        assert_eq!(layout.layout(doc).blocks, baseline.blocks);
        assert!(editor.redo().unwrap());
        assert!(hidden.iter().all(|id| !editor.document().is_live(*id)));
        assert!(editor.undo().unwrap());
        assert!(hidden.iter().all(|id| editor.document().is_live(*id)));
    }
}
