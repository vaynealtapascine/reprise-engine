//! Links and comment anchors through the facade: commands, `State`, queries,
//! geometry, link areas on the display page, batches, copies and the hostile
//! cases (`docs/annotations.md`).
use reprise::*;

const DOC: &str = "00112233445566778899aabbccddeeff";
const COPY: &str = "ffeeddccbbaa99887766554433221100";

fn session(peer: &str) -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: peer.into(),
        }))
        .unwrap()
}

fn apply(s: &mut DocumentSession, commands: Vec<Command>) -> Applied {
    s.apply(&Payload::new(Transaction { commands }))
        .unwrap()
        .data
}

fn refused(s: &mut DocumentSession, commands: Vec<Command>) -> Error {
    s.apply(&Payload::new(Transaction { commands }))
        .unwrap_err()
}

fn paragraph(s: &mut DocumentSession, text: &str) -> String {
    let index = s.state().unwrap().data.blocks.len() as u32;
    apply(
        s,
        vec![Command::InsertBlock {
            parent: None,
            index,
            block_kind: BlockKind::Paragraph,
            text: text.into(),
            style: Style::default(),
        }],
    )
    .blocks[0]
        .clone()
}

fn finish(s: &mut DocumentSession) {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    for _ in 0..10_000 {
        if job.step(s, 1000).unwrap().data.complete {
            return;
        }
    }
    panic!("layout did not finish");
}

fn url(u: &str) -> AnnotationPayload {
    AnnotationPayload::Link {
        target: LinkDestination::Url { url: u.into() },
    }
}

fn thread(t: &str) -> AnnotationPayload {
    AnnotationPayload::Comment { thread: t.into() }
}

fn add(node: &str, start: u32, end: u32, annotation: AnnotationPayload) -> Command {
    Command::AddAnnotation {
        node: node.into(),
        start,
        end,
        annotation,
    }
}

fn get(s: &DocumentSession, id: &str) -> Option<Annotation> {
    s.annotation(&Payload::new(id.to_string())).unwrap().data
}

#[test]
fn add_update_remove_and_undo_through_apply() {
    let mut s = session("1");
    let p = paragraph(&mut s, "read the docs now");
    let applied = apply(
        &mut s,
        vec![
            add(&p, 9, 13, url("https://docs.example")),
            add(&p, 0, 4, thread("t-1")),
        ],
    );
    let (link, note) = (applied.annotations[0].clone(), applied.annotations[1].clone());
    assert_eq!(applied.annotations.len(), 2);
    let a = get(&s, &link).unwrap();
    assert_eq!(a.author, "1");
    assert_eq!(a.status, AnnotationStatus::Active);
    assert_eq!(
        a.spans,
        vec![AnnotationSpan {
            node: p.clone(),
            start: 9,
            end: 13
        }]
    );
    assert_eq!(a.payload, url("https://docs.example"));

    // State is compact: no address, but kind, place and thread.
    let state = s.state().unwrap().data;
    assert_eq!(state.annotations.len(), 2);
    let brief = state.annotations.iter().find(|b| b.id == note).unwrap();
    assert_eq!(brief.kind, AnnotationKind::Comment);
    assert_eq!(brief.thread.as_deref(), Some("t-1"));
    assert!(
        state
            .annotations
            .iter()
            .find(|b| b.id == link)
            .unwrap()
            .thread
            .is_none()
    );

    apply(
        &mut s,
        vec![Command::UpdateAnnotation {
            id: link.clone(),
            annotation: url("https://other.example"),
        }],
    );
    assert_eq!(get(&s, &link).unwrap().payload, url("https://other.example"));
    // A kind cannot change.
    let e = refused(
        &mut s,
        vec![Command::UpdateAnnotation {
            id: link.clone(),
            annotation: thread("t-2"),
        }],
    );
    assert_eq!(e.code(), "annotation.payload-invalid");
    apply(&mut s, vec![Command::RemoveAnnotation { id: link.clone() }]);
    assert!(get(&s, &link).is_none());
    assert_eq!(s.state().unwrap().data.annotations.len(), 1);
    // Each was one undo step; the ID comes back each time.
    assert!(s.undo().unwrap().data);
    assert_eq!(get(&s, &link).unwrap().payload, url("https://other.example"));
    assert!(s.undo().unwrap().data);
    assert_eq!(get(&s, &link).unwrap().payload, url("https://docs.example"));
    assert!(s.undo().unwrap().data);
    assert!(s.state().unwrap().data.annotations.is_empty());
    assert!(get(&s, &note).is_none());
    assert!(s.redo().unwrap().data);
    assert_eq!(get(&s, &link).unwrap().payload, url("https://docs.example"));
    assert!(get(&s, &note).is_some());
}

