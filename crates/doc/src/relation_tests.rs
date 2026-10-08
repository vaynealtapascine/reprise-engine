//! Tests for structural and snapshot targets, deletion policies, succession
//! and copy planning (13, 14, 15, 35).

use std::collections::BTreeSet;

use reprise_text::RangePolicy;

use crate::history::MAX_SNAPSHOT_TEXT;
use crate::relation::builtin::{FOLLOW, REFERENCE};
use crate::relation::{
    CopyAction, CopyCrossing, CopyEntry, CopyInside, CopyPolicy, CopySet, Dependency, DropReason,
    IdMap, OnTargetDeleted, Ownership, RoleSpec, SnapshotOf, SnapshotRef, StructuralQuery,
    plan_copy,
};
use crate::*;

const OTHER: u64 = 2;

fn para(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Paragraph, "body", text)
        .unwrap()
}

fn note(doc: &Document, text: &str) -> NodeId {
    doc.append_block(BlockKind::Annotation, "note", text)
        .unwrap()
}

/// Nests a block under another. Nothing in the public API does that yet; the
/// queries must already work once something does.
fn child(doc: &Document, parent: NodeId, text: &str) -> NodeId {
    let tree = doc.tree("content");
    let id = tree.create(parent.node).unwrap();
    let meta = tree.get_meta(id).unwrap();
    meta.insert("kind", "paragraph").unwrap();
    meta.insert("style", "body").unwrap();
    let t = meta
        .insert_container("text", loro::LoroText::new())
        .unwrap();
    t.insert_utf8(0, text).unwrap();
    NodeId::tree(id)
}

fn resolve(doc: &Document, target: &Target, policy: OnTargetDeleted) -> Outcome {
    doc.resolve_target(target, policy, &mut HistoryCache::default())
}

fn structural(doc: &Document, q: StructuralQuery) -> Outcome {
    resolve(doc, &Target::Structural(q), OnTargetDeleted::Rebind)
}

fn next(from: NodeId, kind: Option<BlockKind>) -> StructuralQuery {
    StructuralQuery::NextSibling { from, kind }
}

// ---- the stored form -----------------------------------------------------

#[test]
fn relations_written_by_the_first_engine_still_read() {
    // Exactly what `add_relation` stored before structural and snapshot
    // targets existed.
    let json = r#"{"schema":"reprise.follow","owner":"3@1","targets":{"line":[{"layout":{"query":"line-containing","range":"4@1"}}]},"params":{"offset":{"length":{"em":500}}}}"#;
    let parsed: Relation = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.schema, FOLLOW);
    assert_eq!(
        parsed.first("line"),
        Some(&Target::Layout(LayoutQuery::LineContaining {
            range: RangeId::parse("4@1").unwrap()
        }))
    );
    assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
}

#[test]
fn every_target_kind_round_trips_in_a_stable_form() {
    let n = NodeId::parse("1@1").unwrap();
    let r = RangeId::parse("2@1").unwrap();
    let cases = [
        (
            Target::Structural(next(n, Some(BlockKind::Annotation))),
            r#"{"structural":{"query":"next-sibling","from":"1@1","kind":"annotation"}}"#,
        ),
        (
            Target::Structural(StructuralQuery::NthChild {
                of: None,
                index: 2,
                from_end: true,
                kind: None,
            }),
            r#"{"structural":{"query":"nth-child","index":2,"from_end":true}}"#,
        ),
        (
            Target::Structural(StructuralQuery::Children {
                of: Some(n),
                kind: None,
            }),
            r#"{"structural":{"query":"children","of":"1@1"}}"#,
        ),
        (
            Target::Layout(LayoutQuery::LinesIn { range: r }),
            r#"{"layout":{"query":"lines-in","range":"2@1"}}"#,
        ),
        (
            Target::Layout(LayoutQuery::FirstLine { node: n }),
            r#"{"layout":{"query":"first-line","node":"1@1"}}"#,
        ),
        (
            Target::Snapshot(SnapshotRef {
                version: Revision(vec![(1, 4), (2, 0)]),
                of: SnapshotOf::Range(r),
            }),
            r#"{"snapshot":{"version":[[1,4],[2,0]],"of":{"range":"2@1"}}}"#,
        ),
    ];
    for (target, json) in cases {
        assert_eq!(serde_json::to_string(&target).unwrap(), json);
        assert_eq!(serde_json::from_str::<Target>(json).unwrap(), target);
    }
}

#[test]
fn a_target_kind_from_a_newer_engine_is_unreadable_and_kept() {
    let doc = Document::new(1).unwrap();
    let tree = doc.tree("relations");
    let id = tree.create(None).unwrap();
    let future = r#"{"schema":"reprise.reference","owner":"1@1","targets":{"to":[{"spiral":{"turns":3}}]},"params":{}}"#;
    tree.get_meta(id).unwrap().insert("json", future).unwrap();
    let relations = doc.relations();
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].1, Err(future.to_string()));
    // An unknown query inside a known kind is unreadable too.
    let future = r#"{"schema":"reprise.reference","owner":"1@1","targets":{"to":[{"structural":{"query":"cousin","of":"1@1"}}]},"params":{}}"#;
    assert!(serde_json::from_str::<Relation>(future).is_err());
}

#[test]
fn the_reference_schema_takes_every_class_and_follow_only_layout() {
    let doc = Document::new(1).unwrap();
    let schemas = SchemaRegistry::builtin();
    let a = para(&doc, "a");
    let b = note(&doc, "b");
    let r = doc.add_range(a, 0..1, RangePolicy::FIXED).unwrap();
    let v = doc.revision();
    for t in [
        Target::Node(a),
        Target::Range(r),
        Target::Structural(next(a, None)),
        Target::Layout(LayoutQuery::PageContaining { range: r }),
        Target::Snapshot(SnapshotRef {
            version: v,
            of: SnapshotOf::Node(a),
        }),
    ] {
        doc.add_relation(
            &schemas,
            &Relation::new(REFERENCE).owned_by(b).target("to", t),
        )
        .unwrap();
    }
    let not_layout = Relation::new(FOLLOW)
        .owned_by(b)
        .target("line", Target::Structural(next(a, None)));
    assert!(matches!(
        doc.add_relation(&schemas, &not_layout),
        Err(DocError::Schema(SchemaError::TargetClass { .. }))
    ));
}

// ---- structural queries --------------------------------------------------

