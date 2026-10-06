//! The executor: runs a [`Scenario`] against two peers and checks every
//! oracle after every op.
//!
//! The executor is a pure function of the scenario. It reads no clock, no
//! randomness and no environment, so running it twice must give the same
//! transcript, and a failing scenario fails the same way every time.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use reprise_clipboard::{
    ExportOptions, Exporter, Html, ImportLimits, Native, NativeFragment, Pdf, PlainText, copy_all,
    copy_blocks, copy_selection, import_html, import_plain,
};
use reprise_doc::fragment::CopyBlock;
use reprise_doc::image::ImageData;
use reprise_doc::{
    BlockKind, Document, LengthExpr, NewBlock, NodeId, PageTemplate, PersistenceMode, RangeId,
    Relation, Revision, SchemaRegistry, Target,
};
use reprise_edit::{Applied, Caret};
use reprise_edit::{Command, Editor, Movement, Navigator, Selection, Transaction};
use reprise_font::FontStore;
use reprise_format::{DocumentId, Limits, MigrationRegistry, Package};
use reprise_geom::{Length, PageSpace, Point, Rect};
use reprise_layout::incremental::{JobError, LayoutSession, Viewport};
use reprise_layout::{Engine, LayoutSnapshot};

use crate::authored::{self, Authored, authored, concat_text, live_blocks};
use crate::engine::EngineSpec;
use crate::input::fnv;
use crate::scenario::*;
use crate::start::{self, PEER_A, PEER_B};
use crate::violation::{R, Violation};
use crate::{ensure, oracle, pool};

/// What a run did, for the determinism check and for coverage assertions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// A digest of every rendered or exported byte, in order.
    pub transcript: Vec<u64>,
    /// How often each kind of step happened (and how often it was refused).
    pub counts: BTreeMap<&'static str, usize>,
}

struct Peer {
    editor: Editor,
    /// `(before, after)` of every step since the last time foreign edits
    /// arrived, newest last. Undo must return to `before`, redo to `after`.
    undo_log: Vec<(Authored, Authored)>,
    redo_log: Vec<(Authored, Authored)>,
    initial: Authored,
    /// Whether another replica's edits were merged in.
    foreign: bool,
}

struct World {
    peers: [Peer; 2],
    spec: EngineSpec,
    ranges: Vec<RangeId>,
    dead: Vec<NodeId>,
    ever: BTreeSet<NodeId>,
    clip: Option<NativeFragment>,
    report: Report,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    MustSucceed,
    MustFail,
    MayFail,
}

enum Model {
    None,
    Insert {
        node: NodeId,
        at: usize,
        text: &'static str,
        before: String,
    },
    Delete {
        node: NodeId,
        range: std::ops::Range<usize>,
        before: String,
    },
    Split {
        node: NodeId,
        at: usize,
        before: String,
    },
    Join {
        first: NodeId,
        before_first: String,
        before_second: String,
    },
}

thread_local! {
    static CURRENT_OP: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Runs bytes as a scenario. A panic anywhere is reported as a violation of
/// the `no-panic` oracle, naming the op that was running.
pub fn run_bytes(data: &[u8]) -> Result<Report, Violation> {
    run_scenario(&Scenario::decode(data))
}

pub fn run_scenario(scenario: &Scenario) -> Result<Report, Violation> {
    CURRENT_OP.with(|c| c.set(None));
    match catch_unwind(AssertUnwindSafe(|| execute(scenario))) {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "non-string panic".into());
            Err(Violation {
                oracle: "no-panic",
                op: CURRENT_OP.with(|c| c.get()),
                detail: message,
            })
        }
    }
}

/// Runs the scenario twice and demands identical transcripts.
pub fn run_twice(scenario: &Scenario) -> Result<Report, Violation> {
    let first = run_scenario(scenario)?;
    let second = run_scenario(scenario)?;
    ensure!(
        first.transcript == second.transcript,
        "scenario-repeatable",
        "two runs of one scenario rendered different bytes ({} vs {} digests)",
        first.transcript.len(),
        second.transcript.len()
    );
    Ok(first)
}

