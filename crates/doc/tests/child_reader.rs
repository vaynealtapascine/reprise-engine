//! The bounded, resumable child reader (`Document::top_reader`,
//! `children_reader`, `read_children`) reads what `top` and `children_of` read,
//! in pieces: a call does a bounded amount of work, whatever the size of the
//! domain, of a flow host in it, or of the host's text, and reading a prefix
//! costs the prefix.
use reprise_doc::{
    BlockKind, ChildReader, DECODE_UNIT_BYTES, Document, Domain, NewBlock, NodeId, ReadLimits,
};

fn story(doc: &Document, count: usize) -> NodeId {
    let blocks: Vec<_> = (0..count)
        .map(|i| NewBlock::new(BlockKind::Paragraph, "", &format!("block {i}")))
        .collect();
    doc.create_story(&blocks).unwrap()
}

fn limits(units: usize) -> ReadLimits {
    ReadLimits {
        units,
        blocks: usize::MAX,
        host_bytes: usize::MAX,
    }
}

/// Reads `reader` to the end, `chunk` units a call, checking each call's bound.
fn drain(doc: &Document, mut reader: ChildReader, chunk: usize) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut calls = 0;
    loop {
        let mut piece = Vec::new();
        let read = doc.read_children(&mut reader, limits(chunk), &mut piece);
        assert!(
            read.looked <= chunk.max(1),
            "{} IDs for {chunk}",
            read.looked
        );
        assert!(
            read.units <= chunk.max(1),
            "{} units for {chunk}: small hosts never overspend",
            read.units
        );
        assert!(read.blocks == piece.len());
        out.extend(piece);
        calls += 1;
        assert!(calls < 1_000_000);
        if read.done {
            assert!(reader.is_done());
            return out;
        }
    }
}

#[test]
fn chunks_of_any_size_read_what_top_reads() {
    let doc = Document::new(1).unwrap();
    let s = story(&doc, 20);
    let kids = doc.top(Domain::Story(s));
    // Dead children are looked at and skipped.
    for dead in [1usize, 2, 7, 19] {
        doc.delete_block(kids[dead]).unwrap();
    }
    for i in 0..5 {
        doc.append_block(BlockKind::Paragraph, "", &format!("body {i}"))
            .unwrap();
    }
    doc.commit();
    for chunk in [0usize, 1, 2, 3, 7, 64, usize::MAX] {
        assert_eq!(
            drain(&doc, doc.top_reader(Domain::Story(s)), chunk),
            doc.top(Domain::Story(s)),
            "story, chunk {chunk}"
        );
        assert_eq!(
            drain(&doc, doc.top_reader(Domain::Body), chunk),
            doc.top(Domain::Body),
            "body, chunk {chunk}"
        );
    }
}

#[test]
fn a_flow_host_stands_for_all_its_paragraphs_and_a_table_reads_by_level() {
    let doc = Document::new(1).unwrap();
    let host = doc
        .append_block(BlockKind::Paragraph, "", "one two three")
        .unwrap();
    let tail = doc.split_block(host, 3).unwrap();
    doc.split_block(tail, 4).unwrap();
    let table = doc
        .append_table(reprise_doc::TableColumns {
            columns: vec![reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Content,
            }],
        })
        .unwrap();
    let rows: Vec<_> = (0..3)
        .map(|_| doc.append_table_row(table, false).unwrap())
        .collect();
    doc.commit();
    for chunk in [1usize, 2, 64] {
        assert_eq!(
            drain(&doc, doc.top_reader(Domain::Body), chunk),
            doc.top(Domain::Body),
            "chunk {chunk}"
        );
        assert_eq!(
            drain(&doc, doc.children_reader(table), chunk),
            rows,
            "chunk {chunk}"
        );
        assert_eq!(
            drain(&doc, doc.children_reader(table), chunk),
            doc.children_of(table)
        );
    }
    assert!(doc.top(Domain::Body).len() >= 4, "the host expands");
}

