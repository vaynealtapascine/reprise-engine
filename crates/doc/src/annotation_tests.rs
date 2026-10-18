//! Annotation behaviour: edges, structure edits, undo, peers and hostile state.
use crate::annotation::*;
use crate::text::RangePolicy;
use crate::{BlockKind, Document, NodeId, PersistenceMode};

fn doc(text: &str) -> (Document, NodeId) {
    let doc = Document::new(1).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "", text).unwrap();
    doc.commit();
    (doc, node)
}

fn url(u: &str) -> AnnotationData {
    AnnotationData::Link(LinkTarget::Url { url: u.into() })
}

fn comment(thread: &str) -> AnnotationData {
    AnnotationData::Comment(CommentAnchor {
        thread: thread.into(),
    })
}

fn pieces(doc: &Document, id: AnnotationId) -> Vec<(NodeId, std::ops::Range<usize>)> {
    match doc.annotation(id).map(|a| a.state) {
        Some(AnnotationState::Active { pieces, .. }) => {
            pieces.into_iter().map(|p| (p.node, p.bytes)).collect()
        }
        other => panic!("not active: {other:?}"),
    }
}

#[test]
fn kinds_declare_consistent_specs() {
    for kind in Kind::ALL {
        let spec = kind.spec();
        assert_eq!(Kind::parse(spec.name), Some(kind));
        assert_eq!(
            spec.on_empty == OnEmpty::Orphan,
            spec.policy.empty == crate::text::Empty::Keep
        );
    }
    assert_eq!(Kind::Link.spec().policy, RangePolicy::FIXED);
}

#[test]
fn url_validation_allowlists_schemes() {
    for ok in [
        "https://example.com",
        "HTTP://example.com/a?b#c",
        "mailto:a@b.c",
        "tel:+441234",
        "https://user@host:8080/",
    ] {
        assert_eq!(validate_url(ok), Ok(()), "{ok}");
    }
    for (bad, code) in [
        ("javascript:alert(1)", LINK_SCHEME),
        ("JavaScript:alert(1)", LINK_SCHEME),
        ("data:text/html,<b>", LINK_SCHEME),
        ("vbscript:x", LINK_SCHEME),
        ("file:///etc/passwd", LINK_SCHEME),
        ("", LINK_INVALID),
        ("/relative", LINK_INVALID),
        ("#frag", LINK_INVALID),
        ("//host", LINK_INVALID),
        ("https:", LINK_INVALID),
        ("https://", LINK_INVALID),
        ("http:/x", LINK_INVALID),
        (" https://a", LINK_INVALID),
        ("java\tscript:x", LINK_INVALID),
        ("https://a\u{0}b", LINK_INVALID),
        ("http:\\\\evil", LINK_INVALID),
        ("mailto:", LINK_INVALID),
        ("https://exa\u{202e}mple.com", LINK_INVALID),
        ("https://a\u{200b}b.example", LINK_INVALID),
        ("https://a.example/\u{feff}", LINK_INVALID),
        ("https://a.example/\u{2066}x\u{2069}", LINK_INVALID),
    ] {
        assert_eq!(validate_url(bad).unwrap_err().code(), Some(code), "{bad:?}");
    }
    let huge = format!("https://a/{}", "x".repeat(1 << 20));
    assert_eq!(validate_url(&huge).unwrap_err().code(), Some(LINK_LENGTH));
}

#[test]
fn links_do_not_grow_at_either_edge() {
    let (doc, p) = doc("see here now");
    let id = doc
        .add_annotation(p, 4..8, &url("https://a.example"))
        .unwrap();
    let text = doc.block(p).unwrap().text;
    text.insert(8, "X").unwrap();
    text.insert(4, "Y").unwrap();
    assert_eq!(pieces(&doc, id), [(p, 5..9)]);
    text.insert(7, "Z").unwrap();
    assert_eq!(pieces(&doc, id), [(p, 5..10)]);
}

