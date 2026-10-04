//! Orchestrator review: the equivalence harness in `incremental.rs` applies
//! 16 edit kinds in a fixed rotation. This one draws edit kinds from a seeded
//! generator, runs longer walks, and adds edits it lacks: deleting blocks that
//! others point at (anchors, owners, relation targets), editing note and float
//! text in place, frame-relative style expressions followed by a template
//! change (style resolution depends on frame geometry), a concurrent peer
//! deleting a block that is edited locally, and new notes, floats and follows
//! mid-walk. After every edit, the session must equal `Engine::layout`.

use reprise_doc::expr::Expr;
use reprise_doc::relation::builtin::{FLOAT, NOTE};
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    Authored, BlockKind, Document, LengthExpr, Param, Property, Relation, Style, Target,
};
use reprise_fixtures::spike::follow;
use reprise_fixtures::templates::{long_text, responsive_columns, two_columns};
use reprise_fixtures::{OTHER_PEER, hostile, spike};
use reprise_geom::Length;
use reprise_layout::Engine;
use reprise_layout::incremental::LayoutSession;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*: deterministic on every platform.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

fn live(doc: &Document) -> Vec<reprise_doc::NodeId> {
    doc.document_order()
        .into_iter()
        .filter(|&n| doc.block(n).is_ok())
        .collect()
}

fn boundary(text: &str, rng: &mut Rng) -> usize {
    let bounds: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect();
    bounds[rng.below(bounds.len())]
}

fn edit(engine: &Engine, doc: &Document, rng: &mut Rng) {
    let nodes = live(doc);
    let Some(&node) = nodes.get(rng.below(nodes.len())) else {
        doc.append_block(BlockKind::Paragraph, "body", "restart")
            .unwrap();
        return;
    };
    let kind = doc.kind_of(node);
    let block = doc.block(node).unwrap();
    let text = block.text.to_string();
    match rng.below(12) {
        0 => {
            let at = boundary(&text, rng);
            let pieces = [
                "a ",
                "\u{5d0}\u{5d1} ",
                "e\u{301}",
                "\u{2028}",
                "office ",
                "  ",
            ];
            block
                .text
                .insert(at, pieces[rng.below(pieces.len())])
                .unwrap();
        }
        1 => {
            let a = boundary(&text, rng);
            let b = boundary(&text, rng);
            let (a, b) = (a.min(b), a.max(b));
            if a < b {
                block.text.delete(a..b).unwrap();
            }
        }
        2 => {
            // Whatever points at it: anchors, owners, relation targets.
            doc.delete_block(node).unwrap();
        }
        3 => {
            // Edit an annotation (note, float or follow owner) in place.
            if let Some(&owner) = nodes
                .iter()
                .find(|&&n| doc.kind_of(n) == Some(BlockKind::Annotation))
            {
                let b = doc.block(owner).unwrap();
                b.text.insert(0, "longer note text ").unwrap();
            }
        }
        4 => {
            let mut style = Style::default();
            let exprs = ["frame-width / 20", "1em + 1pt", "page-height / 30"];
            if let Ok(e) = Expr::parse(exprs[rng.below(exprs.len())]) {
                style.set(Property::Size, Authored::Expr(e));
            }
            doc.set_overrides(node, &style).unwrap();
        }
        5 => {
            match rng.below(3) {
                0 => doc.set_page_template(&two_columns()).unwrap(),
                1 => doc.set_page_template(&responsive_columns()).unwrap(),
                _ => doc
                    .set_page_template(&reprise_doc::PageTemplate::builtin())
                    .unwrap(),
            };
        }
        6 => {
            doc.define_style(
                "note",
                &Style {
                    parent: Some("body".into()),
                    size: Some(LengthExpr::Em(500 + rng.below(1000) as i32)),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        7 => {
            // A peer deletes the block while it is edited here.
            let peer = doc.fork(OTHER_PEER).unwrap();
            peer.delete_block(node).unwrap();
            block.text.insert(0, "edited while deleted ").unwrap();
            doc.merge(&peer).unwrap();
        }
        8 | 9 | 10 => {
            if kind != Some(BlockKind::Paragraph) || text.is_empty() {
                return;
            }
            let end = text.chars().next().map_or(0, char::len_utf8);
            let range = doc.add_range(node, 0..end, RangePolicy::FIXED).unwrap();
            let owner = doc
                .append_block(BlockKind::Annotation, "note", "an owner")
                .unwrap();
            let relation = match rng.below(3) {
                0 => follow(owner, range),
                1 => Relation::new(NOTE)
                    .owned_by(owner)
                    .target("anchor", Target::Range(range)),
                _ => Relation::new(FLOAT)
                    .owned_by(owner)
                    .target("anchor", Target::Range(range))
                    .param("width", Param::Length(LengthExpr::Pt(Length::from_pt(40)))),
            };
            let _ = doc.add_relation(&engine.schemas, &relation);
        }
        _ => {
            doc.append_block(BlockKind::Paragraph, "body", &long_text(1))
                .unwrap();
        }
    }
}

#[test]
fn random_walks_stay_equivalent_to_full_layout() {
    let fixtures = [
        "deleted_targets",
        "style_bases",
        "notes_nested_three_deep",
        "float_moves_its_anchor",
        "spiral_text",
        "table_row_taller_than_page",
        "incremental_page_seam",
        "concurrent_policy_deletion",
        "bidi_line_override",
    ];
    let mut docs: Vec<(String, Engine, Document)> = hostile::all()
        .unwrap()
        .into_iter()
        .filter(|f| fixtures.contains(&f.name))
        .map(|f| (f.name.to_string(), f.engine, f.doc))
        .collect();
    docs.push((
        "spike".into(),
        reprise_fixtures::engine(),
        spike::document().unwrap().doc,
    ));
    assert_eq!(docs.len(), fixtures.len() + 1, "every named fixture exists");
    for (i, (name, engine, doc)) in docs.iter().enumerate() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (i as u64 + 1));
        let mut session = LayoutSession::new(engine);
        for step in 0..60 {
            edit(engine, doc, &mut rng);
            doc.commit();
            let incremental = session.layout(doc).unwrap();
            assert_eq!(incremental, engine.layout(doc), "{name}, step {step}");
        }
    }
}
