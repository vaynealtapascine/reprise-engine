//! Manual phase probe matching Astra 5. Compare minima of multiple runs;
//! scheduler noise is deliberately not a pass/fail performance assertion.
use reprise_doc::{BlockKind, Document};
use reprise_fixtures::{engine, spike};
use reprise_layout::incremental::{LayoutSession, Viewport};
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "manual performance measurement; compare minima of several runs"]
fn phase_timing() {
    for n in [1000, 10000] {
        let t = Instant::now();
        let doc = Document::new(1).unwrap();
        spike::define_styles(&doc).unwrap();
        for _ in 0..n { doc.append_block(BlockKind::Paragraph, "body", "a tiny paragraph").unwrap(); }
        doc.commit();
        println!("paragraphs {n} construct {:?}", t.elapsed());
        let nodes = doc.blocks();
        let t = Instant::now();
        for _ in 0..10 { for &node in &nodes { black_box(doc.block(node).unwrap()); } }
        println!("paragraphs {n} block10 {:?}", t.elapsed());
        let t = Instant::now();
        for _ in 0..3 { for &node in &nodes { black_box(doc.computed_style(node).unwrap()); } }
        println!("paragraphs {n} styles3 {:?}", t.elapsed());
        let e = engine();
        let t = Instant::now();
        let mut session = LayoutSession::new(&e);
        let mut job = session.start(&doc, Viewport::Pages(0..1));
        let mut complete = false;
        for _ in 0..10000 { if job.step(16).unwrap().complete { complete = true; break; } }
        assert!(complete);
        println!("paragraphs {n} layout {:?}", t.elapsed());
    }
}
