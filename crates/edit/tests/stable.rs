//! Stable carets across remote edits, and per-user undo across merges (29).

use reprise_doc::{BlockKind, Document, NodeId};
use reprise_edit::{
    Affinity, Caret, Command, Editor, Resolution, Selection, StableCaret, StableSelection,
};
use reprise_fixtures::{OTHER_PEER, PEER, engine};

fn editor(peer: u64, base: &Document) -> Editor {
    Editor::new(base.fork(peer).unwrap(), engine().schemas.clone())
}

fn base(texts: &[&str]) -> (Document, Vec<NodeId>) {
    let doc = Document::new(PEER).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    let ids = texts
        .iter()
        .map(|t| doc.append_block(BlockKind::Paragraph, "body", t).unwrap())
        .collect();
    doc.commit();
    (doc, ids)
}

fn sync(from: &Editor, to: &mut Editor) {
    let packet = from
        .document()
        .export_delta(&to.document().version_vector())
        .unwrap();
    to.import_packet(&packet).unwrap();
}

fn caret(node: NodeId, offset: usize, affinity: Affinity) -> Caret {
    Caret {
        node,
        offset,
        affinity,
    }
}

fn text(e: &Editor, n: NodeId) -> String {
    e.document().block(n).unwrap().text.to_string()
}

#[test]
fn caret_from_before_a_split_follows_a_later_copy_move_of_the_tail() {
    let (doc, nodes) = base(&["abcd"]);
    let original = StableCaret::anchor(&doc, caret(nodes[0], 3, Affinity::Downstream)).unwrap();
    let mut e = Editor::new(doc, engine().schemas.clone());
    let tail = e
        .apply_command(Command::SplitBlock {
            node: nodes[0],
            at: 2,
        })
        .unwrap()
        .blocks[0];
    let moved = e
        .apply_command(Command::MoveBlock {
            node: tail,
            parent: None,
            index: 0,
        })
        .unwrap()
        .blocks[0];
    assert_eq!(
        original.resolve(e.document()),
        Resolution::Exact(caret(moved, 1, Affinity::Downstream))
    );
    e.undo().unwrap();
    assert_eq!(
        original.resolve(e.document()),
        Resolution::Exact(caret(tail, 1, Affinity::Downstream))
    );
    e.redo().unwrap();
    assert_eq!(
        original.resolve(e.document()),
        Resolution::Exact(caret(moved, 1, Affinity::Downstream))
    );
}

#[test]
fn carets_follow_remote_insertions_by_affinity() {
    let (doc, ids) = base(&["hello world"]);
    let (mut a, mut b) = (editor(PEER, &doc), editor(OTHER_PEER, &doc));
    let p = ids[0];
    let down = StableCaret::anchor(a.document(), caret(p, 5, Affinity::Downstream)).unwrap();
    let up = StableCaret::anchor(a.document(), caret(p, 5, Affinity::Upstream)).unwrap();
    let before = StableCaret::anchor(a.document(), caret(p, 2, Affinity::Downstream)).unwrap();
    b.apply_command(Command::InsertText {
        node: p,
        at: 5,
        text: "XYZ".into(),
    })
    .unwrap();
    b.apply_command(Command::InsertText {
        node: p,
        at: 0,
        text: ">>".into(),
    })
    .unwrap();
    sync(&b, &mut a);
    assert_eq!(text(&a, p), ">>helloXYZ world");
    assert_eq!(
        down.resolve(a.document()),
        Resolution::Exact(caret(p, 10, Affinity::Downstream))
    );
    assert_eq!(
        up.resolve(a.document()),
        Resolution::Exact(caret(p, 7, Affinity::Upstream))
    );
    assert_eq!(
        before.resolve(a.document()),
        Resolution::Exact(caret(p, 4, Affinity::Downstream))
    );
}