#[test]
fn next_and_previous_sibling() {
    let doc = Document::new(1).unwrap();
    let (a, n1, b, n2) = (
        para(&doc, "a"),
        note(&doc, "1"),
        para(&doc, "b"),
        note(&doc, "2"),
    );
    let one = |o: Outcome| match o.found {
        Found::Node(n) => Some(n),
        _ => None,
    };
    // One match.
    assert_eq!(one(structural(&doc, next(a, None))), Some(n1));
    assert_eq!(
        one(structural(&doc, next(a, Some(BlockKind::Paragraph)))),
        Some(b)
    );
    assert_eq!(
        one(structural(&doc, next(n1, Some(BlockKind::Annotation)))),
        Some(n2)
    );
    let prev = |from, kind| StructuralQuery::PreviousSibling { from, kind };
    assert_eq!(one(structural(&doc, prev(b, None))), Some(n1));
    assert_eq!(
        one(structural(&doc, prev(n2, Some(BlockKind::Paragraph)))),
        Some(b)
    );
    // Zero matches: past the end, before the start, or none of that kind.
    for q in [
        next(n2, None),
        next(b, Some(BlockKind::Paragraph)),
        prev(a, None),
        prev(n1, Some(BlockKind::Annotation)),
    ] {
        let o = structural(&doc, q);
        assert_eq!(o.binding, Binding::Missing);
        assert_eq!(
            o.cause,
            Cause::NoMatch,
            "a query that matches nothing isn't a deletion"
        );
        assert!(!o.is_deleted());
    }
}

#[test]
fn nth_child_counts_from_either_end_and_by_kind() {
    let doc = Document::new(1).unwrap();
    let (a, n1, b, n2) = (
        para(&doc, "a"),
        note(&doc, "1"),
        para(&doc, "b"),
        note(&doc, "2"),
    );
    let nth = |index, from_end, kind| StructuralQuery::NthChild {
        of: None,
        index,
        from_end,
        kind,
    };
    let node = |q| match structural(&doc, q).found {
        Found::Node(n) => Some(n),
        _ => None,
    };
    assert_eq!(node(nth(0, false, None)), Some(a));
    assert_eq!(node(nth(3, false, None)), Some(n2));
    assert_eq!(node(nth(0, true, None)), Some(n2));
    assert_eq!(node(nth(3, true, None)), Some(a));
    assert_eq!(node(nth(1, false, Some(BlockKind::Annotation))), Some(n2));
    assert_eq!(node(nth(1, true, Some(BlockKind::Paragraph))), Some(a));
    assert_eq!(node(nth(0, false, Some(BlockKind::Annotation))), Some(n1));
    // Zero matches, including indices that would overflow.
    for (i, e) in [(4, false), (4, true), (u32::MAX, false), (u32::MAX, true)] {
        assert_eq!(structural(&doc, nth(i, e, None)).cause, Cause::NoMatch);
    }
    assert_eq!(
        structural(&doc, nth(2, false, Some(BlockKind::Paragraph))).cause,
        Cause::NoMatch
    );
    let _ = b;
}

#[test]
fn first_last_and_all_children_and_parent() {
    let doc = Document::new(1).unwrap();
    let empty = |q| structural(&doc, q);
    let first = StructuralQuery::FirstChild {
        of: None,
        kind: None,
    };
    let all = StructuralQuery::Children {
        of: None,
        kind: None,
    };
    assert_eq!(
        empty(first.clone()).cause,
        Cause::NoMatch,
        "an empty document has no first block"
    );
    assert_eq!(empty(all.clone()).cause, Cause::NoMatch);

    let (a, n1, b) = (para(&doc, "a"), note(&doc, "1"), para(&doc, "b"));
    assert_eq!(structural(&doc, first).found, Found::Node(a));
    let last = StructuralQuery::LastChild {
        of: None,
        kind: Some(BlockKind::Annotation),
    };
    assert_eq!(structural(&doc, last).found, Found::Node(n1));
    // Several matches come as a list, in order.
    assert_eq!(structural(&doc, all).found, Found::Nodes(vec![a, n1, b]));
    let paragraphs = StructuralQuery::Children {
        of: None,
        kind: Some(BlockKind::Paragraph),
    };
    assert_eq!(structural(&doc, paragraphs).found, Found::Nodes(vec![a, b]));
    // One match is still a list for a query that can have several.
    let notes = StructuralQuery::Children {
        of: None,
        kind: Some(BlockKind::Annotation),
    };
    assert_eq!(structural(&doc, notes).found, Found::Nodes(vec![n1]));
    // A top-level block has no parent node.
    let parent = structural(&doc, StructuralQuery::Parent { of: a });
    assert_eq!(
        (parent.binding, parent.cause),
        (Binding::Missing, Cause::NoMatch)
    );
}

#[test]
fn queries_work_on_nested_blocks() {
    let doc = Document::new(1).unwrap();
    let stanza = para(&doc, "stanza");
    let after = para(&doc, "after");
    let (l1, l2, l3) = (
        child(&doc, stanza, "one"),
        child(&doc, stanza, "two"),
        child(&doc, stanza, "three"),
    );
    let of = Some(stanza);
    let node = |q| match structural(&doc, q).found {
        Found::Node(n) => Some(n),
        _ => None,
    };
    assert_eq!(
        node(StructuralQuery::FirstChild { of, kind: None }),
        Some(l1)
    );
    assert_eq!(
        node(StructuralQuery::LastChild { of, kind: None }),
        Some(l3)
    );
    assert_eq!(node(next(l1, None)), Some(l2));
    assert_eq!(
        node(next(l3, None)),
        None,
        "siblings stop at the end of the stanza"
    );
    assert_eq!(node(StructuralQuery::Parent { of: l2 }), Some(stanza));
    assert_eq!(
        structural(&doc, StructuralQuery::Children { of, kind: None }).found,
        Found::Nodes(vec![l1, l2, l3])
    );
    // The top level doesn't see the lines.
    assert_eq!(doc.children(None), vec![stanza, after]);
    assert_eq!(doc.document_order(), vec![stanza, l1, l2, l3, after]);
    // Deleting the stanza takes its lines with it.
    doc.delete_block(stanza).unwrap();
    assert!(!doc.is_live(l2));
    assert_eq!(doc.document_order(), vec![after]);
}

#[test]
fn very_deep_nesting_does_not_overflow_the_stack() {
    let doc = Document::new(1).unwrap();
    let mut at = para(&doc, "root");
    for _ in 0..3000 {
        at = child(&doc, at, "x");
    }
    assert_eq!(doc.document_order().len(), 3001);
    assert!(matches!(
        structural(&doc, StructuralQuery::Parent { of: at }).found,
        Found::Node(_)
    ));
}

#[test]
fn a_deleted_anchor_is_not_a_query_that_matches_nothing() {
    let doc = Document::new(1).unwrap();
    let (a, b) = (para(&doc, "a"), para(&doc, "b"));
    doc.delete_block(a).unwrap();
    let q = Target::Structural(next(a, None));
    for policy in [
        OnTargetDeleted::KeepMissing,
        OnTargetDeleted::Delete,
        OnTargetDeleted::Rebind,
    ] {
        let o = resolve(&doc, &q, policy);
        assert_eq!(o.binding, Binding::Missing);
        assert_eq!(o.cause, Cause::Deleted(Gone::Node(a)), "{policy:?}");
    }
    // An anchor that never existed is the same.
    let forged = NodeId::parse("99@77").unwrap();
    let o = resolve(
        &doc,
        &Target::Structural(next(forged, None)),
        OnTargetDeleted::Rebind,
    );
    assert_eq!(o.cause, Cause::Deleted(Gone::Node(forged)));
    let _ = b;
}

