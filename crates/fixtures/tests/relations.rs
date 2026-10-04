//! Relation targets, queries and deletion policies through the whole engine
//! (13, 14, 15). The unit tests in `reprise-doc` cover resolution; these cover
//! what layout makes of it: statuses, resolutions and diagnostics.

use reprise_diag::Severity;
use reprise_doc::relation::builtin::REFERENCE;
use reprise_doc::relation::{
    CopyCrossing, CopyInside, CopyPolicy, OnTargetDeleted, Ownership, RoleSpec,
};
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, DocError, Document, LayoutQuery, NodeId, RangeId, Relation, RelationSchema,
    SchemaId, SchemaRegistry, StructuralQuery, Target, TargetClass,
};
use reprise_fixtures::spike::{HALLWAY, OPENING};
use reprise_fixtures::{OTHER_PEER, PEER, engine, hostile};
use reprise_layout::{
    Engine, LayoutSnapshot, LineRef, RelationLayout, RelationStatus, Resolution, Subject,
};

const MANY: SchemaId = SchemaId::new("tests.many");
const OPTIONAL: SchemaId = SchemaId::new("tests.optional");

fn schema(id: SchemaId, classes: &[TargetClass], min: u32, max: Option<u32>) -> RelationSchema {
    RelationSchema {
        id,
        version: 1,
        ownership: Ownership::Owned,
        roles: vec![RoleSpec {
            name: "to".into(),
            accepts: classes.to_vec().into(),
            min,
            max,
        }],
        params: Vec::new(),
        on_target_deleted: OnTargetDeleted::Rebind,
        on_copy: CopyPolicy {
            inside: CopyInside::Duplicate,
            crossing: CopyCrossing::KeepOutside,
        },
    }
}

/// An engine that also knows a role taking any number of layout targets, and
/// an optional role.
fn engine_with_roles() -> Engine {
    let mut e = engine();
    e.schemas
        .register(schema(MANY, &[TargetClass::Layout], 1, None))
        .unwrap();
    e.schemas
        .register(schema(
            OPTIONAL,
            &[TargetClass::Structural, TargetClass::Node],
            0,
            None,
        ))
        .unwrap();
    e
}

struct Case {
    doc: Document,
    engine: Engine,
    owner: NodeId,
}

fn case() -> Case {
    let doc = Document::new(PEER).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    let owner = doc
        .append_block(BlockKind::Paragraph, "body", "The owner.")
        .unwrap();
    Case {
        doc,
        engine: engine_with_roles(),
        owner,
    }
}

impl Case {
    fn relate(&self, schema: SchemaId, target: Target) {
        let relation = Relation::new(schema)
            .owned_by(self.owner)
            .target("to", target);
        self.doc
            .add_relation(&self.engine.schemas, &relation)
            .unwrap();
    }

    fn layout(&self) -> LayoutSnapshot {
        self.doc.commit();
        self.engine.layout(&self.doc)
    }
}

fn of_schema<'a>(snapshot: &'a LayoutSnapshot, schema: &SchemaId) -> Vec<&'a RelationLayout> {
    snapshot
        .relations
        .iter()
        .filter(|r| &r.schema == schema)
        .collect()
}

fn severity(snapshot: &LayoutSnapshot, code: &str) -> Vec<Severity> {
    snapshot
        .diagnostics_with(code)
        .map(|d| d.severity)
        .collect()
}