#[test]
fn comments_grow_at_the_end_only() {
    let (doc, p) = doc("see here now");
    let id = doc.add_annotation(p, 4..8, &comment("t1")).unwrap();
    let text = doc.block(p).unwrap().text;
    text.insert(8, "X").unwrap();
    text.insert(4, "Y").unwrap();
    assert_eq!(pieces(&doc, id), [(p, 5..10)]);
}

#[test]
fn deleting_all_text_orphans_comments_and_hides_links() {
    let (doc, p) = doc("abcdefg");
    let link = doc
        .add_annotation(p, 2..5, &url("https://a.example"))
        .unwrap();
    let note = doc.add_annotation(p, 2..5, &comment("t1")).unwrap();
    doc.delete_text(p, 1..6).unwrap();
    assert_eq!(doc.annotation(link).unwrap().state, AnnotationState::Gone);
    assert_eq!(
        doc.annotation(note).unwrap().state,
        AnnotationState::Orphaned {
            node: Some(p),
            at: 1
        }
    );
    let at = doc.annotations_at(p, 1);
    assert_eq!(at.annotations.len(), 1);
    assert_eq!(at.annotations[0].id, note);
    assert_eq!(doc.annotations_of_kind(Kind::Link).annotations.len(), 0);
    assert_eq!(doc.annotations_of_kind(Kind::Comment).annotations.len(), 1);
}

#[test]
fn undo_after_deleting_all_text_restores_both_kinds() {
    let (doc, p) = doc("abcdefg");
    let mut undo = doc.undo_stack();
    let link = doc
        .add_annotation(p, 2..5, &url("https://a.example"))
        .unwrap();
    let note = doc.add_annotation(p, 3..6, &comment("t1")).unwrap();
    doc.commit_step();
    doc.delete_text(p, 1..6).unwrap();
    doc.commit_step();
    assert!(doc.annotation(note).unwrap().is_orphaned());
    for _ in 0..3 {
        assert!(undo.undo().unwrap());
        assert_eq!(doc.block(p).unwrap().text.to_string(), "abcdefg");
        assert_eq!(pieces(&doc, link), [(p, 2..5)]);
        assert_eq!(pieces(&doc, note), [(p, 3..6)]);
        assert!(undo.redo().unwrap());
        assert_eq!(doc.annotation(link).unwrap().state, AnnotationState::Gone);
        assert!(doc.annotation(note).unwrap().is_orphaned());
    }
    // Undo the deletion, then the creation: same IDs come back on redo.
    assert!(undo.undo().unwrap());
    assert!(undo.undo().unwrap());
    assert!(doc.annotation(link).is_none());
    assert!(undo.redo().unwrap());
    assert_eq!(pieces(&doc, link), [(p, 2..5)]);
}

#[test]
fn split_and_join_keep_one_annotation() {
    let (doc, p) = doc("one two three");
    let id = doc
        .add_annotation(p, 2..9, &url("https://a.example"))
        .unwrap();
    let tail = doc.split_block(p, 5).unwrap();
    assert_eq!(pieces(&doc, id), [(p, 2..5), (tail, 0..4)]);
    assert_eq!(doc.annotations_at(tail, 0).annotations[0].id, id);
    doc.join_blocks(p, tail).unwrap();
    assert_eq!(pieces(&doc, id), [(p, 2..9)]);
}

#[test]
fn copying_joins_and_moves_keep_identity() {
    let doc = Document::new(1).unwrap();
    let a = doc.append_block(BlockKind::Paragraph, "", "first").unwrap();
    let b = doc
        .append_block(BlockKind::Paragraph, "", "second")
        .unwrap();
    doc.commit();
    let mut undo = doc.undo_stack();
    let id = doc
        .add_annotation(b, 1..4, &url("https://a.example"))
        .unwrap();
    doc.commit_step();
    // Different hosts: the join copies the text.
    doc.join_blocks(a, b).unwrap();
    doc.commit_step();
    assert_eq!(pieces(&doc, id), [(a, 6..9)]);
    assert!(undo.undo().unwrap());
    assert_eq!(pieces(&doc, id), [(b, 1..4)]);
    assert!(undo.redo().unwrap());
    assert_eq!(pieces(&doc, id), [(a, 6..9)]);
}