// ---- succession and rebinding --------------------------------------------

#[test]
fn a_deleted_node_with_one_successor_is_rebound_to_it() {
    let doc = Document::new(1).unwrap();
    let (old, new, other) = (para(&doc, "old"), para(&doc, "new"), para(&doc, "other"));
    doc.supersede(old, new).unwrap();
    doc.delete_block(old).unwrap();
    let o = resolve(&doc, &Target::Node(old), OnTargetDeleted::Rebind);
    assert_eq!((o.binding, o.found), (Binding::Rebound, Found::Node(new)));
    assert_eq!(o.cause, Cause::Succeeded { from: old });
    // The neighbours are not evidence: a node deleted with no successor is missing.
    doc.delete_block(other).unwrap();
    let o = resolve(&doc, &Target::Node(other), OnTargetDeleted::Rebind);
    assert_eq!(
        (o.binding, o.cause),
        (Binding::Missing, Cause::Deleted(Gone::Node(other)))
    );
    // A live node is just valid.
    assert_eq!(
        resolve(&doc, &Target::Node(new), OnTargetDeleted::Rebind).binding,
        Binding::Valid
    );
}

#[test]
fn queries_anchored_on_a_deleted_node_follow_its_successor() {
    let doc = Document::new(1).unwrap();
    let (old, new, c) = (para(&doc, "old"), para(&doc, "new"), para(&doc, "c"));
    doc.supersede(old, new).unwrap();
    doc.delete_block(old).unwrap();
    let o = structural(&doc, next(old, None));
    assert_eq!((o.binding, o.found), (Binding::Rebound, Found::Node(c)));
    // The successor has no next sibling: rebound, but the question has no answer.
    let o = structural(&doc, next(c, None));
    assert_eq!(o.cause, Cause::NoMatch);
    let o = resolve(
        &doc,
        &Target::Layout(LayoutQuery::FirstLine { node: old }),
        OnTargetDeleted::Rebind,
    );
    assert_eq!((o.binding, o.found), (Binding::Rebound, Found::Node(new)));
}

#[test]
fn equally_good_successors_are_ambiguous_and_listed_in_document_order() {
    let doc = Document::new(1).unwrap();
    let (old, x, y) = (para(&doc, "old"), para(&doc, "x"), para(&doc, "y"));
    // Recorded in the other order, to show that document order is what counts.
    doc.supersede(old, y).unwrap();
    doc.supersede(old, x).unwrap();
    doc.delete_block(old).unwrap();
    let o = resolve(&doc, &Target::Node(old), OnTargetDeleted::Rebind);
    assert_eq!(o.binding, Binding::Ambiguous);
    assert_eq!(o.found, Found::Nodes(vec![x, y]));
    let o = structural(&doc, next(old, None));
    assert_eq!(
        (o.binding, o.found),
        (Binding::Ambiguous, Found::Nodes(vec![x, y]))
    );
    // Only one of them survives: that one is the answer.
    doc.delete_block(y).unwrap();
    let o = resolve(&doc, &Target::Node(old), OnTargetDeleted::Rebind);
    assert_eq!((o.binding, o.found), (Binding::Rebound, Found::Node(x)));
}

#[test]
fn the_nearest_generation_wins() {
    let doc = Document::new(1).unwrap();
    let (a, b, c, d) = (
        para(&doc, "a"),
        para(&doc, "b"),
        para(&doc, "c"),
        para(&doc, "d"),
    );
    doc.supersede(a, b).unwrap();
    doc.supersede(a, c).unwrap();
    doc.supersede(b, d).unwrap();
    // a -> {b, c}; b -> d. b and c are one generation away, d is two.
    doc.delete_block(a).unwrap();
    doc.delete_block(b).unwrap();
    let o = resolve(&doc, &Target::Node(a), OnTargetDeleted::Rebind);
    assert_eq!((o.binding, o.found), (Binding::Rebound, Found::Node(c)));
    // With c gone too, the next generation is the only one left.
    doc.delete_block(c).unwrap();
    let o = resolve(&doc, &Target::Node(a), OnTargetDeleted::Rebind);
    assert_eq!((o.binding, o.found), (Binding::Rebound, Found::Node(d)));
}

#[test]
fn succession_ends_on_cycles_and_runaway_chains() {
    let doc = Document::new(1).unwrap();
    let (a, b) = (para(&doc, "a"), para(&doc, "b"));
    doc.supersede(a, b).unwrap();
    doc.supersede(b, a).unwrap();
    doc.delete_block(a).unwrap();
    doc.delete_block(b).unwrap();
    assert_eq!(
        doc.succession(a),
        Succession::None,
        "a cycle of the dead has no heir"
    );

    // A chain longer than the limit.
    let mut chain = vec![para(&doc, "0")];
    for i in 1..=(MAX_SUCCESSION_DEPTH + 5) {
        chain.push(para(&doc, &i.to_string()));
    }
    for w in chain.windows(2) {
        doc.supersede(w[0], w[1]).unwrap();
    }
    let (first, last) = (chain[0], chain[chain.len() - 1]);
    for n in &chain[..chain.len() - 1] {
        doc.delete_block(*n).unwrap();
    }
    assert_eq!(doc.succession(first), Succession::LimitReached);
    let o = resolve(&doc, &Target::Node(first), OnTargetDeleted::Rebind);
    assert_eq!((o.binding, o.cause), (Binding::Missing, Cause::RebindLimit));
    // Within the limit it is found.
    assert_eq!(
        doc.succession(chain[chain.len() - 3]),
        Succession::Live(vec![last])
    );
}

#[test]
fn supersede_checks_its_arguments() {
    let doc = Document::new(1).unwrap();
    let (a, b) = (para(&doc, "a"), para(&doc, "b"));
    assert!(doc.supersede(a, a).is_err());
    doc.delete_block(b).unwrap();
    assert!(matches!(doc.supersede(a, b), Err(DocError::NoNode(n)) if n == b));
    // A deleted node can be superseded, by a live one.
    assert!(doc.supersede(b, a).is_ok());
    assert!(doc.successors(a).is_empty());
    assert!(doc.successors(NodeId::parse("5@5").unwrap()).is_empty());
    assert!(matches!(
        doc.supersede(NodeId::parse("5@5").unwrap(), a),
        Err(DocError::NoNode(_))
    ));
}

// ---- ranges --------------------------------------------------------------

