//! Flow text tests: paragraphs as break markers (see `docs/flow.md`).

use loro::{Container, LoroMap, ValueOrContainer};
use reprise_text::{Affinity, BREAK, RangePolicy, TextError};

use crate::relation::builtin::FOLLOW;
use crate::{
    BlockKind, DocError, Document, LayoutQuery, NewBlock, NodeId, RangeState, Relation,
    SchemaRegistry, Target,
};

fn text(doc: &Document, id: NodeId) -> String {
    doc.block(id).unwrap().text.to_string()
}

/// Every live block in document order, with its text.
fn dump(doc: &Document) -> Vec<String> {
    doc.document_order()
        .into_iter()
        .filter_map(|id| Some(doc.block(id).ok()?.text.to_string()))
        .collect()
}

/// Sends what `from` has and `to` lacks as a sync packet.
fn sync(from: &Document, to: &Document) {
    from.commit();
    to.commit();
    let packet = from.export_delta(&to.version_vector()).unwrap();
    to.import_packet(&packet).unwrap();
}

fn both_ways(a: &Document, b: &Document) {
    sync(a, b);
    sync(b, a);
    assert_eq!(dump(a), dump(b), "replicas converge");
}

fn one(s: &str) -> (Document, NodeId) {
    let doc = Document::new(1).unwrap();
    let p = doc.append_block(BlockKind::Paragraph, "body", s).unwrap();
    doc.commit();
    (doc, p)
}

#[test]
fn a_split_is_a_break_and_moves_no_text() {
    let (doc, p) = one("hello world");
    let host_text = doc.text_of_any(p).unwrap();
    let before = host_text.len();
    let q = doc.split_block(p, 5).unwrap();
    doc.commit();
    assert!(q.is_break());
    assert_eq!(q.host(), p);
    assert_eq!(NodeId::parse(&q.to_string()), Some(q));
    assert_eq!(text(&doc, p), "hello");
    assert_eq!(text(&doc, q), " world");
    assert_eq!(doc.blocks(), vec![p, q]);
    assert_eq!(doc.parent_of(q), Some(None));
    assert_eq!(doc.kind_of(q), Some(BlockKind::Paragraph));
    assert_eq!(doc.block(q).unwrap().style.as_deref(), Some("body"));
    // One character was added to the shared text: the break.
    assert_eq!(host_text.len(), before + BREAK.len_utf8());
    // Both views edit the shared text.
    doc.block(q).unwrap().text.insert(0, "big").unwrap();
    doc.block(p).unwrap().text.insert(5, ",").unwrap();
    assert_eq!(text(&doc, p), "hello,");
    assert_eq!(text(&doc, q), "big world");
}

/// The review's first failure: a delete made concurrently with a split was
/// lost, because the split copied the text the delete was aimed at.
#[test]
fn a_concurrent_delete_survives_a_split() {
    let (a, p) = one("one two three four");
    let b = a.fork(2).unwrap();
    let q = a.split_block(p, 8).unwrap(); // "one two " | "three four"
    a.commit();
    b.block(p).unwrap().text.delete(13..18).unwrap(); // " four"
    b.commit();
    both_ways(&a, &b);
    assert_eq!(text(&a, p), "one two ");
    assert_eq!(text(&a, q), "three");
}

/// The review's second failure: a typo fixed concurrently with a split
/// landed in the wrong paragraph.
#[test]
fn a_concurrent_fix_lands_in_its_paragraph() {
    let (a, p) = one("first para. secnod para.");
    let b = a.fork(2).unwrap();
    let q = a.split_block(p, 12).unwrap();
    a.commit();
    let t = b.block(p).unwrap().text;
    t.delete(15..17).unwrap(); // "no" of "secnod"
    t.insert(15, "on").unwrap();
    b.commit();
    both_ways(&a, &b);
    assert_eq!(text(&a, p), "first para. ");
    assert_eq!(text(&a, q), "second para.");
}