#[test]
fn a_prefix_costs_the_prefix() {
    let doc = Document::new(1).unwrap();
    let s = story(&doc, 5_000);
    doc.commit();
    let mut reader = doc.top_reader(Domain::Story(s));
    assert_eq!(reader.remaining(), 5_000);
    let mut out = Vec::new();
    let read = doc.read_children(&mut reader, limits(100), &mut out);
    assert!(!read.done);
    assert_eq!((read.looked, read.units), (100, 100));
    assert_eq!(out.len(), 100);
    assert_eq!(reader.remaining(), 4_900, "the rest was not looked at");
    // A reader of something that is not a live domain reads nothing.
    let dead = doc.top(Domain::Story(s))[0];
    assert!(
        doc.read_children(&mut doc.children_reader(dead), limits(8), &mut out)
            .done
    );
    assert_eq!(out.len(), 100);
    let gone = story(&doc, 1);
    doc.delete_block(gone).unwrap();
    assert!(
        doc.read_children(
            &mut doc.top_reader(Domain::Story(gone)),
            limits(8),
            &mut out
        )
        .done
    );
    assert_eq!(out.len(), 100);
}

/// A host of `paragraphs` paragraphs: one physical child of the body.
fn big_host(doc: &Document, paragraphs: usize) -> NodeId {
    let host = doc.append_block(BlockKind::Paragraph, "", "x").unwrap();
    let mut tail = host;
    for _ in 1..paragraphs {
        tail = doc.split_block(tail, 0).unwrap();
    }
    doc.commit();
    host
}

#[test]
fn one_flow_host_is_expanded_a_bounded_piece_at_a_time() {
    let doc = Document::new(1).unwrap();
    big_host(&doc, 200);
    let all = doc.top(Domain::Body);
    assert_eq!(all.len(), 200, "one physical child, two hundred paragraphs");
    for chunk in [1usize, 7, 64] {
        let mut reader = doc.top_reader(Domain::Body);
        assert_eq!(reader.remaining(), 1);
        let (mut out, mut calls) = (Vec::new(), 0);
        loop {
            let before = out.len();
            let read = doc.read_children(&mut reader, limits(chunk), &mut out);
            // A call makes at most `chunk` blocks, not the host's two hundred.
            assert!(out.len() - before <= chunk, "chunk {chunk}");
            assert!(read.units <= chunk, "chunk {chunk}: {read:?}");
            calls += 1;
            if calls == 1 && chunk < 200 {
                assert!(!reader.is_done(), "the host is half expanded");
                assert_eq!(reader.remaining(), 0, "its ID was looked at");
            }
            if read.done {
                break;
            }
        }
        assert_eq!(out, all, "chunk {chunk}");
        assert!(calls >= 200 / chunk, "chunk {chunk}: {calls} calls");
    }
}

#[test]
fn a_block_limit_stops_a_call_in_the_middle_of_a_host() {
    let doc = Document::new(1).unwrap();
    big_host(&doc, 50);
    let mut reader = doc.top_reader(Domain::Body);
    let mut out = Vec::new();
    let limits = ReadLimits {
        units: usize::MAX,
        blocks: 3,
        host_bytes: usize::MAX,
    };
    let read = doc.read_children(&mut reader, limits, &mut out);
    assert_eq!((read.blocks, out.len(), read.done), (3, 3, false));
    let again = doc.read_children(&mut reader, limits, &mut out);
    assert_eq!((again.blocks, out.len()), (3, 6));
}

