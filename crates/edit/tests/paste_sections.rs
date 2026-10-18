//! S9: the sections envelope of a fragment, copy and paste (sections §14, N7).
use reprise_doc::fragment::{CopyBlock, Fragment};
use reprise_doc::fragment_sections::{Envelope, EnvelopeMode, MAX_STORIES};
use reprise_doc::sections::{BreakKind, SectionId, SectionProperties, StartRecord};
use reprise_doc::{BlockKind, Document, Domain, NewBlock, NodeId, SchemaRegistry};
use reprise_edit::{Editor, Reason};
use serde_json::{Value, json};

fn schemas() -> SchemaRegistry {
    SchemaRegistry::builtin()
}

fn doc(peer: u64) -> Document {
    Document::new(peer).unwrap()
}

fn para(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "body", text)
        .unwrap()
}

fn story(doc: &Document, texts: &[&str]) -> NodeId {
    let blocks: Vec<_> = texts
        .iter()
        .map(|t| NewBlock::new(BlockKind::Paragraph, "body", t))
        .collect();
    doc.create_story(&blocks).unwrap()
}

fn props(entries: &[(&str, Value)]) -> SectionProperties {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn start(doc: &Document, node: NodeId, kind: BreakKind, entries: &[(&str, Value)]) {
    let life = doc.mint_lifetime(node).unwrap();
    doc.activate_start(node, &life, kind, &props(entries))
        .unwrap();
}

fn first(doc: &Document, entries: &[(&str, Value)]) {
    doc.write_section_properties(SectionId::First, "0", &props(entries), &[])
        .unwrap();
}

fn id(story: NodeId) -> Value {
    json!(story.to_string())
}

fn record(doc: &Document, node: NodeId) -> Option<StartRecord> {
    doc.start_records()
        .into_iter()
        .find(|r| r.id == SectionId::Block(node))
}

fn text(doc: &Document, node: NodeId) -> String {
    doc.block(node).unwrap().text.to_string()
}

fn story_text(doc: &Document, story: NodeId) -> Vec<String> {
    doc.top(Domain::Story(story))
        .into_iter()
        .map(|n| text(doc, n))
        .collect()
}

fn whole(nodes: &[NodeId]) -> Vec<CopyBlock> {
    nodes
        .iter()
        .map(|&node| CopyBlock { node, bytes: None })
        .collect()
}

fn envelope(fragment: &Fragment) -> Envelope {
    fragment.envelope().unwrap().expect("the copy has sections")
}

fn binding(view: &reprise_doc::sections::SectionView, key: &str) -> (String, Option<NodeId>) {
    let (slot, variant) = {
        let rest = key.strip_prefix("region.").unwrap();
        rest.split_once('.').unwrap()
    };
    let b = view
        .bindings
        .iter()
        .find(|b| b.slot == slot && b.variant == variant)
        .unwrap();
    (b.authored.clone(), b.resolved)
}

fn view_of(doc: &Document, node: NodeId) -> reprise_doc::sections::SectionView {
    doc.section_table()
        .into_iter()
        .find(|v| v.id == SectionId::Block(node))
        .unwrap()
}

/// Five paragraphs: a section start at 1 (the selection's start), a section
/// start at 2 and a page break at 3, with a header bound by `first`.
fn source() -> (Document, Vec<NodeId>, NodeId) {
    let doc = doc(1);
    let header = story(&doc, &["Running head"]);
    first(&doc, &[("region.header.default", id(header))]);
    let p: Vec<_> = (0..5).map(|i| para(&doc, &format!("p{i}"))).collect();
    start(
        &doc,
        p[1],
        BreakKind::Section,
        &[("columns.count", json!(2))],
    );
    start(
        &doc,
        p[2],
        BreakKind::Section,
        &[("columns.count", json!(3)), ("future.key", json!("kept"))],
    );
    start(&doc, p[3], BreakKind::Page, &[]);
    doc.commit();
    (doc, p, header)
}

#[test]
fn only_the_boundaries_inside_a_selection_are_copied() {
    let (doc, p, _) = source();
    let fragment = doc
        .copy_fragment("src", &whole(&p[1..5]), &schemas())
        .unwrap();
    let e = envelope(&fragment);
    assert_eq!(e.mode, EnvelopeMode::Partial);
    // The start at the selection's start stays behind, as it does when the
    // text is deleted; the two inside it come along, with their raw keys.
    let blocks: Vec<_> = e.boundaries.iter().map(|b| b.block).collect();
    assert_eq!(blocks, [p[2], p[3]]);
    assert!(e.boundaries.iter().all(|b| b.active));
    assert_eq!(e.boundaries[0].props["columns.count"], json!(3));
    assert_eq!(e.boundaries[0].props["future.key"], json!("kept"));
    assert_eq!(e.boundaries[1].kind, BreakKind::Page);
    assert!(e.context.is_none() && e.geometry.is_none());
    // No lifetime travels: nothing in the envelope names one.
    let raw = fragment.sections.as_deref().unwrap();
    for r in doc.start_records() {
        if let Some(life) = r.life.filter(|l| l != "0") {
            assert!(!raw.contains(&life), "{life}");
        }
    }
    // A selection with no boundary inside has no envelope.
    let plain = doc
        .copy_fragment("src", &whole(&p[0..1]), &schemas())
        .unwrap();
    assert!(plain.sections.is_none());
}

#[test]
fn paste_writes_every_boundary_on_the_copy_of_its_block_with_a_fresh_lifetime() {
    let (src, p, _) = source();
    let fragment = src
        .copy_fragment("src", &whole(&p[1..5]), &schemas())
        .unwrap();
    let dst = doc(2);
    let before = para(&dst, "existing");
    dst.commit();
    let mut editor = Editor::new(dst, schemas());
    let pasted = editor.paste(&fragment, None, "dst").unwrap();
    let d = editor.document();
    let new = |old: NodeId| pasted.ids.nodes[&old];
    assert!(record(d, before).is_none() && record(d, new(p[1])).is_none());
    let two = record(d, new(p[2])).unwrap();
    assert!(two.active && two.kind == Some(BreakKind::Section));
    assert_eq!(two.own["columns.count"], json!(3));
    assert_eq!(two.own["future.key"], json!("kept"));
    let three = record(d, new(p[3])).unwrap();
    assert_eq!(three.kind, Some(BreakKind::Page));
    // The lifetime is the destination's: an operation of the paste's peer.
    let source_life = record(&src, p[2]).unwrap().life.unwrap();
    assert_ne!(two.life.unwrap(), source_life);
    // One undo step takes all of it back; redo brings back the same records.
    assert_eq!(editor.undo_count(), 1);
    let lives = (two.life.clone(), three.life.clone());
    editor.undo().unwrap();
    assert!(editor.document().effective_starts().is_empty());
    editor.redo().unwrap();
    let again = (
        record(editor.document(), new(p[2])).unwrap().life,
        record(editor.document(), new(p[3])).unwrap().life,
    );
    assert_eq!(again, lives);
}

#[test]
fn the_first_copied_section_takes_what_it_inherited_so_a_paste_picks_up_no_foreign_header() {
    let (src, p, header) = source();
    let fragment = src
        .copy_fragment("src", &whole(&p[2..5]), &schemas())
        .unwrap();
    let e = envelope(&fragment);
    // p[2] starts a section that inherits `first`'s header, which is not in
    // the copy: it is written as the header it is here, and the cells nothing
    // ever bound as explicit none.
    let b = &e.boundaries[0];
    assert!(matches!(
        &b.bindings["region.header.default"],
        reprise_doc::fragment_sections::Binding::Story { .. }
    ));
    assert!(matches!(
        &b.bindings["region.footer.default"],
        reprise_doc::fragment_sections::Binding::None
    ));
    assert_eq!(b.bindings.len(), 9);
    assert_eq!(e.stories.len(), 1);
    assert_eq!(e.stories[0].source, header);

    let dst = doc(2);
    let theirs = story(&dst, &["THEIR HEAD"]);
    first(&dst, &[("region.header.default", id(theirs))]);
    let a = para(&dst, "mine");
    dst.commit();
    let mut editor = Editor::new(dst, schemas());
    let pasted = editor.paste(&fragment, None, "dst").unwrap();
    let d = editor.document();
    let new = pasted.ids.nodes[&p[2]];
    let (authored, resolved) = binding(&view_of(d, new), "region.header.default");
    assert_eq!(authored, "story");
    let copy = resolved.unwrap();
    assert_ne!(copy, theirs);
    assert_eq!(story_text(d, copy), ["Running head"]);
    assert_eq!(pasted.stories[&header], copy);
    // The destination's own text before it keeps the destination's header.
    assert_eq!(text(d, a), "mine");
    assert_eq!(binding(&view_of(d, new), "region.footer.default").0, "none");
}

#[test]
fn a_paste_into_the_same_document_refers_to_the_story_and_another_copies_it_once() {
    let (src, p, header) = source();
    // Two sections bind the same story; one copy of it comes along.
    start(
        &src,
        p[4],
        BreakKind::Section,
        &[("region.footer.default", id(header))],
    );
    src.commit();
    let fragment = src
        .copy_fragment("src", &whole(&p[2..5]), &schemas())
        .unwrap();
    assert_eq!(envelope(&fragment).stories.len(), 1);

    // Into the document it came from: the same story, no copy.
    let stories_before = src.stories().len();
    let mut same = Editor::new(src, schemas());
    let pasted = same.paste(&fragment, None, "src").unwrap();
    assert_eq!(same.document().stories().len(), stories_before);
    assert_eq!(pasted.stories[&header], header);
    let new = pasted.ids.nodes[&p[4]];
    assert_eq!(
        binding(&view_of(same.document(), new), "region.footer.default"),
        ("story".into(), Some(header))
    );

    // Into another: one new story for both references.
    let mut other = Editor::new(doc(3), schemas());
    let pasted = other.paste(&fragment, None, "elsewhere").unwrap();
    let d = other.document();
    assert_eq!(d.stories().len(), 1);
    let copy = d.stories()[0];
    assert_eq!(pasted.stories[&header], copy);
    let head = pasted.ids.nodes[&p[2]];
    let foot = pasted.ids.nodes[&p[4]];
    assert_eq!(
        binding(&view_of(d, head), "region.header.default").1,
        Some(copy)
    );
    assert_eq!(
        binding(&view_of(d, foot), "region.footer.default").1,
        Some(copy)
    );
}

#[test]
fn a_complete_copy_keeps_the_leading_boundary_inactive_records_context_and_geometry() {
    let src = doc(1);
    let held = story(&src, &["Held by first only", "second line"]);
    first(
        &src,
        &[
            ("columns.count", json!(2)),
            ("region.footer.default", id(held)),
            ("unknown.key", json!(7)),
        ],
    );
    let p: Vec<_> = (0..3).map(|i| para(&src, &format!("p{i}"))).collect();
    // The body prefix was deleted: the first block carries an effective section.
    start(
        &src,
        p[0],
        BreakKind::Section,
        &[("columns.count", json!(3))],
    );
    // An inactive record on a live block.
    start(&src, p[1], BreakKind::Page, &[("future.key", json!("old"))]);
    src.deactivate_start(p[1]).unwrap();
    // A binding to a story that is gone: kept as it was.
    let ghost = story(&src, &["gone"]);
    start(
        &src,
        p[2],
        BreakKind::Section,
        &[("region.header.default", id(ghost))],
    );
    src.delete_block(ghost).unwrap();
    // A record on a dead block: dropped, and counted.
    let dead = para(&src, "dead");
    start(&src, dead, BreakKind::Section, &[]);
    src.delete_block(dead).unwrap();
    src.set_geometry_mode("pageless").unwrap();
    src.commit();

    let fragment = src.copy_all_fragment("src", &schemas()).unwrap();
    let e = envelope(&fragment);
    assert_eq!(e.mode, EnvelopeMode::Complete);
    assert_eq!(e.boundaries.len(), 3);
    assert_eq!(e.boundaries[0].block, p[0], "the leading boundary travels");
    assert!(!e.boundaries[1].active);
    assert!(e.context.is_some() && e.geometry.is_some());
    assert_eq!(e.dropped, 1);
    // `first`'s only story is in the table.
    assert_eq!(e.stories.len(), 1);

    let mut editor = Editor::new(doc(2), schemas());
    let pasted = editor.paste(&fragment, None, "dst").unwrap();
    let d = editor.document();
    let n = |old: NodeId| pasted.ids.nodes[&old];
    assert!(record(d, n(p[0])).unwrap().active);
    assert_eq!(record(d, n(p[0])).unwrap().own["columns.count"], json!(3));
    let inactive = record(d, n(p[1])).unwrap();
    assert!(!inactive.active && inactive.life.is_some());
    assert_eq!(inactive.own["future.key"], json!("old"));
    assert_eq!(
        record(d, n(p[2])).unwrap().own["region.header.default"],
        id(ghost),
        "a missing story is written unchanged"
    );
    let table = d.section_table();
    assert_eq!(table[0].id, SectionId::First);
    assert_eq!(table[0].own["columns.count"], json!(2));
    assert_eq!(table[0].own["unknown.key"], json!(7));
    let copy = table[0]
        .bindings
        .iter()
        .find(|b| b.slot == "footer")
        .unwrap();
    assert_eq!(
        story_text(d, copy.resolved.unwrap()),
        ["Held by first only", "second line"]
    );
    assert_eq!(d.geometry_mode_record().unwrap()["mode"], json!("pageless"));
    assert!(
        pasted
            .notes
            .iter()
            .any(|n| n.code == "clipboard.section-dropped" && n.message.contains('1')),
        "{:?}",
        pasted.notes
    );
}

#[test]
fn context_and_geometry_apply_to_an_empty_destination_only() {
    let src = doc(1);
    first(&src, &[("columns.count", json!(2))]);
    para(&src, "text");
    src.set_geometry_mode("pageless").unwrap();
    src.commit();
    let fragment = src.copy_all_fragment("src", &schemas()).unwrap();

    let dst = doc(2);
    para(&dst, "already here");
    dst.commit();
    let mut editor = Editor::new(dst, schemas());
    let pasted = editor.paste(&fragment, None, "dst").unwrap();
    assert!(
        pasted
            .notes
            .iter()
            .any(|n| n.code == "clipboard.section-context"),
        "{:?}",
        pasted.notes
    );
    let d = editor.document();
    assert!(
        d.start_records().is_empty() && d.geometry_mode_record().is_none(),
        "nothing of `first` or the geometry was written"
    );
}

#[test]
fn pasting_into_a_story_drops_boundaries_and_bindings_with_a_warning() {
    let (src, p, _) = source();
    let fragment = src
        .copy_fragment("src", &whole(&p[2..5]), &schemas())
        .unwrap();
    let dst = doc(2);
    let held = story(&dst, &["header text"]);
    let block = dst.top(Domain::Story(held))[0];
    dst.commit();
    let mut editor = Editor::new(dst, schemas());
    let pasted = editor.paste(&fragment, Some((block, 6)), "dst").unwrap();
    let d = editor.document();
    assert!(d.start_records().is_empty(), "no boundary in a story");
    assert_eq!(d.stories().len(), 1, "no story came with it");
    assert!(story_text(d, held).iter().any(|t| t.contains("p2")));
    assert!(
        pasted
            .notes
            .iter()
            .any(|n| n.code == "clipboard.section-context"),
        "{:?}",
        pasted.notes
    );
}

#[test]
fn a_boundary_on_the_block_joined_into_the_caret_has_no_place_and_is_dropped() {
    let src = doc(1);
    let a = para(&src, "alpha");
    let b = para(&src, "beta");
    let c = para(&src, "gamma");
    start(&src, b, BreakKind::Section, &[]);
    start(&src, c, BreakKind::Page, &[]);
    src.commit();
    let fragment = src
        .copy_fragment("src", &whole(&[b, c]), &schemas())
        .unwrap();
    assert_eq!(envelope(&fragment).boundaries.len(), 1, "c only");
    let _ = a;
    let dst = doc(2);
    let host = para(&dst, "0123456789");
    dst.commit();
    let mut editor = Editor::new(dst, schemas());
    let pasted = editor.paste(&fragment, Some((host, 4)), "dst").unwrap();
    let d = editor.document();
    // The page break sits on the pasted copy of `gamma`.
    let gamma = pasted.ids.nodes[&c];
    assert_eq!(record(d, gamma).unwrap().kind, Some(BreakKind::Page));
    assert_eq!(d.effective_starts().len(), 1);
}

#[test]
fn a_malformed_envelope_is_refused_before_anything_is_written() {
    let (src, p, _) = source();
    let good = src
        .copy_fragment("src", &whole(&p[2..5]), &schemas())
        .unwrap();
    let envelope: Value = serde_json::from_str(good.sections.as_deref().unwrap()).unwrap();
    let with = |edit: &dyn Fn(&mut Value)| {
        let mut e = envelope.clone();
        edit(&mut e);
        Fragment {
            sections: Some(e.to_string()),
            ..good.clone()
        }
    };
    let long = "x".repeat(2049);
    let many: serde_json::Map<String, Value> =
        (0..65).map(|i| (format!("k{i}"), json!(i))).collect();
    let cases: Vec<(&str, Fragment)> = vec![
        (
            "not json",
            Fragment {
                sections: Some("{".into()),
                ..good.clone()
            },
        ),
        ("a future version", with(&|e| e["v"] = json!(2))),
        ("an unknown field", with(&|e| e["surprise"] = json!(1))),
        (
            "a boundary on no block of the copy",
            with(&|e| e["boundaries"][0]["block"] = json!("999@7")),
        ),
        (
            "a binding to a story that is not in the table",
            with(&|e| {
                e["boundaries"][0]["bindings"]["region.header.default"] =
                    json!({"t": "story", "label": "nope"})
            }),
        ),
        (
            "a duplicate story label",
            with(&|e| {
                let s = e["stories"][0].clone();
                let mut other = s.clone();
                other["source"] = json!("5@5");
                e["stories"] = json!([s, other]);
            }),
        ),
        (
            "too many keys",
            with(&|e| e["boundaries"][0]["props"] = Value::Object(many.clone())),
        ),
        (
            "an oversize value",
            with(&|e| e["boundaries"][0]["props"]["k"] = json!(long)),
        ),
        (
            "a binding key that is not a binding",
            with(&|e| e["boundaries"][0]["bindings"]["columns.count"] = json!({"t": "none"})),
        ),
        (
            "geometry in a partial envelope",
            with(&|e| e["geometry"] = json!({"v": 1, "mode": "paged"})),
        ),
    ];
    for (what, fragment) in cases {
        let dst = doc(2);
        para(&dst, "target");
        dst.commit();
        let revision = dst.revision();
        let mut editor = Editor::new(dst, schemas());
        let error = editor.paste(&fragment, None, "dst").unwrap_err();
        assert!(
            matches!(error.reason, Reason::Fragment(_) | Reason::CopyLimit(_)),
            "{what}: {error:?}"
        );
        assert_eq!(editor.document().revision(), revision, "{what}");
        assert_eq!((editor.undo_count(), editor.redo_count()), (0, 0), "{what}");
        assert!(editor.document().start_records().is_empty(), "{what}");
    }
}

/// A fragment whose envelope carries `count` empty stories and nothing else,
/// bound from `bound` of them by `first` and a section.
fn many_stories(count: usize) -> Fragment {
    let src = doc(1);
    para(&src, "body");
    src.commit();
    let mut fragment = src.copy_all_fragment("src", &schemas()).unwrap();
    let stories: Vec<Value> = (0..count)
        .map(|i| json!({"label": format!("s{i}"), "source": format!("{}@9", i + 1), "blocks": []}))
        .collect();
    let envelope = json!({
        "v": 1,
        "mode": "complete",
        "context": {"bindings": {
            "region.header.default": {"t": "story", "label": "s0"},
            "region.header.first": {"t": "story", "label": "s0"},
            "region.footer.default": {"t": "story", "label": "s1"},
        }},
        "stories": stories,
    });
    fragment.sections = Some(envelope.to_string());
    fragment
}

#[test]
fn the_story_table_holds_sixteen_thousand_three_hundred_eighty_four_and_refuses_one_more() {
    assert_eq!(MAX_STORIES, 16_384);
    // At the limit: every story is made, each once however many references
    // it has.
    let ok = many_stories(MAX_STORIES);
    ok.validate().unwrap();
    let mut editor = Editor::new(doc(2), schemas());
    let pasted = editor.paste(&ok, None, "dst").unwrap();
    assert_eq!(editor.document().stories().len(), MAX_STORIES);
    assert_eq!(pasted.stories.len(), MAX_STORIES);
    assert_eq!(editor.undo_count(), 1);

    // One more: refused as a copy limit, before anything is written, with the
    // document and both undo stacks as they were.
    let over = many_stories(MAX_STORIES + 1);
    let target = doc(3);
    para(&target, "kept");
    target.commit();
    let revision = target.revision();
    let mut editor = Editor::new(target, schemas());
    let error = editor.paste(&over, None, "dst").unwrap_err();
    assert_eq!(error.reason, Reason::CopyLimit("transferred stories"));
    assert_eq!(error.note().code.as_str(), "clipboard.limit");
    assert_eq!(editor.document().revision(), revision);
    assert_eq!((editor.undo_count(), editor.redo_count()), (0, 0));
    assert!(editor.document().stories().is_empty());
}

#[test]
fn copying_counts_every_story_orphans_and_running_only_ones_included() {
    // A document whose stories are all orphans but one bound by `first`.
    let src = doc(1);
    let bound = story(&src, &["bound"]);
    first(&src, &[("region.header.default", id(bound))]);
    para(&src, "body");
    for _ in 0..MAX_STORIES - 1 {
        src.create_story(&[]).unwrap();
    }
    src.commit();
    assert_eq!(src.stories().len(), MAX_STORIES);
    let fragment = src.copy_document_fragment("src", &schemas()).unwrap();
    assert_eq!(envelope(&fragment).stories.len(), MAX_STORIES);

    // One orphan more, and the copy is refused rather than truncated.
    src.create_story(&[]).unwrap();
    src.commit();
    let error = src.copy_document_fragment("src", &schemas()).unwrap_err();
    assert_eq!(
        error,
        reprise_doc::fragment::FragmentError::Limit("transferred stories")
    );
}