#[test]
fn overlapping_annotations_are_independent() {
    let (doc, p) = doc("abcdefghij");
    let ids: Vec<_> = (0..5)
        .map(|i| {
            doc.add_annotation(p, i..i + 5, &comment(&format!("t{i}")))
                .unwrap()
        })
        .collect();
    let at = doc.annotations_at(p, 4);
    assert_eq!(at.annotations.iter().map(|a| a.id).collect::<Vec<_>>(), ids);
    assert_eq!(doc.annotations_in(p, 0..1).annotations.len(), 1);
    assert_eq!(doc.annotations_in(p, 8..9).annotations.len(), 1);
    doc.remove_annotation(ids[0]).unwrap();
    assert!(doc.annotation(ids[0]).is_none());
    assert_eq!(doc.annotations_at(p, 4).annotations.len(), 4);
}

#[test]
fn concurrent_payload_edit_and_deletion_converge() {
    let (a, p) = doc("abcdefg");
    let link = a
        .add_annotation(p, 2..5, &url("https://a.example"))
        .unwrap();
    let note = a.add_annotation(p, 2..5, &comment("t1")).unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    a.delete_text(p, 1..6).unwrap();
    b.update_annotation(link, &url("https://b.example"))
        .unwrap();
    b.update_annotation(note, &comment("t2")).unwrap();
    b.block(p).unwrap().text.insert(3, "Q").unwrap();
    a.commit();
    b.commit();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    for d in [&a, &b] {
        assert_eq!(d.block(p).unwrap().text.to_string(), "aQg");
        assert_eq!(d.annotation(link).unwrap().data, url("https://b.example"));
        assert_eq!(d.annotation(note).unwrap().data, comment("t2"));
    }
    // The concurrent typing landed inside: the comment holds it, the link too.
    assert_eq!(a.annotation(note), b.annotation(note));
    assert_eq!(a.annotation(link), b.annotation(link));
}

#[test]
fn concurrent_overlapping_additions_both_survive() {
    let (a, p) = doc("abcdefg");
    let b = a.fork(2).unwrap();
    let x = a.add_annotation(p, 1..5, &comment("x")).unwrap();
    let y = b.add_annotation(p, 3..7, &comment("y")).unwrap();
    a.commit();
    b.commit();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    assert_ne!(x, y);
    assert_eq!(a.annotation(x).unwrap().author, 1);
    assert_eq!(a.annotation(y).unwrap().author, 2);
    for d in [&a, &b] {
        assert_eq!(d.annotations_at(p, 4).annotations.len(), 2);
    }
}

#[test]
fn invalid_authoring_writes_nothing() {
    let (doc, p) = doc("abc");
    doc.commit();
    let before = doc.revision();
    for (bytes, data, code) in [
        (0..0, url("https://a"), EMPTY),
        (0..2, url("javascript:alert(1)"), LINK_SCHEME),
        (0..2, comment("bad thread"), PAYLOAD_INVALID),
        (0..2, comment(""), PAYLOAD_INVALID),
    ] {
        assert_eq!(
            doc.add_annotation(p, bytes, &data).unwrap_err().code(),
            Some(code)
        );
    }
    assert!(doc.add_annotation(p, 0..9, &url("https://a")).is_err());
    let dangling = AnnotationData::Link(LinkTarget::Range {
        range: crate::RangeId::parse("5@77").unwrap(),
    });
    assert!(doc.add_annotation(p, 0..1, &dangling).is_err());
    doc.commit();
    assert_eq!(doc.revision(), before);
    assert!(!doc.has_annotations());
}