#[test]
fn a_link_with_its_style_is_one_undo_step() {
    let mut s = session("1");
    let p = paragraph(&mut s, "visit our site");
    let applied = apply(
        &mut s,
        vec![
            add(&p, 10, 14, url("https://site.example")),
            Command::FormatText {
                node: p.clone(),
                start: 10,
                end: 14,
                style: TextStyle {
                    decoration: Decoration {
                        underline: Some(true),
                        strike: None,
                    },
                    color: Some([0, 0, 238, 255]),
                    ..TextStyle::default()
                },
            },
        ],
    );
    let link = applied.annotations[0].clone();
    let state = s.state().unwrap().data;
    let runs = state.blocks[0].formatting.clone().unwrap();
    assert!(runs.iter().any(|r| r.start == 10 && r.style.decoration.underline == Some(true)));
    assert!(s.undo().unwrap().data);
    assert!(get(&s, &link).is_none());
    assert!(s.state().unwrap().data.blocks[0].formatting.is_none());
}

#[test]
fn refusals_carry_stable_codes_and_change_nothing() {
    let mut s = session("1");
    let p = paragraph(&mut s, "some text");
    let before = s.state().unwrap().data.revision;
    for (annotation, code) in [
        (url("javascript:alert(1)"), "annotation.link-scheme"),
        (url("data:text/html,x"), "annotation.link-scheme"),
        (url("/relative"), "annotation.link-invalid"),
        (url(&format!("https://a.example/{}", "x".repeat(9000))), "annotation.link-length"),
        (thread("has space"), "annotation.payload-invalid"),
        (thread(""), "annotation.payload-invalid"),
    ] {
        let e = refused(&mut s, vec![add(&p, 0, 4, annotation)]);
        assert_eq!(e.code(), code);
    }
    let e = refused(&mut s, vec![add(&p, 2, 2, url("https://a.example"))]);
    assert_eq!(e.code(), "annotation.empty");
    assert!(
        s.apply(&Payload::new(Transaction {
            commands: vec![add(&p, 0, 99, url("https://a.example"))]
        }))
        .is_err()
    );
    let e = refused(
        &mut s,
        vec![Command::RemoveAnnotation { id: "3@9".into() }],
    );
    assert_eq!(e.code(), "annotation.missing");
    let e = refused(
        &mut s,
        vec![Command::RemoveAnnotation {
            id: "not an id".into(),
        }],
    );
    assert_eq!(e.code(), "bindings.id");
    // A good command after a bad one in one transaction: nothing is written.
    let e = refused(
        &mut s,
        vec![
            add(&p, 0, 4, url("https://a.example")),
            add(&p, 0, 4, url("javascript:1")),
        ],
    );
    assert_eq!(e.code(), "annotation.link-scheme");
    assert!(
        matches!(&e, Error::Core { command: Some(1), .. }),
        "the refused command is named: {e:?}"
    );
    assert_eq!(s.state().unwrap().data.revision, before);
    assert!(s.state().unwrap().data.annotations.is_empty());
}