#[test]
fn layout_queries_answer_zero_one_and_several() {
    let c = case();
    let long = c
        .doc
        .append_block(BlockKind::Paragraph, "body", OPENING)
        .unwrap();
    let short = c
        .doc
        .append_block(BlockKind::Paragraph, "body", "Short.")
        .unwrap();
    let range = |node: NodeId, bytes: std::ops::Range<usize>| -> RangeId {
        c.doc.add_range(node, bytes, RangePolicy::FIXED).unwrap()
    };
    let (first, last, all) = (
        range(long, 0..3),
        range(long, OPENING.len() - 3..OPENING.len()),
        range(long, 0..OPENING.len()),
    );
    let in_short = range(short, 0..5);

    let queries = [
        LayoutQuery::LineContaining { range: first },
        LayoutQuery::PreviousLine { range: first },
        LayoutQuery::PreviousLine { range: last },
        LayoutQuery::NextLine { range: first },
        LayoutQuery::NextLine { range: last },
        LayoutQuery::FirstLine { node: long },
        LayoutQuery::LastLine { node: long },
        LayoutQuery::FirstLine { node: short },
        LayoutQuery::LastLine { node: short },
        LayoutQuery::LinesIn { range: all },
        LayoutQuery::LinesIn { range: in_short },
        LayoutQuery::FrameContaining { range: last },
        LayoutQuery::PageContaining { range: last },
        LayoutQuery::PreviousLine { range: in_short },
        LayoutQuery::NextLine { range: in_short },
    ];
    for q in &queries {
        c.relate(MANY, Target::Layout(q.clone()));
    }
    let snapshot = c.layout();
    let results = of_schema(&snapshot, &MANY);
    assert_eq!(results.len(), queries.len());
    let n = snapshot.block(long).unwrap().lines.len();
    assert!(n >= 3, "the opening wraps onto several lines");
    let line = |node, line| Some(Resolution::Line(LineRef { node, line }));
    let lines = |node, range: std::ops::Range<usize>| {
        Some(Resolution::Lines(
            range.map(|line| LineRef { node, line }).collect(),
        ))
    };
    let expected: Vec<(Option<Resolution>, RelationStatus)> = vec![
        (line(long, 0), RelationStatus::Valid),
        (None, RelationStatus::Missing), // nothing before the first line
        (line(long, n - 2), RelationStatus::Valid),
        (line(long, 1), RelationStatus::Valid),
        (None, RelationStatus::Missing), // nothing after the last line
        (line(long, 0), RelationStatus::Valid),
        (line(long, n - 1), RelationStatus::Valid),
        (line(short, 0), RelationStatus::Valid),
        (line(short, 0), RelationStatus::Valid),
        (lines(long, 0..n), RelationStatus::Valid), // several, in a role that takes many
        (lines(short, 0..1), RelationStatus::Valid), // one, still a list
        (Some(Resolution::Frame(0)), RelationStatus::Valid),
        (Some(Resolution::Page(0)), RelationStatus::Valid),
        (None, RelationStatus::Missing),
        (None, RelationStatus::Missing),
    ];
    for ((r, (want, status)), q) in results.iter().zip(expected).zip(&queries) {
        assert_eq!(r.targets[0].resolved, want, "{q:?}");
        assert_eq!(r.status, status, "{q:?}");
    }
    // Zero matches are errors here: the role needs a target.
    let no_match = severity(&snapshot, "relation.no-match");
    assert_eq!(no_match.len(), 4);
    assert!(no_match.iter().all(|s| *s == Severity::Error));
}

#[test]
fn a_query_with_several_lines_is_ambiguous_in_a_role_that_takes_one() {
    let c = case();
    let long = c
        .doc
        .append_block(BlockKind::Paragraph, "body", OPENING)
        .unwrap();
    let short = c
        .doc
        .append_block(BlockKind::Paragraph, "body", "Short.")
        .unwrap();
    let many = c
        .doc
        .add_range(long, 0..OPENING.len(), RangePolicy::FIXED)
        .unwrap();
    let one = c.doc.add_range(short, 0..5, RangePolicy::FIXED).unwrap();
    c.relate(
        REFERENCE,
        Target::Layout(LayoutQuery::LinesIn { range: many }),
    );
    c.relate(
        REFERENCE,
        Target::Layout(LayoutQuery::LinesIn { range: one }),
    );
    let snapshot = c.layout();
    let results = of_schema(&snapshot, &REFERENCE);
    let n = snapshot.block(long).unwrap().lines.len();

    // Several: ambiguous, every candidate listed, not applied, a warning.
    assert_eq!(results[0].status, RelationStatus::Ambiguous);
    assert!(!results[0].applied);
    assert_eq!(
        results[0].targets[0].resolved,
        Some(Resolution::Lines(
            (0..n).map(|line| LineRef { node: long, line }).collect()
        ))
    );
    assert_eq!(
        severity(&snapshot, "relation.ambiguous"),
        vec![Severity::Warning]
    );
    // One: nothing to choose.
    assert_eq!(results[1].status, RelationStatus::Valid);
    assert!(results[1].applied);
}

