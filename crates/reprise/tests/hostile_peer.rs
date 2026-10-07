//! A hostile peer writes seeded raw store operations (wrong types, unknown
//! keys, tree moves and physical deletes, garbage records) and reaches the
//! victim only through a sync packet. The victim must import, lay out,
//! render every page in every backend, export, save and reopen without
//! panicking, and report only documented diagnostic codes.
use std::collections::BTreeSet;

use reprise::*;
use reprise_doc::hostile::{Rng, scribble};

const DOC: &str = "00112233445566778899aabbccddeeff";

fn documented_codes() -> BTreeSet<String> {
    let contracts = include_str!("../../../docs/contracts.md");
    let start = contracts.find("Codes in use:").unwrap_or(0);
    contracts[start..]
        .lines()
        .take_while(|l| l.starts_with('|') || l.trim().is_empty() || l.starts_with("Codes"))
        .flat_map(|l| l.split('`').skip(1).step_by(2).map(str::to_owned))
        .collect()
}

fn layout(s: &mut DocumentSession) -> LayoutProgress {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions {
            max_pages: 40,
            ..LayoutOptions::default()
        }))
        .unwrap();
    for _ in 0..100_000 {
        let progress = job.step(s, 64).unwrap().data;
        if progress.complete {
            return progress;
        }
    }
    panic!("explicit test job bound exceeded")
}

fn victim() -> DocumentSession {
    let mut s = Workspace::new()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: "1".into(),
        }))
        .unwrap();
    s.import_text(&Payload::new(TextImport {
        text: "<h1>Title</h1><p>one <b>two</b> three</p>\
               <table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>\
               <p>after the table</p>"
            .into(),
        html: true,
        at: None,
    }))
    .unwrap();
    s
}

fn vector(clocks: &[Clock]) -> Vec<(u64, i32)> {
    clocks
        .iter()
        .map(|c| (c.peer.parse().unwrap(), c.counter))
        .collect()
}

fn packet(bytes: Vec<u8>) -> Payload<SyncPacket> {
    let (header, _) = reprise_doc::sync::read_header(&bytes).unwrap();
    let clocks = |v: &[(u64, i32)]| {
        v.iter()
            .map(|(p, c)| Clock {
                peer: p.to_string(),
                counter: *c,
            })
            .collect()
    };
    Payload::new(SyncPacket {
        document_id: DOC.into(),
        from_peer: "66".into(),
        format: u32::from(header.format),
        features: header.features.names(),
        kind: SyncKind::Delta,
        since: clocks(&header.since),
        vector: clocks(&header.until),
        content: Bytes { bytes },
    })
}

fn observe(s: &mut DocumentSession, codes: &BTreeSet<String>, case: &str) -> Vec<String> {
    let progress = layout(s);
    let mut seen = Vec::new();
    for &page in &progress.pages {
        seen.push(s.display_json(page).unwrap().data);
        s.svg(page).unwrap();
        s.png(page, 100).unwrap();
    }
    // Exports may refuse a malformed document, but only with a documented
    // code (clipboard export reads every node strictly; see the report).
    for format in [
        ExportFormat::Pdf,
        ExportFormat::Html,
        ExportFormat::PlainText,
    ] {
        if let Err(e) = s.export(&Payload::new(format)) {
            assert!(
                codes.contains(e.code()),
                "{case}: undocumented export code {}",
                e.code()
            );
            seen.push(format!("export refused: {}", e.code()));
        }
    }
    for d in s.diagnostics().data {
        assert!(
            codes.contains(&d.code),
            "{case}: undocumented code {}",
            d.code
        );
    }
    let state = s.state().unwrap().data;
    seen.push(format!("{:?}", state.blocks));
    seen
}

#[test]
fn hostile_raw_operations_never_break_the_pipeline() {
    let codes = documented_codes();
    let mut imported = 0;
    let mut reported = BTreeSet::new();
    for seed in 0..24_u64 {
        let case = format!("seed {seed}");
        let mut s = victim();
        let snapshot = s
            .sync_export(&Payload::new(SyncRequest { since: None }))
            .unwrap();
        let attacker = reprise_doc::Document::new(66).unwrap();
        attacker
            .import_packet(&snapshot.data.content.bytes)
            .unwrap();
        scribble(&attacker, &mut Rng::new(seed), 80);
        let bytes = attacker
            .export_delta(&vector(&s.sync_info().data.vector))
            .unwrap();
        s.sync_import(&packet(bytes))
            .unwrap_or_else(|e| panic!("{case}: {e}"));
        imported += 1;
        let seen = observe(&mut s, &codes, &case);
        reported.extend(
            s.state()
                .unwrap()
                .data
                .diagnostics
                .into_iter()
                .map(|d| d.code),
        );
        let saved = s.save().unwrap().data.bytes;
        let mut reopened = Workspace::new()
            .open(
                &Payload::new(Open {
                    peer_id: "3".into(),
                }),
                &saved,
            )
            .unwrap();
        assert_eq!(
            observe(&mut reopened, &codes, &case),
            seen,
            "{case}: reopen"
        );
    }
    assert_eq!(imported, 24);
    for code in ["collab.malformed-node", "layout.malformed-block"] {
        assert!(
            reported.contains(code),
            "the fuzz never reached {code}: {reported:?}"
        );
    }
}