/// The review's third failure: a join discarded a concurrent fix to the
/// joined paragraph.
#[test]
fn a_concurrent_fix_survives_a_join() {
    let (a, p) = one("ab");
    let q = a.split_block(p, 1).unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    a.join_blocks(p, q).unwrap();
    a.commit();
    b.block(q).unwrap().text.insert(1, "X").unwrap();
    b.commit();
    both_ways(&a, &b);
    assert_eq!(dump(&a), vec!["abX".to_string()]);
    assert!(!a.is_live(q));
    assert_eq!(a.successors(q), vec![p]);
}

#[test]
fn concurrent_splits_of_one_paragraph_keep_both_breaks() {
    let (a, p) = one("aaa bbb ccc");
    let b = a.fork(2).unwrap();
    a.split_block(p, 4).unwrap();
    b.split_block(p, 8).unwrap();
    both_ways(&a, &b);
    assert_eq!(dump(&a), vec!["aaa ", "bbb ", "ccc"]);
}

#[test]
fn concurrent_enter_at_one_place_makes_an_empty_paragraph() {
    let (a, p) = one("ab");
    let b = a.fork(2).unwrap();
    a.split_block(p, 1).unwrap();
    b.split_block(p, 1).unwrap();
    both_ways(&a, &b);
    assert_eq!(dump(&a), vec!["a", "", "b"]);
}

#[test]
fn undo_and_redo_of_a_split_keep_the_new_paragraph_id() {
    let (doc, p) = one("hello world");
    let mut undo = doc.undo_stack();
    let q = doc.split_block(p, 5).unwrap();
    doc.commit_step();
    assert_eq!(undo.undo_count(), 1, "staging is not a step of its own");
    // A collaborator marks a word in the new paragraph.
    let other = doc.fork(2).unwrap();
    let r = other.add_range(q, 1..6, RangePolicy::FIXED).unwrap();
    sync(&other, &doc);
    assert!(undo.undo().unwrap());
    assert_eq!(dump(&doc), vec!["hello world"]);
    assert!(!doc.is_live(q));
    assert!(undo.redo().unwrap());
    assert_eq!(dump(&doc), vec!["hello", " world"]);
    assert!(doc.is_live(q), "redo brings back the same paragraph");
    assert_eq!(
        doc.resolve_range(r),
        RangeState::Valid {
            node: q,
            bytes: 1..6
        }
    );
}

#[test]
fn a_step_with_text_then_a_split_undoes_as_one() {
    let (doc, p) = one("AB");
    let mut undo = doc.undo_stack();
    doc.block(p).unwrap().text.insert(1, "hello world").unwrap();
    let q = doc.split_block(p, 6).unwrap();
    doc.block(q).unwrap().text.insert(0, "x").unwrap();
    doc.commit_step();
    assert_eq!(dump(&doc), vec!["Ahello", "x worldB"]);
    assert_eq!(undo.undo_count(), 1);
    assert!(undo.undo().unwrap());
    assert_eq!(dump(&doc), vec!["AB"]);
    assert!(undo.redo().unwrap());
    assert_eq!(dump(&doc), vec!["Ahello", "x worldB"]);
    assert!(doc.is_live(q));
}

#[test]
fn undo_of_a_join_brings_the_paragraph_back() {
    let (doc, p) = one("ab");
    let q = doc.split_block(p, 1).unwrap();
    doc.commit_step();
    let mut undo = doc.undo_stack();
    doc.join_blocks(p, q).unwrap();
    doc.commit_step();
    assert_eq!(dump(&doc), vec!["ab"]);
    undo.undo().unwrap();
    assert_eq!(dump(&doc), vec!["a", "b"]);
    assert!(doc.is_live(q));
}

#[test]
fn deleting_a_break_paragraph_and_undoing_keeps_its_id() {
    let (doc, p) = one("one two three");
    let q = doc.split_block(p, 4).unwrap();
    let r = doc.split_block(q, 4).unwrap();
    doc.commit_step();
    let mut undo = doc.undo_stack();
    doc.delete_block(q).unwrap();
    doc.commit_step();
    assert_eq!(dump(&doc), vec!["one ", "three"]);
    assert!(!doc.is_live(q));
    undo.undo().unwrap();
    assert_eq!(dump(&doc), vec!["one ", "two ", "three"]);
    assert!(doc.is_live(q) && doc.is_live(r));
}