#[test]
fn hostile_envelopes_are_reported_and_kept_verbatim() {
    let (doc, p) = doc("abc");
    let id = doc
        .add_annotation(p, 0..3, &url("https://a.example"))
        .unwrap();
    let meta = doc.tree("ranges").get_meta(id.0).unwrap();
    for raw in [
        r#"{"version":2,"kind":"link","payload":{"type":"url","url":"https://a"}}"#.to_string(),
        r#"{"version":1,"kind":"future","payload":{}}"#.to_string(),
        r#"{"version":1,"kind":"link","payload":{"type":"url","url":"javascript:x"}}"#.to_string(),
        r#"{"version":1,"kind":"link","payload":{"type":"url","url":"https://a","x":1}}"#
            .to_string(),
        r#"{"version":1,"kind":"comment","payload":{"thread":"a b"}}"#.to_string(),
        "[".repeat(10_000),
        format!(
            r#"{{"version":1,"kind":"link","payload":{{"type":"url","url":"https://{}"}}}}"#,
            "a".repeat(1 << 20)
        ),
    ] {
        meta.insert(KEY, raw.clone()).unwrap();
        doc.commit();
        let before = doc.revision();
        assert!(doc.annotation(id).is_none());
        assert!(doc.annotation_notes().iter().any(|n| n.code == UNREADABLE));
        assert_eq!(doc.annotation_count(), 1);
        assert!(doc.update_annotation(id, &url("https://b")).is_err());
        assert_eq!(doc.revision(), before);
        assert_eq!(crate::get_str(&meta, KEY), Some(raw));
        assert!(doc.has_annotations());
    }
    meta.insert(KEY, 5_i64).unwrap();
    assert!(doc.annotation(id).is_none());
    doc.remove_annotation(id).unwrap();
}

#[test]
fn round_trips_both_persistence_modes() {
    let (doc, p) = doc("abcdef");
    let link = doc
        .add_annotation(p, 1..3, &url("https://a.example"))
        .unwrap();
    let note = doc.add_annotation(p, 2..5, &comment("t1")).unwrap();
    for mode in [PersistenceMode::History, PersistenceMode::Shallow] {
        let back = Document::import(&doc.try_export(mode).unwrap(), 3).unwrap();
        assert_eq!(back.annotation(link), doc.annotation(link));
        assert_eq!(back.annotation(note), doc.annotation(note));
    }
}

#[test]
fn a_hundred_thousand_annotations_are_bounded() {
    let (doc, p) = doc("abcdefghij");
    let raw = url("https://a.example").envelope().unwrap();
    let tree = doc.tree("ranges");
    // A hostile peer's records: stored anchors copied from nine real ranges.
    let templates: Vec<_> = (0..9)
        .map(|i| {
            let id = doc.add_range(p, i..i + 1, RangePolicy::FIXED).unwrap();
            let meta = tree.get_meta(id.0).unwrap();
            let get = |k| crate::get_str(&meta, k).unwrap();
            (get("node"), get("start"), get("end"), get("policy1"))
        })
        .collect();
    for i in 0..100_000usize {
        let (node, start, end, policy) = &templates[i % 9];
        let id = tree.create(None).unwrap();
        let meta = tree.get_meta(id).unwrap();
        meta.insert("node", node.as_str()).unwrap();
        meta.insert("start", start.as_str()).unwrap();
        meta.insert("end", end.as_str()).unwrap();
        meta.insert("policy1", policy.as_str()).unwrap();
        meta.insert(KEY, raw.as_str()).unwrap();
    }
    doc.commit();
    let before = doc.revision();
    assert!(doc.annotation_notes().iter().any(|n| n.code == LIMIT));
    assert_eq!(doc.annotation_count(), 100_000);
    let at = doc.annotations_at(p, 0);
    assert!(at.truncated);
    assert_eq!(at.annotations.len(), MAX_QUERY_RESULTS);
    assert!(doc.annotations_of_kind(Kind::Link).truncated);
    assert_eq!(
        doc.add_annotation(p, 0..1, &url("https://a"))
            .unwrap_err()
            .code(),
        Some(LIMIT)
    );
    assert_eq!(doc.revision(), before);
}