#[test]
fn structural_targets_resolve_through_layout() {
    let c = case();
    let a = c
        .doc
        .append_block(BlockKind::Paragraph, "body", HALLWAY)
        .unwrap();
    let note = c
        .doc
        .append_block(BlockKind::Annotation, "note", "n")
        .unwrap();
    let b = c
        .doc
        .append_block(BlockKind::Paragraph, "body", "B.")
        .unwrap();
    let anchor = c.doc.add_range(a, 0..3, RangePolicy::FIXED).unwrap();
    c.doc
        .add_relation(
            &c.engine.schemas,
            &reprise_fixtures::spike::follow(note, anchor),
        )
        .unwrap();
    let next = |from, kind| Target::Structural(StructuralQuery::NextSibling { from, kind });
    c.relate(OPTIONAL, next(a, Some(BlockKind::Paragraph)));
    c.relate(OPTIONAL, next(b, None)); // zero matches
    c.relate(
        OPTIONAL,
        Target::Structural(StructuralQuery::Children {
            of: None,
            kind: None,
        }),
    );
    c.relate(REFERENCE, next(a, Some(BlockKind::Annotation)));
    let snapshot = c.layout();
    let optional = of_schema(&snapshot, &OPTIONAL);
    assert_eq!(optional[0].targets[0].resolved, Some(Resolution::Node(b)));
    assert_eq!(optional[0].status, RelationStatus::Valid);
    assert!(!optional[0].applied, "tests.optional has no behaviour");
    // Zero matches in an optional role: reported, but nothing is lost.
    assert_eq!(optional[1].status, RelationStatus::Missing);
    // Several in a role that takes any number: just a list.
    assert_eq!(
        optional[2].targets[0].resolved,
        Some(Resolution::Nodes(vec![c.owner, a, note, b]))
    );
    assert_eq!(optional[2].status, RelationStatus::Valid);
    let no_match = snapshot
        .diagnostics_with("relation.no-match")
        .filter(|d| d.subject == Subject::Relation(optional[1].id))
        .map(|d| d.severity)
        .collect::<Vec<_>>();
    assert_eq!(no_match, vec![Severity::Info]);
    // The same in a role that needs one.
    let reference = of_schema(&snapshot, &REFERENCE);
    assert_eq!(
        reference[0].targets[0].resolved,
        Some(Resolution::Node(note))
    );
    assert!(reference[0].applied);
}

#[test]
fn gone_targets_are_a_warning_in_an_optional_role_and_an_error_in_a_required_one() {
    let c = case();
    let target = c
        .doc
        .append_block(BlockKind::Paragraph, "body", "t")
        .unwrap();
    c.relate(OPTIONAL, Target::Node(target));
    c.relate(REFERENCE, Target::Node(target));
    c.doc.delete_block(target).unwrap();
    let snapshot = c.layout();
    let by_relation = |schema: &SchemaId| {
        let id = of_schema(&snapshot, schema)[0].id;
        snapshot
            .diagnostics_with("relation.missing-target")
            .filter(|d| d.subject == Subject::Relation(id))
            .map(|d| d.severity)
            .collect::<Vec<_>>()
    };
    assert_eq!(by_relation(&OPTIONAL), vec![Severity::Warning]);
    assert_eq!(by_relation(&REFERENCE), vec![Severity::Error]);
}

/// The fixture's policies, applied to a document where the deletion happened
/// locally instead of on another peer.
fn local_policy_deletion() -> Result<(Engine, Document), DocError> {
    let mut engine = engine();
    hostile::policy_schemas(&mut engine.schemas)?;
    let doc = Document::new(PEER)?;
    reprise_fixtures::spike::define_styles(&doc)?;
    let para = |text: &str| doc.append_block(BlockKind::Paragraph, "body", text);
    let owner = para("The owner of three relations.")?;
    let rebinds = para("A target that gets replaced.")?;
    let keeps = para("A target that is only missed.")?;
    let deletes = para("A target that takes its relation.")?;
    let heir = para("The replacement.")?;
    let relation = |schema: SchemaId, target| {
        Relation::new(schema)
            .owned_by(owner)
            .target("to", Target::Node(target))
    };
    doc.add_relation(&engine.schemas, &relation(REFERENCE, rebinds))?;
    doc.add_relation(
        &engine.schemas,
        &relation(SchemaId::new("fixtures.keep"), keeps),
    )?;
    doc.add_relation(
        &engine.schemas,
        &relation(SchemaId::new("fixtures.delete"), deletes),
    )?;
    doc.commit();
    doc.supersede(rebinds, heir)?;
    doc.block(heir)?.text.insert(0, "Edited: ")?;
    for node in [rebinds, keeps, deletes] {
        doc.delete_block(node)?;
    }
    doc.commit();
    Ok((engine, doc))
}

