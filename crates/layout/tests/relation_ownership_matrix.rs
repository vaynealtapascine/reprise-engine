//! Round 4 of the B3 fixes: what a float, note or follow relation makes of a
//! block (`ownership.rs`) never reaches a composition unless the memo that
//! reuses the composition is keyed by it.
//!
//! The first matrix of this class (`review_round3_matrix.rs`) was top-level
//! only, and a table cell's annotation escaped it. This one puts the block
//! everywhere composition reads it: at the top of the body and of a running
//! header, and inside a table cell of each. Each block carries an undefined
//! style, so preparing it says so (`style.parent-missing`), which a cached
//! layout can only get right when it knows whether the flow prepared it.
//! Every layout of a persistent session is compared, whole and diagnostics
//! included, with a full layout of the same document, through every add and
//! remove of a relation, from both starting states.

mod common;
use common::*;
use reprise_doc::relation::{SchemaId, builtin};
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, ColumnWidth, Document, LayoutQuery, NodeId, Param, Relation, RelationId, Target,
};
use reprise_layout::incremental::LayoutSession;
use reprise_layout::{Engine, LayoutSnapshot, Subject};
use std::collections::BTreeSet;

const MISSING: &str = "missing-review-style";

#[derive(Clone, Copy, Debug)]
enum Place {
    Body,
    Header,
    BodyCell,
    HeaderCell,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Paragraph,
    Annotation,
    Image,
}

const PLACES: [Place; 4] = [
    Place::Body,
    Place::Header,
    Place::BodyCell,
    Place::HeaderCell,
];
const KINDS: [Kind; 3] = [Kind::Paragraph, Kind::Annotation, Kind::Image];

struct World {
    engine: Engine,
    doc: Document,
    /// The block under test.
    subject: NodeId,
    /// A body paragraph the relations anchor to.
    anchor: NodeId,
}

fn world(place: Place, kind: Kind) -> World {
    let (mut engine, doc) = setup();
    let anchor = block(&doc, "body text the relations anchor to");
    let hash = engine
        .assets
        .insert(&include_bytes!("../../../fixtures/images/red-1x1.png")[..])
        .unwrap();
    let image = reprise_doc::image::ImageData::new(hash);
    let in_cell = matches!(place, Place::BodyCell | Place::HeaderCell);
    let in_header = matches!(place, Place::Header | Place::HeaderCell);
    let (top, subject) = if in_cell {
        let table = doc
            .append_table(two_columns([ColumnWidth::Content, ColumnWidth::Content]))
            .unwrap();
        let row = doc.append_table_row(table, false).unwrap();
        let cell = doc.append_table_cell(row, 0).unwrap();
        doc.append_cell_block(cell, BlockKind::Paragraph, "s", "cell")
            .unwrap();
        let subject = match kind {
            Kind::Paragraph => {
                doc.append_cell_block(cell, BlockKind::Paragraph, MISSING, "subject")
            }
            Kind::Annotation => {
                doc.append_cell_block(cell, BlockKind::Annotation, MISSING, "subject")
            }
            Kind::Image => doc
                .append_cell_block(cell, BlockKind::Image, MISSING, "subject")
                .inspect(|&n| doc.set_image(n, &image).unwrap()),
        }
        .unwrap();
        (table, subject)
    } else {
        let subject = match kind {
            Kind::Paragraph => doc.append_block(BlockKind::Paragraph, MISSING, "subject"),
            Kind::Annotation => doc.append_block(BlockKind::Annotation, MISSING, "subject"),
            Kind::Image => doc.append_image(MISSING, &image, "subject"),
        }
        .unwrap();
        (subject, subject)
    };
    if in_header {
        let story = doc.create_story(&[]).unwrap();
        doc.move_block(top, Some(story), 0).unwrap();
        bind_header(&doc, story);
    }
    doc.commit();
    World {
        engine,
        doc,
        subject,
        anchor,
    }
}

fn owned_by(w: &World, schema: &SchemaId) -> RelationId {
    let range = w.doc.add_range(w.anchor, 0..1, RangePolicy::FIXED).unwrap();
    let relation = Relation::new(schema.clone()).owned_by(w.subject);
    let relation = match schema {
        s if *s == builtin::NOTE => relation
            .target("anchor", Target::Range(range))
            .param("placement", Param::Text("page".into())),
        s if *s == builtin::FLOAT => relation
            .target("anchor", Target::Range(range))
            .param("side", Param::Text("top".into())),
        _ => relation.target(
            "line",
            Target::Layout(LayoutQuery::LineContaining { range }),
        ),
    };
    w.doc.add_relation(&w.engine.schemas, &relation).unwrap()
}

/// Lays `w` out in `session` and checks it is what a full layout is, noting
/// any difference in `bad`. Returns the full layout.
fn agrees(
    w: &World,
    session: &mut LayoutSession<'_>,
    what: &str,
    bad: &mut Vec<String>,
) -> LayoutSnapshot {
    let cached = session.layout(&w.doc).unwrap();
    let full = w.engine.layout(&w.doc);
    if cached != full {
        bad.push(format!(
            "{what}: the session's layout is not a full one\n  cached: {:?}\n  full:   {:?}",
            diagnostics(&cached),
            diagnostics(&full),
        ));
    }
    full
}

fn diagnostics(s: &LayoutSnapshot) -> Vec<String> {
    s.diagnostics
        .iter()
        .map(|d| format!("{} {:?}", d.code.as_str(), d.subject))
        .collect()
}

/// How many times the flow said the block's style is undefined: once for each
/// time it prepared it.
fn prepared(s: &LayoutSnapshot, node: NodeId) -> usize {
    s.diagnostics
        .iter()
        .filter(|d| d.code.as_str() == "style.parent-missing" && d.subject == Subject::Node(node))
        .count()
}

