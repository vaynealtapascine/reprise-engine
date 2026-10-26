//! B3 fixes (the Astra B3 review), finding 3: story ownership in envelopes,
//! beyond the review's own probes in `review_b3.rs`.
use reprise_doc::fragment::{Fragment, FragmentRange};
use reprise_doc::relation::builtin;
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, Document, Domain, NewBlock, NodeId, PersistenceMode, Relation, SchemaRegistry,
};
use reprise_edit::{Command, Editor, Reason, Transaction};
use serde_json::{Value, json};

fn base() -> Fragment {
    let d = Document::new(1).unwrap();
    d.append_block(BlockKind::Paragraph, "", "body").unwrap();
    d.commit();
    d.copy_all_fragment("source", &SchemaRegistry::builtin())
        .unwrap()
}

fn story_blocks(f: &Fragment, n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| {
            let mut b = serde_json::to_value(&f.blocks[0]).unwrap();
            b["id"] = json!(format!("{}@99", i + 1));
            b["text"] = json!("story");
            b
        })
        .collect()
}

fn with_envelope(mut f: Fragment, envelope: Value) -> Fragment {
    f.sections = Some(envelope.to_string());
    f
}

/// A destination whose undo and redo stacks are both populated, and one
/// paragraph in the body and one in a story.
fn destination() -> (Editor, NodeId, NodeId) {
    let d = Document::new(2).unwrap();
    let body = d
        .append_block(BlockKind::Paragraph, "", "existing")
        .unwrap();
    let story = d
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "", "held")])
        .unwrap();
    let held = d.top(Domain::Story(story))[0];
    d.commit();
    let mut e = Editor::new(d, SchemaRegistry::builtin());
    for text in ["a", "b"] {
        e.apply(&Transaction::from(vec![Command::InsertText {
            node: body,
            at: 0,
            text: text.into(),
        }]))
        .unwrap();
    }
    assert!(e.undo().unwrap());
    assert!(e.undo_count() > 0 && e.redo_count() > 0);
    (e, body, held)
}

type History = (Vec<u8>, reprise_doc::Revision, usize, usize);

fn history(e: &Editor) -> History {
    (
        e.document().export(PersistenceMode::History),
        e.document().revision(),
        e.undo_count(),
        e.redo_count(),
    )
}

/// The review's two probes name a story block parented into the body or into
/// another story; these are the other ways a component can reach out.
#[test]
fn ranges_relations_and_ids_stay_in_the_component_that_carries_them() {
    let f = base();
    let body = f.blocks[0].id;
    let story = story_blocks(&f, 2);
    let range = |node: &str, end: Option<&str>| -> Value {
        let mut r = serde_json::to_value(FragmentRange {
            id: reprise_doc::RangeId::parse("1@77").unwrap(),
            node: NodeId::parse(node).unwrap(),
            bytes: 0..1,
            policy: RangePolicy::FIXED,
            end: None,
            annotation: None,
        })
        .unwrap();
        if let Some(end) = end {
            r["end"] = json!({"node": end, "at": 1});
        }
        r
    };
    let stories = |blocks: Vec<Value>, ranges: Vec<Value>| {
        json!({"v": 1, "mode": "complete", "stories": [
            {"label": "s0", "source": "1@8", "blocks": blocks, "ranges": ranges}
        ]})
    };
    let mut cases: Vec<(&str, Fragment)> = Vec::new();
    cases.push((
        "a story range on a body block",
        with_envelope(
            f.clone(),
            stories(vec![story[0].clone()], vec![range(&body.to_string(), None)]),
        ),
    ));
    cases.push((
        "a story range ending in the body",
        with_envelope(
            f.clone(),
            stories(story.clone(), vec![range("1@99", Some(&body.to_string()))]),
        ),
    ));
    cases.push(("a body range ending in a story", {
        let mut g = with_envelope(f.clone(), stories(story.clone(), vec![]));
        let mut r: Value = range(&body.to_string(), None);
        r["end"] = json!({"node": "2@99", "at": 1});
        g.ranges = vec![serde_json::from_value(r).unwrap()];
        g
    }));
    cases.push(("a body range on a story block", {
        let mut g = with_envelope(f.clone(), stories(story.clone(), vec![]));
        g.ranges = vec![serde_json::from_value(range("1@99", None)).unwrap()];
        g
    }));
    cases.push(("a relation owned by a story block", {
        let mut g = with_envelope(f.clone(), stories(story.clone(), vec![]));
        g.relations = vec![reprise_doc::fragment::FragmentRelation {
            id: reprise_doc::RelationId::parse("1@55").unwrap(),
            relation: Relation::new(builtin::NOTE).owned_by(NodeId::parse("1@99").unwrap()),
        }];
        g
    }));
    cases.push(("a story block with the ID of a body block", {
        let mut b = story[0].clone();
        b["id"] = json!(body.to_string());
        with_envelope(f.clone(), stories(vec![b], vec![]))
    }));
    // The control: the same envelope with everything inside its own
    // component is accepted.
    let ok = with_envelope(
        f.clone(),
        stories(story.clone(), vec![range("1@99", Some("2@99"))]),
    );
    let mut e = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    e.paste(&ok, None, "dest").unwrap();
    assert_eq!(e.document().stories().len(), 1);
    for (what, fragment) in cases {
        let (mut e, _, _) = destination();
        let before = history(&e);
        let err = e.paste(&fragment, None, "dest").unwrap_err();
        assert!(
            matches!(err.reason, Reason::Fragment(_)),
            "{what}: {:?}",
            err.reason
        );
        assert!(history(&e) == before, "{what}: the paste changed something");
        // The same refusal when the bytes are decoded, before any paste.
        let envelope = fragment.envelope().unwrap().unwrap();
        assert!(envelope.validate_in(&fragment).is_err(), "{what}");
    }
}