#[test]
fn range_targets_report_rebinding_and_loss() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "one two three");
    let r = doc.add_range(p, 4..7, RangePolicy::FIXED).unwrap();
    let t = Target::Range(r);
    let o = resolve(&doc, &t, OnTargetDeleted::Rebind);
    assert_eq!((o.binding, o.cause), (Binding::Valid, Cause::Live));
    doc.block(p).unwrap().text.delete(6..7).unwrap();
    let o = resolve(&doc, &t, OnTargetDeleted::Rebind);
    assert_eq!(
        (o.binding, o.cause),
        (Binding::Rebound, Cause::RangeRebound)
    );
    doc.block(p).unwrap().text.delete(4..6).unwrap();
    let o = resolve(&doc, &t, OnTargetDeleted::Rebind);
    assert_eq!(
        (o.binding, o.cause),
        (Binding::Missing, Cause::Deleted(Gone::Range(r)))
    );
}

// ---- deletion policies, locally and under collaboration ------------------

fn policy_schema(id: &'static str, on_target_deleted: OnTargetDeleted) -> RelationSchema {
    RelationSchema {
        id: SchemaId::new(id),
        version: 1,
        ownership: Ownership::Owned,
        roles: vec![RoleSpec {
            name: "to".into(),
            accepts: vec![
                TargetClass::Node,
                TargetClass::Range,
                TargetClass::Structural,
            ]
            .into(),
            min: 1,
            max: None,
        }],
        params: Vec::new(),
        on_target_deleted,
        on_copy: CopyPolicy {
            inside: CopyInside::Duplicate,
            crossing: CopyCrossing::KeepOutside,
        },
    }
}

struct Policies {
    schemas: SchemaRegistry,
    doc: Document,
    owner: NodeId,
    target: NodeId,
    heir: NodeId,
    /// One relation per policy, each to `target`, plus one to the range in it.
    relations: Vec<(RelationId, OnTargetDeleted)>,
}

fn policies() -> Policies {
    let mut schemas = SchemaRegistry::builtin();
    let kinds = [
        ("t.rebind", OnTargetDeleted::Rebind),
        ("t.keep", OnTargetDeleted::KeepMissing),
        ("t.delete", OnTargetDeleted::Delete),
    ];
    for (id, policy) in kinds {
        schemas.register(policy_schema(id, policy)).unwrap();
    }
    let doc = Document::new(1).unwrap();
    let (owner, target, heir) = (
        para(&doc, "owner"),
        para(&doc, "target"),
        para(&doc, "heir"),
    );
    let range = doc.add_range(target, 0..3, RangePolicy::FIXED).unwrap();
    let mut relations = Vec::new();
    for (id, policy) in kinds {
        for t in [
            Target::Node(target),
            Target::Range(range),
            Target::Structural(next(target, None)),
        ] {
            let rel = Relation::new(SchemaId::new(id))
                .owned_by(owner)
                .target("to", t);
            relations.push((doc.add_relation(&schemas, &rel).unwrap(), policy));
        }
    }
    doc.commit();
    Policies {
        schemas,
        doc,
        owner,
        target,
        heir,
        relations,
    }
}

/// Every relation's resolution, as comparable data.
type State = Vec<(RelationId, bool, Vec<(Binding, Found, Cause)>)>;

fn state(p: &Policies, doc: &Document) -> State {
    let mut history = HistoryCache::default();
    doc.relations()
        .into_iter()
        .map(|(id, r)| {
            let r = r.unwrap();
            let schema = p.schemas.get(&r.schema).unwrap();
            let resolved = doc.resolve_relation(schema, &r, &mut history);
            let targets = resolved
                .targets
                .into_iter()
                .map(|t| (t.outcome.binding, t.outcome.found, t.outcome.cause))
                .collect();
            (id, resolved.deleted, targets)
        })
        .collect()
}

#[test]
fn each_policy_under_local_deletion() {
    let p = policies();
    // Before: the node and range are valid; the query has no next block.
    for (_, deleted, targets) in state(&p, &p.doc) {
        assert!(!deleted);
        assert!(targets[0].0 == Binding::Valid || targets[0].2 == Cause::NoMatch);
    }
    p.doc.supersede(p.target, p.heir).unwrap();
    p.doc.delete_block(p.target).unwrap();
    let after = state(&p, &p.doc);
    // Relations were added per policy in the order node, range, query.
    let at = |i: usize| (after[i].1, after[i].2[0].clone());
    let gone_node = Cause::Deleted(Gone::Node(p.target));

    // Rebind: the node follows its heir; the range cannot (its text went
    // with the block); the query is asked of the heir, which is last.
    assert_eq!(
        at(0),
        (
            false,
            (
                Binding::Rebound,
                Found::Node(p.heir),
                Cause::Succeeded { from: p.target }
            )
        )
    );
    assert_eq!(at(1).1.0, Binding::Missing);
    assert!(matches!(at(1).1.2, Cause::Deleted(Gone::Range(_))));
    assert_eq!(
        at(2),
        (false, (Binding::Missing, Found::Nothing, Cause::NoMatch))
    );
    // KeepMissing: nothing is rebound even though an heir exists, and the
    // relation is kept.
    assert_eq!(
        at(3),
        (false, (Binding::Missing, Found::Nothing, gone_node))
    );
    assert_eq!(at(4).1.0, Binding::Missing);
    assert_eq!(
        at(5),
        (false, (Binding::Missing, Found::Nothing, gone_node))
    );
    // Delete: every one of them is no longer in effect.
    for (i, relation) in after.iter().enumerate().skip(6) {
        assert!(relation.1, "relation {i} is deleted");
        assert_eq!(relation.2[0].0, Binding::Deleted);
        assert_eq!(relation.2[0].1, Found::Nothing);
    }
}

/// Deleting the target on another peer and merging gives exactly the state of
/// deleting it here (29): same bindings, same findings, same causes.
#[test]
fn concurrent_deletion_equals_local_deletion_for_every_policy() {
    // Local.
    let local = policies();
    local.doc.supersede(local.target, local.heir).unwrap();
    local.doc.delete_block(local.target).unwrap();
    local.doc.commit();

    // Peer 1 supersedes; peer 2, who never saw that, deletes. Merged both ways.
    let p = policies();
    let other = p.doc.fork(OTHER).unwrap();
    p.doc.supersede(p.target, p.heir).unwrap();
    other.delete_block(p.target).unwrap();
    p.doc.merge(&other).unwrap();
    other.merge(&p.doc).unwrap();

    assert_eq!(p.doc.revision(), other.revision());
    let merged = state(&p, &p.doc);
    assert_eq!(merged, state(&p, &other), "both replicas agree");
    assert_eq!(
        merged,
        state(&local, &local.doc),
        "and agree with a local deletion"
    );
    assert!(
        merged.iter().any(|(_, deleted, _)| *deleted),
        "the Delete relations are deleted"
    );
}

