//! Raw hostile operations through the sync boundary: nothing may panic, and
//! every replica reads the same state the same way.

use crate::hostile::{Rng, scribble};
use crate::relation::builtin::FOLLOW;
use crate::text::RangePolicy;
use crate::{
    BlockKind, Document, LayoutQuery, NewBlock, PersistenceMode, RangeId, Relation, SchemaRegistry,
    Style, Target,
};

fn victim() -> Document {
    let doc = Document::new(1).unwrap();
    doc.define_style("body", &Style::default()).unwrap();
    let p = doc
        .append_block(BlockKind::Paragraph, "body", "one two three")
        .unwrap();
    let n = doc
        .append_block(BlockKind::Annotation, "body", "note")
        .unwrap();
    doc.insert_block_at(
        Some(p),
        0,
        &NewBlock::new(BlockKind::Paragraph, "", "child"),
    )
    .unwrap();
    let r = doc.add_range(p, 4..7, RangePolicy::FIXED).unwrap();
    let rel = Relation::new(FOLLOW).owned_by(n).target(
        "line",
        Target::Layout(LayoutQuery::LineContaining { range: r }),
    );
    doc.add_relation(&SchemaRegistry::builtin(), &rel).unwrap();
    doc.commit();
    doc
}

/// Everything a reader derives from the document, as text.
fn read_all(doc: &Document) -> String {
    let mut out = String::new();
    for id in doc.document_order() {
        out.push_str(&format!(
            "{id} {:?} {:?}\n",
            doc.parent_of(id),
            doc.kind_of(id)
        ));
        match doc.block(id) {
            Ok(b) => out.push_str(&format!("  {:?} {:?} {}\n", b.kind, b.style, b.text)),
            Err(e) => out.push_str(&format!("  {e}\n")),
        }
        if let Ok(s) = doc.computed_style(id) {
            out.push_str(&format!("  {:?}\n", s.size));
        }
        let _ = doc.image(id);
        let _ = doc.table_role(id);
    }
    let tree = doc.tree("ranges");
    let mut ranges = tree.nodes();
    ranges.sort();
    for id in ranges {
        let id = RangeId(id);
        out.push_str(&format!(
            "{id} {:?} {:?}\n",
            doc.resolve_range(id),
            doc.range_policy(id).ok()
        ));
    }
    for (id, rel) in doc.relations() {
        out.push_str(&format!("{id} {}\n", rel.is_ok()));
    }
    for finding in doc.audit() {
        let note = finding.note;
        out.push_str(&format!(
            "{} {} {}\n",
            finding.node,
            note.code.as_str(),
            note.message
        ));
    }
    out
}

#[test]
fn hostile_peer_ops_import_without_panic_and_read_identically() {
    for seed in 0..48_u64 {
        let base = victim();
        let attacker = base.fork(66).unwrap();
        let honest = base.fork(2).unwrap();
        let mut rng = Rng::new(seed);
        scribble(&attacker, &mut rng, 60);
        // The honest peer edits concurrently through the document API.
        let first = honest.blocks()[0];
        honest.block(first).unwrap().text.insert(0, "x").unwrap();
        honest.commit();

        let from_attacker = attacker.export_delta(&base.version_vector()).unwrap();
        let from_honest = honest.export_delta(&base.version_vector()).unwrap();
        base.import_packet(&from_attacker)
            .unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        base.import_packet(&from_honest).unwrap();
        honest.import_packet(&from_attacker).unwrap();
        assert_eq!(base.revision(), honest.revision(), "seed {seed}");
        let seen = read_all(&base);
        assert_eq!(seen, read_all(&honest), "seed {seed}: replicas read alike");

        let saved = base.try_export(PersistenceMode::History).unwrap();
        let reopened = Document::import(&saved, 3).unwrap();
        assert_eq!(read_all(&reopened), seen, "seed {seed}: reopen reads alike");
    }
}