#[test]
fn deleting_the_head_keeps_the_rest_of_the_flow() {
    let (doc, p) = one("one two");
    let q = doc.split_block(p, 4).unwrap();
    doc.commit_step();
    let other = doc.fork(2).unwrap();
    let mut undo = doc.undo_stack();
    doc.delete_block(p).unwrap();
    doc.commit_step();
    assert!(!doc.is_live(p));
    assert_eq!(doc.blocks(), vec![q]);
    // Typing into the deleted head at the same time is kept, in the paragraph
    // that now comes first.
    other.block(p).unwrap().text.insert(0, "zero ").unwrap();
    both_ways(&doc, &other);
    assert_eq!(dump(&doc), vec!["zero two"]);
    undo.undo().unwrap();
    // Undo puts the head's text back; where it lands next to the concurrent
    // insertion is the store's ordering of the two.
    let after = dump(&doc);
    assert_eq!(after.len(), 2);
    assert!(after[0].contains("zero ") && after[0].contains("one "));
    assert_eq!(after[1], "two");
    assert!(doc.is_live(p));
}

#[test]
fn splitting_after_head_deletion_keeps_the_original_paragraph_first() {
    for concurrent_prefix in ["", "zero "] {
        for at in [0, 2] {
            let (doc, head) = one("one two");
            let surviving = doc.split_block(head, 4).unwrap();
            doc.commit_step();
            let peer = doc.fork(2).unwrap();
            doc.delete_block(head).unwrap();
            doc.commit_step();
            peer.block(head)
                .unwrap()
                .text
                .insert(0, concurrent_prefix)
                .unwrap();
            both_ways(&doc, &peer);
            let before = doc.block(surviving).unwrap().text.to_string();
            let mut undo = doc.undo_stack();
            let tail = doc.split_block(surviving, at).unwrap();
            doc.commit_step();
            assert_eq!(doc.blocks(), [surviving, tail]);
            assert_eq!(dump(&doc), [&before[..at], &before[at..]]);
            assert!(undo.undo().unwrap());
            assert_eq!(doc.blocks(), [surviving]);
            assert_eq!(dump(&doc), std::slice::from_ref(&before));
            assert!(undo.redo().unwrap());
            assert_eq!(doc.blocks(), [surviving, tail]);
            assert_eq!(dump(&doc), [&before[..at], &before[at..]]);
        }
    }
}

#[test]
fn a_lone_head_is_deleted_as_a_block() {
    let (doc, p) = one("only");
    doc.delete_block(p).unwrap();
    assert!(doc.is_soft_deleted(p));
    assert!(doc.blocks().is_empty());
}

#[test]
fn ranges_follow_their_text_across_a_concurrent_split() {
    let (a, p) = one("alpha beta gamma");
    let r = a.add_range(p, 11..16, RangePolicy::FIXED).unwrap(); // "gamma"
    a.commit();
    let b = a.fork(2).unwrap();
    let q = b.split_block(p, 6).unwrap();
    both_ways(&a, &b);
    assert_eq!(
        a.resolve_range(r),
        RangeState::Valid {
            node: q,
            bytes: 5..10
        }
    );
    assert_eq!(
        a.locate(
            p,
            &a.block(q).unwrap().text.anchor(0, Affinity::After).unwrap()
        )
        .map(|x| x.0),
        Some(q)
    );
}

#[test]
fn a_range_across_a_break_reports_its_first_paragraph() {
    let (doc, p) = one("abcdef");
    let r = doc.add_range(p, 1..5, RangePolicy::FIXED).unwrap();
    let q = doc.split_block(p, 3).unwrap();
    assert_eq!(
        doc.resolve_range(r),
        RangeState::Valid {
            node: p,
            bytes: 1..3
        }
    );
    assert_eq!(text(&doc, q), "def");
}