#[test]
fn deleted_characters_and_blocks_take_the_documented_fallback() {
    let (doc, ids) = base(&["first", "middle", "last"]);
    let (mut a, mut b) = (editor(PEER, &doc), editor(OTHER_PEER, &doc));
    let at = |n, o| StableCaret::anchor(a.document(), caret(n, o, Affinity::Downstream)).unwrap();
    let in_word = at(ids[1], 3);
    let in_middle = at(ids[1], 1);
    let in_first = at(ids[0], 2);
    let in_last = at(ids[2], 4);
    // The anchored character goes: its neighbours' gap.
    b.apply_command(Command::DeleteText {
        node: ids[1],
        range: 2..4,
    })
    .unwrap();
    sync(&b, &mut a);
    assert_eq!(
        in_word.resolve(a.document()),
        Resolution::Moved(caret(ids[1], 2, Affinity::Downstream))
    );
    // The block goes: the end of the nearest live caret block before it.
    b.apply_command(Command::DeleteBlock { node: ids[1] })
        .unwrap();
    sync(&b, &mut a);
    assert_eq!(
        in_middle.resolve(a.document()),
        Resolution::Moved(caret(ids[0], 5, Affinity::Downstream))
    );
    // Nothing before it: the start of the nearest after it.
    b.apply_command(Command::DeleteBlock { node: ids[0] })
        .unwrap();
    sync(&b, &mut a);
    assert_eq!(
        in_first.resolve(a.document()),
        Resolution::Moved(caret(ids[2], 0, Affinity::Downstream))
    );
    // Nothing left at all.
    b.apply_command(Command::DeleteBlock { node: ids[2] })
        .unwrap();
    sync(&b, &mut a);
    assert_eq!(in_last.resolve(a.document()), Resolution::Gone);
}

#[test]
fn a_joined_block_maps_to_its_successor() {
    let (doc, ids) = base(&["abc", "defgh"]);
    let (mut a, mut b) = (editor(PEER, &doc), editor(OTHER_PEER, &doc));
    let in_second =
        StableCaret::anchor(a.document(), caret(ids[1], 2, Affinity::Downstream)).unwrap();
    b.apply_command(Command::JoinBlocks {
        first: ids[0],
        second: ids[1],
    })
    .unwrap();
    sync(&b, &mut a);
    assert_eq!(text(&a, ids[0]), "abcdefgh");
    assert_eq!(
        in_second.resolve(a.document()),
        Resolution::Exact(caret(ids[0], 5, Affinity::Downstream)),
        "exact for a join with no later edits"
    );
}

#[test]
fn a_caret_split_from_its_cluster_is_floored_to_a_grapheme_boundary() {
    let (doc, ids) = base(&["ab"]);
    let (mut a, mut b) = (editor(PEER, &doc), editor(OTHER_PEER, &doc));
    let up = StableCaret::anchor(a.document(), caret(ids[0], 1, Affinity::Upstream)).unwrap();
    let down = StableCaret::anchor(a.document(), caret(ids[0], 1, Affinity::Downstream)).unwrap();
    b.apply_command(Command::InsertText {
        node: ids[0],
        at: 1,
        text: "\u{301}".into(),
    })
    .unwrap();
    sync(&b, &mut a);
    assert_eq!(
        up.resolve(a.document()),
        Resolution::Moved(caret(ids[0], 0, Affinity::Upstream))
    );
    assert_eq!(
        down.resolve(a.document()),
        Resolution::Exact(caret(ids[0], 3, Affinity::Downstream))
    );
}

#[test]
fn garbage_and_foreign_anchors_degrade_without_panicking() {
    let (doc, ids) = base(&["text", "more"]);
    let other = {
        let d = Document::new(5).unwrap();
        let n = d
            .append_block(BlockKind::Paragraph, "", "elsewhere")
            .unwrap();
        d.commit();
        (d, vec![n])
    };
    let foreign =
        StableCaret::anchor(&other.0, caret(other.1[0], 3, Affinity::Downstream)).unwrap();
    let mut seed = 7_u64;
    for len in 0..300 {
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                (seed >> 33) as u8
            })
            .collect();
        let garbage = StableCaret {
            node: ids[1],
            anchor: bytes,
            affinity: Affinity::Upstream,
        };
        assert!(garbage.resolve(&doc).caret().is_some());
    }
    let misplaced = StableCaret {
        node: ids[0],
        ..foreign.clone()
    };
    assert_eq!(
        misplaced.resolve(&doc),
        Resolution::Moved(caret(ids[0], 0, Affinity::Downstream))
    );
    // An anchor whose node isn't in this document: no live position to start from.
    assert!(foreign.resolve(&doc).caret().is_some());
}

