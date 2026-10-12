//! Random edits of two peers over paragraphs that hold inline objects
//! (`docs/inline.md`): inserting, deleting across, splitting, joining, undoing,
//! resizing and syncing must keep every object's character and record in step,
//! converge, and lay out the same incrementally as from scratch.

use reprise_doc::text::OBJECT;
use reprise_doc::{BlockKind, Document, InlineObject, InlinePatch, NodeId};
use reprise_edit::{Command, Editor, Reason, Transaction};
use reprise_fixtures::{OTHER_PEER, PEER, engine};
use reprise_layout::incremental::LayoutSession;

const SEEDS: u64 = 6;
const STEPS: usize = 110;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn make_object(rng: &mut Rng, asset: &str) -> InlineObject {
    match rng.below(6) {
        0 | 1 => InlineObject::image(asset, "pic")
            .with("width", format!("pt:{}", (4 + rng.below(40)) * 1024))
            .with("height", format!("pt:{}", (4 + rng.below(40)) * 1024)),
        2 => InlineObject::image(&"ab".repeat(32), "missing"),
        3 => InlineObject::field("page-number"),
        4 => InlineObject::field("text").with("text", "t".repeat(1 + rng.below(4))),
        _ => InlineObject {
            kind: "equation".into(),
            ..Default::default()
        },
    }
}

/// The paragraphs of a document, in order.
fn paragraphs(doc: &Document) -> Vec<NodeId> {
    doc.document_order()
        .into_iter()
        .filter(|&n| doc.block(n).is_ok())
        .collect()
}

fn boundaries(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect()
}

/// Every block's text and objects, for comparing replicas.
fn state(doc: &Document) -> Vec<(NodeId, String, Vec<(usize, InlineObject)>)> {
    paragraphs(doc)
        .into_iter()
        .map(|n| {
            let text = doc.block(n).unwrap().text.to_string();
            let objects = doc
                .inline_objects(n)
                .unwrap()
                .into_iter()
                .map(|e| (e.at, e.object))
                .collect();
            (n, text, objects)
        })
        .collect()
}

/// Each U+FFFC of each block is one live object, in order, and nothing else is.
fn check_objects(doc: &Document, case: &str) {
    let mut ids = std::collections::BTreeSet::new();
    for node in paragraphs(doc) {
        let text = doc.block(node).unwrap().text.to_string();
        let marks: Vec<usize> = text
            .char_indices()
            .filter(|&(_, c)| c == OBJECT)
            .map(|(i, _)| i)
            .collect();
        let entries = doc.inline_objects(node).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.at).collect::<Vec<_>>(),
            marks,
            "{case}: {node} objects match their characters"
        );
        for e in entries {
            assert!(ids.insert(e.id), "{case}: {} is on two blocks", e.id);
            assert_eq!(e.id.host(), node.host());
        }
    }
}

fn sync(a: &mut Editor, b: &mut Editor) {
    let to_b = a
        .document()
        .export_delta(&b.document().version_vector())
        .unwrap();
    b.import_packet(&to_b).unwrap();
    let to_a = b
        .document()
        .export_delta(&a.document().version_vector())
        .unwrap();
    a.import_packet(&to_a).unwrap();
    // A second round: the first import may have been what the other lacked.
    let to_b = a
        .document()
        .export_delta(&b.document().version_vector())
        .unwrap();
    b.import_packet(&to_b).unwrap();
}

type Stats = std::collections::BTreeMap<&'static str, usize>;