#[test]
fn inserting_a_block_between_paragraphs_embeds_it() {
    let (doc, p) = one("one two");
    let q = doc.split_block(p, 4).unwrap();
    doc.commit_step();
    let mut undo = doc.undo_stack();
    let img = doc
        .insert_block_at(None, 1, &NewBlock::new(BlockKind::Image, "", "alt"))
        .unwrap();
    doc.commit_step();
    assert_eq!(doc.blocks(), vec![p, img, q]);
    assert_eq!(doc.parent_of(img), Some(None));
    assert_eq!(undo.undo_count(), 1);
    undo.undo().unwrap();
    assert_eq!(doc.blocks(), vec![p, q]);
    undo.redo().unwrap();
    assert_eq!(doc.blocks(), vec![p, img, q], "same node, same place");
    // Moving it out of the flow, to the end.
    doc.move_block(img, None, 2).unwrap();
    doc.commit_step();
    assert_eq!(doc.blocks(), vec![p, q, img]);
    // And back between them.
    doc.move_block(img, None, 1).unwrap();
    doc.commit_step();
    assert_eq!(doc.blocks(), vec![p, img, q]);
}

#[test]
fn embedded_text_blocks_are_flows_of_their_own() {
    let (doc, p) = one("one two");
    let q = doc.split_block(p, 4).unwrap();
    let e = doc
        .insert_block_at(
            None,
            1,
            &NewBlock::new(BlockKind::Paragraph, "", "pasted text"),
        )
        .unwrap();
    let f = doc.split_block(e, 7).unwrap();
    doc.commit();
    assert_eq!(doc.blocks(), vec![p, e, f, q]);
    assert_eq!(dump(&doc), vec!["one ", "pasted ", "text", "two"]);
    assert_eq!(doc.parent_of(f), Some(None));
}

#[test]
fn a_paragraph_of_a_flow_does_not_move_as_a_node() {
    let (doc, p) = one("ab");
    let q = doc.split_block(p, 1).unwrap();
    assert!(matches!(
        doc.move_block(q, None, 0),
        Err(DocError::Malformed(..))
    ));
    assert!(matches!(
        doc.move_block(p, None, 1),
        Err(DocError::Malformed(..))
    ));
}

#[test]
fn relations_target_break_paragraphs_and_follow_joins() {
    let (doc, p) = one("text note");
    let q = doc.split_block(p, 5).unwrap();
    doc.commit_step();
    let schemas = SchemaRegistry::builtin();
    let r = doc.add_range(q, 0..4, RangePolicy::FIXED).unwrap();
    let rel = Relation::new(FOLLOW).owned_by(q).target(
        "line",
        Target::Layout(LayoutQuery::LineContaining { range: r }),
    );
    let id = doc.add_relation(&schemas, &rel).unwrap();
    assert_eq!(doc.relations(), vec![(id, Ok(rel))]);
    doc.join_blocks(p, q).unwrap();
    assert_eq!(doc.succession(q), crate::Succession::Live(vec![p]));
    assert_eq!(
        doc.resolve_range(r),
        RangeState::Valid {
            node: p,
            bytes: 5..9
        }
    );
}

#[test]
fn the_break_character_is_never_authored_text() {
    let (doc, p) = one("ab");
    let t = doc.block(p).unwrap().text;
    assert!(matches!(t.insert(1, "\u{FDD0}"), Err(TextError::Reserved)));
    assert!(
        doc.append_block(BlockKind::Paragraph, "", "x\u{FDD0}y")
            .is_err()
    );
    assert!(
        doc.stage_block(&NewBlock::new(BlockKind::Paragraph, "", "\u{FDD0}"))
            .is_err()
    );
}

fn raw_text(doc: &Document, p: NodeId) -> loro::LoroText {
    doc.text_of_any(p).unwrap().loro().clone()
}

#[test]
fn hostile_break_characters_without_records_are_hidden() {
    let (doc, p) = one("ab");
    raw_text(&doc, p).insert(1, "\u{FDD0}\u{FDD0}").unwrap();
    doc.commit();
    assert_eq!(dump(&doc), vec!["ab"]);
    assert_eq!(doc.blocks(), vec![p]);
    // Editing around them still works.
    let t = doc.block(p).unwrap().text;
    t.insert(1, "X").unwrap();
    t.delete(0..1).unwrap();
    assert_eq!(text(&doc, p), "Xb");
    assert_eq!(raw_text(&doc, p).to_string().matches(BREAK).count(), 2);
}

