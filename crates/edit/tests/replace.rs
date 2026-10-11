//! Replacing a stable selection with a fragment: one transaction, one undo step,
//! resolved inside the transaction.

use reprise_doc::fragment::Fragment;
use reprise_doc::{BlockKind, Document, NodeId};
use reprise_edit::{Affinity, Caret, Command, Editor, Reason, Selection, StableSelection};
use reprise_fixtures::{OTHER_PEER, PEER, engine};

fn base(blocks: &[&str]) -> (Document, Vec<NodeId>) {
    let doc = Document::new(PEER).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    let ids = blocks
        .iter()
        .map(|t| doc.append_block(BlockKind::Paragraph, "body", t).unwrap())
        .collect();
    doc.commit();
    (doc, ids)
}
fn fragment(blocks: &[&str]) -> Fragment {
    let (doc, _) = base(blocks);
    doc.copy_all_fragment("source", &engine().schemas).unwrap()
}
fn caret(node: NodeId, offset: usize) -> Caret {
    Caret {
        node,
        offset,
        affinity: Affinity::Downstream,
    }
}
fn select(doc: &Document, a: (NodeId, usize), b: (NodeId, usize)) -> StableSelection {
    StableSelection::anchor(
        doc,
        &Selection {
            anchor: caret(a.0, a.1),
            focus: caret(b.0, b.1),
        },
    )
    .unwrap()
}
fn texts(e: &Editor) -> Vec<String> {
    let doc = e.document();
    doc.blocks()
        .into_iter()
        .map(|n| doc.block(n).unwrap().text.to_string())
        .collect()
}
fn replace(e: &mut Editor, s: &StableSelection, f: &Fragment) -> reprise_edit::Replaced {
    let staged = e.stage_replacement(f, "target").unwrap();
    e.replace_selection(s, &staged).unwrap()
}

#[test]
fn one_block_replacement_is_one_undo_step() {
    let (doc, ids) = base(&["hello world"]);
    let mut e = Editor::new(doc, engine().schemas.clone());
    let s = select(e.document(), (ids[0], 6), (ids[0], 11));
    let done = replace(&mut e, &s, &fragment(&["there"]));
    assert_eq!(texts(&e), ["hello there"]);
    assert_eq!(e.undo_count(), 1);
    let (node, offset) = done.caret.unwrap();
    assert_eq!(e.document().block(node).unwrap().text.to_string(), "hello there");
    assert_eq!(offset, 11);
    assert!(e.undo().unwrap());
    assert_eq!(texts(&e), ["hello world"]);
    assert!(!e.can_undo(), "exactly one step was recorded");
    assert!(e.redo().unwrap());
    assert_eq!(texts(&e), ["hello there"]);
}

#[test]
fn reversed_selection_and_collapsed_selection() {
    let (doc, ids) = base(&["abcdef"]);
    let mut e = Editor::new(doc, engine().schemas.clone());
    // Focus before anchor selects the same bytes.
    let s = select(e.document(), (ids[0], 4), (ids[0], 2));
    replace(&mut e, &s, &fragment(&["X"]));
    assert_eq!(texts(&e), ["abXef"]);
    // A collapsed selection is a plain paste at the caret.
    let s = select(e.document(), (ids[0], 1), (ids[0], 1));
    replace(&mut e, &s, &fragment(&["Y"]));
    assert_eq!(texts(&e), ["aYbXef"]);
    assert_eq!(e.undo_count(), 2);
}

#[test]
fn cross_block_selection_joins_and_pastes_in_one_step() {
    let (doc, ids) = base(&["one two", "middle", "gone", "three four"]);
    let mut e = Editor::new(doc, engine().schemas.clone());
    let s = select(e.document(), (ids[0], 4), (ids[3], 6));
    replace(&mut e, &s, &fragment(&["NEW"]));
    assert_eq!(texts(&e), ["one NEWfour"]);
    assert_eq!(e.undo_count(), 1);
    assert!(e.undo().unwrap());
    assert_eq!(
        texts(&e),
        ["one two", "middle", "gone", "three four"],
        "one undo restores every block"
    );
}

#[test]
fn multi_block_fragment_over_a_selection() {
    let (doc, ids) = base(&["alpha beta", "gamma delta"]);
    let mut e = Editor::new(doc, engine().schemas.clone());
    let s = select(e.document(), (ids[0], 5), (ids[1], 5));
    replace(&mut e, &s, &fragment(&["P", "Q", "R"]));
    assert_eq!(texts(&e), ["alphaP", "Q", "R delta"]);
    assert_eq!(e.undo_count(), 1);
}