#[test]
fn each_policy_behaves_the_same_whether_the_delete_was_local_or_merged() {
    let (engine, local) = local_policy_deletion().unwrap();
    let local = engine.layout(&local);
    let merged = hostile::concurrent_policy_deletion().unwrap();
    let theirs = merged.engine.layout(&merged.doc);
    let replica = merged.engine.layout(merged.replica.as_ref().unwrap());

    for snapshot in [&theirs, &replica] {
        assert_eq!(
            serde_json::to_string(&local.relations).unwrap(),
            serde_json::to_string(&snapshot.relations).unwrap(),
            "same statuses and resolutions"
        );
        let codes = |s: &LayoutSnapshot| -> Vec<_> {
            s.diagnostics
                .iter()
                .filter(|d| d.code.as_str().starts_with("relation."))
                .map(|d| (d.severity, d.code.as_str().to_string(), d.subject.clone()))
                .collect()
        };
        assert_eq!(codes(&local), codes(snapshot), "same diagnostics");
    }

    let by = |name: &str| {
        local
            .relations
            .iter()
            .find(|r| r.schema.as_str() == name)
            .unwrap()
    };
    // Rebind: rebound to the heir, in effect.
    let rebind = by("reprise.reference");
    assert_eq!(rebind.status, RelationStatus::Rebound);
    assert!(rebind.applied);
    assert!(matches!(
        rebind.targets[0].resolved,
        Some(Resolution::Node(_))
    ));
    // KeepMissing: kept, missing, an error because the role needs a target.
    let keep = by("fixtures.keep");
    assert_eq!(keep.status, RelationStatus::Missing);
    assert_eq!(keep.targets[0].resolved, None);
    // Delete: not in effect, and said so at info: it is what the schema asked for.
    let delete = by("fixtures.delete");
    assert_eq!(delete.status, RelationStatus::Deleted);
    assert!(!delete.applied);
    assert_eq!(
        severity(&local, "relation.target-deleted"),
        vec![Severity::Info]
    );
    assert_eq!(
        severity(&local, "relation.missing-target"),
        vec![Severity::Error]
    );
    assert_eq!(severity(&local, "relation.rebound"), vec![Severity::Info]);
}

#[test]
fn snapshot_targets_carry_the_text_as_it_was() {
    let fixture = hostile::snapshot_targets().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    let results = of_schema(&snapshot, &REFERENCE);
    let Some(Resolution::Snapshot(node)) = &results[0].targets[0].resolved else {
        panic!("a found snapshot: {:?}", results[0])
    };
    assert_eq!(node.text, "The original wording.");
    assert!(node.exists_now && !node.unchanged);
    let Some(Resolution::Snapshot(range)) = &results[1].targets[0].resolved else {
        panic!()
    };
    assert_eq!(range.text, "original");
    assert!(!range.exists_now, "the words were replaced");
    // Both found, then two no-matches, then four versions that can't be read.
    let statuses: Vec<_> = results.iter().map(|r| r.status).collect();
    use RelationStatus::{Missing, Valid};
    assert_eq!(
        statuses,
        [
            Valid, Valid, Missing, Missing, Missing, Missing, Missing, Missing
        ]
    );
    assert_eq!(
        severity(&snapshot, "relation.snapshot-unavailable"),
        vec![Severity::Error; 4]
    );
    assert_eq!(
        severity(&snapshot, "relation.no-match"),
        vec![Severity::Error; 2]
    );
}

#[test]
fn compacted_history_loses_old_snapshots_and_keeps_the_document() {
    let fixture = hostile::snapshot_compacted().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    assert_eq!(snapshot.blocks.len(), 2, "the document itself is whole");
    assert!(
        of_schema(&snapshot, &REFERENCE)
            .iter()
            .all(|r| r.status == RelationStatus::Missing)
    );
    assert_eq!(
        severity(&snapshot, "relation.snapshot-unavailable"),
        vec![Severity::Error; 2]
    );
}

