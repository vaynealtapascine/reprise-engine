//! Orchestrator review test: `open` writes a base style into documents that
//! lack one. Two replicas opening the same legacy package each write it
//! concurrently under their own peer; after exchanging updates they must
//! converge to one base style and byte-identical layout, in either order.

use reprise::*;
use reprise_doc::{Document, PersistenceMode};

fn legacy_package() -> Vec<u8> {
    // A document authored before the facade existed: no "" style at all.
    let doc = Document::new(7).unwrap();
    assert!(doc.style("").is_none());
    doc.commit();
    reprise_format::Package::new(
        &doc,
        reprise_format::DocumentId([0x5a; 16]),
        PersistenceMode::History,
    )
    .unwrap()
    .save()
    .unwrap()
}

fn open(bytes: &[u8], peer: &str) -> DocumentSession {
    Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: peer.into(),
            }),
            bytes,
        )
        .unwrap()
}

fn type_block(s: &mut DocumentSession, text: &str) {
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertBlock {
            parent: None,
            index: 0,
            block_kind: BlockKind::Paragraph,
            text: text.into(),
            style: Style::default(),
        }],
    }))
    .unwrap();
}

fn finish(s: &mut DocumentSession) -> String {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    for _ in 0..10_000 {
        if job.step(s, 64).unwrap().data.complete {
            return s.display_json(0).unwrap().data;
        }
    }
    panic!("layout did not finish");
}

#[test]
fn concurrent_opens_of_a_legacy_package_converge() {
    let bytes = legacy_package();
    for order in [false, true] {
        let mut a = open(&bytes, "11");
        let mut b = open(&bytes, "12");
        type_block(&mut a, "office אבג");
        type_block(&mut b, "ffi مرحبا");

        let (from_a, from_b) = (a.export_updates().unwrap(), b.export_updates().unwrap());
        if order {
            b.import_updates(&from_a).unwrap();
            a.import_updates(&from_b).unwrap();
        } else {
            a.import_updates(&from_b).unwrap();
            b.import_updates(&from_a).unwrap();
        }
        assert_eq!(a.sync_info().data.vector, b.sync_info().data.vector);

        let (ja, jb) = (finish(&mut a), finish(&mut b));
        assert_eq!(ja, jb, "replicas diverged (order {order})");
        assert!(ja.contains("office") && ja.contains("ffi"));
        for s in [&a, &b] {
            assert!(
                s.diagnostics()
                    .data
                    .iter()
                    .all(|d| d.severity == Severity::Info),
                "{:?}",
                s.diagnostics().data
            );
        }

        // Saving and reopening on a third peer keeps the converged layout and
        // must not add a second base-style write.
        let saved = a.save().unwrap().data.bytes;
        let mut c = open(&saved, "13");
        assert_eq!(c.sync_info().data.vector, a.sync_info().data.vector);
        assert_eq!(finish(&mut c), ja);
    }
}