/// Adds and removes a `schema` relation owned by the block of `place` and
/// `kind` through one session, from either starting state. Notes in `seen` the
/// ownership changes the block's own preparation.
fn toggled(
    place: Place,
    kind: Kind,
    schema: &SchemaId,
    owned_first: bool,
    bad: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) {
    let w = world(place, kind);
    let label = format!("{place:?} {kind:?} {schema:?} owned_first={owned_first}");
    let mut relation = owned_first.then(|| owned_by(&w, schema));
    w.doc.commit();
    let mut session = LayoutSession::new(&w.engine);
    let mut last = agrees(&w, &mut session, &format!("{label}, first layout"), bad);
    for step in 1..=3 {
        let what = match relation.take() {
            Some(id) => {
                w.doc.delete_relation(id).unwrap();
                "removed"
            }
            None => {
                relation = Some(owned_by(&w, schema));
                "added"
            }
        };
        w.doc.commit();
        let now = agrees(
            &w,
            &mut session,
            &format!("{label}, {what} (step {step})"),
            bad,
        );
        if prepared(&now, w.subject) != prepared(&last, w.subject) {
            seen.insert(format!("{place:?} {kind:?}"));
        }
        last = now;
    }
}

#[test]
fn ownership_of_a_block_anywhere_composition_reads_it_is_keyed() {
    let mut bad = Vec::new();
    let mut seen = BTreeSet::new();
    for place in PLACES {
        for kind in KINDS {
            for schema in [builtin::NOTE, builtin::FLOAT, builtin::FOLLOW] {
                for owned_first in [false, true] {
                    toggled(place, kind, &schema, owned_first, &mut bad, &mut seen);
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} disagreements:\n{}",
        bad.len(),
        bad.join("\n")
    );
    // Not vacuous: ownership does change what the flow prepares of an
    // annotation in a cell, in the body and in a running story.
    for expected in ["BodyCell Annotation", "HeaderCell Annotation"] {
        assert!(seen.contains(expected), "{expected} not seen in {seen:?}");
    }
}

/// An image or annotation two relations name owner: one goes, the other stays.
#[test]
fn two_relations_on_one_block_leave_the_composition_of_the_other_keyed() {
    let mut bad = Vec::new();
    for place in PLACES {
        for kind in [Kind::Annotation, Kind::Image] {
            for (first, second) in [
                (builtin::FOLLOW, builtin::NOTE),
                (builtin::FOLLOW, builtin::FLOAT),
                (builtin::NOTE, builtin::FLOAT),
            ] {
                let w = world(place, kind);
                let label = format!("{place:?} {kind:?} {first:?}+{second:?}");
                let mut session = LayoutSession::new(&w.engine);
                agrees(
                    &w,
                    &mut session,
                    &format!("{label}, first layout"),
                    &mut bad,
                );
                let a = owned_by(&w, &first);
                w.doc.commit();
                agrees(&w, &mut session, &format!("{label}, first added"), &mut bad);
                let b = owned_by(&w, &second);
                w.doc.commit();
                agrees(
                    &w,
                    &mut session,
                    &format!("{label}, second added"),
                    &mut bad,
                );
                w.doc.delete_relation(a).unwrap();
                w.doc.commit();
                agrees(
                    &w,
                    &mut session,
                    &format!("{label}, first removed"),
                    &mut bad,
                );
                w.doc.delete_relation(b).unwrap();
                w.doc.commit();
                agrees(&w, &mut session, &format!("{label}, both gone"), &mut bad);
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} disagreements:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// The key is no looser than it must be: a relation on a block the story does
/// not hold leaves the story's composition alone, and one on a block it holds
/// makes it again.
#[test]
fn only_a_relation_on_a_block_the_story_holds_makes_it_compose_again() {
    for schema in [builtin::NOTE, builtin::FLOAT] {
        let w = world(Place::HeaderCell, Kind::Annotation);
        let mut session = LayoutSession::new(&w.engine);
        let mut bad = Vec::new();
        agrees(&w, &mut session, "first layout", &mut bad);
        // The body paragraph owns this one: no block of the header.
        let outside = {
            let range = w.doc.add_range(w.anchor, 0..1, RangePolicy::FIXED).unwrap();
            let relation = Relation::new(schema.clone())
                .owned_by(w.anchor)
                .target("anchor", Target::Range(range));
            let relation = if schema == builtin::NOTE {
                relation.param("placement", Param::Text("page".into()))
            } else {
                relation.param("side", Param::Text("top".into()))
            };
            w.doc.add_relation(&w.engine.schemas, &relation).unwrap()
        };
        w.doc.commit();
        agrees(&w, &mut session, "relation outside the story", &mut bad);
        let outside_counters = session.counters();
        assert_eq!(
            outside_counters.running_compositions, 0,
            "{schema:?}: a relation on a body paragraph recomposed the header"
        );
        assert!(outside_counters.running_reused > 0, "{schema:?}");
        w.doc.delete_relation(outside).unwrap();
        w.doc.commit();
        agrees(
            &w,
            &mut session,
            "relation outside the story gone",
            &mut bad,
        );
        let inside = owned_by(&w, &schema);
        w.doc.commit();
        agrees(
            &w,
            &mut session,
            "relation on the cell annotation",
            &mut bad,
        );
        assert!(
            session.counters().running_compositions > 0,
            "{schema:?}: a relation on the header's cell annotation reused the old composition"
        );
        w.doc.delete_relation(inside).unwrap();
        w.doc.commit();
        agrees(
            &w,
            &mut session,
            "relation on the cell annotation gone",
            &mut bad,
        );
        assert!(session.counters().running_compositions > 0, "{schema:?}");
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
