use reprise_doc::{BlockKind, Document, LengthExpr, Style};
use reprise_fixtures::{hostile, spike};
use reprise_geom::Length;
use reprise_layout::{Engine, incremental::LayoutSession};

fn check(session: &mut LayoutSession<'_>, engine: &Engine, doc: &Document, label: &str) {
    doc.commit();
    assert_eq!(session.layout(doc), engine.layout(doc), "{label}");
}
fn edits(engine: &Engine, doc: &Document, label: &str) {
    let mut session = LayoutSession::new(engine);
    check(&mut session, engine, doc, label);
    let mut seed = 0x52657072697365_u64;
    for turn in 0..24 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let nodes: Vec<_> = doc
            .document_order()
            .into_iter()
            .filter(|&n| doc.block(n).is_ok())
            .collect();
        let Some(&node) = nodes.get((seed as usize) % nodes.len().max(1)) else {
            continue;
        };
        let block = doc.block(node).unwrap();
        match turn % 8 {
            0 => {
                block
                    .text
                    .insert(0, "edited \u{e9} \u{5d0}\u{5d1} ")
                    .unwrap();
            }
            1 => {
                let text = block.text.to_string();
                if let Some(c) = text.chars().next() {
                    block.text.delete(0..c.len_utf8()).unwrap();
                }
            }
            2 => {
                doc.set_overrides(
                    node,
                    &Style {
                        size: Some(LengthExpr::Pt(Length::from_pt(12))),
                        ..Style::default()
                    },
                )
                .unwrap();
            }
            3 => {
                doc.define_style(
                    "body",
                    &Style {
                        size: Some(LengthExpr::Pt(Length::from_pt(10 + (seed % 5) as i32))),
                        ..Style::default()
                    },
                )
                .unwrap();
            }
            4 => {
                let peer = doc.fork(2).unwrap();
                peer.block(node).unwrap().text.insert(0, "peer ").unwrap();
                block.text.insert(0, "local ").unwrap();
                doc.merge(&peer).unwrap();
            }
            5 => {
                doc.append_block(
                    BlockKind::Paragraph,
                    "body",
                    "new paragraph crosses pages ".repeat(10).as_str(),
                )
                .unwrap();
            }
            6 => {
                doc.set_overrides(node, &Style::default()).unwrap();
            }
            _ => {
                let n = doc
                    .append_block(BlockKind::Paragraph, "body", "temporary")
                    .unwrap();
                doc.delete_block(n).unwrap();
            }
        }
        check(&mut session, engine, doc, &format!("{label}, edit {turn}"));
    }
}
#[test]
fn every_hostile_fixture_and_spike_stays_equivalent_after_edits() {
    for fixture in hostile::all().unwrap() {
        edits(&fixture.engine, &fixture.doc, fixture.name);
    }
    let spike = spike::document().unwrap();
    edits(&reprise_fixtures::engine(), &spike.doc, "spike");
}