#[test]
fn hostile_records_are_hidden_or_ignored() {
    let (doc, p) = one("abc");
    let q = doc.split_block(p, 1).unwrap();
    doc.commit();
    let rec = match doc.break_records().get(&q.to_string()) {
        Some(ValueOrContainer::Container(Container::Map(m))) => m,
        _ => panic!("record"),
    };
    // A future version: hidden.
    rec.insert("v", 2).unwrap();
    doc.commit();
    assert_eq!(dump(&doc), vec!["abc"]);
    rec.insert("v", 1).unwrap();
    rec.insert("kind", "future-kind").unwrap();
    doc.commit();
    // An unknown kind: listed, unreadable, like an unknown block kind.
    assert!(matches!(doc.block(q), Err(DocError::Malformed(_, "kind"))));
    rec.insert("kind", "paragraph").unwrap();
    // An embed naming a node that is not the host's child: ignored.
    let other = doc.append_block(BlockKind::Paragraph, "", "other").unwrap();
    let t = raw_text(&doc, p);
    t.insert(0, "\u{FDD0}").unwrap();
    let id = t
        .get_cursor(0, loro::cursor::Side::Left)
        .and_then(|c| c.id)
        .unwrap();
    let key = NodeId::at_break(p.node, id).to_string();
    let forged = doc
        .break_records()
        .insert_container(&key, LoroMap::new())
        .unwrap();
    forged.insert("v", 1).unwrap();
    forged.insert("active", true).unwrap();
    forged.insert("embed", other.to_string()).unwrap();
    doc.commit();
    assert_eq!(doc.blocks(), vec![p, q, other]);
    assert_eq!(dump(&doc), vec!["a", "bc", "other"]);
}

#[test]
fn concurrent_first_breaks_of_one_host_both_keep_their_records() {
    let (a, p) = one("aaaa bbbb");
    let b = a.fork(2).unwrap();
    let qa = a.split_block(p, 2).unwrap();
    let qb = b.split_block(p, 7).unwrap();
    both_ways(&a, &b);
    assert!(a.is_live(qa) && a.is_live(qb));
    assert_eq!(dump(&a), vec!["aa", "aa bb", "bb"]);
}

#[test]
fn typing_in_one_paragraph_reports_only_that_paragraph() {
    let (a, p) = one("one two three");
    let q = a.split_block(p, 4).unwrap();
    let r = a.split_block(q, 4).unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    b.block(q).unwrap().text.insert(1, "w").unwrap();
    b.commit();
    let packet = b.export_delta(&a.version_vector()).unwrap();
    let report = a.import_packet(&packet).unwrap();
    assert!(report.blocks.contains(&q));
    assert!(!report.blocks.contains(&p), "{report:?}");
    assert!(!report.blocks.contains(&r), "{report:?}");
}

#[test]
fn flows_survive_saving_and_reopening() {
    let (doc, p) = one("one two");
    let q = doc.split_block(p, 4).unwrap();
    doc.commit();
    let bytes = doc.export(crate::PersistenceMode::default());
    let back = Document::import(&bytes, 3).unwrap();
    assert_eq!(back.blocks(), vec![p, q]);
    assert_eq!(dump(&back), vec!["one ", "two"]);
}

#[test]
fn anchors_at_paragraph_edges_stay_in_their_paragraph() {
    let (doc, p) = one("ab");
    let q = doc.split_block(p, 1).unwrap();
    let end_of_p = doc
        .block(p)
        .unwrap()
        .text
        .anchor(1, Affinity::After)
        .unwrap();
    let start_of_q = doc
        .block(q)
        .unwrap()
        .text
        .anchor(0, Affinity::Before)
        .unwrap();
    // Typing at the boundary, in both paragraphs.
    doc.block(p).unwrap().text.insert(1, "x").unwrap();
    doc.block(q).unwrap().text.insert(0, "y").unwrap();
    assert_eq!(dump(&doc), vec!["ax", "yb"]);
    assert_eq!(
        doc.locate(p, &end_of_p).map(|(n, r)| (n, r.offset())),
        Some((p, 2))
    );
    assert_eq!(
        doc.locate(q, &start_of_q).map(|(n, r)| (n, r.offset())),
        Some((q, 0))
    );
}