#[test]
fn queries_report_what_is_at_a_place() {
    let mut s = session("1");
    let a = paragraph(&mut s, "one two three four");
    let b = paragraph(&mut s, "second paragraph");
    let applied = apply(
        &mut s,
        vec![
            add(&a, 0, 7, url("https://one.example")),
            add(&a, 4, 13, thread("t-2")),
            add(&b, 0, 6, url("mailto:me@example.com")),
        ],
    );
    let ids = applied.annotations;
    let at = |node: &str, at: u32| {
        s.annotations_at(&Payload::new(AnnotationsAt {
            node: node.into(),
            at,
        }))
        .unwrap()
        .data
    };
    let ids_of = |list: &AnnotationList| list.annotations.iter().map(|a| a.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids_of(&at(&a, 5)), vec![ids[0].clone(), ids[1].clone()]);
    assert_eq!(ids_of(&at(&a, 2)), vec![ids[0].clone()]);
    assert_eq!(ids_of(&at(&a, 7)), vec![ids[1].clone()], "end is exclusive");
    assert!(at(&a, 14).annotations.is_empty());
    let within = s
        .annotations_in(&Payload::new(AnnotationsIn {
            node: a.clone(),
            start: 12,
            end: 18,
        }))
        .unwrap()
        .data;
    assert_eq!(ids_of(&within), vec![ids[1].clone()]);
    let links = s
        .annotations_of_kind(&Payload::new(AnnotationKind::Link))
        .unwrap()
        .data;
    assert_eq!(ids_of(&links), vec![ids[0].clone(), ids[2].clone()]);
    assert!(!links.truncated);
    let comments = s
        .annotations_of_kind(&Payload::new(AnnotationKind::Comment))
        .unwrap()
        .data;
    assert_eq!(comments.annotations.len(), 1);
    assert!(
        s.annotations_at(&Payload::new(AnnotationsAt {
            node: "nonsense".into(),
            at: 0
        }))
        .is_err()
    );
}

#[test]
fn a_deleted_comment_is_orphaned_and_undo_brings_the_anchor_back() {
    let mut s = session("1");
    let p = paragraph(&mut s, "keep this sentence here");
    let applied = apply(
        &mut s,
        vec![
            add(&p, 5, 18, thread("t-1")),
            add(&p, 5, 18, url("https://gone.example")),
        ],
    );
    let (note, link) = (applied.annotations[0].clone(), applied.annotations[1].clone());
    apply(
        &mut s,
        vec![Command::DeleteText {
            node: p.clone(),
            start: 4,
            end: 18,
        }],
    );
    let a = get(&s, &note).unwrap();
    assert_eq!(a.status, AnnotationStatus::Orphaned);
    assert_eq!(
        a.spans,
        vec![AnnotationSpan {
            node: p.clone(),
            start: 4,
            end: 4
        }]
    );
    assert!(get(&s, &link).is_none(), "a link whose text is gone disappears");
    let state = s.state().unwrap().data;
    assert_eq!(state.annotations.len(), 1);
    assert_eq!(state.annotations[0].status, AnnotationStatus::Orphaned);
    finish(&mut s);
    let geometry = s
        .annotation_geometry(&Payload::new(note.clone()))
        .unwrap()
        .data
        .unwrap();
    assert_eq!(geometry.rects.len(), 1);
    assert_eq!(geometry.rects[0].frame_rect.width, 0);
    assert!(s.undo().unwrap().data);
    let a = get(&s, &note).unwrap();
    assert_eq!(a.status, AnnotationStatus::Active);
    assert_eq!(a.spans[0].start, 5);
    assert_eq!(a.spans[0].end, 18);
    assert_eq!(get(&s, &link).unwrap().status, AnnotationStatus::Active);
}