#[test]
fn a_host_longer_than_the_limit_is_refused_and_one_dearer_than_the_call_runs_alone() {
    let doc = Document::new(1).unwrap();
    let small = doc.append_block(BlockKind::Paragraph, "", "small").unwrap();
    let text = "y".repeat(10 * DECODE_UNIT_BYTES);
    let large = doc.append_block(BlockKind::Paragraph, "", &text).unwrap();
    doc.commit();

    // Over the limit: the call stops in front of the host, and so does the
    // next; nothing of it is decoded.
    let mut reader = doc.top_reader(Domain::Body);
    let mut out = Vec::new();
    let cap = ReadLimits {
        units: usize::MAX,
        blocks: usize::MAX,
        host_bytes: 2 * DECODE_UNIT_BYTES,
    };
    let read = doc.read_children(&mut reader, cap, &mut out);
    assert!(read.refused && !read.done, "{read:?}");
    assert_eq!((out.as_slice(), read.decoded), ([small].as_slice(), 5));
    let read = doc.read_children(&mut reader, cap, &mut out);
    assert!(
        read.refused && read.blocks == 0 && read.decoded == 0,
        "{read:?}"
    );

    // Within it, a decode dearer than the units of a call that has done other
    // things waits for a call of its own, which overspends.
    let mut reader = doc.top_reader(Domain::Body);
    let mut out = Vec::new();
    let read = doc.read_children(&mut reader, limits(4), &mut out);
    assert!(read.deferred && !read.done, "{read:?}");
    assert_eq!((out.as_slice(), read.decoded), ([small].as_slice(), 5));
    let read = doc.read_children(&mut reader, limits(4), &mut out);
    assert_eq!(read.decoded, text.len());
    assert!(read.units > 4, "the decode alone is {} units", read.units);
    assert_eq!(read.units, text.len() / DECODE_UNIT_BYTES);
    assert_eq!(out, [small, large]);
}

/// A small deterministic generator, so the documents are the same every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
}

#[test]
fn any_flow_of_splits_joins_deletions_and_embedded_blocks_reads_like_top() {
    for seed in 1..=12u64 {
        let doc = Document::new(1).unwrap();
        let story = story(&doc, 3);
        let mut rng = Rng(seed);
        for step in 0..60 {
            // Edit the story's top level or the body's, and a flow host's
            // paragraphs among them.
            let (domain, parent) = if rng.next(2) == 0 {
                (Domain::Body, None)
            } else {
                (Domain::Story(story), Some(story))
            };
            let blocks = doc.top(domain);
            let pick = blocks.get(rng.next(blocks.len())).copied();
            match (rng.next(6), pick) {
                (0, _) => {
                    let text = format!("paragraph {step} {}", "x".repeat(rng.next(40)));
                    let _ = doc.insert_block_at(
                        parent,
                        rng.next(blocks.len() + 1),
                        &NewBlock::new(BlockKind::Paragraph, "", &text),
                    );
                }
                (1 | 2, Some(block)) => {
                    let len = doc.block(block).map_or(0, |b| b.text.len());
                    let _ = doc.split_block(block, rng.next(len + 1));
                }
                (3, Some(block)) => {
                    let _ = doc.delete_block(block);
                }
                (4, Some(block)) => {
                    // Join with the block after it, when there is one.
                    let at = blocks.iter().position(|b| *b == block).unwrap_or(0);
                    if let Some(next) = blocks.get(at + 1) {
                        let _ = doc.join_blocks(block, *next);
                    }
                }
                _ => {
                    let _ = doc.append_block(BlockKind::Paragraph, "", "tail");
                }
            }
            doc.commit();
            if step % 6 == 5 {
                for chunk in [1usize, 3, 64] {
                    for domain in [Domain::Body, Domain::Story(story)] {
                        assert_eq!(
                            drain(&doc, doc.top_reader(domain), chunk),
                            doc.top(domain),
                            "seed {seed}, step {step}, chunk {chunk}, {domain:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_block_embedded_in_a_host_is_expanded_in_place_and_read_once() {
    let doc = Document::new(1).unwrap();
    let host = doc
        .append_block(BlockKind::Paragraph, "", "one two")
        .unwrap();
    let tail = doc.split_block(host, 3).unwrap();
    // A block placed between the host's paragraphs is embedded in it; split,
    // it is a host of its own inside the first.
    let inner = doc
        .insert_block_at(
            None,
            1,
            &NewBlock::new(BlockKind::Paragraph, "", "inner text"),
        )
        .unwrap();
    let inner_tail = doc.split_block(inner, 5).unwrap();
    doc.commit();
    let expected = vec![host, inner, inner_tail, tail];
    assert_eq!(doc.top(Domain::Body), expected);
    for chunk in [1usize, 2, 64] {
        let reader = doc.top_reader(Domain::Body);
        assert_eq!(reader.remaining(), 1, "only the outer host is a child");
        assert_eq!(drain(&doc, reader, chunk), expected, "chunk {chunk}");
    }
}