fn step(editor: &mut Editor, rng: &mut Rng, asset: &str, case: &str, stats: &mut Stats) {
    let doc = editor.document();
    let blocks = paragraphs(doc);
    if blocks.is_empty() {
        return;
    }
    let node = blocks[rng.below(blocks.len())];
    let text = doc.block(node).unwrap().text.to_string();
    let cuts = boundaries(&text);
    let at = cuts[rng.below(cuts.len())];
    let command = match rng.below(14) {
        0..=2 => Some(Command::InsertText {
            node,
            at,
            text: ["ab ", "xy", " c "][rng.below(3)].into(),
        }),
        3..=5 => Some(Command::InsertInline {
            node,
            at,
            object: make_object(rng, asset),
        }),
        6 | 7 => {
            let other = cuts[rng.below(cuts.len())];
            Some(Command::DeleteText {
                node,
                range: at.min(other)..at.max(other),
            })
        }
        8 => Some(Command::SplitBlock { node, at }),
        9 => {
            let next = blocks
                .iter()
                .position(|&b| b == node)
                .and_then(|i| blocks.get(i + 1));
            next.map(|&second| Command::JoinBlocks {
                first: node,
                second,
            })
        }
        10 => doc
            .inline_objects(node)
            .unwrap()
            .get(rng.below(8))
            .map(|entry| Command::UpdateInline {
                node,
                at: entry.at,
                patch: InlinePatch {
                    params: [(
                        "width".to_string(),
                        Some(format!("pt:{}", (2 + rng.below(60)) * 1024)),
                    )]
                    .into(),
                    ..Default::default()
                },
            }),
        11 => {
            if editor.undo().unwrap() {
                *stats.entry("undo").or_default() += 1;
            }
            None
        }
        12 => {
            if editor.redo().unwrap() {
                *stats.entry("redo").or_default() += 1;
            }
            None
        }
        _ => None,
    };
    if let Some(command) = command {
        let kind = match &command {
            Command::InsertText { .. } => "insert-text",
            Command::InsertInline { .. } => "insert-inline",
            Command::DeleteText { range, .. }
                if text.get(range.clone()).is_some_and(|t| t.contains(OBJECT)) =>
            {
                "delete-over-object"
            }
            Command::DeleteText { .. } => "delete-text",
            Command::SplitBlock { .. } => "split",
            Command::JoinBlocks { .. } => "join",
            Command::UpdateInline { .. } => "update-inline",
            _ => "other",
        };
        match editor.apply(&Transaction::new().with(command.clone())) {
            Ok(_) => *stats.entry(kind).or_default() += 1,
            // A refusal is fine (an update of an object an undo removed, a
            // join with children); an internal failure is not.
            Err(e) => assert!(
                !matches!(e.reason, Reason::Store(_)),
                "{case}: {command:?} failed inside the store: {e:?}"
            ),
        }
    }
}

#[test]
fn two_peers_editing_objects_stay_consistent_and_converge() {
    let mut layouts = 0;
    let mut stats = Stats::new();
    for seed in 0..SEEDS {
        let case = format!("seed {seed}");
        let mut rng = Rng(0x1f2e3d4c + seed * 7919);
        let engine = engine();
        let mut engine = engine;
        let asset = engine.assets.insert(reprise_fixtures::inline::RED).unwrap();
        let base = Document::new(PEER).unwrap();
        reprise_fixtures::spike::define_styles(&base).unwrap();
        for text in ["alpha beta gamma", "delta epsilon", "zeta"] {
            base.append_block(BlockKind::Paragraph, "body", text)
                .unwrap();
        }
        base.commit();
        let mut a = Editor::new(base.fork(PEER).unwrap(), engine.schemas.clone());
        let mut b = Editor::new(base.fork(OTHER_PEER).unwrap(), engine.schemas.clone());
        let mut session_a = LayoutSession::new(&engine);
        for i in 0..STEPS {
            for editor in [&mut a, &mut b] {
                for _ in 0..1 + rng.below(3) {
                    step(editor, &mut rng, &asset, &case, &mut stats);
                }
            }
            check_objects(a.document(), &case);
            check_objects(b.document(), &case);
            if rng.below(4) == 0 {
                sync(&mut a, &mut b);
                assert_eq!(
                    state(a.document()),
                    state(b.document()),
                    "{case}: step {i} converges"
                );
                check_objects(a.document(), &case);
            }
            if i % 20 == 19 {
                a.document().commit();
                let full = engine.layout(a.document());
                let incremental = session_a.layout(a.document()).unwrap();
                assert_eq!(incremental, full, "{case}: step {i} incremental layout");
                layouts += 1;
            }
        }
        sync(&mut a, &mut b);
        assert_eq!(state(a.document()), state(b.document()), "{case}: final");
        // Every live object of the final document has a record that reads.
        let audit = a.document().inline_audit();
        assert!(audit.orphan_markers.is_empty(), "{case}: {audit:?}");
        let _ = audit.orphan_records;
    }
    assert!(layouts >= 30, "{layouts} layouts compared");
    // The run really did exercise objects through every operation.
    for (op, least) in [
        ("insert-inline", 150),
        ("delete-over-object", 20),
        ("split", 30),
        ("join", 15),
        ("update-inline", 20),
        ("undo", 30),
        ("redo", 5),
    ] {
        assert!(
            stats.get(op).copied().unwrap_or(0) >= least,
            "only {:?} of {op}: {stats:?}",
            stats.get(op)
        );
    }
}