#[test]
fn hostile_fuzz_reaches_the_states_it_claims_to() {
    let mut codes = std::collections::BTreeSet::new();
    let mut unreadable_relations = 0;
    for seed in 0..48_u64 {
        let base = victim();
        let attacker = base.fork(66).unwrap();
        scribble(&attacker, &mut Rng::new(seed), 60);
        base.import_packet(&attacker.export_delta(&base.version_vector()).unwrap())
            .unwrap();
        codes.extend(
            base.audit()
                .into_iter()
                .map(|f| f.note.code.as_str().to_owned()),
        );
        unreadable_relations += base.relations().iter().filter(|(_, r)| r.is_err()).count();
    }
    for code in [
        crate::invariants::MALFORMED_NODE,
        crate::invariants::TREE_TOMBSTONE,
    ] {
        assert!(
            codes.contains(code.as_str()),
            "{code:?} never found: {codes:?}"
        );
    }
    assert!(unreadable_relations > 0);
}

/// Mutates numbers inside a real delta's JSON (positions, lengths, IDs,
/// counters), so operations are well-formed JSON but wrong for the state.
#[test]
fn mutated_json_operations_never_panic() {
    /// Replaces the `target`-th scalar leaf; returns how many leaves remain.
    fn mutate(value: &mut serde_json::Value, target: &mut usize, rng: &mut Rng) {
        match value {
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(|v| mutate(v, target, rng));
            }
            serde_json::Value::Object(map) => {
                map.values_mut().for_each(|v| mutate(v, target, rng));
            }
            leaf => {
                if *target == 0 {
                    let numbers = [
                        0_i64,
                        1,
                        -1,
                        2,
                        7,
                        4096,
                        i64::from(i32::MAX),
                        i64::from(u32::MAX),
                    ];
                    *leaf = if leaf.is_string() && rng.chance(50) {
                        serde_json::json!(
                            rng.pick(&["0@1", "1@66", "999@66", "cid:root-content:Tree", "x"])
                                .copied()
                                .unwrap_or("")
                        )
                    } else {
                        serde_json::json!(rng.pick(&numbers).copied().unwrap_or(0))
                    };
                }
                *target = target.wrapping_sub(1);
            }
        }
    }
    let mut accepted = 0;
    for seed in 0..200_u64 {
        let base = victim();
        let attacker = base.fork(66).unwrap();
        let mut rng = Rng::new(seed);
        scribble(&attacker, &mut rng, 30);
        let packet = attacker.export_delta(&base.version_vector()).unwrap();
        let (header, body) = crate::sync::read_header(&packet).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(body).unwrap();
        let mut target = rng.below(4000) % (body.len() / 4).max(1);
        mutate(&mut json, &mut target, &mut rng);
        let body = serde_json::to_vec(&json).unwrap();
        let tampered = crate::sync::tests_support::write(&header, &body);
        if base.import_packet(&tampered).is_ok() {
            accepted += 1;
        }
        let _ = read_all(&base);
        let saved = base.try_export(PersistenceMode::History).unwrap();
        Document::import(&saved, 3).unwrap();
    }
    assert!(
        accepted > 20,
        "only {accepted} mutated packets were accepted"
    );
}

#[test]
fn operations_outside_the_vocabulary_are_refused_whole() {
    for seed in 0..12_u64 {
        let base = victim();
        let attacker = base.fork(66).unwrap();
        let mut rng = Rng::new(seed);
        scribble(&attacker, &mut rng, 5);
        crate::hostile::scribble_outside(&attacker, &mut rng);
        let before = base.revision();
        let packet = attacker.export_delta(&base.version_vector()).unwrap();
        assert!(
            matches!(
                base.import_packet(&packet),
                Err(crate::sync::SyncError::Invalid(_))
            ),
            "seed {seed}"
        );
        assert_eq!(base.revision(), before, "nothing applied");
    }
}