#[test]
fn a_delete_merged_from_another_peer_deletes_without_any_edit() {
    let p = policies();
    let other = p.doc.fork(OTHER).unwrap();
    other.delete_block(p.target).unwrap();
    let before = p.doc.revision();
    p.doc.merge(&other).unwrap();
    // Evaluating policies wrote nothing: the merge is the only change.
    let dead = p.doc.dead_relations(&p.schemas);
    assert_eq!(p.doc.dead_relations(&p.schemas), dead);
    let after_merge = p.doc.revision();
    assert_ne!(before, after_merge);
    let _ = state(&p, &p.doc);
    assert_eq!(p.doc.revision(), after_merge);

    // The relations with the Delete policy whose target is the node or the
    // range in it are dead; the structural one is anchored on the deleted
    // node and is dead as well.
    let expected: Vec<RelationId> = p
        .relations
        .iter()
        .filter(|r| r.1 == OnTargetDeleted::Delete)
        .map(|r| r.0)
        .collect();
    let mut dead_sorted = dead.clone();
    dead_sorted.sort();
    let mut expected_sorted = expected;
    expected_sorted.sort();
    assert_eq!(dead_sorted, expected_sorted);
    // The owner is untouched, and so are the relations of other policies.
    assert!(p.doc.is_live(p.owner));
    let keep: BTreeSet<_> = p
        .relations
        .iter()
        .filter(|r| r.1 != OnTargetDeleted::Delete)
        .map(|r| r.0)
        .collect();
    assert!(dead.iter().all(|d| !keep.contains(d)));
}

/// What undo does to a deleted block (07): Loro alone would restore it as a
/// new node with a new ID, so deletion changes a metadata flag (`lifecycle.rs`)
/// and undo reveals the same node. Relations to the block need no
/// succession link: they are simply valid again, under every policy.
///
/// This replaces the test that recorded Loro's behaviour, a new node that
/// succession had to connect to the old one; the flag avoids that loss of ID.
#[test]
fn an_undone_deletion_is_the_same_node() {
    let p = policies();
    let mut undo = loro::UndoManager::new(&p.doc.doc);
    p.doc.delete_block(p.target).unwrap();
    p.doc.commit();
    assert!(!p.doc.is_live(p.target));
    let node = Target::Node(p.target);
    assert_eq!(
        resolve(&p.doc, &node, OnTargetDeleted::Rebind).binding,
        Binding::Missing
    );
    assert!(undo.undo().unwrap());
    assert!(p.doc.is_live(p.target), "undo restores the same ID");
    assert_eq!(p.doc.block(p.target).unwrap().text.to_string(), "target");
    for policy in [
        OnTargetDeleted::Rebind,
        OnTargetDeleted::KeepMissing,
        OnTargetDeleted::Delete,
    ] {
        let o = resolve(&p.doc, &node, policy);
        assert_eq!(
            (o.binding, o.found),
            (Binding::Valid, Found::Node(p.target))
        );
    }
    assert!(undo.redo().unwrap());
    assert!(!p.doc.is_live(p.target));
}

#[test]
fn legacy_physical_tree_tombstones_stay_deleted() {
    let p = policies();
    p.doc.doc.get_tree("content").delete(p.target.node).unwrap();
    p.doc.commit();
    assert!(!p.doc.is_live(p.target));
    assert!(p.doc.block(p.target).is_err());
    assert!(p.doc.restore_block(p.target).is_err());
    for policy in [
        OnTargetDeleted::Rebind,
        OnTargetDeleted::KeepMissing,
        OnTargetDeleted::Delete,
    ] {
        assert!(resolve(&p.doc, &Target::Node(p.target), policy).is_deleted());
    }
}

/// A link recorded on the new node and one recorded on the old are the same
/// evidence, and two peers can record them at the same time.
#[test]
fn successors_recorded_on_either_side_are_merged() {
    let doc = Document::new(1).unwrap();
    let (old, a, b) = (para(&doc, "old"), para(&doc, "a"), para(&doc, "b"));
    let other = doc.fork(OTHER).unwrap();
    doc.supersede(old, a).unwrap(); // on `old`, while it is alive
    other.delete_block(old).unwrap();
    other.supersede(old, b).unwrap(); // on `b`, because `old` is gone
    doc.merge(&other).unwrap();
    other.merge(&doc).unwrap();
    for d in [&doc, &other] {
        assert_eq!(d.successors(old), vec![a, b]);
        assert_eq!(d.succession(old), Succession::Live(vec![a, b]));
    }
}

#[test]
fn a_delete_policy_relation_is_not_dead_while_only_a_query_misses() {
    let mut schemas = SchemaRegistry::builtin();
    schemas
        .register(policy_schema("t.delete", OnTargetDeleted::Delete))
        .unwrap();
    let doc = Document::new(1).unwrap();
    let (a, b) = (para(&doc, "a"), para(&doc, "b"));
    let rel = Relation::new(SchemaId::new("t.delete"))
        .owned_by(a)
        .target("to", Target::Structural(next(b, None)));
    doc.add_relation(&schemas, &rel).unwrap();
    assert!(
        doc.dead_relations(&schemas).is_empty(),
        "no next block is not a deleted block"
    );
    doc.delete_block(b).unwrap();
    assert_eq!(doc.dead_relations(&schemas).len(), 1);
}

#[test]
fn one_deleted_target_deletes_the_whole_relation() {
    let mut schemas = SchemaRegistry::builtin();
    schemas
        .register(policy_schema("t.delete", OnTargetDeleted::Delete))
        .unwrap();
    let doc = Document::new(1).unwrap();
    let (o, a, b) = (para(&doc, "o"), para(&doc, "a"), para(&doc, "b"));
    let rel = Relation::new(SchemaId::new("t.delete"))
        .owned_by(o)
        .target("to", Target::Node(a))
        .target("to", Target::Node(b));
    doc.add_relation(&schemas, &rel).unwrap();
    doc.delete_block(b).unwrap();
    let (_, relation) = doc.relations().remove(0);
    let schema = schemas.get(&SchemaId::new("t.delete")).unwrap();
    let r = doc.resolve_relation(schema, &relation.unwrap(), &mut HistoryCache::default());
    assert!(r.deleted);
    assert!(
        r.targets
            .iter()
            .all(|t| t.outcome.binding == Binding::Deleted && t.outcome.found == Found::Nothing)
    );
    assert_eq!(r.targets[0].outcome.cause, Cause::RelationDeleted);
    assert_eq!(r.targets[1].outcome.cause, Cause::Deleted(Gone::Node(b)));
}

// ---- snapshot targets ----------------------------------------------------

fn snap(version: &Revision, of: SnapshotOf) -> SnapshotRef {
    SnapshotRef {
        version: version.clone(),
        of,
    }
}