#[test]
fn selections_resolve_identically_on_every_replica_after_concurrent_edits() {
    let (doc, ids) = base(&["the quick brown fox", "jumps over", "the lazy dog"]);
    let mut peers: Vec<Editor> = (1..=3).map(|p| editor(p, &doc)).collect();
    let selection = Selection {
        anchor: caret(ids[0], 4, Affinity::Downstream),
        focus: caret(ids[2], 8, Affinity::Upstream),
    };
    let stable = StableSelection::anchor(peers[0].document(), &selection).unwrap();
    let mut seed = 99_u64;
    for round in 0..40 {
        for (i, e) in peers.iter_mut().enumerate() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(i as u64 + 1);
            let live = e.document().blocks();
            let node = live[(seed >> 40) as usize % live.len()];
            let len = e.document().block(node).unwrap().text.len();
            let at = (seed >> 20) as usize % (len + 1);
            let command = match (seed >> 50) % 4 {
                0 if len > 0 => Command::DeleteText {
                    node,
                    range: at.min(len - 1)..len.min(at + 3).max(at.min(len - 1) + 1),
                },
                1 if round % 13 == 0 && live.len() > 2 => Command::DeleteBlock { node },
                _ => Command::InsertText {
                    node,
                    at,
                    text: "\u{5d0}e".into(),
                },
            };
            let _ = e.apply_command(command);
        }
        let packets: Vec<_> = peers
            .iter()
            .map(|e| e.document().export_delta(&[]).unwrap())
            .collect();
        for e in &mut peers {
            for p in &packets {
                e.import_packet(p).unwrap();
            }
        }
        let resolved: Vec<_> = peers.iter().map(|e| stable.resolve(e.document())).collect();
        assert!(
            resolved.windows(2).all(|w| w[0] == w[1]),
            "round {round}: {resolved:?}"
        );
    }
}

#[test]
fn undo_after_interleaved_remote_edits_removes_only_local_text() {
    let (doc, ids) = base(&["base"]);
    let (mut a, mut b) = (editor(PEER, &doc), editor(OTHER_PEER, &doc));
    let p = ids[0];
    for i in 0..5 {
        a.apply_command(Command::InsertText {
            node: p,
            at: 0,
            text: format!("a{i}"),
        })
        .unwrap();
        sync(&a, &mut b);
        let end = b.document().block(p).unwrap().text.len();
        b.apply_command(Command::InsertText {
            node: p,
            at: end,
            text: format!("b{i}"),
        })
        .unwrap();
        sync(&b, &mut a);
    }
    assert_eq!(text(&a, p), "a4a3a2a1a0baseb0b1b2b3b4");
    while a.can_undo() {
        let (done, report) = a.undo_report().unwrap();
        assert!(done && report.blocks.contains(&p));
    }
    assert_eq!(text(&a, p), "baseb0b1b2b3b4", "remote text stays");
    // Redo after another remote edit restores only local steps.
    b.apply_command(Command::InsertText {
        node: p,
        at: 0,
        text: "R".into(),
    })
    .unwrap();
    sync(&b, &mut a);
    assert!(a.redo_report().unwrap().0);
    assert_eq!(text(&a, p), "a0Rbaseb0b1b2b3b4");
    sync(&a, &mut b);
    assert_eq!(text(&a, p), text(&b, p));
}

#[test]
fn undo_stack_stays_bounded_across_hundreds_of_merges() {
    let (doc, ids) = base(&["base"]);
    let (mut a, mut b) = (editor(PEER, &doc), editor(OTHER_PEER, &doc));
    a.set_undo_limit(20);
    for i in 0..600 {
        b.apply_command(Command::InsertText {
            node: ids[0],
            at: 0,
            text: "r".into(),
        })
        .unwrap();
        sync(&b, &mut a);
        if i % 10 == 0 {
            a.apply_command(Command::InsertText {
                node: ids[0],
                at: 0,
                text: "l".into(),
            })
            .unwrap();
        }
    }
    assert_eq!(a.undo_count(), 20, "merges add no steps; the limit holds");
    while a.can_undo() {
        a.undo().unwrap();
    }
    let s = text(&a, ids[0]);
    assert_eq!(s.matches('l').count(), 60 - 20);
    assert_eq!(s.matches('r').count(), 600);
}
