//! The bounded, resumable child reader (`Document::top_reader`,
//! `children_reader`, `read_children`) reads what `top` and `children_of` read,
//! in pieces: a read looks at a bounded number of child IDs, whatever the
//! domain's size, and reading a prefix costs the prefix.
use reprise_doc::{BlockKind, ChildReader, Document, Domain, NewBlock, NodeId};

fn story(doc: &Document, count: usize) -> NodeId {
    let blocks: Vec<_> = (0..count)
        .map(|i| NewBlock::new(BlockKind::Paragraph, "", &format!("block {i}")))
        .collect();
    doc.create_story(&blocks).unwrap()
}

/// Reads `reader` to the end, `chunk` IDs a call, checking each call's bound.
fn drain(doc: &Document, mut reader: ChildReader, chunk: usize) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut calls = 0;
    loop {
        let before = reader.remaining();
        let mut piece = Vec::new();
        let done = doc.read_children(&mut reader, chunk, &mut piece);
        let looked = before - reader.remaining();
        assert!(looked <= chunk.max(1), "{looked} IDs looked at for {chunk}");
        assert!(looked >= 1 || before == 0, "a call makes progress");
        out.extend(piece);
        calls += 1;
        assert!(calls < 100_000);
        if done {
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
    assert!(!doc.read_children(&mut reader, 100, &mut out));
    assert_eq!(out.len(), 100);
    assert_eq!(reader.remaining(), 4_900, "the rest was not looked at");
    // A reader of something that is not a live domain reads nothing.
    let dead = doc.top(Domain::Story(s))[0];
    assert!(doc.read_children(&mut doc.children_reader(dead), 8, &mut out));
    assert_eq!(out.len(), 100);
    let gone = story(&doc, 1);
    doc.delete_block(gone).unwrap();
    assert!(doc.read_children(&mut doc.top_reader(Domain::Story(gone)), 8, &mut out));
    assert_eq!(out.len(), 100);
}
