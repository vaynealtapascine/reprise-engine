//! Orchestrator review: undo, redo and merging interleaved on two peers. The
//! kernel's property tests check that random edits converge and that undo
//! after redo restores state; this mixes both, with undo and redo landing
//! after the other peer's merged edits, deletes and moves. At every sync
//! point both replicas must hold the same live blocks, text and layout, and
//! no ID handed out for a new block may ever repeat.

use std::collections::BTreeSet;

use reprise_doc::NewBlock;
use reprise_doc::{BlockKind, Document, SchemaRegistry, Style};
use reprise_edit::{Command, Editor, Transaction};
use reprise_fixtures::{OTHER_PEER, PEER, engine};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
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

fn boundary(text: &str, rng: &mut Rng) -> usize {
    let b: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect();
    b[rng.below(b.len())]
}

fn random_command(doc: &Document, rng: &mut Rng) -> Option<Command> {
    let blocks = doc.blocks();
    let node = *blocks.get(rng.below(blocks.len()))?;
    let text = doc.block(node).ok()?.text.to_string();
    Some(match rng.below(7) {
        0 => Command::InsertText {
            node,
            at: boundary(&text, rng),
            text: ["x", " \u{5d0}", "e\u{301}", "ffi "][rng.below(4)].into(),
        },
        1 => {
            let (a, b) = (boundary(&text, rng), boundary(&text, rng));
            Command::DeleteText {
                node,
                range: a.min(b)..a.max(b),
            }
        }
        2 => Command::SplitBlock {
            node,
            at: boundary(&text, rng),
        },
        3 => Command::InsertBlock {
            parent: None,
            index: rng.below(blocks.len() + 1),
            block: NewBlock {
                kind: BlockKind::Paragraph,
                style: "body".into(),
                overrides: Style::default(),
                text: "inserted".into(),
            },
        },
        4 => Command::DeleteBlock { node },
        5 => Command::MoveBlock {
            node,
            parent: None,
            index: rng.below(blocks.len()),
        },
        _ => {
            let second = *blocks.get(rng.below(blocks.len()))?;
            Command::JoinBlocks {
                first: node,
                second,
            }
        }
    })
}

fn state(doc: &Document) -> Vec<(reprise_doc::NodeId, String)> {
    doc.blocks()
        .into_iter()
        .map(|n| (n, doc.block(n).unwrap().text.to_string()))
        .collect()
}

#[test]
fn undo_redo_and_merges_interleaved_on_two_peers_converge() {
    for seed in 1..=8u64 {
        let base = Document::new(PEER).unwrap();
        reprise_fixtures::spike::define_styles(&base).unwrap();
        for t in ["first paragraph", "second one", "third and last"] {
            base.append_block(BlockKind::Paragraph, "body", t).unwrap();
        }
        base.commit();
        let other = base.fork(OTHER_PEER).unwrap();
        let mut a = Editor::new(base, SchemaRegistry::builtin());
        let mut b = Editor::new(other, SchemaRegistry::builtin());
        let mut rng = Rng(0xD1B5_4A32_D192_ED03 ^ seed);
        let mut created = BTreeSet::new();
        for step in 0..120 {
            let editor = if rng.below(2) == 0 { &mut a } else { &mut b };
            match rng.below(10) {
                0..=5 => {
                    if let Some(command) = random_command(editor.document(), &mut rng) {
                        // Invalid commands are rejected whole; that is fine.
                        if let Ok(applied) = editor.apply(&Transaction::new().with(command)) {
                            for id in applied.blocks {
                                assert!(created.insert(id), "seed {seed} step {step}: ID reused");
                            }
                        }
                    }
                }
                6 => {
                    editor.undo().unwrap();
                }
                7 => {
                    editor.redo().unwrap();
                }
                _ => {
                    let (da, db) = (
                        a.document().fork(PEER).unwrap(),
                        b.document().fork(OTHER_PEER).unwrap(),
                    );
                    a.merge(&db).unwrap();
                    b.merge(&da).unwrap();
                    a.document().commit();
                    b.document().commit();
                    assert_eq!(
                        state(a.document()),
                        state(b.document()),
                        "seed {seed} step {step}: replicas diverged"
                    );
                    let engine = engine();
                    assert_eq!(
                        engine.layout(a.document()).blocks,
                        engine.layout(b.document()).blocks,
                        "seed {seed} step {step}: layouts diverged"
                    );
                }
            }
        }
    }
}