#[test]
fn a_snapshot_has_the_text_as_it_was_and_whether_it_still_exists() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "one two three");
    let r = doc.add_range(p, 4..7, RangePolicy::FIXED).unwrap();
    let v1 = doc.revision();
    doc.block(p).unwrap().text.insert(0, "zero ").unwrap();
    doc.block(p).unwrap().text.delete(9..12).unwrap(); // "two"
    let v2 = doc.revision();
    assert_ne!(v1, v2);

    // The whole block, as it was.
    let SnapshotState::Found(c) = doc.resolve_snapshot(&snap(&v1, SnapshotOf::Node(p))) else {
        panic!("v1 is available")
    };
    assert_eq!((c.text.as_str(), c.bytes.clone()), ("one two three", 0..13));
    assert!(c.exists_now && !c.unchanged && !c.truncated);
    assert_eq!(c.node, p);
    // The range, as it was.
    let SnapshotState::Found(c) = doc.resolve_snapshot(&snap(&v1, SnapshotOf::Range(r))) else {
        panic!()
    };
    assert_eq!((c.text.as_str(), c.bytes), ("two", 4..7));
    assert!(!c.exists_now, "the range's text has been deleted since");
    // The same version, unchanged text.
    let SnapshotState::Found(c) = doc.resolve_snapshot(&snap(&v2, SnapshotOf::Node(p))) else {
        panic!()
    };
    assert!(c.exists_now && c.unchanged);

    // A deleted block is still found at a version where it was alive.
    doc.delete_block(p).unwrap();
    let SnapshotState::Found(c) = doc.resolve_snapshot(&snap(&v1, SnapshotOf::Node(p))) else {
        panic!()
    };
    assert!(!c.exists_now && !c.unchanged);
    // And resolving as a target gives a valid one, whatever the policy.
    for policy in [
        OnTargetDeleted::Rebind,
        OnTargetDeleted::KeepMissing,
        OnTargetDeleted::Delete,
    ] {
        let t = Target::Snapshot(snap(&v1, SnapshotOf::Node(p)));
        assert_eq!(resolve(&doc, &t, policy).binding, Binding::Valid);
    }
}

#[test]
fn a_subject_that_did_not_exist_yet_is_not_there() {
    let doc = Document::new(1).unwrap();
    let a = para(&doc, "a");
    let early = doc.revision();
    let b = para(&doc, "b");
    let r = doc.add_range(b, 0..1, RangePolicy::FIXED).unwrap();
    assert_eq!(
        doc.resolve_snapshot(&snap(&early, SnapshotOf::Node(b))),
        SnapshotState::NotThere
    );
    assert_eq!(
        doc.resolve_snapshot(&snap(&early, SnapshotOf::Range(r))),
        SnapshotState::NotThere
    );
    assert!(matches!(
        doc.resolve_snapshot(&snap(&early, SnapshotOf::Node(a))),
        SnapshotState::Found(_)
    ));
    // The empty revision is the document before anything.
    let empty = Revision(vec![]);
    assert_eq!(
        doc.resolve_snapshot(&snap(&empty, SnapshotOf::Node(a))),
        SnapshotState::NotThere
    );
    let o = resolve(
        &doc,
        &Target::Snapshot(snap(&early, SnapshotOf::Node(b))),
        OnTargetDeleted::Rebind,
    );
    assert_eq!((o.binding, o.cause), (Binding::Missing, Cause::NotThere));
}

#[test]
fn versions_that_cannot_be_read_are_reported_not_panicked_on() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "text");
    let ok = doc.revision();
    let (peer, counter) = ok.0[0];
    let unavailable = |v: Revision| doc.resolve_snapshot(&snap(&v, SnapshotOf::Node(p)));
    for (version, why) in [
        (Revision(vec![(peer, counter + 1)]), VersionError::Unknown),
        (Revision(vec![(peer, i32::MAX)]), VersionError::Unknown),
        (Revision(vec![(u64::MAX, 0)]), VersionError::Unknown),
        (Revision(vec![(9, 0)]), VersionError::Unknown),
        (Revision(vec![(peer, -1)]), VersionError::Malformed),
        (Revision(vec![(peer, i32::MIN)]), VersionError::Malformed),
        (
            Revision(vec![(peer, 0), (peer, 0)]),
            VersionError::Malformed,
        ),
        (
            Revision(vec![(peer, 0), (peer, 1)]),
            VersionError::Malformed,
        ),
        (Revision(vec![(2, 0), (1, 0)]), VersionError::Malformed),
    ] {
        assert_eq!(
            unavailable(version.clone()),
            SnapshotState::Unavailable(why),
            "{version:?}"
        );
    }
    // A very long garbage revision.
    let long = Revision((0..10_000u64).map(|i| (i + 100, 0)).collect());
    assert_eq!(
        unavailable(long),
        SnapshotState::Unavailable(VersionError::Unknown)
    );
    // The document still works afterwards.
    assert!(matches!(unavailable(ok), SnapshotState::Found(_)));
    para(&doc, "still editable");
}

#[test]
fn a_version_from_an_unmerged_peer_becomes_available_after_the_merge() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "base");
    let other = doc.fork(OTHER).unwrap();
    other.block(p).unwrap().text.insert(4, "!").unwrap();
    other.commit();
    let theirs = other.revision();
    let reference = snap(&theirs, SnapshotOf::Node(p));
    assert_eq!(
        doc.resolve_snapshot(&reference),
        SnapshotState::Unavailable(VersionError::Unknown)
    );
    // Peer 1 edits at the same time, so that after the merge the frontier has two peers.
    doc.block(p).unwrap().text.insert(0, ">").unwrap();
    doc.commit();
    doc.merge(&other).unwrap();
    let SnapshotState::Found(c) = doc.resolve_snapshot(&reference) else {
        panic!("merged history makes the version known")
    };
    assert_eq!(
        c.text, "base!",
        "the version is peer 2's, without peer 1's concurrent edit"
    );
    let both = doc.revision();
    assert_eq!(both.0.len(), 2);
    let SnapshotState::Found(c) = doc.resolve_snapshot(&snap(&both, SnapshotOf::Node(p))) else {
        panic!()
    };
    assert_eq!(c.text, ">base!");
}

#[test]
fn compacted_history_is_unavailable_but_the_present_is_not() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "first");
    let old = doc.revision();
    doc.block(p).unwrap().text.insert(5, " second").unwrap();
    doc.commit();
    let kept = doc.revision();
    doc.block(p).unwrap().text.insert(12, " third").unwrap();
    doc.commit();
    let later = doc.revision();
    doc.block(p).unwrap().text.insert(18, " fourth").unwrap();
    doc.commit();
    let now = doc.revision();

    let compacted = doc.compact_history(&kept, 1).unwrap();
    assert_eq!(
        compacted.block(p).unwrap().text.to_string(),
        "first second third fourth"
    );
    assert_eq!(
        compacted.resolve_snapshot(&snap(&old, SnapshotOf::Node(p))),
        SnapshotState::Unavailable(VersionError::Compacted)
    );
    // Loro opens only the present version of a compacted document.
    for v in [&kept, &later] {
        assert_eq!(
            compacted.resolve_snapshot(&snap(v, SnapshotOf::Node(p))),
            SnapshotState::Unavailable(VersionError::Compacted)
        );
    }
    let SnapshotState::Found(c) = compacted.resolve_snapshot(&snap(&now, SnapshotOf::Node(p)))
    else {
        panic!("the present is available")
    };
    assert_eq!(c.text, "first second third fourth");
    assert!(c.exists_now && c.unchanged);
    // The original still has all of it.
    assert!(matches!(
        doc.resolve_snapshot(&snap(&old, SnapshotOf::Node(p))),
        SnapshotState::Found(_)
    ));
    // An unknown or malformed point is an error, not a panic.
    assert!(doc.compact_history(&Revision(vec![(7, 7)]), 1).is_err());
    assert!(doc.compact_history(&Revision(vec![(1, -3)]), 1).is_err());
    // The compacted document can keep being edited.
    compacted.block(p).unwrap().text.insert(0, "> ").unwrap();
    compacted.commit();
    assert_eq!(
        compacted.block(p).unwrap().text.to_string(),
        "> first second third fourth"
    );
}