/// The index resolves anchors per host; it must agree with `range_extent`.
#[test]
fn indexed_resolution_matches_range_extent_under_flow_edits() {
    let (doc, head) = doc("alpha beta gamma delta epsilon zeta eta theta");
    let mut ids = Vec::new();
    for (i, (s, e)) in [(0, 5), (3, 14), (6, 30), (20, 44), (10, 11), (40, 45)]
        .into_iter()
        .enumerate()
    {
        let data = if i % 2 == 0 {
            url("https://a.example")
        } else {
            comment("t")
        };
        ids.push(doc.add_annotation(head, s..e, &data).unwrap());
    }
    let mut seed = 7u64;
    let mut next = |n: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n.max(1)
    };
    for round in 0..60 {
        let paras = doc.children(None);
        let p = paras[next(paras.len())];
        let len = doc.block(p).unwrap().text.len();
        match round % 5 {
            0 if len > 1 => {
                let at = next(len);
                let at = (0..=at)
                    .rev()
                    .find(|&i| doc.block(p).unwrap().text.to_string().is_char_boundary(i))
                    .unwrap();
                doc.split_block(p, at).unwrap();
            }
            1 if paras.len() > 1 => {
                let i = next(paras.len() - 1);
                let _ = doc.join_blocks(paras[i], paras[i + 1]);
            }
            2 => {
                doc.block(p)
                    .unwrap()
                    .text
                    .insert(next(len + 1).min(len), "xy")
                    .unwrap();
            }
            3 if len > 2 => {
                let s = next(len - 1);
                doc.delete_text(p, s..(s + 2).min(len)).unwrap();
            }
            4 if paras.len() > 2 => {
                doc.delete_block(p).unwrap();
            }
            _ => {}
        }
        doc.commit();
        for &id in &ids {
            let expected = doc.range_extent(id).map(|x| (x.start, x.end));
            match doc.annotation(id).map(|a| a.state) {
                Some(AnnotationState::Active { pieces, .. }) => {
                    let (s, e) = expected.expect("index active, extent missing");
                    let first = pieces.first().unwrap();
                    let last = pieces.last().unwrap();
                    assert_eq!((first.node, first.bytes.start), s, "round {round}");
                    assert_eq!((last.node, last.bytes.end), e, "round {round}");
                }
                Some(AnnotationState::Orphaned { node: Some(n), at }) => {
                    if let Some((s, e)) = expected {
                        assert_eq!(s, (n, at), "round {round}");
                        assert!(s == e || e.1 == 0 || s.1 == doc.block(s.0).unwrap().text.len());
                    }
                }
                _ => {}
            }
        }
    }
}

#[test]
fn undo_annotations_after_deleted_atom() {
    use crate::annotation::{AnnotationData, CommentAnchor, LinkTarget};
    use crate::{BlockKind as BK, Document, InlineObject};
    let doc = Document::new(1).unwrap();
    let p = doc.append_block(BK::Paragraph, "", "abcd").unwrap();
    doc.insert_inline(p, 2, &InlineObject::field("text").with("text", "x"))
        .unwrap();
    doc.commit_step();
    doc.delete_text(p, 2..5).unwrap();
    doc.commit_step();
    let link = doc
        .add_annotation(
            p,
            1..3,
            &AnnotationData::Link(LinkTarget::Url {
                url: "https://example.com".into(),
            }),
        )
        .unwrap();
    let comment = doc
        .add_annotation(
            p,
            1..3,
            &AnnotationData::Comment(CommentAnchor {
                thread: "thread".into(),
            }),
        )
        .unwrap();
    doc.commit_step();
    let before_link = doc.annotation(link).unwrap();
    let before_comment = doc.annotation(comment).unwrap();
    let mut undo = doc.undo_stack();
    doc.delete_text(p, 0..4).unwrap();
    doc.commit_step();
    for _ in 0..3 {
        assert!(undo.undo().unwrap());
        assert_eq!(doc.block(p).unwrap().text.to_string(), "abcd");
        println!(
            "link before={before_link:?} after={:?}; comment before={before_comment:?} after={:?}",
            doc.annotation(link),
            doc.annotation(comment)
        );
        assert_eq!(doc.annotation(link).unwrap(), before_link);
        assert_eq!(doc.annotation(comment).unwrap(), before_comment);
        assert!(undo.redo().unwrap());
        assert_eq!(doc.block(p).unwrap().text.to_string(), "");
    }
    let remote = doc.fork(2).unwrap();
    let changed = AnnotationData::Comment(CommentAnchor {
        thread: "remote-edit".into(),
    });
    remote.update_annotation(comment, &changed).unwrap();
    remote.commit();
    doc.merge(&remote).unwrap();
    assert!(undo.undo().unwrap());
    assert_eq!(doc.annotation(comment).unwrap().data, changed);
    assert_eq!(doc.annotation(comment).unwrap().state, before_comment.state);
}