#[test]
fn ambiguous_and_missing_rebinds_are_reported_with_their_candidates() {
    let fixture = hostile::structural_matches().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    let results = of_schema(&snapshot, &REFERENCE);
    let statuses: Vec<_> = results.iter().map(|r| r.status).collect();
    use RelationStatus::{Ambiguous, Missing, Valid};
    // One match; two zero matches; several matches; two successors; none.
    assert_eq!(
        statuses,
        [Valid, Missing, Missing, Ambiguous, Ambiguous, Missing]
    );
    let Some(Resolution::Nodes(halves)) = &results[4].targets[0].resolved else {
        panic!()
    };
    assert_eq!(halves.len(), 2, "both halves are candidates");
    assert!(
        results.iter().all(|r| r.applied == (r.status == Valid)),
        "only a valid one is applied"
    );
    let mut severities = snapshot
        .diagnostics
        .iter()
        .filter(|d| d.code.as_str().starts_with("relation."))
        .map(|d| (d.code.as_str(), d.severity))
        .collect::<Vec<_>>();
    severities.sort();
    severities.dedup();
    assert_eq!(
        severities,
        [
            ("relation.ambiguous", Severity::Warning),
            ("relation.missing-target", Severity::Error),
            ("relation.no-match", Severity::Error),
            ("relation.self-reference", Severity::Info),
        ]
    );
}

#[test]
fn a_self_reference_is_resolved_and_reported_not_refused() {
    let fixture = hostile::self_reference().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    let owned: Vec<_> = of_schema(&snapshot, &REFERENCE);
    // Five of six references resolve to their own owner; the sixth, the
    // parent of a top-level block, matches nothing.
    assert_eq!(
        severity(&snapshot, "relation.self-reference"),
        vec![Severity::Info; 5]
    );
    assert_eq!(owned.iter().filter(|r| r.applied).count(), 5);
    assert_eq!(owned.last().unwrap().status, RelationStatus::Missing);
}

#[test]
fn registered_schemas_without_behaviour_still_report_their_targets() {
    let mut c = case();
    c.engine.schemas = {
        let mut s = SchemaRegistry::builtin();
        s.register(schema(OPTIONAL, &[TargetClass::Node], 0, None))
            .unwrap();
        s
    };
    let t = c
        .doc
        .append_block(BlockKind::Paragraph, "body", "t")
        .unwrap();
    c.relate(OPTIONAL, Target::Node(t));
    let snapshot = c.layout();
    let r = of_schema(&snapshot, &OPTIONAL)[0];
    assert_eq!(r.status, RelationStatus::Valid);
    assert!(!r.applied);
    assert_eq!(r.targets[0].resolved, Some(Resolution::Node(t)));
    assert_eq!(snapshot.diagnostics_with("relation.not-applied").count(), 1);
}

#[test]
fn two_peers_resolve_relations_identically_after_a_merge() {
    // A query anchored on a block the other peer deleted.
    let c = case();
    let a = c
        .doc
        .append_block(BlockKind::Paragraph, "body", "a")
        .unwrap();
    let b = c
        .doc
        .append_block(BlockKind::Paragraph, "body", "b")
        .unwrap();
    c.relate(
        REFERENCE,
        Target::Structural(StructuralQuery::NextSibling {
            from: a,
            kind: None,
        }),
    );
    c.doc.commit();
    let other = c.doc.fork(OTHER_PEER).unwrap();
    other.delete_block(b).unwrap();
    c.doc.block(a).unwrap().text.insert(1, "!").unwrap();
    c.doc.merge(&other).unwrap();
    other.merge(&c.doc).unwrap();
    let (x, y) = (c.engine.layout(&c.doc), c.engine.layout(&other));
    assert_eq!(x.to_json(), y.to_json());
    assert_eq!(of_schema(&x, &REFERENCE)[0].status, RelationStatus::Missing);
}

#[test]
fn follow_accepts_previous_first_and_singleton_lines_queries() {
    use reprise_doc::relation::builtin::FOLLOW;
    for query in 0..3 {
        let doc = Document::new(PEER).unwrap();
        reprise_fixtures::spike::define_styles(&doc).unwrap();
        let p = doc
            .append_block(BlockKind::Paragraph, "body", OPENING)
            .unwrap();
        let note = doc
            .append_block(BlockKind::Annotation, "note", "A note.")
            .unwrap();
        let e = engine();
        let before = e.layout(&doc);
        let last = before.block(p).unwrap().lines.last().unwrap().text.clone();
        let range = doc
            .add_range(p, last.start + 1..last.end, RangePolicy::FIXED)
            .unwrap();
        let q = match query {
            0 => LayoutQuery::PreviousLine { range },
            1 => LayoutQuery::FirstLine { node: p },
            _ => LayoutQuery::LinesIn { range },
        };
        let id = doc
            .add_relation(
                &e.schemas,
                &Relation::new(FOLLOW)
                    .owned_by(note)
                    .target("line", Target::Layout(q)),
            )
            .unwrap();
        doc.commit();
        let snapshot = e.layout(&doc);
        let result = snapshot.relation(id).unwrap();
        assert!(result.applied, "{result:?}");
        assert_eq!(result.status, RelationStatus::Valid);
        let n = snapshot.block(p).unwrap().lines.len();
        let want = LineRef {
            node: p,
            line: match query {
                0 => n - 2,
                1 => 0,
                _ => n - 1,
            },
        };
        let resolved = &result.targets[0].resolved;
        assert!(
            resolved == &Some(Resolution::Line(want))
                || resolved == &Some(Resolution::Lines(vec![want]))
        );
        assert_eq!(
            snapshot.block(note).unwrap().lines[0].rect.origin.y,
            snapshot.line(want).unwrap().rect.origin.y
        );
    }
}

