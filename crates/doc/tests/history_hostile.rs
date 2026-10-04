//! Orchestrator review: forged and odd revisions must never reach Loro in a
//! form that panics. `Document::at` checks that a revision is sorted, has one
//! entry per peer and names known operations, but a revision can pass all of
//! that and still be strange: one entry can causally precede another (not a
//! true frontier), a counter can fall inside a change, and peers and
//! counters can be extreme. Every one must give a version or a
//! `VersionError`, never a panic.

use reprise_doc::{BlockKind, Document, Revision, SnapshotOf, SnapshotRef, SnapshotState};

#[test]
fn odd_revisions_resolve_or_report_without_panicking() {
    let one = Document::new(1).unwrap();
    let a = one
        .append_block(BlockKind::Paragraph, "body", "first words")
        .unwrap();
    one.commit();
    let early = one.revision();
    let two = one.fork(2).unwrap();
    // Peer 2 edits after seeing peer 1's block, so peer 2's operations
    // causally follow peer 1's.
    two.block(a).unwrap().text.insert(0, "later ").unwrap();
    two.commit();
    one.merge(&two).unwrap();
    let (p1, c1) = early.0[0];
    let late = one.revision();
    let p2_last = late.0.iter().find(|(p, _)| *p == 2).map(|e| e.1).unwrap();

    let revisions = [
        // Not an antichain: peer 1's early op precedes peer 2's op.
        Revision(vec![(p1, c1), (2, p2_last)]),
        // Counters inside a change rather than at its end.
        Revision(vec![(p1, 0)]),
        Revision(vec![(p1, c1 / 2), (2, 0)]),
        // Extremes.
        Revision(vec![(u64::MAX, 0)]),
        Revision(vec![(p1, i32::MAX)]),
        Revision(vec![(0, 0), (p1, c1)]),
        Revision(vec![]),
        early.clone(),
        late.clone(),
    ];
    for version in revisions {
        for of in [SnapshotOf::Node(a)] {
            let state = one.resolve_snapshot(&SnapshotRef {
                version: version.clone(),
                of,
            });
            // Any answer is fine; the point is that there is one. A version
            // that resolves must give the block's text as some prefix state.
            if let SnapshotState::Found(content) = state {
                assert!(content.text.ends_with("first words"), "{version:?}");
            }
        }
        let _ = one.at(&version);
    }
}