/// Range ends were stored as bytes, which the JSON delta encoding turned into
/// a list of numbers on the receiving replica. Every form reads the same.
#[test]
fn range_ends_read_from_every_stored_form() {
    let (doc, p) = one("alpha beta");
    let r = doc.add_range(p, 6..10, RangePolicy::FIXED).unwrap();
    let meta = doc.tree("ranges").get_meta(r.0).unwrap();
    let raw = |key| crate::ranges::stored_anchor(meta.get(key)).unwrap();
    let (start, end) = (raw("start"), raw("end"));
    let valid = RangeState::Valid {
        node: p,
        bytes: 6..10,
    };
    assert_eq!(doc.resolve_range(r), valid);
    meta.insert("start", start.clone()).unwrap();
    meta.insert("end", end.clone()).unwrap();
    assert_eq!(doc.resolve_range(r), valid, "bytes");
    let list = |b: &[u8]| {
        loro::LoroValue::List(
            b.iter()
                .map(|&x| i64::from(x).into())
                .collect::<Vec<_>>()
                .into(),
        )
    };
    meta.insert("start", list(&start)).unwrap();
    meta.insert("end", list(&end)).unwrap();
    assert_eq!(doc.resolve_range(r), valid, "a list of byte values");
    meta.insert("end", loro::LoroValue::List(vec![999i64.into()].into()))
        .unwrap();
    assert_eq!(doc.resolve_range(r), RangeState::Missing { node: Some(p) });
}

#[test]
fn a_range_with_an_empty_first_slice_still_spans_the_next_paragraph() {
    let (doc, head) = one("abcd");
    let range = doc.add_range(head, 2..4, RangePolicy::EXPANDING).unwrap();
    let tail = doc.split_block(head, 2).unwrap();
    assert!(!matches!(
        doc.resolve_range(range),
        RangeState::Missing { .. }
    ));
    let extent = doc.range_extent(range).unwrap();
    assert_eq!(extent.start, (head, 2));
    assert_eq!(extent.end, (tail, 2));
    assert!(!extent.rebound);
}

#[test]
fn copying_a_split_tail_keeps_ranges_created_before_the_split() {
    let (doc, head) = one("abcd");
    let range = doc.add_range(head, 2..4, RangePolicy::FIXED).unwrap();
    let tail = doc.split_block(head, 2).unwrap();
    let fragment = doc
        .copy_fragment(
            "test",
            &[crate::fragment::CopyBlock {
                node: tail,
                bytes: None,
            }],
            &SchemaRegistry::builtin(),
        )
        .unwrap();
    assert_eq!(fragment.ranges.len(), 1);
    assert_eq!(fragment.ranges[0].id, range);
    assert_eq!(fragment.ranges[0].bytes, 0..2);
}

#[test]
fn excessive_embed_depth_is_hidden_and_reported_without_repair() {
    let (doc, mut host) = one("ab");
    for _ in 0..=crate::MAX_FLOW_DEPTH {
        let tail = doc.split_block(host, 1).unwrap();
        let index = doc.blocks().iter().position(|&n| n == tail).unwrap();
        host = doc
            .insert_block_at(None, index, &NewBlock::new(BlockKind::Paragraph, "", "ab"))
            .unwrap();
    }
    doc.commit();
    let revision = doc.revision();
    assert!(!doc.document_order().contains(&host));
    assert!(
        doc.audit()
            .iter()
            .any(|f| f.node == host && f.note.code == crate::invariants::FLOW_DEPTH)
    );
    assert_eq!(doc.revision(), revision);
}

#[test]
fn copying_lineage_skips_inactive_markers_inside_the_source() {
    let (doc, source) = one("abcd");
    let target = doc
        .append_block(BlockKind::Paragraph, "body", "prefix")
        .unwrap();
    let tail = doc.split_block(source, 2).unwrap();
    let anchor = doc
        .block(tail)
        .unwrap()
        .text
        .anchor(1, Affinity::After)
        .unwrap();
    doc.join_blocks(source, tail).unwrap();
    doc.join_blocks(target, source).unwrap();
    assert_eq!(text(&doc, target), "prefixabcd");
    let (node, mapped) = doc.transferred_anchor(source, &anchor).unwrap();
    assert_eq!(node, target);
    assert_eq!(
        doc.block(target)
            .unwrap()
            .text
            .resolve(&mapped)
            .unwrap()
            .offset(),
        9
    );
}
