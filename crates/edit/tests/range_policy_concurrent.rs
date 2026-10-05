//! Orchestrator review: a range's authored policy is now persisted and
//! carried through reanchoring. Here two peers reanchor the same ranges
//! concurrently, one splitting the block inside them and the other joining
//! it with the next block, then merge both ways. Both replicas must agree on
//! every surviving range's policy, and each policy must still be the one
//! it was created with.

use reprise_doc::text::RangePolicy;
use reprise_doc::{BlockKind, Document, SchemaRegistry};
use reprise_edit::{Command, Editor, Transaction};
use reprise_fixtures::{OTHER_PEER, PEER};

#[test]
fn concurrent_split_and_join_keep_authored_policies() {
    let doc = Document::new(PEER).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "body", "first block with words")
        .unwrap();
    let b = doc
        .append_block(BlockKind::Paragraph, "body", "second block")
        .unwrap();
    let policies = [
        RangePolicy::EXPANDING,
        RangePolicy::FIXED,
        RangePolicy::POINT,
    ];
    let ranges: Vec<_> = policies
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let at = 6 + i;
            let end = if p == RangePolicy::POINT { at } else { at + 5 };
            (doc.add_range(a, at..end, p).unwrap(), p)
        })
        .collect();
    doc.commit();
    let other = doc.fork(OTHER_PEER).unwrap();
    let mut one = Editor::new(doc, SchemaRegistry::builtin());
    let mut two = Editor::new(other, SchemaRegistry::builtin());
    one.apply(&Transaction::new().with(Command::SplitBlock { node: a, at: 8 }))
        .unwrap();
    two.apply(&Transaction::new().with(Command::JoinBlocks {
        first: a,
        second: b,
    }))
    .unwrap();
    let (d1, d2) = (
        one.document().fork(PEER).unwrap(),
        two.document().fork(OTHER_PEER).unwrap(),
    );
    one.merge(&d2).unwrap();
    two.merge(&d1).unwrap();
    for (id, authored) in ranges {
        let left = one.document().range_policy(id);
        let right = two.document().range_policy(id);
        assert_eq!(left.is_ok(), right.is_ok(), "{id}: both replicas agree");
        if let (Ok(l), Ok(r)) = (left, right) {
            assert_eq!(l, r, "{id}: same policy on both replicas");
            assert_eq!(l, Some(authored), "{id}: still the authored policy");
        }
    }
}
