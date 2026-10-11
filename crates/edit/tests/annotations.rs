//! Annotations through the editing kernel: commands, undo, paste and refusal.
use reprise_doc::annotation::{
    AnnotationData, AnnotationState, CommentAnchor, Kind, LINK_SCHEME, LinkTarget, MISSING,
};
use reprise_doc::fragment::{ANNOTATION_DROPPED, CopyBlock};
use reprise_doc::{BlockKind, Document, NodeId, SchemaRegistry};
use reprise_edit::{Command, Editor, Reason, Transaction};

fn editor(texts: &[&str]) -> (Editor, Vec<NodeId>) {
    let doc = Document::new(1).unwrap();
    let nodes = texts
        .iter()
        .map(|t| doc.append_block(BlockKind::Paragraph, "", t).unwrap())
        .collect();
    doc.commit();
    (Editor::new(doc, SchemaRegistry::builtin()), nodes)
}

fn url(u: &str) -> AnnotationData {
    AnnotationData::Link(LinkTarget::Url { url: u.into() })
}

fn comment(t: &str) -> AnnotationData {
    AnnotationData::Comment(CommentAnchor { thread: t.into() })
}

fn span(doc: &Document, id: reprise_doc::RangeId) -> Option<Vec<(NodeId, std::ops::Range<usize>)>> {
    match doc.annotation(id)?.state {
        AnnotationState::Active { pieces, .. } => {
            Some(pieces.into_iter().map(|p| (p.node, p.bytes)).collect())
        }
        _ => None,
    }
}

#[test]
fn add_update_remove_are_single_undo_steps() {
    let (mut ed, n) = editor(&["read the docs"]);
    let p = n[0];
    let applied = ed
        .apply_command(Command::AddAnnotation {
            node: p,
            range: 9..13,
            data: url("https://docs.example"),
        })
        .unwrap();
    let id = applied.annotations[0];
    assert_eq!(span(ed.document(), id), Some(vec![(p, 9..13)]));
    ed.apply_command(Command::UpdateAnnotation {
        id,
        data: url("https://other.example"),
    })
    .unwrap();
    assert_eq!(
        ed.document().annotation(id).unwrap().data,
        url("https://other.example")
    );
    ed.apply_command(Command::RemoveAnnotation { id }).unwrap();
    assert!(ed.document().annotation(id).is_none());
    ed.undo().unwrap();
    assert_eq!(
        ed.document().annotation(id).unwrap().data,
        url("https://other.example")
    );
    ed.undo().unwrap();
    assert_eq!(
        ed.document().annotation(id).unwrap().data,
        url("https://docs.example")
    );
    ed.undo().unwrap();
    assert!(ed.document().annotation(id).is_none());
    ed.redo().unwrap();
    assert_eq!(span(ed.document(), id), Some(vec![(p, 9..13)]));
}

#[test]
fn typing_at_edges_and_inside_follows_each_kind() {
    let (mut ed, n) = editor(&["one two three"]);
    let p = n[0];
    let tx = Transaction::new()
        .with(Command::AddAnnotation {
            node: p,
            range: 4..7,
            data: url("https://a.example"),
        })
        .with(Command::AddAnnotation {
            node: p,
            range: 4..7,
            data: comment("c1"),
        });
    let ids = ed.apply(&tx).unwrap().annotations;
    let insert = |ed: &mut Editor, at, text: &str| {
        ed.apply_command(Command::InsertText {
            node: p,
            at,
            text: text.into(),
        })
        .unwrap();
    };
    insert(&mut ed, 7, "!"); // end
    insert(&mut ed, 4, "<"); // start
    insert(&mut ed, 6, "w"); // inside
    assert_eq!(ed.document().block(p).unwrap().text.to_string(), "one <twwo! three");
    assert_eq!(span(ed.document(), ids[0]), Some(vec![(p, 5..9)]));
    assert_eq!(span(ed.document(), ids[1]), Some(vec![(p, 5..10)]));
}

#[test]
fn deleting_a_comments_text_then_undo_restores_the_anchor() {
    let (mut ed, n) = editor(&["keep this sentence here"]);
    let p = n[0];
    let id = ed
        .apply_command(Command::AddAnnotation {
            node: p,
            range: 5..18,
            data: comment("thread-1"),
        })
        .unwrap()
        .annotations[0];
    ed.apply_command(Command::DeleteText {
        node: p,
        range: 4..18,
    })
    .unwrap();
    let a = ed.document().annotation(id).unwrap();
    assert_eq!(
        a.state,
        AnnotationState::Orphaned {
            node: Some(p),
            at: 4
        }
    );
    assert_eq!(ed.document().annotations_of_kind(Kind::Comment).annotations.len(), 1);
    ed.undo().unwrap();
    assert_eq!(span(ed.document(), id), Some(vec![(p, 5..18)]));
    ed.redo().unwrap();
    assert!(ed.document().annotation(id).unwrap().is_orphaned());
    ed.undo().unwrap();
    assert_eq!(span(ed.document(), id), Some(vec![(p, 5..18)]));
}