#[test]
fn geometry_and_link_areas_need_a_layout_and_follow_the_text() {
    let mut s = session("1");
    let p = paragraph(&mut s, "read the manual before you start");
    let applied = apply(
        &mut s,
        vec![
            add(&p, 9, 15, url("https://manual.example")),
            add(&p, 0, 4, thread("t-1")),
        ],
    );
    let (link, note) = (applied.annotations[0].clone(), applied.annotations[1].clone());
    // No layout yet.
    assert_eq!(
        s.annotation_geometry(&Payload::new(link.clone())).unwrap_err().code(),
        "bindings.layout-required"
    );
    assert_eq!(s.page_links(0).unwrap_err().code(), "bindings.layout-required");
    finish(&mut s);
    let geometry = s
        .annotation_geometry(&Payload::new(link.clone()))
        .unwrap()
        .data
        .unwrap();
    assert_eq!(geometry.id, link);
    assert!(!geometry.rects.is_empty());
    let r = &geometry.rects[0];
    assert_eq!((r.page, r.node.as_str(), r.line), (0, p.as_str(), 0));
    assert!(r.bounds.width > 0 && r.bounds.height > 0);
    assert_eq!(geometry.top.unwrap().page, 0);
    assert_eq!(geometry.top.unwrap().y, r.bounds.y);
    // The link area is on the page, the comment is not a link.
    let page = s.page_links(0).unwrap().data;
    assert_eq!(page.links.len(), 1);
    assert_eq!(page.links[0].id, link);
    assert_eq!(
        page.links[0].target,
        LinkDestination::Url {
            url: "https://manual.example".into()
        }
    );
    assert_eq!(page.links[0].rects, geometry.rects);
    // The display page carries the same areas and its list is unchanged.
    let display = s.display_page(0).unwrap().data;
    assert_eq!(display.links, page.links);
    let json_with = s.display_json(0).unwrap().data;
    apply(&mut s, vec![Command::RemoveAnnotation { id: link.clone() }]);
    finish(&mut s);
    assert!(s.display_page(0).unwrap().data.links.is_empty());
    assert_eq!(s.display_json(0).unwrap().data, json_with, "links never paint");
    // The comment has geometry too, and a missing one is null.
    assert!(
        s.annotation_geometry(&Payload::new(note))
            .unwrap()
            .data
            .is_some()
    );
    assert!(
        s.annotation_geometry(&Payload::new(link))
            .unwrap()
            .data
            .is_none()
    );
    assert_eq!(s.page_links(99).unwrap_err().code(), "bindings.invalid");
}

#[test]
fn batches_respect_the_expected_revision_and_map_the_selection() {
    let mut s = session("1");
    let p = paragraph(&mut s, "link this text please");
    let revision = s.state().unwrap().data.revision;
    let report = s
        .batch(&Payload::new(Batch {
            commands: vec![
                add(&p, 5, 9, url("https://batch.example")),
                Command::InsertText {
                    node: p.clone(),
                    at: 0,
                    text: ">> ".into(),
                },
            ],
            expected_revision: Some(revision.clone()),
            selection: None,
        }))
        .unwrap()
        .data;
    assert_eq!(report.applied.annotations.len(), 1);
    let id = report.applied.annotations[0].clone();
    // Typed text before the link moved it, as text does.
    assert_eq!(get(&s, &id).unwrap().spans[0].start, 8);
    // The revision moved: a stale batch is refused and writes nothing.
    let stale = s
        .batch(&Payload::new(Batch {
            commands: vec![Command::RemoveAnnotation { id: id.clone() }],
            expected_revision: Some(revision),
            selection: None,
        }))
        .unwrap_err();
    assert_eq!(stale.code(), "bindings.stale");
    assert!(get(&s, &id).is_some());
    // A failing second command leaves the first undone.
    let e = s
        .batch(&Payload::new(Batch {
            commands: vec![
                Command::RemoveAnnotation { id: id.clone() },
                Command::RemoveAnnotation { id: "1@77".into() },
            ],
            expected_revision: None,
            selection: None,
        }))
        .unwrap_err();
    assert_eq!(e.code(), "annotation.missing");
    assert!(get(&s, &id).is_some());
}