#[test]
fn snapshot_text_is_capped_at_a_character_boundary() {
    let doc = Document::new(1).unwrap();
    let text = "é".repeat(MAX_SNAPSHOT_TEXT); // two bytes each
    let p = para(&doc, &text);
    let v = doc.revision();
    let SnapshotState::Found(c) = doc.resolve_snapshot(&snap(&v, SnapshotOf::Node(p))) else {
        panic!()
    };
    assert!(c.truncated);
    assert!(c.text.len() <= MAX_SNAPSHOT_TEXT && c.text.chars().all(|ch| ch == 'é'));
    assert_eq!(c.bytes, 0..text.len());
    assert!(
        c.unchanged,
        "the comparison uses the whole text, not the cut one"
    );
}

#[test]
fn the_history_cache_keeps_a_few_versions_and_gives_the_same_answers() {
    let doc = Document::new(1).unwrap();
    let p = para(&doc, "0");
    let mut versions = Vec::new();
    for i in 0..(HistoryCache::CAPACITY * 3) {
        doc.block(p)
            .unwrap()
            .text
            .insert(0, &i.to_string())
            .unwrap();
        doc.commit();
        versions.push(doc.revision());
    }
    let mut cache = HistoryCache::default();
    for round in 0..2 {
        for v in &versions {
            let reference = snap(v, SnapshotOf::Node(p));
            assert_eq!(
                cache.resolve(&doc, &reference),
                doc.resolve_snapshot(&reference),
                "round {round}"
            );
        }
    }
}

// ---- dependencies and remapping ------------------------------------------

#[test]
fn targets_report_what_they_read() {
    let n = NodeId::parse("1@1").unwrap();
    let r = RangeId::parse("2@1").unwrap();
    let v = Revision(vec![(1, 9)]);
    assert_eq!(Target::Node(n).dependencies(), vec![Dependency::Node(n)]);
    assert_eq!(
        Target::Structural(next(n, None)).dependencies(),
        vec![Dependency::Node(n), Dependency::Tree]
    );
    assert_eq!(
        Target::Structural(StructuralQuery::Children {
            of: None,
            kind: None
        })
        .dependencies(),
        vec![Dependency::Tree]
    );
    assert_eq!(
        Target::Layout(LayoutQuery::NextLine { range: r }).dependencies(),
        vec![Dependency::Range(r), Dependency::LayoutOfRange(r)]
    );
    assert_eq!(
        Target::Layout(LayoutQuery::LastLine { node: n }).dependencies(),
        vec![Dependency::Node(n), Dependency::LayoutOfNode(n)]
    );
    assert!(
        Target::Snapshot(snap(&v, SnapshotOf::Node(n)))
            .dependencies()
            .contains(&Dependency::History(v))
    );
    let rel = Relation::new(REFERENCE)
        .owned_by(n)
        .target("to", Target::Node(n))
        .target("to", Target::Range(r));
    assert_eq!(
        rel.dependencies(),
        vec![Dependency::Node(n), Dependency::Range(r)]
    );
}

// ---- copy planning ---------------------------------------------------------

struct Copying {
    schemas: SchemaRegistry,
    nodes: Vec<NodeId>,
    ranges: Vec<RangeId>,
}

fn copy_schema(
    id: &'static str,
    ownership: Ownership,
    inside: CopyInside,
    crossing: CopyCrossing,
) -> RelationSchema {
    let mut s = policy_schema(id, OnTargetDeleted::Rebind);
    s.ownership = ownership;
    s.on_copy = CopyPolicy { inside, crossing };
    s
}

fn copying() -> Copying {
    let doc = Document::new(1).unwrap();
    let nodes: Vec<NodeId> = (0..4).map(|i| para(&doc, &format!("block {i}"))).collect();
    let ranges: Vec<RangeId> = nodes[..2]
        .iter()
        .map(|&n| doc.add_range(n, 0..3, RangePolicy::FIXED).unwrap())
        .collect();
    let mut schemas = SchemaRegistry::builtin();
    use CopyCrossing as X;
    use CopyInside as I;
    use Ownership::{Independent, Owned};
    for s in [
        copy_schema("c.owned-dup-keep", Owned, I::Duplicate, X::KeepOutside),
        copy_schema("c.owned-dup-drop", Owned, I::Duplicate, X::Drop),
        copy_schema("c.owned-drop-keep", Owned, I::Drop, X::KeepOutside),
        copy_schema("c.free-dup-keep", Independent, I::Duplicate, X::KeepOutside),
        copy_schema("c.free-dup-drop", Independent, I::Duplicate, X::Drop),
    ] {
        schemas.register(s).unwrap();
    }
    Copying {
        schemas,
        nodes,
        ranges,
    }
}

fn rel(schema: &'static str, owner: Option<NodeId>, targets: Vec<Target>) -> Relation {
    let mut r = Relation::new(SchemaId::new(schema));
    r.owner = owner;
    for t in targets {
        r = r.target("to", t);
    }
    r
}

fn plan(
    c: &Copying,
    relations: Vec<Result<Relation, String>>,
    copied: &CopySet,
) -> Vec<CopyAction> {
    let ids: Vec<(RelationId, Result<Relation, String>)> = relations
        .into_iter()
        .enumerate()
        .map(|(i, r)| (RelationId::parse(&format!("{i}@9")).unwrap(), r))
        .collect();
    let entries = plan_copy(&c.schemas, &ids, copied);
    assert!(entries.windows(2).all(|w| w[0].relation < w[1].relation));
    ids.iter()
        .filter_map(|(id, _)| {
            entries
                .iter()
                .find(|e: &&CopyEntry| e.relation == *id)
                .map(|e| e.action)
        })
        .collect()
}

fn set(nodes: &[NodeId], ranges: &[RangeId]) -> CopySet {
    CopySet {
        nodes: nodes.iter().copied().collect(),
        ranges: ranges.iter().copied().collect(),
    }
}