/// `paragraphs` paragraphs of "ab" in one flow, and the last paragraph.
fn flow_of(paragraphs: usize) -> (Document, NodeId, NodeId) {
    let (doc, head) = doc(&"ab".repeat(paragraphs));
    let mut tail = head;
    for _ in 1..paragraphs {
        tail = doc.split_block(tail, 2).unwrap();
    }
    doc.commit();
    (doc, head, tail)
}

/// Steps the next annotation index build takes (see `annotation::charge`): the
/// flows are read first so only endpoint placement is counted.
fn index_steps(doc: &Document, hosts: &[NodeId]) -> usize {
    for host in hosts {
        let _ = doc.flow(host.node);
    }
    take_steps();
    let _ = doc.annotation_index();
    take_steps()
}

/// R6: endpoint placement costs the paragraphs once per host plus a binary
/// search per endpoint. A scan per endpoint would cost paragraphs x annotations.
#[test]
fn endpoint_placement_does_not_scan_the_flow_per_annotation() {
    const ANNOTATIONS: usize = 40;
    for paragraphs in [32usize, 256] {
        let (doc, head, tail) = flow_of(paragraphs);
        let ids: Vec<_> = (0..ANNOTATIONS)
            .map(|i| {
                let (s, e) = [(0, 1), (1, 2), (0, 2)][i % 3];
                doc.add_annotation(tail, s..e, &comment("t")).unwrap()
            })
            .collect();
        doc.commit();
        let steps = index_steps(&doc, &[head]);
        let budget = paragraphs + 2 * ANNOTATIONS * (paragraphs.ilog2() as usize + 2);
        assert!(
            steps <= budget,
            "{paragraphs} paragraphs, {ANNOTATIONS} annotations: {steps} steps (budget {budget})"
        );
        for id in ids {
            assert_eq!(pieces(&doc, id).first().map(|(n, _)| *n), Some(tail));
        }
    }
}

/// R6, across hosts: an annotation from one top-level paragraph into the next
/// is placed from one document order read per build, not one per annotation.
#[test]
fn cross_host_annotations_read_the_document_order_once() {
    const ANNOTATIONS: usize = 30;
    for hosts in [40usize, 240] {
        let doc = Document::new(1).unwrap();
        let nodes: Vec<_> = (0..hosts)
            .map(|_| doc.append_block(BlockKind::Paragraph, "", "abcd").unwrap())
            .collect();
        doc.commit();
        let ids: Vec<_> = (0..ANNOTATIONS)
            .map(|i| {
                let id = doc
                    .add_annotation(nodes[i], 1..3, &comment("t"))
                    .unwrap();
                doc.stage_fragment_range_end(
                    crate::RangeId(id.0),
                    nodes[i + 1],
                    2,
                    RangePolicy::FIXED,
                )
                .unwrap();
                id
            })
            .collect();
        doc.commit();
        let steps = index_steps(&doc, &nodes);
        let budget = 2 * hosts + 8 * ANNOTATIONS;
        assert!(
            steps <= budget,
            "{hosts} hosts, {ANNOTATIONS} annotations: {steps} steps (budget {budget})"
        );
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(
                pieces(&doc, *id),
                vec![(nodes[i], 1..4), (nodes[i + 1], 0..2)]
            );
        }
    }
}