/// For libFuzzer and tests: panics with a reproduction on any violation.
pub fn check_bytes(data: &[u8]) -> Report {
    match run_bytes(data) {
        Ok(report) => report,
        Err(v) => panic!(
            "{v}\nscenario bytes ({}): {}\nscenario: {:#?}",
            data.len(),
            hex(data),
            Scenario::decode(data)
        ),
    }
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

fn execute(scenario: &Scenario) -> R<Report> {
    let started = start::start(&scenario.start).map_err(|e| Violation::new("start", e))?;
    let [a, b] = started.docs;
    let schemas = SchemaRegistry::builtin();
    let make = |doc: Document| {
        let editor = Editor::new(doc, SchemaRegistry::builtin());
        let initial = authored(editor.document());
        Peer {
            editor,
            undo_log: Vec::new(),
            redo_log: Vec::new(),
            initial,
            foreign: false,
        }
    };
    let _ = schemas;
    let mut world = World {
        peers: [make(a), make(b)],
        spec: EngineSpec::default(),
        ranges: started.ranges,
        dead: Vec::new(),
        ever: BTreeSet::new(),
        clip: None,
        report: Report::default(),
    };
    let mut index = 0;
    while index < scenario.ops.len() {
        let engine = world.spec.build();
        let mut cx = Cx {
            engine: &engine,
            sessions: [LayoutSession::new(&engine), LayoutSession::new(&engine)],
            w: &mut world,
        };
        let mut engine_changed = None;
        while index < scenario.ops.len() {
            if let Op::Engine(change) = &scenario.ops[index] {
                engine_changed = Some(*change);
                index += 1;
                break;
            }
            CURRENT_OP.with(|c| c.set(Some(index)));
            cx.step(&scenario.ops[index]).map_err(|mut v| {
                v.op = Some(index);
                v
            })?;
            index += 1;
        }
        drop(cx);
        if let Some(change) = engine_changed {
            *world.report.counts.entry("engine").or_default() += 1;
            world.spec.apply(change);
        }
    }
    CURRENT_OP.with(|c| c.set(None));
    let engine = world.spec.build();
    let mut cx = Cx {
        engine: &engine,
        sessions: [LayoutSession::new(&engine), LayoutSession::new(&engine)],
        w: &mut world,
    };
    cx.epilogue()?;
    drop(cx);
    Ok(world.report)
}

struct Cx<'e, 'w> {
    engine: &'e Engine,
    sessions: [LayoutSession<'e>; 2],
    w: &'w mut World,
}

// ---------------------------------------------------------------- selectors

fn pick_block(doc: &Document, dead: &[NodeId], selector: u16) -> Option<NodeId> {
    if selector & 0x8000 != 0 && !dead.is_empty() {
        return dead
            .get(usize::from(selector & 0x7fff) % dead.len())
            .copied();
    }
    let live = live_blocks(doc);
    live.get(usize::from(selector) % live.len().max(1)).copied()
}

/// A byte offset in `text`, and whether it is a valid one (on a character
/// boundary and within the text). Selectors with the top bit set deliberately
/// pick invalid offsets.
fn pick_offset(text: &str, selector: u16) -> (usize, bool) {
    if selector & 0x8000 != 0 {
        let raw = usize::from(selector & 0x7fff) % (text.len() + 3);
        return (raw, raw <= text.len() && text.is_char_boundary(raw));
    }
    let bounds: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect();
    (bounds[usize::from(selector) % bounds.len()], true)
}

fn text_of(doc: &Document, node: NodeId) -> Option<String> {
    doc.block(node).ok().map(|b| b.text.to_string())
}

fn char_bag(text: &str) -> Vec<char> {
    let mut chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    chars.sort_unstable();
    chars
}

fn viewport(byte: u8) -> Viewport {
    if byte & 0x80 != 0 {
        let at = |shift: u8| Length::from_pt(i32::from((byte >> shift) & 7) * 60 - 60);
        Viewport::Rect {
            page: usize::from(byte & 3),
            rect: Rect::<PageSpace>::new(
                Point::new(at(2), at(4)),
                Length::from_pt(100),
                Length::from_pt(80),
            ),
        }
    } else {
        let start = usize::from(byte & 3);
        let end = usize::from((byte >> 2) & 7);
        Viewport::Pages(start..end)
    }
}

fn first_difference(a: &str, b: &str) -> String {
    let at = a
        .bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let window = |s: &str| {
        let from = s.floor_char_boundary(at.saturating_sub(120));
        let to = s.floor_char_boundary((at + 160).min(s.len()));
        s[from..to].to_owned()
    };
    format!(
        "first difference at byte {at}:\n  left:  {}\n  right: {}",
        window(a),
        window(b)
    )
}

fn snapshots_equal(what: &'static str, left: &LayoutSnapshot, right: &LayoutSnapshot) -> R {
    if left == right {
        return Ok(());
    }
    Err(Violation::new(
        what,
        first_difference(&left.to_json(), &right.to_json()),
    ))
}

impl Cx<'_, '_> {
    fn count(&mut self, key: &'static str) {
        *self.w.report.counts.entry(key).or_default() += 1;
    }

    fn refresh_ever(&mut self) {
        for peer in &self.w.peers {
            self.w.ever.extend(live_blocks(peer.editor.document()));
        }
        self.w.ever.extend(self.w.dead.iter().copied());
    }

    fn step(&mut self, op: &Op) -> R {
        self.refresh_ever();
        self.count("ops");
        match op {
            Op::Edit { peer, cmds } => self.edit(usize::from(*peer), cmds),
            Op::Undo { peer } => self.undo_redo(usize::from(*peer), true),
            Op::Redo { peer } => self.undo_redo(usize::from(*peer), false),
            Op::Sync(mode) => self.sync(*mode),
            Op::Copy { peer, sel, wire } => self.copy(usize::from(*peer), sel, *wire),
            Op::Paste {
                peer,
                at,
                other_namespace,
            } => {
                let Some(clip) = self.w.clip.clone() else {
                    self.count("paste.skipped");
                    return Ok(());
                };
                self.paste_fragment(usize::from(*peer), &clip, *at, *other_namespace)
            }
            Op::InsertImage {
                peer,
                image,
                size,
                alt,
                at,
            } => self.insert_image(usize::from(*peer), *image, *size, *alt, *at),
            Op::ImportText {
                peer,
                html,
                text,
                at,
            } => self.import_text(usize::from(*peer), *html, *text, *at),
            Op::Template { peer, which } => self.template(usize::from(*peer), *which),
            Op::DefineStyle {
                peer,
                name,
                variant,
                family,
                parent,
            } => self.define_style(usize::from(*peer), *name, *variant, *family, *parent),
            Op::Engine(_) => Ok(()),
            Op::Layout { peer, mode } => self.layout(usize::from(*peer), mode),
            Op::SaveReopen {
                peer,
                shallow,
                embed_fonts,
                as_peer,
            } => self.save_reopen(usize::from(*peer), *shallow, *embed_fonts, *as_peer),
            Op::Render { peer, flags } => self.render(usize::from(*peer), *flags),
            Op::Export { peer, kind } => self.export(usize::from(*peer), *kind),
            Op::Navigate { peer, seed } => self.navigate(usize::from(*peer), *seed),
        }
    }

    // ------------------------------------------------------------- editing

    fn build(&mut self, p: usize, cmd: &Cmd) -> Option<(Command, Expect, Model)> {
        let doc = self.w.peers[p].editor.document();
        let dead = &self.w.dead;
        let pool_ranges = &self.w.ranges;
        let plain = |node: NodeId| doc.table_role(node).ok().flatten().is_none();
        Some(match cmd {
            Cmd::InsertText { block, at, text } => {
                let node = pick_block(doc, dead, *block)?;
                let before = text_of(doc, node);
                let (offset, valid) = pick_offset(before.as_deref().unwrap_or(""), *at);
                let ins = pool::text(usize::from(*text));
                let expect = match (valid, before.is_some() && plain(node)) {
                    (false, true) => Expect::MustFail,
                    (true, true) if !dead.contains(&node) => Expect::MustSucceed,
                    _ => Expect::MayFail,
                };
                let model = match before {
                    Some(before) if valid && !dead.contains(&node) => Model::Insert {
                        node,
                        at: offset,
                        text: ins,
                        before,
                    },
                    _ => Model::None,
                };
                (
                    Command::InsertText {
                        node,
                        at: offset,
                        text: ins.into(),
                    },
                    expect,
                    model,
                )
            }
            Cmd::DeleteText { block, from, to } => {
                let node = pick_block(doc, dead, *block)?;
                let before = text_of(doc, node);
                let t = before.clone().unwrap_or_default();
                let (a, va) = pick_offset(&t, *from);
                let (b, vb) = pick_offset(&t, *to);
                let range = a..b;
                let ok = va && vb && a <= b;
                let expect = if !(va && vb) || a > b {
                    if before.is_some() && !dead.contains(&node) {
                        Expect::MustFail
                    } else {
                        Expect::MayFail
                    }
                } else if before.is_some() && plain(node) && !dead.contains(&node) {
                    Expect::MustSucceed
                } else {
                    Expect::MayFail
                };
                let model = match before {
                    Some(before) if ok && !dead.contains(&node) => Model::Delete {
                        node,
                        range: range.clone(),
                        before,
                    },
                    _ => Model::None,
                };
                (Command::DeleteText { node, range }, expect, model)
            }
            Cmd::Split { block, at } => {
                let node = pick_block(doc, dead, *block)?;
                let before = text_of(doc, node);
                let (offset, valid) = pick_offset(before.as_deref().unwrap_or(""), *at);
                let model = match before {
                    Some(before) if valid && plain(node) && !dead.contains(&node) => Model::Split {
                        node,
                        at: offset,
                        before,
                    },
                    _ => Model::None,
                };
                (
                    Command::SplitBlock { node, at: offset },
                    Expect::MayFail,
                    model,
                )
            }
            Cmd::Join { first, second } => {
                let a = pick_block(doc, dead, *first)?;
                let b = pick_block(doc, dead, *second)?;
                let model = match (text_of(doc, a), text_of(doc, b)) {
                    (Some(x), Some(y))
                        if plain(a) && plain(b) && !dead.contains(&a) && !dead.contains(&b) =>
                    {
                        Model::Join {
                            first: a,
                            before_first: x,
                            before_second: y,
                        }
                    }
                    _ => Model::None,
                };
                (
                    Command::JoinBlocks {
                        first: a,
                        second: b,
                    },
                    Expect::MayFail,
                    model,
                )
            }
            Cmd::InsertBlock {
                parent,
                index,
                kind,
                style,
                text,
            } => {
                let parent = parent.and_then(|p| pick_block(doc, dead, p));
                let siblings = doc.children(parent).len();
                let kind = match kind % 10 {
                    0..=5 => BlockKind::Paragraph,
                    6..=8 => BlockKind::Annotation,
                    _ => BlockKind::Image,
                };
                let block = NewBlock::new(
                    kind,
                    pool::style_name(usize::from(*style)),
                    pool::text(usize::from(*text)),
                );
                (
                    Command::InsertBlock {
                        parent,
                        index: usize::from(*index) % (siblings + 1),
                        block,
                    },
                    Expect::MayFail,
                    Model::None,
                )
            }
            Cmd::DeleteBlock { block } => {
                let node = pick_block(doc, dead, *block)?;
                (Command::DeleteBlock { node }, Expect::MayFail, Model::None)
            }
            Cmd::MoveBlock {
                block,
                parent,
                index,
            } => {
                let node = pick_block(doc, dead, *block)?;
                let parent = parent.and_then(|p| pick_block(doc, dead, p));
                let siblings = doc.children(parent).len();
                (
                    Command::MoveBlock {
                        node,
                        parent,
                        index: usize::from(*index) % (siblings + 1),
                    },
                    Expect::MayFail,
                    Model::None,
                )
            }
            Cmd::SetOverride {
                block,
                variant,
                family,
                parent,
            } => {
                let node = pick_block(doc, dead, *block)?;
                let style = pool::style(
                    usize::from(*variant),
                    usize::from(*family),
                    usize::from(*parent),
                );
                (
                    Command::SetStyleOverride { node, style },
                    Expect::MayFail,
                    Model::None,
                )
            }
            Cmd::AddRelation {
                kind,
                owner,
                target,
                aux,
            } => {
                let owner_node = pick_block(doc, dead, *owner)?;
                let target_node = pick_block(doc, dead, *target)?;
                let range = (!pool_ranges.is_empty())
                    .then(|| pool_ranges[usize::from(*target) % pool_ranges.len()]);
                let layout_target = |r: Option<RangeId>| match r {
                    Some(range) => {
                        Target::Layout(reprise_doc::LayoutQuery::LineContaining { range })
                    }
                    None => {
                        Target::Layout(reprise_doc::LayoutQuery::FirstLine { node: target_node })
                    }
                };
                let pt = |n: i32| LengthExpr::Pt(Length::from_pt(n));
                let relation = match kind % 8 {
                    0 => follow_to(owner_node, layout_target(range)),
                    1 => Relation::new(reprise_doc::relation::builtin::NOTE)
                        .owned_by(owner_node)
                        .target(
                            "anchor",
                            range.map_or(Target::Node(target_node), Target::Range),
                        ),
                    2 => Relation::new(reprise_doc::relation::builtin::FLOAT)
                        .owned_by(owner_node)
                        .target("anchor", range.map_or(layout_target(None), Target::Range))
                        .param(
                            "width",
                            reprise_doc::Param::Length(pt(20 + i32::from(*aux % 80))),
                        )
                        .param(
                            "side",
                            reprise_doc::Param::Text(
                                ["left", "right", "top", "bottom", "sideways"]
                                    [usize::from(*aux) % 5]
                                    .into(),
                            ),
                        ),
                    3 => Relation::new(reprise_doc::relation::builtin::REFERENCE)
                        .owned_by(owner_node)
                        .target("to", Target::Node(target_node)),
                    4 => reprise_doc::reading::before(owner_node, target_node),
                    5 => Relation::new(reprise_doc::relation::SchemaId::new("fuzz.unknown"))
                        .owned_by(owner_node)
                        .target("x", Target::Node(target_node)),
                    6 => follow_to(owner_node, layout_target(range)).param(
                        "offset",
                        reprise_doc::Param::Length(LengthExpr::Pt(Length::MAX)),
                    ),
                    _ => Relation::new(reprise_doc::relation::builtin::REFERENCE)
                        .owned_by(owner_node)
                        .target(
                            "to",
                            Target::Structural(reprise_doc::StructuralQuery::NextSibling {
                                from: target_node,
                                kind: None,
                            }),
                        ),
                };
                (
                    Command::AddRelation { relation },
                    Expect::MayFail,
                    Model::None,
                )
            }
            Cmd::RemoveRelation { index } => {
                let relations = doc.relations();
                let (id, _) = relations.get(usize::from(*index) % relations.len().max(1))?;
                (
                    Command::RemoveRelation { id: *id },
                    Expect::MayFail,
                    Model::None,
                )
            }
        })
    }

    fn edit(&mut self, p: usize, cmds: &[Cmd]) -> R {
        let mut tx = Transaction::new();
        let mut single = None;
        for cmd in cmds {
            if let Some((command, expect, model)) = self.build(p, cmd) {
                tx.push(command);
                single = Some((expect, model));
            }
        }
        if tx.is_empty() {
            self.count("edit.skipped");
            return Ok(());
        }
        let (expect, model) = match (tx.len(), single) {
            (1, Some(single)) => single,
            _ => (Expect::MayFail, Model::None),
        };
        self.apply(p, &tx, expect, model)
    }

    fn apply(&mut self, p: usize, tx: &Transaction, expect: Expect, model: Model) -> R {
        let peer = &mut self.w.peers[p];
        let pre = authored(peer.editor.document());
        let revision = peer.editor.document().revision();
        let (undo, redo) = (peer.editor.undo_count(), peer.editor.redo_count());
        let result = peer.editor.apply(tx);
        match result {
            Err(error) => {
                self.count("edit.refused");
                ensure!(
                    expect != Expect::MustSucceed,
                    "valid-edit-accepted",
                    "a valid command was refused: {error} ({:?}); commands: {:?}",
                    error.reason,
                    tx.commands()
                );
                oracle::documented("edit", [error.note().code.as_str()])?;
                let peer = &self.w.peers[p];
                Self::unchanged(
                    peer,
                    &pre,
                    &revision,
                    undo,
                    redo,
                    "refused-edit-changes-nothing",
                )
            }
            Ok(applied) => {
                self.count("edit.applied");
                ensure!(
                    expect != Expect::MustFail,
                    "invalid-edit-refused",
                    "an invalid command was accepted: {:?}",
                    tx.commands()
                );
                self.check_model(p, &applied, &model)?;
                self.new_ids(applied.blocks.iter().copied())?;
                if let Some(Command::DeleteBlock { node }) = tx.commands().first()
                    && tx.len() == 1
                {
                    self.w.dead.push(*node);
                }
                self.record_step(p, undo, pre);
                Ok(())
            }
        }
    }

    fn unchanged(
        peer: &Peer,
        pre: &Authored,
        revision: &Revision,
        undo: usize,
        redo: usize,
        oracle: &'static str,
    ) -> R {
        let doc = peer.editor.document();
        let now = authored(doc);
        ensure!(
            &now == pre,
            oracle,
            "authored state changed:\n{}",
            authored::diff(pre, &now)
        );
        ensure!(
            &doc.revision() == revision,
            oracle,
            "revision changed from {revision:?} to {:?}",
            doc.revision()
        );
        ensure!(
            peer.editor.undo_count() == undo && peer.editor.redo_count() == redo,
            oracle,
            "undo/redo counts changed: ({undo}, {redo}) to ({}, {})",
            peer.editor.undo_count(),
            peer.editor.redo_count()
        );
        Ok(())
    }

    fn record_step(&mut self, p: usize, undo_before: usize, pre: Authored) {
        let peer = &mut self.w.peers[p];
        if peer.editor.undo_count() > undo_before {
            let post = authored(peer.editor.document());
            peer.undo_log.push((pre, post));
            peer.redo_log.clear();
        }
    }

    fn new_ids(&mut self, ids: impl IntoIterator<Item = NodeId>) -> R {
        for id in ids {
            ensure!(
                !self.w.ever.contains(&id),
                "ids-never-reused",
                "a new block got the already-used ID {id}"
            );
            self.w.ever.insert(id);
        }
        Ok(())
    }

    fn check_model(&self, p: usize, applied: &Applied, model: &Model) -> R {
        let doc = self.w.peers[p].editor.document();
        match model {
            Model::None => {}
            Model::Insert {
                node,
                at,
                text,
                before,
            } => {
                let mut expected = before.clone();
                expected.insert_str(*at, text);
                ensure!(
                    text_of(doc, *node).as_deref() == Some(&expected),
                    "insert-text-model",
                    "{node}: expected {expected:?} after inserting {text:?} at {at} of {before:?}, got {:?}",
                    text_of(doc, *node)
                );
            }
            Model::Delete {
                node,
                range,
                before,
            } => {
                let mut expected = before.clone();
                expected.replace_range(range.clone(), "");
                ensure!(
                    text_of(doc, *node).as_deref() == Some(&expected),
                    "delete-text-model",
                    "{node}: expected {expected:?} after deleting {range:?} of {before:?}, got {:?}",
                    text_of(doc, *node)
                );
            }
            Model::Split { node, at, before } => {
                let new = applied.blocks.first().copied();
                ensure!(new.is_some(), "split-model", "split returned no new block");
                let (head, tail) = before.split_at(*at);
                ensure!(
                    text_of(doc, *node).as_deref() == Some(head)
                        && new.and_then(|n| text_of(doc, n)).as_deref() == Some(tail),
                    "split-model",
                    "{node}: split of {before:?} at {at} gave {:?} and {:?}",
                    text_of(doc, *node),
                    new.and_then(|n| text_of(doc, n))
                );
            }
            Model::Join {
                first,
                before_first,
                before_second,
            } => {
                let joined = if first_is_second(&applied.effects) {
                    before_first.clone()
                } else {
                    format!("{before_first}{before_second}")
                };
                if !first_is_second(&applied.effects) {
                    ensure!(
                        text_of(doc, *first).as_deref() == Some(&joined),
                        "join-model",
                        "{first}: expected {joined:?}, got {:?}",
                        text_of(doc, *first)
                    );
                }
            }
        }
        Ok(())
    }

    fn undo_redo(&mut self, p: usize, undo: bool) -> R {
        let peer = &mut self.w.peers[p];
        let can = if undo {
            peer.editor.can_undo()
        } else {
            peer.editor.can_redo()
        };
        let result = if undo {
            peer.editor.undo()
        } else {
            peer.editor.redo()
        };
        let moved = match result {
            Ok(moved) => moved,
            Err(error) => {
                return Err(Violation::new(
                    "undo-redo-succeeds",
                    format!("{} failed: {error}", if undo { "undo" } else { "redo" }),
                ));
            }
        };
        ensure!(
            moved == can,
            "undo-redo-agrees-with-can",
            "can_{} was {can} but the call returned {moved}",
            if undo { "undo" } else { "redo" }
        );
        if !moved {
            self.count("undo-redo.empty");
            return Ok(());
        }
        let now = authored(peer.editor.document());
        if undo {
            if let Some((before, after)) = peer.undo_log.pop() {
                ensure!(
                    now == before,
                    "undo-restores-state",
                    "undo did not return to the state before the step:\n{}",
                    authored::diff(&before, &now)
                );
                peer.redo_log.push((before, after));
            }
        } else if let Some((before, after)) = peer.redo_log.pop() {
            ensure!(
                now == after,
                "redo-restores-state",
                "redo did not return to the state after the step:\n{}",
                authored::diff(&after, &now)
            );
            peer.undo_log.push((before, after));
        }
        self.count(if undo { "undo" } else { "redo" });
        Ok(())
    }

    // ----------------------------------------------------------------- sync

    fn sync(&mut self, mode: SyncMode) -> R {
        self.count("sync");
        let [a, b] = &mut self.w.peers;
        let wrap = |e: reprise_edit::EditError| Violation::new("sync-succeeds", e.to_string());
        let fork = |d: &Document, peer| {
            d.fork(peer)
                .map_err(|e| Violation::new("sync-succeeds", e.to_string()))
        };
        let converges = match mode {
            SyncMode::AThenB => {
                a.editor.merge(b.editor.document()).map_err(wrap)?;
                b.editor.merge(a.editor.document()).map_err(wrap)?;
                true
            }
            SyncMode::BThenA => {
                b.editor.merge(a.editor.document()).map_err(wrap)?;
                a.editor.merge(b.editor.document()).map_err(wrap)?;
                true
            }
            SyncMode::Crossed => {
                let (fa, fb) = (
                    fork(a.editor.document(), PEER_A)?,
                    fork(b.editor.document(), PEER_B)?,
                );
                a.editor.merge(&fb).map_err(wrap)?;
                b.editor.merge(&fa).map_err(wrap)?;
                true
            }
            SyncMode::ViaBytes => {
                let export = |d: &Document| {
                    d.try_export(PersistenceMode::History)
                        .map_err(|e| Violation::new("sync-succeeds", e.to_string()))
                };
                let (ba, bb) = (export(a.editor.document())?, export(b.editor.document())?);
                let import = |bytes: &[u8]| {
                    Document::import(bytes, 3)
                        .map_err(|e| Violation::new("sync-succeeds", e.to_string()))
                };
                let (ia, ib) = (import(&ba)?, import(&bb)?);
                a.editor.merge(&ib).map_err(wrap)?;
                b.editor.merge(&ia).map_err(wrap)?;
                true
            }
            SyncMode::OneWay { from_a } => {
                if from_a {
                    b.editor.merge(a.editor.document()).map_err(wrap)?;
                } else {
                    a.editor.merge(b.editor.document()).map_err(wrap)?;
                }
                false
            }
        };
        for peer in &mut self.w.peers {
            peer.foreign = true;
            peer.undo_log.clear();
            peer.redo_log.clear();
        }
        if converges {
            self.converged("peers-converge")?;
        }
        Ok(())
    }

    /// Both replicas hold the same authored state, ranges, revision and layout.
    fn converged(&mut self, oracle: &'static str) -> R {
        let [a, b] = &self.w.peers;
        let (da, db) = (a.editor.document(), b.editor.document());
        da.commit();
        db.commit();
        let (xa, xb) = (authored(da), authored(db));
        ensure!(
            xa == xb,
            oracle,
            "replicas differ after a full sync:\n{}",
            authored::diff(&xa, &xb)
        );
        let (ra, rb) = (
            authored::ranges(da, &self.w.ranges),
            authored::ranges(db, &self.w.ranges),
        );
        ensure!(
            ra == rb,
            oracle,
            "range resolution differs: {ra:?} vs {rb:?}"
        );
        ensure!(
            da.revision() == db.revision(),
            oracle,
            "revisions differ: {:?} vs {:?}",
            da.revision(),
            db.revision()
        );
        let (la, lb) = (self.engine.layout(da), self.engine.layout(db));
        snapshots_equal("peers-converge-layout", &la, &lb)?;
        oracle::structure(&la)?;
        oracle::reading_order(da, &la)?;
        oracle::diagnostics(&la)
    }

    // ------------------------------------------------------ copy and paste

    fn copy(&mut self, p: usize, sel: &CopySel, wire: bool) -> R {
        let doc = self.w.peers[p].editor.document();
        let schemas = self.w.peers[p].editor.schemas();
        let layout = wire.then(|| self.engine.layout(doc));
        let fonts = Some(&self.engine.fonts);
        let (result, expected) = match sel {
            CopySel::All => (
                copy_all(doc, "ns", schemas, layout.as_ref(), fonts),
                Some(concat_text(doc)),
            ),
            CopySel::Blocks(blocks) => {
                let mut chosen = Vec::new();
                let mut single = None;
                for (block, range) in blocks {
                    let Some(node) = pick_block(doc, &self.w.dead, *block) else {
                        continue;
                    };
                    let text = text_of(doc, node).unwrap_or_default();
                    let bytes = range.map(|(a, b)| {
                        (
                            pick_offset(&text, a).0..pick_offset(&text, b).0,
                            text.clone(),
                        )
                    });
                    single = bytes
                        .as_ref()
                        .map(|(r, t)| t.get(r.clone()).map(str::to_owned));
                    chosen.push(CopyBlock {
                        node,
                        bytes: bytes.map(|b| b.0),
                    });
                }
                let expected = match (chosen.len(), single) {
                    (1, Some(Some(slice))) => Some(slice),
                    _ => None,
                };
                (
                    copy_blocks(doc, "ns", &chosen, schemas, layout.as_ref(), fonts),
                    expected,
                )
            }
        };
        match result {
            Err(error) => {
                self.count("copy.refused");
                oracle::documented("clipboard", [error.note().code.as_str()])
            }
            Ok(native) => {
                self.count("copy.ok");
                oracle::documented("clipboard", native.notes.iter().map(|n| n.code.as_str()))?;
                native.validate().map_err(|e| {
                    Violation::new(
                        "native-fragment-valid",
                        format!("a fresh copy is invalid: {e}"),
                    )
                })?;
                let text: String = native
                    .fragment
                    .blocks
                    .iter()
                    .map(|b| b.text.as_str())
                    .collect();
                if let Some(expected) = expected {
                    ensure!(
                        text == expected,
                        "copy-preserves-text",
                        "copied {text:?}, the source holds {expected:?}"
                    );
                }
                let native = if wire {
                    let bytes = native.encode().map_err(|e| {
                        Violation::new("native-encodes", format!("a fresh copy won't encode: {e}"))
                    })?;
                    let decoded = NativeFragment::decode(&bytes).map_err(|e| {
                        Violation::new(
                            "native-roundtrips",
                            format!("encoded bytes don't decode: {e}"),
                        )
                    })?;
                    let again = decoded.encode().map_err(|e| {
                        Violation::new(
                            "native-roundtrips",
                            format!("decoded copy won't encode: {e}"),
                        )
                    })?;
                    ensure!(
                        again == bytes,
                        "native-roundtrips",
                        "encode, decode, encode changed the bytes"
                    );
                    self.w.report.transcript.push(fnv(&bytes));
                    decoded
                } else {
                    native
                };
                self.w.clip = Some(native);
                Ok(())
            }
        }
    }

    fn paste_fragment(
        &mut self,
        p: usize,
        native: &NativeFragment,
        at: Option<(u16, u16)>,
        other: bool,
    ) -> R {
        let dead = self.w.dead.clone();
        let peer = &mut self.w.peers[p];
        let doc = peer.editor.document();
        let target = at.and_then(|(b, o)| {
            let node = pick_block(doc, &dead, b)?;
            let text = text_of(doc, node)?;
            let (offset, valid) = pick_offset(&text, o);
            valid.then_some((node, offset))
        });
        let before_text = concat_text(doc);
        let expected = match target {
            None => Some(format!(
                "{before_text}{}",
                native
                    .fragment
                    .blocks
                    .iter()
                    .map(|b| b.text.as_str())
                    .collect::<String>()
            )),
            Some((node, offset)) if doc.children(Some(node)).is_empty() => {
                let mut position = offset;
                for live in live_blocks(doc) {
                    if live == node {
                        break;
                    }
                    position += text_of(doc, live).map_or(0, |t| t.len());
                }
                let mut expected = before_text.clone();
                let fragment_text: String = native
                    .fragment
                    .blocks
                    .iter()
                    .map(|b| b.text.as_str())
                    .collect();
                expected.insert_str(position, &fragment_text);
                Some(expected)
            }
            Some(_) => None,
        };
        let pre = authored(doc);
        let revision = doc.revision();
        let (undo, redo) = (peer.editor.undo_count(), peer.editor.redo_count());
        let result = native.paste(
            &mut peer.editor,
            target,
            if other { "elsewhere" } else { "ns" },
        );
        match result {
            Err(error) => {
                self.count("paste.refused");
                oracle::documented("paste", [error.note().code.as_str()])?;
                let peer = &self.w.peers[p];
                Self::unchanged(
                    peer,
                    &pre,
                    &revision,
                    undo,
                    redo,
                    "refused-paste-changes-nothing",
                )
            }
            Ok(pasted) => {
                self.count("paste.ok");
                oracle::documented("paste", pasted.notes.iter().map(|n| n.code.as_str()))?;
                let after = concat_text(self.w.peers[p].editor.document());
                if let Some(expected) = expected {
                    ensure!(
                        after == expected,
                        "paste-preserves-text",
                        "target {target:?}: expected {expected:?} after paste, got {after:?}"
                    );
                }
                let created: Vec<NodeId> = pasted
                    .ids
                    .nodes
                    .values()
                    .copied()
                    .filter(|id| !self.w.ever.contains(id))
                    .collect();
                self.w.ever.extend(created);
                self.record_step(p, undo, pre);
                Ok(())
            }
        }
    }

    fn insert_image(
        &mut self,
        p: usize,
        image: u8,
        size: u8,
        alt: u8,
        at: Option<(u16, u16)>,
    ) -> R {
        let scratch = Document::new(77).map_err(|e| Violation::new("scratch", e.to_string()))?;
        let pt = |n: i32| Some(LengthExpr::Pt(Length::from_pt(n)));
        let (width, height) = match size % 6 {
            0 => (None, None),
            1 => (pt(30), None),
            2 => (None, pt(20)),
            3 => (pt(0), pt(0)),
            4 => (
                Some(LengthExpr::Pt(Length::MAX)),
                Some(LengthExpr::Pt(Length::MIN)),
            ),
            _ => (Some(LengthExpr::Em(-500)), pt(10)),
        };
        let mut data = ImageData::new(pool::image_hash(usize::from(image)));
        data.width = width;
        data.height = height;
        scratch
            .append_image("", &data, pool::text(usize::from(alt)))
            .map_err(|e| Violation::new("scratch", e.to_string()))?;
        scratch.commit();
        let native = copy_all(&scratch, "img", &SchemaRegistry::builtin(), None, None)
            .map_err(|e| Violation::new("scratch", e.to_string()))?;
        self.count("image");
        self.paste_fragment(p, &native, at, true)
    }

    fn import_text(&mut self, p: usize, html: bool, text: u8, at: Option<(u16, u16)>) -> R {
        let (source, imported) = if html {
            let source = pool::html(usize::from(text));
            let imported = import_html(&source, ImportLimits::default());
            (source, imported)
        } else {
            let source = pool::text(usize::from(text)).to_owned();
            let imported = import_plain(&source);
            (source, imported)
        };
        match imported {
            Err(error) => {
                self.count("import.refused");
                oracle::documented("import", [error.note().code.as_str()])
            }
            Ok(import) => {
                self.count("import.ok");
                oracle::documented("import", import.notes.iter().map(|n| n.code.as_str()))?;
                if !html {
                    let text: String = import
                        .fragment
                        .fragment
                        .blocks
                        .iter()
                        .map(|b| b.text.as_str())
                        .collect();
                    ensure!(
                        char_bag(&text) == char_bag(&source),
                        "plain-import-keeps-text",
                        "imported {text:?} from {source:?}"
                    );
                }
                self.paste_fragment(p, &import.fragment, at, true)
            }
        }
    }

    // ------------------------------------------------- styles and templates

    /// An edit made straight on the document and committed as one step, the
    /// way hosts change styles and page setup.
    fn direct(&mut self, p: usize, change: impl FnOnce(&Document) -> bool) -> R {
        let peer = &mut self.w.peers[p];
        let doc = peer.editor.document();
        let pre = authored(doc);
        let undo = peer.editor.undo_count();
        let clean = change(doc);
        doc.commit_step();
        if clean {
            self.record_step(p, undo, pre);
        } else {
            let peer = &mut self.w.peers[p];
            peer.undo_log.clear();
            peer.redo_log.clear();
        }
        Ok(())
    }

    fn template(&mut self, p: usize, which: u8) -> R {
        self.count("template");
        self.direct(p, |doc| match which % 7 {
            0 => doc.set_page_template(&PageTemplate::builtin()).is_ok(),
            1 => doc
                .set_page_template(&reprise_fixtures::templates::two_columns())
                .is_ok(),
            2 => doc
                .set_page_template(&reprise_fixtures::templates::responsive_columns())
                .is_ok(),
            3 => doc
                .define_page_template(&reprise_fixtures::templates::two_columns())
                .is_ok(),
            4 => {
                doc.store_raw_page_template("junk", "{not a template")
                    .is_ok()
                    && doc.use_page_template("junk").is_ok()
            }
            5 => doc.remove_page_template("two-columns").is_ok(),
            _ => doc.use_page_template("no such template").is_ok(),
        })
    }

    fn define_style(&mut self, p: usize, name: u8, variant: u8, family: u8, parent: u8) -> R {
        self.count("define-style");
        let name = pool::style_name(usize::from(name));
        let style = pool::style(
            usize::from(variant),
            usize::from(family),
            usize::from(parent),
        );
        self.direct(p, |doc| doc.define_style(name, &style).is_ok())
    }

    // --------------------------------------------------------------- layout

    fn layout(&mut self, p: usize, mode: &LayoutMode) -> R {
        self.count("layout");
        let doc = self.w.peers[p].editor.document();
        doc.commit();
        let reference = self.engine.layout(doc);
        let again = self.engine.layout(doc);
        ensure!(
            reference.to_json() == again.to_json(),
            "layout-repeatable",
            "two layouts of one document differ"
        );
        oracle::diagnostics(&reference)?;
        oracle::structure(&reference)?;
        oracle::reading_order(doc, &reference)?;
        match mode {
            LayoutMode::Reference => Ok(()),
            LayoutMode::Session => {
                let session = &mut self.sessions[p];
                let incremental = session
                    .layout(doc)
                    .map_err(|e| Violation::new("session-layout", e.to_string()))?;
                snapshots_equal("incremental-equals-reference", &incremental, &reference)
            }
            LayoutMode::Job {
                budgets,
                viewport: v,
                partial,
                cancel_after,
            } => run_job(
                &mut self.sessions[p],
                doc,
                &reference,
                budgets,
                *v,
                *partial,
                *cancel_after,
            ),
        }
    }

    fn render(&mut self, p: usize, flags: u8) -> R {
        self.count("render");
        let doc = self.w.peers[p].editor.document();
        doc.commit();
        let snapshot = self.engine.layout(doc);
        let digests = oracle::render(self.engine, doc, &snapshot, flags)?;
        self.w.report.transcript.extend(digests);
        Ok(())
    }

    // --------------------------------------------------------------- export

    fn export(&mut self, p: usize, kind: u8) -> R {
        self.count("export");
        let doc = self.w.peers[p].editor.document();
        doc.commit();
        let schemas = self.w.peers[p].editor.schemas();
        let snapshot = self.engine.layout(doc);
        let layout = (kind & 0x80 == 0).then_some(&snapshot);
        let options = ExportOptions {
            source_namespace: "ns",
            schemas,
            fonts: Some(&self.engine.fonts),
        };
        let exporter: &dyn Exporter = match kind % 4 {
            0 => &PlainText,
            1 => &Html,
            2 => &Native,
            _ => &Pdf,
        };
        let run = || exporter.export(doc, layout, &options);
        let (first, second) = (run(), run());
        let digest = |r: &Result<
            reprise_clipboard::ExportResult,
            reprise_clipboard::ClipboardError,
        >| match r {
            Ok(r) => Ok(r.bytes.clone()),
            Err(e) => Err(e.to_string()),
        };
        ensure!(
            digest(&first) == digest(&second),
            "export-repeatable",
            "exporting twice gave different results"
        );
        let result = match first {
            Err(error) => {
                self.count("export.refused");
                return oracle::documented("export", [error.note().code.as_str()]);
            }
            Ok(result) => result,
        };
        ensure!(
            result.losses.features.len() == 10,
            "loss-report-complete",
            "{} features reported, 10 expected",
            result.losses.features.len()
        );
        oracle::documented(
            "export",
            result
                .losses
                .features
                .iter()
                .map(|l| l.code.as_str())
                .chain(result.losses.notes.iter().map(|n| n.code.as_str())),
        )?;
        self.w.report.transcript.push(fnv(&result.bytes));
        let everything = concat_text(doc);
        match kind % 4 {
            0 | 1 => {
                let text = String::from_utf8_lossy(&result.bytes).into_owned();
                let imported = if kind % 4 == 0 {
                    import_plain(&text)
                } else {
                    import_html(&text, ImportLimits::default())
                };
                let imported = imported.map_err(|e| {
                    Violation::new(
                        "export-reimports",
                        format!(
                            "the {} export can't be imported again: {e}",
                            ["plain", "HTML"][usize::from(kind % 4)]
                        ),
                    )
                })?;
                let back: String = imported
                    .fragment
                    .fragment
                    .blocks
                    .iter()
                    .map(|b| b.text.as_str())
                    .collect();
                ensure!(
                    char_bag(&back) == char_bag(&everything),
                    "export-keeps-text",
                    "{} export re-imports as {back:?}, document text is {everything:?}",
                    ["plain", "HTML"][usize::from(kind % 4)]
                );
            }
            2 => {
                NativeFragment::decode(&result.bytes).map_err(|e| {
                    Violation::new(
                        "export-reimports",
                        format!("native export won't decode: {e}"),
                    )
                })?;
            }
            _ => {}
        }
        Ok(())
    }

    // ------------------------------------------------------------ save/open

    fn save_reopen(&mut self, p: usize, shallow: bool, embed: bool, as_peer: u8) -> R {
        self.count("save");
        let doc = self.w.peers[p].editor.document();
        doc.commit();
        let mode = if shallow {
            PersistenceMode::Shallow
        } else {
            PersistenceMode::History
        };
        let id = DocumentId([as_peer; 16]);
        let layout = self.engine.layout(doc);
        let package = if embed {
            Package::new_with_resources(
                doc,
                id,
                mode,
                &layout,
                &self.engine.fonts,
                &self.engine.assets,
            )
        } else {
            Package::new(doc, id, mode)
        }
        .map_err(|e| Violation::new("save-succeeds", format!("saving a document failed: {e}")))?;
        let bytes = package.save().map_err(|e| {
            Violation::new("save-succeeds", format!("encoding a package failed: {e}"))
        })?;
        ensure!(
            package.save().ok().as_ref() == Some(&bytes),
            "save-repeatable",
            "saving one package twice gave different bytes"
        );
        self.w.report.transcript.push(fnv(&bytes));
        let peer_id = u64::from(as_peer % 3) + 1;
        let migrations = MigrationRegistry::builtin();
        let mut fonts = FontStore::default();
        let opened = if embed {
            Package::open_with_fonts(&bytes, peer_id, Limits::default(), &migrations, &mut fonts)
        } else {
            Package::open(&bytes, peer_id, Limits::default(), &migrations)
        }
        .map_err(|e| {
            Violation::new(
                "reopen-succeeds",
                format!("a saved package won't open: {e}"),
            )
        })?;
        let opened = Leaky::new(opened);
        oracle::documented("format", opened.notes.iter().map(|n| n.code.as_str()))?;
        let reopened = opened.editable_document().map_err(|e| {
            Violation::new(
                "reopen-succeeds",
                format!("a saved package opens read-only: {e}"),
            )
        })?;
        let (a, b) = (authored(doc), authored(reopened));
        ensure!(
            a == b,
            "reopen-identical",
            "reopened document differs:\n{}",
            authored::diff(&a, &b)
        );
        if !shallow {
            ensure!(
                doc.revision() == reopened.revision(),
                "reopen-identical",
                "revision {:?} became {:?}",
                doc.revision(),
                reopened.revision()
            );
            let (ra, rb) = (
                authored::ranges(doc, &self.w.ranges),
                authored::ranges(reopened, &self.w.ranges),
            );
            ensure!(
                ra == rb,
                "reopen-identical",
                "ranges differ: {ra:?} vs {rb:?}"
            );
            if !embed {
                ensure!(
                    opened.package().save().ok().as_ref() == Some(&bytes),
                    "reopen-resaves-identically",
                    "reopening and saving again changed the bytes"
                );
            }
        }
        // The reopened document lays out as the original did. With embedded
        // resources, the engine is rebuilt from what the package carried.
        let after = if embed {
            let mut assets = reprise_display::AssetStore::default();
            let notes = opened.assets.restore_images(&mut assets);
            oracle::documented("format", notes.iter().map(|n| n.code.as_str()))?;
            let mut restored = self.w.spec.bare();
            restored.fonts = fonts;
            restored.assets = assets;
            restored.layout(reopened)
        } else {
            self.engine.layout(reopened)
        };
        if shallow {
            ensure!(
                after.blocks == layout.blocks
                    && after.pages == layout.pages
                    && after.frames == layout.frames,
                "reopen-lays-out-identically",
                "shallow reopen changed the layout: {}",
                first_difference(&layout.to_json(), &after.to_json())
            );
        } else {
            snapshots_equal("reopen-lays-out-identically", &layout, &after)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ navigation

    fn navigate(&mut self, p: usize, seed: u16) -> R {
        self.count("navigate");
        let doc = self.w.peers[p].editor.document();
        doc.commit();
        let snapshot = self.engine.layout(doc);
        let nav = Navigator::semantic(&snapshot, doc);
        let valid = |caret: Caret| -> R {
            let text = text_of(doc, caret.node);
            ensure!(
                text.as_ref()
                    .is_some_and(|t| caret.offset <= t.len() && t.is_char_boundary(caret.offset)),
                "caret-valid",
                "caret {caret:?} is not a position in a live block ({text:?})"
            );
            Ok(())
        };
        let mut state = u32::from(seed) | 1;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as i32
        };
        // Hit tests anywhere, including far outside every page.
        for _ in 0..4 {
            let page = (next() & 3) as usize;
            let point = Point::<PageSpace>::new(
                Length::from_pt(next() % 700 - 150),
                Length::from_pt(next() % 500 - 100),
            );
            if let Some(hit) = nav.hit(page, point) {
                valid(hit.caret)?;
                ensure!(
                    nav.caret_rect(hit.caret).is_some(),
                    "hit-has-geometry",
                    "hit {hit:?} has no caret rectangle"
                );
            }
        }
        let Some(block) = snapshot
            .blocks
            .get(next() as usize % snapshot.blocks.len().max(1))
        else {
            return Ok(());
        };
        let carets = nav.caret_positions(block.node);
        let Some(&caret) = carets.get(next() as usize % carets.len().max(1)) else {
            return Ok(());
        };
        valid(caret)?;
        const MOVES: [Movement; 20] = [
            Movement::NextGrapheme,
            Movement::PreviousGrapheme,
            Movement::NextWord,
            Movement::PreviousWord,
            Movement::VisualRight,
            Movement::VisualLeft,
            Movement::InlineForward,
            Movement::InlineBackward,
            Movement::LineUp,
            Movement::LineDown,
            Movement::LineStart,
            Movement::LineEnd,
            Movement::LineLeftmost,
            Movement::LineRightmost,
            Movement::LineInlineStart,
            Movement::LineInlineEnd,
            Movement::BlockStart,
            Movement::BlockEnd,
            Movement::DocumentStart,
            Movement::DocumentEnd,
        ];
        let mut focus = caret;
        for movement in MOVES {
            if let Some(moved) = nav.move_caret(caret, movement) {
                valid(moved)?;
                focus = moved;
            }
        }
        let selection = Selection {
            anchor: caret,
            focus,
        };
        let ranges = nav.selection_ranges(&selection);
        let _ = nav.selection_rects(&selection);
        let _ = nav.select_all();
        let _ = nav.select_word(caret);
        let _ = nav.select_line(caret);
        let _ = nav.select_block(caret);
        if ranges.iter().any(|r| !r.bytes.is_empty()) {
            let schemas = self.w.peers[p].editor.schemas();
            if let Ok(copied) = copy_selection(
                doc,
                "ns",
                &selection,
                &snapshot,
                schemas,
                Some(&self.engine.fonts),
            ) {
                oracle::documented("clipboard", copied.notes.iter().map(|n| n.code.as_str()))?;
                let flattened = copied
                    .notes
                    .iter()
                    .any(|n| n.code.as_str() == "clipboard.selection-table");
                if !flattened {
                    let selected: String = ranges
                        .iter()
                        .filter_map(|r| {
                            text_of(doc, r.node)
                                .and_then(|t| t.get(r.bytes.clone()).map(str::to_owned))
                        })
                        .collect();
                    let text: String = copied
                        .fragment
                        .blocks
                        .iter()
                        .map(|b| b.text.as_str())
                        .collect();
                    ensure!(
                        char_bag(&text) == char_bag(&selected),
                        "selection-copy-preserves-text",
                        "selection {selection:?} covers {selected:?}, the copy holds {text:?}"
                    );
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------- epilogue

    /// After the ops: undo everything, redo everything, and sync, checking
    /// state and convergence at every stage.
    fn epilogue(&mut self) -> R {
        for p in 0..2 {
            let mut guard = 0;
            let finals = authored(self.w.peers[p].editor.document());
            let peer = &mut self.w.peers[p];
            let steps = peer.editor.undo_count();
            // Redo-all only returns to the final state if nothing was left on
            // the redo stack when the ops ended.
            let nothing_to_redo = peer.editor.redo_count() == 0;
            while peer.editor.can_undo() {
                guard += 1;
                ensure!(guard < 5000, "undo-terminates", "undo never runs out");
                peer.editor
                    .undo()
                    .map_err(|e| Violation::new("undo-redo-succeeds", e.to_string()))?;
            }
            let undone = authored(peer.editor.document());
            if !peer.foreign {
                ensure!(
                    undone == peer.initial,
                    "undo-everything-restores",
                    "after undoing all {steps} steps of peer {p}:\n{}",
                    authored::diff(&peer.initial, &undone)
                );
            }
            guard = 0;
            while peer.editor.can_redo() {
                guard += 1;
                ensure!(guard < 5000, "redo-terminates", "redo never runs out");
                peer.editor
                    .redo()
                    .map_err(|e| Violation::new("undo-redo-succeeds", e.to_string()))?;
            }
            let redone = authored(peer.editor.document());
            if !peer.foreign && nothing_to_redo {
                ensure!(
                    redone == finals,
                    "redo-everything-restores",
                    "after redoing all steps of peer {p}:\n{}",
                    authored::diff(&finals, &redone)
                );
            }
            peer.undo_log.clear();
            peer.redo_log.clear();
        }
        self.count("epilogue");
        self.sync(SyncMode::AThenB)?;
        // Both peers undo everything and converge again.
        for p in 0..2 {
            let peer = &mut self.w.peers[p];
            let mut guard = 0;
            while peer.editor.can_undo() {
                guard += 1;
                ensure!(guard < 5000, "undo-terminates", "undo never runs out");
                peer.editor
                    .undo()
                    .map_err(|e| Violation::new("undo-redo-succeeds", e.to_string()))?;
            }
        }
        self.sync(SyncMode::Crossed)?;
        for p in 0..2 {
            let peer = &mut self.w.peers[p];
            while peer.editor.can_redo() {
                peer.editor
                    .redo()
                    .map_err(|e| Violation::new("undo-redo-succeeds", e.to_string()))?;
            }
        }
        self.sync(SyncMode::BThenA)
    }
}

/// A `reprise.follow` relation to any layout target.
fn follow_to(owner: NodeId, target: Target) -> Relation {
    Relation::new(reprise_doc::relation::builtin::FOLLOW)
        .owned_by(owner)
        .target("line", target)
}

fn first_is_second(effects: &[reprise_edit::Effect]) -> bool {
    matches!(effects.first(), Some(reprise_edit::Effect::Join { first, second, .. }) if first == second)
}

/// Holds a value that is leaked, not dropped, while a panic unwinds. A Loro
/// document whose lock was poisoned by a panic inside it panics again when
/// dropped, and a panic during unwinding aborts the process, which would hide
/// the original panic from `catch_unwind` (and from the minimiser).
struct Leaky<T>(Option<T>);

impl<T> Leaky<T> {
    fn new(value: T) -> Self {
        Leaky(Some(value))
    }
}

impl<T> std::ops::Deref for Leaky<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
            .as_ref()
            .expect("a Leaky value is present until dropped")
    }
}

impl<T> Drop for Leaky<T> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            std::mem::forget(self.0.take());
        }
    }
}

/// A budgeted job, optionally cancelled part-way, against the reference.
fn run_job(
    session: &mut LayoutSession<'_>,
    doc: &Document,
    reference: &LayoutSnapshot,
    budgets: &[u16],
    viewport_byte: u8,
    partial: bool,
    cancel_after: Option<u8>,
) -> R {
    let cancel_at = cancel_after.map(|c| usize::from(c) % budgets.len().max(1));
    let mut cancelled = false;
    {
        let mut job = session.start(doc, viewport(viewport_byte));
        for (k, &budget) in budgets.iter().enumerate() {
            if cancel_at == Some(k) {
                job.cancel();
                ensure!(
                    matches!(job.step(5), Err(JobError::Cancelled)),
                    "cancelled-job-stays-cancelled",
                    "a cancelled job still stepped"
                );
                ensure!(
                    matches!(job.complete(), Err(JobError::Cancelled)),
                    "cancelled-job-stays-cancelled",
                    "a cancelled job still completed"
                );
                cancelled = true;
                break;
            }
            let budget = usize::from(budget);
            let step = job
                .step(budget)
                .map_err(|e| Violation::new("job-step", format!("step({budget}) failed: {e}")))?;
            ensure!(
                step.used <= budget,
                "job-budget-respected",
                "step({budget}) used {}",
                step.used
            );
            if partial {
                let view = job
                    .partial()
                    .map_err(|e| Violation::new("job-partial", format!("partial() failed: {e}")))?;
                view.publish(doc).map_err(|e| {
                    Violation::new(
                        "job-partial",
                        format!("a current partial won't publish: {e}"),
                    )
                })?;
                if view.coverage().complete {
                    snapshots_equal("incremental-equals-reference", view.snapshot(), reference)?;
                }
            }
        }
        if !cancelled {
            let mut finished = false;
            for _ in 0..64 {
                let step = job
                    .step(usize::MAX)
                    .map_err(|e| Violation::new("job-step", format!("step(MAX) failed: {e}")))?;
                if step.complete {
                    finished = true;
                    break;
                }
            }
            ensure!(
                finished,
                "job-terminates",
                "a job didn't finish in 64 unbounded steps"
            );
            let snapshot = job
                .complete()
                .map_err(|e| Violation::new("job-complete", e.to_string()))?
                .ok_or_else(|| {
                    Violation::new("job-complete", "a finished job returned no snapshot")
                })?;
            snapshots_equal("incremental-equals-reference", &snapshot, reference)?;
        }
    }
    if cancelled {
        // A cancelled job must not poison the session's caches.
        let again = session
            .layout(doc)
            .map_err(|e| Violation::new("session-layout", e.to_string()))?;
        snapshots_equal("cancel-leaves-session-sound", &again, reference)?;
    }
    Ok(())
}

#[allow(dead_code)]
fn _unused(_: BlockKind, _: Option<Revision>) {}