#[test]
fn a_copied_document_keeps_links_and_comments_with_new_identities() {
    let mut s = session("1");
    let p = paragraph(&mut s, "Intro text with a link and a comment");
    let q = paragraph(&mut s, "second paragraph");
    let applied = apply(
        &mut s,
        vec![
            add(&p, 17, 21, url("https://copy.example")),
            add(&p, 28, 35, thread("t-9")),
            add(&q, 0, 6, AnnotationPayload::Link {
                target: LinkDestination::Heading { node: p.clone() },
            }),
            add(&q, 7, 16, thread("t-orphan")),
        ],
    );
    let orphan = applied.annotations[3].clone();
    apply(
        &mut s,
        vec![Command::DeleteText {
            node: q.clone(),
            start: 6,
            end: 16,
        }],
    );
    assert_eq!(get(&s, &orphan).unwrap().status, AnnotationStatus::Orphaned);
    finish(&mut s);
    let bytes = s.save().unwrap().data.bytes;
    let copy = Workspace::new()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    let state = copy.state().unwrap().data;
    assert_eq!(state.annotations.len(), 4, "{:?}", state.annotations);
    let originals: Vec<String> = s.state().unwrap().data.annotations.iter().map(|a| a.id.clone()).collect();
    for a in &state.annotations {
        assert!(!originals.contains(&a.id), "new identity: {}", a.id);
    }
    let kinds: Vec<_> = state.annotations.iter().map(|a| (a.kind, a.status)).collect();
    assert_eq!(
        kinds.iter().filter(|k| k.0 == AnnotationKind::Link).count(),
        2
    );
    assert!(kinds.contains(&(AnnotationKind::Comment, AnnotationStatus::Orphaned)));
    // The heading link follows the copied heading, not the source's.
    let heading_link = state
        .annotations
        .iter()
        .find(|a| a.kind == AnnotationKind::Link && a.spans[0].node == state.blocks[1].id)
        .unwrap();
    let full = copy
        .annotation(&Payload::new(heading_link.id.clone()))
        .unwrap()
        .data
        .unwrap();
    assert_eq!(
        full.payload,
        AnnotationPayload::Link {
            target: LinkDestination::Heading {
                node: state.blocks[0].id.clone()
            }
        }
    );
    // Texts are unchanged.
    assert_eq!(state.blocks[0].text, "Intro text with a link and a comment");
    assert_eq!(state.blocks[1].text, "second");
}

#[test]
fn copy_as_html_carries_links_of_the_copied_part_only() {
    let mut s = session("1");
    let p = paragraph(&mut s, "visit the site today");
    apply(&mut s, vec![add(&p, 10, 14, url("https://site.example/?a=1&b=2"))]);
    finish(&mut s);
    let selection = |start: u32, end: u32| Selection {
        anchor: Caret {
            node: p.clone(),
            offset: start,
            affinity: Affinity::Downstream,
        },
        focus: Caret {
            node: p.clone(),
            offset: end,
            affinity: Affinity::Downstream,
        },
    };
    let exported = s
        .copy_as(&Payload::new(CopyAs {
            selection: selection(0, 20),
            format: CopyFormat::Html,
        }))
        .unwrap()
        .data;
    let html = String::from_utf8(exported.content.bytes).unwrap();
    assert!(html.contains("<a href=\"https://site.example/?a=1&amp;b=2\">"), "{html}");
    // Copying a part that does not include the link leaves it out.
    let exported = s
        .copy_as(&Payload::new(CopyAs {
            selection: selection(0, 9),
            format: CopyFormat::Html,
        }))
        .unwrap()
        .data;
    assert!(!String::from_utf8(exported.content.bytes).unwrap().contains("<a "));
}

#[test]
fn state_lists_a_bounded_number_and_the_queries_the_rest() {
    let mut s = session("1");
    let text = "w ".repeat(1500);
    let p = paragraph(&mut s, &text);
    let mut commands = Vec::new();
    for i in 0..(MAX_STATE_ANNOTATIONS as u32 + 40) {
        commands.push(add(&p, (i % 3000) / 2 * 2, (i % 3000) / 2 * 2 + 1, thread(&format!("t{i}"))));
    }
    for chunk in commands.chunks(500) {
        apply(&mut s, chunk.to_vec());
    }
    let state = s.state().unwrap().data;
    assert_eq!(state.annotations.len(), MAX_STATE_ANNOTATIONS);
    assert!(
        state
            .diagnostics
            .iter()
            .any(|d| d.code == "bindings.limit" && d.message.contains("annotations"))
    );
    let all = s
        .annotations_of_kind(&Payload::new(AnnotationKind::Comment))
        .unwrap()
        .data;
    assert_eq!(all.annotations.len(), 2088);
    assert!(!all.truncated);
}