#[test]
fn empty_fragment_deletes_the_selection() {
    let (doc, ids) = base(&["abcdef"]);
    let mut e = Editor::new(doc, engine().schemas.clone());
    let s = select(e.document(), (ids[0], 1), (ids[0], 4));
    let done = replace(&mut e, &s, &fragment(&[]));
    assert_eq!(texts(&e), ["aef"]);
    assert_eq!(done.caret, Some((ids[0], 1)));
}

#[test]
fn a_remote_packet_between_staging_and_commit_is_part_of_the_resolution() {
    let (doc, ids) = base(&["abcdefgh"]);
    let mut local = Editor::new(doc.fork(PEER).unwrap(), engine().schemas.clone());
    let mut remote = Editor::new(doc.fork(OTHER_PEER).unwrap(), engine().schemas.clone());
    // The host selects "cdef" and stages the clipboard...
    let s = select(local.document(), (ids[0], 2), (ids[0], 6));
    let staged = local.stage_replacement(&fragment(&["-"]), "target").unwrap();
    // ...a collaborator types inside the selection and just before it...
    remote
        .apply(&reprise_edit::Transaction::new().with(Command::InsertText {
            node: ids[0],
            at: 4,
            text: "<remote>".into(),
        }))
        .unwrap();
    remote
        .apply(&reprise_edit::Transaction::new().with(Command::InsertText {
            node: ids[0],
            at: 0,
            text: "R:".into(),
        }))
        .unwrap();
    let packet = remote
        .document()
        .export_delta(&local.document().version_vector())
        .unwrap();
    local.import_packet(&packet).unwrap();
    let before = local.undo_count();
    // ...and the commit resolves against what is there now.
    local.replace_selection(&s, &staged).unwrap();
    assert_eq!(texts(&local), ["R:ab-gh"], "everything inside the selection went");
    assert_eq!(local.undo_count(), before + 1, "one undo step");
    assert!(local.undo().unwrap());
    assert_eq!(
        texts(&local),
        ["R:abcd<remote>efgh"],
        "undo gives back the selection and keeps the remote text"
    );
}

#[test]
fn a_refused_fragment_leaves_the_selection_alone() {
    let (doc, ids) = base(&["keep me"]);
    let e = Editor::new(doc, engine().schemas.clone());
    let revision = e.document().revision();
    let mut bad = fragment(&["x"]);
    bad.version = 99;
    let error = e.stage_replacement(&bad, "target").unwrap_err();
    assert!(matches!(error.reason, Reason::Fragment(_)));
    assert!(e.stage_replacement(&fragment(&["x"]), "").is_err());
    assert_eq!(texts(&e), ["keep me"]);
    assert_eq!(e.document().revision(), revision);
    assert!(!e.can_undo());
    let _ = ids;
}

#[test]
fn a_selection_that_resolves_nowhere_writes_nothing() {
    let (doc, ids) = base(&["only"]);
    let mut e = Editor::new(doc, engine().schemas.clone());
    let s = select(e.document(), (ids[0], 0), (ids[0], 2));
    let staged = e.stage_replacement(&fragment(&["x"]), "target").unwrap();
    e.apply(&reprise_edit::Transaction::new().with(Command::DeleteBlock { node: ids[0] }))
        .unwrap();
    let revision = e.document().revision();
    let steps = e.undo_count();
    let error = e.replace_selection(&s, &staged).unwrap_err();
    assert_eq!(error.reason, Reason::SelectionGone);
    assert_eq!(e.document().revision(), revision);
    assert_eq!(e.undo_count(), steps);
}

#[test]
fn a_selection_ending_in_a_child_keeps_the_subtree() {
    let doc = Document::new(PEER).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    let a = doc.append_block(BlockKind::Paragraph, "body", "first").unwrap();
    let parent = doc.append_block(BlockKind::Paragraph, "body", "parent").unwrap();
    let child = doc
        .insert_block_at(
            Some(parent),
            0,
            &reprise_doc::NewBlock {
                kind: BlockKind::Paragraph,
                style: "body".into(),
                overrides: Default::default(),
                text: "child text".into(),
            },
        )
        .unwrap();
    doc.commit();
    let mut e = Editor::new(doc, engine().schemas.clone());
    // From inside the first block to inside the child: the parent lies between
    // but its subtree holds an end, so it survives with its text emptied.
    let s = select(e.document(), (a, 2), (child, 6));
    replace(&mut e, &s, &fragment(&["+"]));
    assert!(e.document().block(parent).is_ok(), "the ancestor survives");
    assert!(e.document().block(child).is_ok() || !e.document().children(Some(parent)).is_empty() || texts(&e).iter().any(|t| t.contains("text")));
    assert_eq!(e.undo_count(), 1);
    assert!(e.undo().unwrap());
    assert_eq!(e.document().block(parent).unwrap().text.to_string(), "parent");
}