#[test]
fn split_and_join_inside_a_link_keep_one_link() {
    let (mut ed, n) = editor(&["see the manual now"]);
    let p = n[0];
    let id = ed
        .apply_command(Command::AddAnnotation {
            node: p,
            range: 4..14,
            data: url("https://m.example"),
        })
        .unwrap()
        .annotations[0];
    let tail = ed
        .apply_command(Command::SplitBlock { node: p, at: 8 })
        .unwrap()
        .blocks[0];
    assert_eq!(span(ed.document(), id), Some(vec![(p, 4..8), (tail, 0..6)]));
    ed.apply_command(Command::JoinBlocks {
        first: p,
        second: tail,
    })
    .unwrap();
    assert_eq!(span(ed.document(), id), Some(vec![(p, 4..14)]));
    ed.undo().unwrap();
    ed.undo().unwrap();
    assert_eq!(span(ed.document(), id), Some(vec![(p, 4..14)]));
}

#[test]
fn paste_duplicates_links_drops_comments_and_remaps_internal_targets() {
    let (mut src, n) = editor(&["Intro", "see Intro and docs"]);
    let (intro, body) = (n[0], n[1]);
    let ids = src
        .apply(
            &Transaction::new()
                .with(Command::AddAnnotation {
                    node: body,
                    range: 4..9,
                    data: AnnotationData::Link(LinkTarget::Heading { node: intro }),
                })
                .with(Command::AddAnnotation {
                    node: body,
                    range: 14..18,
                    data: url("https://docs.example"),
                })
                .with(Command::AddAnnotation {
                    node: body,
                    range: 0..3,
                    data: comment("t9"),
                }),
        )
        .unwrap()
        .annotations;
    let schemas = SchemaRegistry::builtin();
    let whole = src.document().copy_all_fragment("doc-a", &schemas).unwrap();
    assert!(whole.notes.iter().any(|n| n.code == ANNOTATION_DROPPED));
    assert_eq!(
        whole.ranges.iter().filter(|r| r.annotation.is_some()).count(),
        2
    );
    // Into another document: the heading came along, so the link follows it.
    let (mut dst, _) = editor(&[]);
    let pasted = dst.paste(&whole, None, "doc-b").unwrap();
    let new_intro = pasted.ids.nodes[&intro];
    let heading_link = pasted.ids.ranges[&ids[0]];
    assert_eq!(
        dst.document().annotation(heading_link).unwrap().data,
        AnnotationData::Link(LinkTarget::Heading { node: new_intro })
    );
    let web = pasted.ids.ranges[&ids[1]];
    assert_eq!(
        dst.document().annotation(web).unwrap().data,
        url("https://docs.example")
    );
    assert!(dst.document().annotations_of_kind(Kind::Comment).annotations.is_empty());
    dst.undo().unwrap();
    assert!(dst.document().annotation(web).is_none());
    dst.redo().unwrap();
    assert!(dst.document().annotation(web).is_some());

    // Only the body: the heading target stays outside the copy.
    let part = src
        .document()
        .copy_fragment(
            "doc-a",
            &[CopyBlock {
                node: body,
                bytes: Some(0..18),
            }],
            &schemas,
        )
        .unwrap();
    let (mut other, _) = editor(&[]);
    let pasted = other.paste(&part, None, "doc-b").unwrap();
    assert!(pasted.notes.iter().any(|n| n.code == ANNOTATION_DROPPED));
    assert!(other.document().annotation(pasted.ids.ranges[&ids[0]]).is_none());
    // The same document keeps the outside target.
    let pasted = src.paste(&part, Some((intro, 5)), "doc-a").unwrap();
    assert_eq!(
        src.document()
            .annotation(pasted.ids.ranges[&ids[0]])
            .unwrap()
            .data,
        AnnotationData::Link(LinkTarget::Heading { node: intro })
    );
}

#[test]
fn refusals_carry_stable_codes_and_change_nothing() {
    let (mut ed, n) = editor(&["text"]);
    let p = n[0];
    let before = ed.document().revision();
    let err = ed
        .apply_command(Command::AddAnnotation {
            node: p,
            range: 0..4,
            data: url("javascript:alert(1)"),
        })
        .unwrap_err();
    assert_eq!(err.note().code, LINK_SCHEME);
    assert!(matches!(err.reason, Reason::Annotation(_)));
    let err = ed
        .apply_command(Command::RemoveAnnotation {
            id: reprise_doc::RangeId::parse("3@9").unwrap(),
        })
        .unwrap_err();
    assert_eq!(err.note().code, MISSING);
    // A whole transaction is refused if any command is.
    let err = ed
        .apply(
            &Transaction::new()
                .with(Command::AddAnnotation {
                    node: p,
                    range: 0..2,
                    data: url("https://ok.example"),
                })
                .with(Command::AddAnnotation {
                    node: p,
                    range: 1..1,
                    data: url("https://ok.example"),
                }),
        )
        .unwrap_err();
    assert_eq!(err.command, Some(1));
    assert_eq!(ed.document().revision(), before);
    assert!(!ed.document().has_annotations());
}