#[test]
fn hostile_payloads_are_refused_at_the_boundary() {
    // Unknown fields and kinds do not decode.
    for json in [
        r#"{"version":1,"data":{"commands":[{"kind":"add-annotation","node":"1@1","start":0,"end":1,"annotation":{"kind":"link","target":{"type":"url","url":"https://a.example","extra":1}}}]}}"#,
        r#"{"version":1,"data":{"commands":[{"kind":"add-annotation","node":"1@1","start":0,"end":1,"annotation":{"kind":"bookmark","name":"x"}}]}}"#,
        r#"{"version":1,"data":{"commands":[{"kind":"add-annotation","node":"1@1","start":0,"end":1,"annotation":{"kind":"comment"}}]}}"#,
        r#"{"version":1,"data":{"commands":[{"kind":"add-annotation","node":"1@1","start":-1,"end":1,"annotation":{"kind":"comment","thread":"t"}}]}}"#,
    ] {
        assert!(decode::<Transaction>(json.as_bytes()).is_err(), "{json}");
    }
    let ok = decode::<Transaction>(
        br#"{"version":1,"data":{"commands":[{"kind":"add-annotation","node":"1@1","start":0,"end":1,"annotation":{"kind":"link","target":{"type":"url","url":"https://a.example"}}}]}}"#,
    )
    .unwrap();
    assert!(matches!(ok.data.commands[0], Command::AddAnnotation { .. }));
}

#[test]
fn unreadable_annotations_show_up_in_state_and_diagnostics_for() {
    use reprise_doc::annotation::{AnnotationData, LinkTarget};
    // A hostile peer's envelope: a forbidden scheme in a record that is
    // otherwise a fine annotation. It is kept, reported and never applied.
    let doc = reprise_doc::Document::new(1).unwrap();
    let node = doc
        .append_block(reprise_doc::BlockKind::Paragraph, "", "some linked text")
        .unwrap();
    let good = doc
        .add_annotation(
            node,
            0..4,
            &AnnotationData::Link(LinkTarget::Url {
                url: "https://fine.example".into(),
            }),
        )
        .unwrap();
    let bad = doc
        .add_annotation(
            node,
            5..11,
            &AnnotationData::Link(LinkTarget::Url {
                url: "https://fine.example/2".into(),
            }),
        )
        .unwrap();
    doc.commit();
    reprise_doc::hostile::corrupt_annotation(
        &doc,
        bad,
        r#"{"version":1,"kind":"link","payload":{"type":"url","url":"javascript:alert(1)"}}"#,
    )
    .unwrap();
    let bytes = reprise_format::Package::new(
        &doc,
        reprise_format::DocumentId([7; 16]),
        reprise_doc::PersistenceMode::History,
    )
    .unwrap()
    .save()
    .unwrap();
    let s = Workspace::new()
        .open(&Payload::new(Open { peer_id: "2".into() }), &bytes)
        .unwrap();
    let state = s.state().unwrap().data;
    assert_eq!(state.annotations.len(), 1);
    assert_eq!(state.annotations[0].id, good.to_string());
    assert!(get(&s, &bad.to_string()).is_none());
    let list = s
        .diagnostics_for(&Payload::new(DiagnosticFilter {
            code_prefix: Some("annotation.".into()),
            ..DiagnosticFilter::default()
        }))
        .unwrap()
        .data;
    assert_eq!(list.diagnostics.len(), 1, "{:?}", list.diagnostics);
    assert_eq!(list.diagnostics[0].code, "annotation.unreadable");
    assert_eq!(list.diagnostics[0].severity, Severity::Warning);
    assert!(state.diagnostics.iter().any(|d| d.code == "annotation.unreadable"));
    // It cannot be edited either: the record is a newer engine's, not ours.
    let mut s = s;
    let e = refused(
        &mut s,
        vec![Command::UpdateAnnotation {
            id: bad.to_string(),
            annotation: url("https://fine.example/3"),
        }],
    );
    assert_eq!(e.code(), "annotation.payload-invalid");
}

#[test]
fn links_leave_the_reading_order_ranges_whole() {
    let mut s = session("1");
    let a = paragraph(&mut s, "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda");
    let b = paragraph(&mut s, "second block with a link in it");
    apply(
        &mut s,
        vec![
            add(&a, 6, 40, url("https://one.example")),
            add(&b, 7, 12, thread("t")),
        ],
    );
    finish(&mut s);
    let state = s.state().unwrap().data;
    let steps = s.reading_order().unwrap().data;
    for block in &state.blocks {
        let mut at = 0u32;
        for step in steps.iter().filter(|st| st.node == block.id) {
            assert_eq!(step.start, at, "lines of {} tile its text", block.id);
            at = step.end;
        }
        assert_eq!(at as usize, block.text.len());
    }
}
