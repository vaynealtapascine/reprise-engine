//! One explicit, readable regression per bug the cross-crate fuzzer found.
//!
//! The same scenarios are also kept as minimised byte strings in `corpus/`,
//! which the `cross_crate` test replays. These are written out as scenarios so
//! a reader can see what the bug needed. Bugs in files other workstreams are
//! changing are `#[ignore]`d with a reproduction; see docs/fuzzing.md.

use reprise_doc::{BlockKind, Document};
use reprise_fuzz_harness::engine::EngineSpec;
use reprise_fuzz_harness::scenario::{
    BaseBlock, BaseRange, BaseRelation, BaseSpec, Cmd, EngineChange, LayoutMode, Op, RENDER_PNG,
    Scenario, Start,
};
use reprise_fuzz_harness::{oracle, run_twice};

fn base(text: u8) -> Start {
    Start::Synthetic(BaseSpec {
        blocks: vec![BaseBlock {
            annotation: false,
            style: 0,
            text,
            extra: 1,
        }],
        ranges: vec![],
        relations: vec![],
        template: 0,
        styles: vec![],
    })
}

fn passes(scenario: Scenario) {
    if let Err(violation) = run_twice(&scenario) {
        panic!("{violation}\n{scenario:#?}");
    }
}

/// `display/png.rs` allocated a pixmap for a page of a saturated medium: a
/// terabyte, which aborts the process rather than failing the render.
#[test]
fn png_of_a_saturated_medium_is_refused() {
    passes(Scenario {
        start: base(2),
        ops: vec![
            Op::Engine(EngineChange::Medium(5)),
            Op::Render {
                peer: 0,
                flags: RENDER_PNG,
            },
        ],
    });
}

/// `compose` offered a line break between a space and a ZWJ, which starts a
/// line inside a grapheme cluster. The break only matters where a line fills
/// exactly there, so the prefix is swept, for every composer.
#[test]
fn a_space_before_a_zwj_never_starts_a_line_inside_a_cluster() {
    for composer in 0..4 {
        let mut spec = EngineSpec::default();
        spec.apply(EngineChange::Composer(composer));
        spec.apply(EngineChange::Medium(1));
        let engine = spec.build();
        for words in 0..40 {
            let doc = Document::new(1).unwrap();
            let text = format!("{}  \u{200d}\u{1f467} tail", "word ".repeat(words));
            doc.append_block(BlockKind::Paragraph, "", &text).unwrap();
            doc.commit();
            let snapshot = engine.layout(&doc);
            if let Err(violation) = oracle::structure(&snapshot) {
                panic!("composer {composer}, {words} words: {violation}");
            }
        }
    }
}

/// A range anchored at text that peer B deleted, saved without history
/// (`shallow`) and reopened, panicked inside Loro when layout resolved it.
#[test]
fn a_range_anchored_at_deleted_text_survives_a_shallow_reopen() {
    passes(Scenario {
        start: Start::Synthetic(BaseSpec {
            blocks: vec![BaseBlock {
                annotation: false,
                style: 0,
                text: 5,
                extra: 1,
            }],
            ranges: vec![BaseRange {
                block: 0,
                from: 0,
                to: 0,
                policy: 1,
            }],
            relations: vec![BaseRelation {
                kind: 0,
                owner: 0,
                range: 0,
                aux: 0,
            }],
            template: 0,
            styles: vec![],
        }),
        ops: vec![
            Op::Edit {
                peer: 1,
                cmds: vec![Cmd::DeleteText {
                    block: 0,
                    from: 0,
                    to: 1,
                }],
            },
            Op::SaveReopen {
                peer: 1,
                shallow: true,
                embed_fonts: false,
                as_peer: 0,
            },
            Op::Layout {
                peer: 0,
                mode: LayoutMode::Reference,
            },
        ],
    });
}
