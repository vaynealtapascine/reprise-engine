//! Scenarios: what a byte string decodes to.
//!
//! A [`Scenario`] is a starting point plus a list of [`Op`]s. Ops hold
//! *selectors*, not identities: "the block at index `n` modulo the live
//! blocks". That keeps every scenario meaningful after any mutation of its
//! bytes and lets the executor stay a pure function of the scenario.

use crate::input::Input;

/// The most ops one scenario runs. Bounded so a fuzzer input can't make a run
/// unboundedly long.
pub const MAX_OPS: usize = 48;
/// The most commands in one batched transaction.
pub const MAX_BATCH: usize = 4;
/// The most budgets in one layout job.
pub const MAX_BUDGETS: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scenario {
    pub start: Start,
    pub ops: Vec<Op>,
}

/// How the two peers come to exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// A document built from the pools, forked for the second peer.
    Synthetic(BaseSpec),
    /// A hostile fixture, saved to a package and opened once per peer.
    Fixture(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseSpec {
    pub blocks: Vec<BaseBlock>,
    pub ranges: Vec<BaseRange>,
    pub relations: Vec<BaseRelation>,
    pub template: u8,
    pub styles: Vec<(u8, u8, u8, u8)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseBlock {
    pub annotation: bool,
    pub style: u8,
    pub text: u8,
    pub extra: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseRange {
    pub block: u8,
    pub from: u8,
    pub to: u8,
    pub policy: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseRelation {
    pub kind: u8,
    pub owner: u8,
    pub range: u8,
    pub aux: u8,
}

/// One authored command. Selectors are resolved against the live document at
/// execution time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    InsertText {
        block: u16,
        at: u16,
        text: u8,
    },
    DeleteText {
        block: u16,
        from: u16,
        to: u16,
    },
    Split {
        block: u16,
        at: u16,
    },
    Join {
        first: u16,
        second: u16,
    },
    InsertBlock {
        parent: Option<u16>,
        index: u16,
        kind: u8,
        style: u8,
        text: u8,
    },
    DeleteBlock {
        block: u16,
    },
    MoveBlock {
        block: u16,
        parent: Option<u16>,
        index: u16,
    },
    SetOverride {
        block: u16,
        variant: u8,
        family: u8,
        parent: u8,
    },
    AddRelation {
        kind: u8,
        owner: u16,
        target: u16,
        aux: u8,
    },
    RemoveRelation {
        index: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMode {
    /// A takes B's edits, then B takes A's (now merged) state.
    AThenB,
    BThenA,
    /// Both take the other's state as it was before either merged.
    Crossed,
    /// Like `Crossed`, through exported and imported bytes.
    ViaBytes,
    /// Only one direction: replicas may stay apart.
    OneWay {
        from_a: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopySel {
    All,
    Blocks(Vec<(u16, Option<(u16, u16)>)>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineChange {
    Composer(u8),
    Medium(u8),
    MaxPages(u8),
    RegisterFont(u8),
    RegisterImage(u8),
    RemoveImage(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    /// `Engine::layout` twice, and the invariants on the snapshot.
    Reference,
    /// The peer's long-lived `LayoutSession`, compared with the reference.
    Session,
    /// A budgeted `LayoutJob`.
    Job {
        budgets: Vec<u16>,
        viewport: u8,
        partial: bool,
        cancel_after: Option<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Edit {
        peer: u8,
        cmds: Vec<Cmd>,
    },
    Undo {
        peer: u8,
    },
    Redo {
        peer: u8,
    },
    Sync(SyncMode),
    Copy {
        peer: u8,
        sel: CopySel,
        wire: bool,
    },
    Paste {
        peer: u8,
        at: Option<(u16, u16)>,
        other_namespace: bool,
    },
    InsertImage {
        peer: u8,
        image: u8,
        size: u8,
        alt: u8,
        at: Option<(u16, u16)>,
    },
    ImportText {
        peer: u8,
        html: bool,
        text: u8,
        at: Option<(u16, u16)>,
    },
    Template {
        peer: u8,
        which: u8,
    },
    DefineStyle {
        peer: u8,
        name: u8,
        variant: u8,
        family: u8,
        parent: u8,
    },
    Engine(EngineChange),
    Layout {
        peer: u8,
        mode: LayoutMode,
    },
    SaveReopen {
        peer: u8,
        shallow: bool,
        embed_fonts: bool,
        as_peer: u8,
    },
    Render {
        peer: u8,
        flags: u8,
    },
    Export {
        peer: u8,
        kind: u8,
    },
    Navigate {
        peer: u8,
        seed: u16,
    },
}

pub const RENDER_SVG: u8 = 1;
pub const RENDER_PNG: u8 = 2;
pub const RENDER_PDF: u8 = 4;
pub const RENDER_PDF_ORDERED: u8 = 8;
pub const RENDER_DEBUG: u8 = 16;

impl Scenario {
    /// Total: every byte string decodes.
    pub fn decode(data: &[u8]) -> Scenario {
        let mut input = Input::new(data);
        let start = decode_start(&mut input);
        let mut ops = Vec::new();
        while !input.is_empty() && ops.len() < MAX_OPS {
            ops.push(decode_op(&mut input));
        }
        Scenario { start, ops }
    }
}

fn decode_start(input: &mut Input<'_>) -> Start {
    if input.u8() % 8 == 0 {
        return Start::Fixture(input.u8());
    }
    let blocks = (0..1 + input.below(5))
        .map(|_| BaseBlock {
            annotation: input.chance(48),
            style: input.u8(),
            text: input.u8(),
            extra: input.u8(),
        })
        .collect();
    let ranges = (0..input.below(5))
        .map(|_| BaseRange {
            block: input.u8(),
            from: input.u8(),
            to: input.u8(),
            policy: input.u8(),
        })
        .collect();
    let relations = (0..input.below(4))
        .map(|_| BaseRelation {
            kind: input.u8(),
            owner: input.u8(),
            range: input.u8(),
            aux: input.u8(),
        })
        .collect();
    let template = input.u8();
    let styles = (0..input.below(3))
        .map(|_| (input.u8(), input.u8(), input.u8(), input.u8()))
        .collect();
    Start::Synthetic(BaseSpec {
        blocks,
        ranges,
        relations,
        template,
        styles,
    })
}

fn peer(input: &mut Input<'_>) -> u8 {
    input.u8() & 1
}

fn at(input: &mut Input<'_>) -> Option<(u16, u16)> {
    input.chance(160).then(|| (input.u16(), input.u16()))
}

fn decode_cmd(input: &mut Input<'_>) -> Cmd {
    match input.below(16) {
        0..=3 => Cmd::InsertText {
            block: input.u16(),
            at: input.u16(),
            text: input.u8(),
        },
        4..=5 => Cmd::DeleteText {
            block: input.u16(),
            from: input.u16(),
            to: input.u16(),
        },
        6 => Cmd::Split {
            block: input.u16(),
            at: input.u16(),
        },
        7 => Cmd::Join {
            first: input.u16(),
            second: input.u16(),
        },
        8 => Cmd::InsertBlock {
            parent: input.chance(40).then(|| input.u16()),
            index: input.u16(),
            kind: input.u8(),
            style: input.u8(),
            text: input.u8(),
        },
        9 => Cmd::DeleteBlock { block: input.u16() },
        10 => Cmd::MoveBlock {
            block: input.u16(),
            parent: input.chance(40).then(|| input.u16()),
            index: input.u16(),
        },
        11 => Cmd::SetOverride {
            block: input.u16(),
            variant: input.u8(),
            family: input.u8(),
            parent: input.u8(),
        },
        12..=14 => Cmd::AddRelation {
            kind: input.u8(),
            owner: input.u16(),
            target: input.u16(),
            aux: input.u8(),
        },
        _ => Cmd::RemoveRelation { index: input.u16() },
    }
}

fn decode_sync(input: &mut Input<'_>) -> SyncMode {
    match input.below(6) {
        0 => SyncMode::AThenB,
        1 => SyncMode::BThenA,
        2 => SyncMode::Crossed,
        3 => SyncMode::ViaBytes,
        4 => SyncMode::OneWay { from_a: true },
        _ => SyncMode::OneWay { from_a: false },
    }
}

fn decode_layout(input: &mut Input<'_>) -> LayoutMode {
    match input.below(4) {
        0 => LayoutMode::Reference,
        1 => LayoutMode::Session,
        _ => LayoutMode::Job {
            budgets: (0..1 + input.below(MAX_BUDGETS))
                .map(|_| match input.below(8) {
                    0 => 0,
                    1 => u16::MAX,
                    _ => u16::from(input.u8() % 12),
                })
                .collect(),
            viewport: input.u8(),
            partial: input.chance(160),
            cancel_after: input.chance(40).then(|| input.u8()),
        },
    }
}

fn decode_op(input: &mut Input<'_>) -> Op {
    match input.u8() {
        0..=95 => {
            let count = if input.chance(40) {
                2 + input.below(MAX_BATCH - 1)
            } else {
                1
            };
            Op::Edit {
                peer: peer(input),
                cmds: (0..count).map(|_| decode_cmd(input)).collect(),
            }
        }
        96..=110 => Op::Undo { peer: peer(input) },
        111..=120 => Op::Redo { peer: peer(input) },
        121..=135 => Op::Sync(decode_sync(input)),
        136..=146 => Op::Copy {
            peer: peer(input),
            sel: if input.chance(96) {
                CopySel::All
            } else {
                CopySel::Blocks(
                    (0..1 + input.below(3))
                        .map(|_| {
                            (
                                input.u16(),
                                input.chance(120).then(|| (input.u16(), input.u16())),
                            )
                        })
                        .collect(),
                )
            },
            wire: input.chance(128),
        },
        147..=160 => Op::Paste {
            peer: peer(input),
            at: at(input),
            other_namespace: input.chance(64),
        },
        161..=166 => Op::InsertImage {
            peer: peer(input),
            image: input.u8(),
            size: input.u8(),
            alt: input.u8(),
            at: at(input),
        },
        167..=174 => Op::ImportText {
            peer: peer(input),
            html: input.chance(128),
            text: input.u8(),
            at: at(input),
        },
        175..=180 => Op::Template {
            peer: peer(input),
            which: input.u8(),
        },
        181..=190 => Op::DefineStyle {
            peer: peer(input),
            name: input.u8(),
            variant: input.u8(),
            family: input.u8(),
            parent: input.u8(),
        },
        191..=200 => Op::Engine(match input.below(6) {
            0 => EngineChange::Composer(input.u8()),
            1 => EngineChange::Medium(input.u8()),
            2 => EngineChange::MaxPages(input.u8()),
            3 => EngineChange::RegisterFont(input.u8()),
            4 => EngineChange::RegisterImage(input.u8()),
            _ => EngineChange::RemoveImage(input.u8()),
        }),
        201..=225 => Op::Layout {
            peer: peer(input),
            mode: decode_layout(input),
        },
        226..=232 => Op::SaveReopen {
            peer: peer(input),
            shallow: input.chance(64),
            embed_fonts: input.chance(128),
            as_peer: input.u8(),
        },
        233..=240 => Op::Render {
            peer: peer(input),
            flags: input.u8(),
        },
        241..=249 => Op::Export {
            peer: peer(input),
            kind: input.u8(),
        },
        _ => Op::Navigate {
            peer: peer(input),
            seed: input.u16(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::seed_bytes;

    #[test]
    fn every_byte_string_decodes_within_bounds() {
        for len in [0, 1, 7, 64, 513, 4096] {
            for seed in 0..8 {
                let scenario = Scenario::decode(&seed_bytes(seed, len));
                assert!(scenario.ops.len() <= MAX_OPS);
            }
        }
        assert!(Scenario::decode(&[]).ops.is_empty());
        assert!(Scenario::decode(&[0xff; 100_000]).ops.len() <= MAX_OPS);
    }

    #[test]
    fn decoding_is_a_function_of_the_bytes() {
        let bytes = seed_bytes(11, 300);
        assert_eq!(Scenario::decode(&bytes), Scenario::decode(&bytes));
    }

    #[test]
    fn random_bytes_reach_every_op_kind() {
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..40 {
            for op in Scenario::decode(&seed_bytes(seed, 600)).ops {
                let name = format!("{op:?}");
                seen.insert(name.split([' ', '(', '{']).next().unwrap_or("").to_owned());
            }
        }
        for kind in [
            "Edit",
            "Undo",
            "Redo",
            "Sync",
            "Copy",
            "Paste",
            "InsertImage",
            "ImportText",
            "Template",
            "DefineStyle",
            "Engine",
            "Layout",
            "SaveReopen",
            "Render",
            "Export",
            "Navigate",
        ] {
            assert!(seen.contains(kind), "{kind} is unreachable: {seen:?}");
        }
    }
}