#[test]
fn follow_lines_across_frames_is_ambiguous_without_choosing_a_line() {
    let fixture = hostile::follow_lines_across_frames().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    let r = &snapshot.relations[0];
    assert_eq!(r.status, RelationStatus::Ambiguous);
    assert!(!r.applied);
    let Some(Resolution::Lines(lines)) = &r.targets[0].resolved else {
        panic!("all candidates recorded")
    };
    assert!(lines.len() > 6);
    assert!(
        lines
            .windows(2)
            .any(|p| snapshot.line(p[0]).unwrap().frame != snapshot.line(p[1]).unwrap().frame)
    );
    assert_eq!(
        severity(&snapshot, "relation.ambiguous"),
        vec![Severity::Warning]
    );
}

#[test]
fn follow_applies_each_deleted_target_policy() {
    use reprise_doc::relation::builtin::{FOLLOW, follow};
    for policy in [
        OnTargetDeleted::Rebind,
        OnTargetDeleted::KeepMissing,
        OnTargetDeleted::Delete,
    ] {
        let doc = Document::new(PEER).unwrap();
        reprise_fixtures::spike::define_styles(&doc).unwrap();
        let p = doc
            .append_block(BlockKind::Paragraph, "body", "Old line.")
            .unwrap();
        let successor = doc
            .append_block(BlockKind::Paragraph, "body", "New line.")
            .unwrap();
        let note = doc
            .append_block(BlockKind::Annotation, "note", "A note.")
            .unwrap();
        let mut e = engine();
        let mut schema = follow();
        schema.on_target_deleted = policy;
        // Use a fresh registry so the built-in ID has the policy under test.
        e.schemas = SchemaRegistry::default();
        e.schemas.register(schema).unwrap();
        let id = doc
            .add_relation(
                &e.schemas,
                &Relation::new(FOLLOW)
                    .owned_by(note)
                    .target("line", Target::Layout(LayoutQuery::FirstLine { node: p })),
            )
            .unwrap();
        doc.supersede(p, successor).unwrap();
        doc.delete_block(p).unwrap();
        doc.commit();
        let snapshot = e.layout(&doc);
        let r = snapshot.relation(id).unwrap();
        match policy {
            OnTargetDeleted::Rebind => {
                assert_eq!(r.status, RelationStatus::Rebound);
                assert!(r.applied);
                assert_eq!(
                    r.targets[0].resolved,
                    Some(Resolution::Line(LineRef {
                        node: successor,
                        line: 0
                    }))
                );
                assert_eq!(
                    severity(&snapshot, "relation.rebound"),
                    vec![Severity::Info]
                );
            }
            OnTargetDeleted::KeepMissing => {
                assert_eq!(r.status, RelationStatus::Missing);
                assert!(!r.applied);
                assert_eq!(
                    severity(&snapshot, "relation.missing-target"),
                    vec![Severity::Error]
                );
            }
            OnTargetDeleted::Delete => {
                assert_eq!(r.status, RelationStatus::Deleted);
                assert!(!r.applied);
                assert_eq!(
                    severity(&snapshot, "relation.target-deleted"),
                    vec![Severity::Info]
                );
            }
        }
    }
}

#[test]
fn legacy_follow_diagnostics_keep_their_codes_and_severities() {
    let fixture = hostile::deleted_targets().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    assert_eq!(
        severity(&snapshot, "relation.missing-target"),
        vec![Severity::Error; 2]
    );
    assert_eq!(
        severity(&snapshot, "relation.rebound"),
        vec![Severity::Info]
    );
    assert_eq!(
        severity(&snapshot, "relation.owner-not-placeable"),
        vec![Severity::Error]
    );
    assert!(severity(&snapshot, "relation.self-reference").is_empty());
}