#[test]
fn copy_inside_applies_the_inside_policy() {
    let c = copying();
    let (n, r) = (&c.nodes, &c.ranges);
    let everything = set(&n[..3], &r[..2]);
    let targets = vec![
        Target::Node(n[1]),
        Target::Range(r[0]),
        Target::Structural(next(n[1], None)),
        Target::Layout(LayoutQuery::LineContaining { range: r[1] }),
    ];
    let relations = vec![
        Ok(rel("c.owned-dup-keep", Some(n[0]), targets.clone())),
        Ok(rel("c.owned-dup-drop", Some(n[0]), targets.clone())),
        Ok(rel("c.owned-drop-keep", Some(n[0]), targets.clone())),
        Ok(rel("c.free-dup-keep", None, targets.clone())),
        Ok(rel("c.free-dup-drop", None, vec![])),
    ];
    use CopyAction::*;
    assert_eq!(
        plan(&c, relations, &everything),
        vec![
            Duplicate,
            Duplicate,
            Drop(DropReason::PolicyInside),
            Duplicate
        ]
    );
}

#[test]
fn copy_crossing_applies_the_crossing_policy_for_every_target_class() {
    let c = copying();
    let (n, r) = (&c.nodes, &c.ranges);
    // The copy takes block 0 and range 0 only.
    let copied = set(&n[..1], &r[..1]);
    let outside = [
        Target::Node(n[2]),
        Target::Range(r[1]),
        Target::Structural(next(n[3], None)),
        Target::Layout(LayoutQuery::LinesIn { range: r[1] }),
        Target::Layout(LayoutQuery::FirstLine { node: n[2] }),
        // Never inside a copy: the root, and a version of the source's history.
        Target::Structural(StructuralQuery::Children {
            of: None,
            kind: None,
        }),
        Target::Snapshot(snap(&Revision(vec![(1, 3)]), SnapshotOf::Node(n[0]))),
    ];
    for t in outside {
        let keep = plan(
            &c,
            vec![Ok(rel(
                "c.owned-dup-keep",
                Some(n[0]),
                vec![Target::Node(n[0]), t.clone()],
            ))],
            &copied,
        );
        assert_eq!(keep, vec![CopyAction::KeepOutside], "{t:?}");
        let drop = plan(
            &c,
            vec![Ok(rel("c.owned-dup-drop", Some(n[0]), vec![t.clone()]))],
            &copied,
        );
        assert_eq!(
            drop,
            vec![CopyAction::Drop(DropReason::PolicyCrossing)],
            "{t:?}"
        );
    }
    // Crossing wins over a drop-inside policy; that one only drops what is inside.
    let both = plan(
        &c,
        vec![Ok(rel(
            "c.owned-drop-keep",
            Some(n[0]),
            vec![Target::Node(n[2])],
        ))],
        &copied,
    );
    assert_eq!(both, vec![CopyAction::KeepOutside]);
}

#[test]
fn owned_relations_go_with_their_owner_and_independent_ones_with_any_target() {
    let c = copying();
    let (n, r) = (&c.nodes, &c.ranges);
    let copied = set(&[n[0]], &[]);
    let relations = vec![
        // Owner outside: not part of the copy, even if its target is inside.
        Ok(rel(
            "c.owned-dup-keep",
            Some(n[1]),
            vec![Target::Node(n[0])],
        )),
        // Owner inside, nothing else.
        Ok(rel("c.owned-dup-keep", Some(n[0]), vec![])),
        // Independent, touching the fragment: crossing.
        Ok(rel(
            "c.free-dup-keep",
            None,
            vec![Target::Node(n[0]), Target::Node(n[1])],
        )),
        Ok(rel(
            "c.free-dup-drop",
            None,
            vec![Target::Node(n[0]), Target::Node(n[1])],
        )),
        // Independent, not touching it: not part of the copy.
        Ok(rel(
            "c.free-dup-keep",
            None,
            vec![Target::Node(n[1]), Target::Range(r[0])],
        )),
        // Independent with no targets touches nothing.
        Ok(rel("c.free-dup-keep", None, vec![])),
    ];
    use CopyAction::*;
    assert_eq!(
        plan(&c, relations, &copied),
        vec![Duplicate, KeepOutside, Drop(DropReason::PolicyCrossing)]
    );
}

#[test]
fn unknown_schemas_are_dropped_and_unreadable_relations_are_left_alone() {
    let c = copying();
    let n = &c.nodes;
    let copied = set(&n[..1], &[]);
    let relations = vec![
        Ok(rel("elsewhere.unknown", Some(n[0]), vec![])),
        Err("{not json".to_string()),
        Ok(rel("elsewhere.unknown", Some(n[1]), vec![])),
    ];
    assert_eq!(
        plan(&c, relations, &copied),
        vec![CopyAction::Drop(DropReason::UnknownSchema)]
    );
    // Nothing copied, nothing planned. A huge set doesn't matter.
    assert!(plan_copy(&c.schemas, &[], &set(&n[..], &[])).is_empty());
}

#[test]
fn remapping_replaces_only_copied_ids() {
    let c = copying();
    let (n, r) = (&c.nodes, &c.ranges);
    let (new_owner, new_range) = (
        NodeId::parse("50@9").unwrap(),
        RangeId::parse("51@9").unwrap(),
    );
    let map = IdMap {
        nodes: [(n[0], new_owner)].into(),
        ranges: [(r[0], new_range)].into(),
    };
    let snapshot = Target::Snapshot(snap(&Revision(vec![(1, 2)]), SnapshotOf::Node(n[0])));
    let original = rel(
        "c.owned-dup-keep",
        Some(n[0]),
        vec![
            Target::Node(n[0]),
            Target::Node(n[2]),
            Target::Range(r[0]),
            Target::Range(r[1]),
            Target::Structural(next(n[0], Some(BlockKind::Annotation))),
            Target::Structural(StructuralQuery::Children {
                of: None,
                kind: None,
            }),
            Target::Layout(LayoutQuery::NextLine { range: r[0] }),
            Target::Layout(LayoutQuery::LastLine { node: n[0] }),
            snapshot.clone(),
        ],
    )
    .param("k", Param::Int(3));
    let copy = original.remapped(&map);
    assert_eq!(copy.owner, Some(new_owner));
    assert_eq!(copy.params, original.params);
    let to = &copy.targets["to"];
    assert_eq!(
        to,
        &vec![
            Target::Node(new_owner),
            Target::Node(n[2]),
            Target::Range(new_range),
            Target::Range(r[1]),
            Target::Structural(next(new_owner, Some(BlockKind::Annotation))),
            Target::Structural(StructuralQuery::Children {
                of: None,
                kind: None
            }),
            Target::Layout(LayoutQuery::NextLine { range: new_range }),
            Target::Layout(LayoutQuery::LastLine { node: new_owner }),
            snapshot,
        ]
    );
    // An empty map changes nothing.
    assert_eq!(original.remapped(&IdMap::default()), original);
}
