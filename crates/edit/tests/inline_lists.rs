//! List items that hold inline objects, through the kernel (`docs/inline.md`,
//! `docs/roles-and-lists.md`): the planner tracks list membership and object
//! positions side by side, so Enter, Backspace and split/join in one
//! transaction must move both with the text, and undo must put both back.

use reprise_doc::text::OBJECT;
use reprise_doc::{
    BlockKind, Document, InlineObject, InlinePatch, ListDefinition, NodeId, SchemaRegistry,
};
use reprise_edit::{Command, Editor, ListRef, Transaction};

fn field(text: &str) -> InlineObject {
    InlineObject::field("text").with("text", text)
}

/// One paragraph "ab■cd■ef" ("■" an object: X after "ab", Y after "cd"),
/// made a numbered list item.
fn item() -> (Editor, NodeId) {
    let doc = Document::new(1).unwrap();
    let node = doc
        .append_block(BlockKind::Paragraph, "body", "abcdef")
        .unwrap();
    doc.insert_inline(node, 4, &field("Y")).unwrap();
    doc.insert_inline(node, 2, &field("X")).unwrap();
    doc.commit();
    let mut editor = Editor::new(doc, SchemaRegistry::builtin());
    editor
        .apply(
            &Transaction::new()
                .with(Command::CreateList {
                    definition: ListDefinition::numbered(),
                })
                .with(Command::SetListItem {
                    node,
                    list: ListRef::New(0),
                    level: 0,
                }),
        )
        .unwrap();
    (editor, node)
}

/// Each block as "marker text" with objects shown as the text they carry:
/// `1. ab[X]cd[Y]ef`. Reads the objects' records, not just the characters.
fn view(editor: &Editor) -> Vec<String> {
    let doc = editor.document();
    doc.document_order()
        .into_iter()
        .filter(|&n| doc.block(n).is_ok())
        .map(|n| {
            let text = doc.block(n).unwrap().text.to_string();
            let objects = doc.inline_objects(n).unwrap();
            let mut shown = String::new();
            let mut next = objects.iter();
            for (i, c) in text.char_indices() {
                if c == OBJECT {
                    let entry = next.next().expect("a record for each character");
                    assert_eq!(entry.at, i);
                    shown.push_str(&format!("[{}]", entry.object.param("text").unwrap()));
                } else {
                    shown.push(c);
                }
            }
            assert!(next.next().is_none(), "no record without a character");
            match doc.list_item(n) {
                Some(item) => format!("{} {shown}", item.marker),
                None => shown,
            }
        })
        .collect()
}

#[test]
fn enter_in_an_item_splits_its_objects_and_keeps_the_list() {
    let (mut editor, node) = item();
    assert_eq!(view(&editor), ["1. ab[X]cd[Y]ef"]);
    // Enter between the objects, after "cd" is not it: after the first object.
    editor
        .apply_command(Command::Enter {
            node,
            at: "ab".len() + OBJECT.len_utf8(),
        })
        .unwrap();
    assert_eq!(view(&editor), ["1. ab[X]", "2. cd[Y]ef"]);
    // Both halves are items of the same list.
    let doc = editor.document();
    let order = doc.document_order();
    assert_eq!(
        doc.list_item(order[0]).unwrap().list,
        doc.list_item(order[1]).unwrap().list
    );
    // One undo takes the whole key back.
    assert!(editor.undo().unwrap());
    assert_eq!(view(&editor), ["1. ab[X]cd[Y]ef"]);
    assert!(editor.redo().unwrap());
    assert_eq!(view(&editor), ["1. ab[X]", "2. cd[Y]ef"]);
}

#[test]
fn enter_at_the_start_moves_every_object_to_the_new_item() {
    let (mut editor, node) = item();
    editor
        .apply_command(Command::Enter { node, at: 0 })
        .unwrap();
    assert_eq!(view(&editor), ["1. ", "2. ab[X]cd[Y]ef"]);
    assert!(editor.undo().unwrap());
    assert_eq!(view(&editor), ["1. ab[X]cd[Y]ef"]);
}

#[test]
fn backspace_joins_two_items_with_objects_in_both() {
    let (mut editor, node) = item();
    editor
        .apply_command(Command::Enter {
            node,
            at: "ab".len() + OBJECT.len_utf8(),
        })
        .unwrap();
    let second = editor.document().document_order()[1];
    // The first Backspace takes the marker off the second item, the next
    // joins it: the objects of both land in one paragraph, in order.
    editor
        .apply_command(Command::BackspaceAtStart { node: second })
        .unwrap();
    assert_eq!(view(&editor), ["1. ab[X]", "cd[Y]ef"]);
    editor
        .apply_command(Command::BackspaceAtStart { node: second })
        .unwrap();
    assert_eq!(view(&editor), ["1. ab[X]cd[Y]ef"]);
    // Undo walks back one key at a time.
    assert!(editor.undo().unwrap());
    assert_eq!(view(&editor), ["1. ab[X]", "cd[Y]ef"]);
    assert!(editor.undo().unwrap());
    assert_eq!(view(&editor), ["1. ab[X]", "2. cd[Y]ef"]);
    assert!(editor.undo().unwrap());
    assert_eq!(view(&editor), ["1. ab[X]cd[Y]ef"]);
}

#[test]
fn one_transaction_can_split_resize_and_insert_in_an_item() {
    let (mut editor, node) = item();
    // Split after the first object, then change that object and put another
    // in front of it, all against the head the planner is still modelling.
    editor
        .apply(
            &Transaction::new()
                .with(Command::SplitBlock {
                    node,
                    at: "ab".len() + OBJECT.len_utf8(),
                })
                .with(Command::UpdateInline {
                    node,
                    at: 2,
                    patch: InlinePatch {
                        params: [("text".to_string(), Some("X2".to_string()))].into(),
                        ..Default::default()
                    },
                })
                .with(Command::InsertInline {
                    node,
                    at: 0,
                    object: field("W"),
                }),
        )
        .unwrap();
    let shown = view(&editor);
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert_eq!(shown[0], "1. [W]ab[X2]");
    // The split is a plain split: the tail is a paragraph or an item, but it
    // carries "cd[Y]ef" with its object either way.
    assert!(shown[1].ends_with("cd[Y]ef"), "{shown:?}");
    assert!(editor.undo().unwrap());
    assert_eq!(view(&editor), ["1. ab[X]cd[Y]ef"]);
}

#[test]
fn deleting_across_an_item_boundary_keeps_the_records_in_step() {
    let (mut editor, node) = item();
    editor
        .apply_command(Command::Enter {
            node,
            at: "ab".len() + OBJECT.len_utf8(),
        })
        .unwrap();
    let order = editor.document().document_order();
    let (first, second) = (order[0], order[1]);
    // Join them directly, then delete the first object with its neighbours.
    editor
        .apply_command(Command::JoinBlocks { first, second })
        .unwrap();
    assert_eq!(view(&editor), ["1. ab[X]cd[Y]ef"]);
    editor
        .apply_command(Command::DeleteText {
            node: first,
            range: 1.."ab".len() + OBJECT.len_utf8() + 1,
        })
        .unwrap();
    // "a" + "d[Y]ef": the first object is gone with its characters.
    assert_eq!(view(&editor), ["1. ad[Y]ef"]);
    assert!(editor.undo().unwrap());
    assert_eq!(view(&editor), ["1. ab[X]cd[Y]ef"]);
}
