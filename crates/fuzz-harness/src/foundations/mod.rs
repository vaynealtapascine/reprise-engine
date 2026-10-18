//! The wave-1 foundations, fuzzed through the public facade.
//!
//! Wave 1 added paragraph styles and named styles with durable identities,
//! `batch` with `expected_revision`, `import_copy`, roles and lists, inline
//! objects and fields, annotations and preflight refusal. `facade.rs` knows
//! none of them; this scenario does. It drives 2 or 3 replicas that share
//! history, plus independent documents made by `import_copy`, with the whole
//! vocabulary: single commands, batches (fresh, stale or no
//! `expected_revision`), interaction recipes, clipboard copy and paste of
//! mixed ranges, HTML import, undo and redo, sync in several orders, save and
//! reopen, layout in budgeted steps, queries, exports and rendering.
//!
//! After each step it checks:
//!
//! -   nothing panics, and every code a host sees is documented;
//! -   `State` is coherent (atoms match U+FFFC characters, spans are in range,
//!     list items name lists, resolved styles exist);
//! -   a refused command or batch leaves the document byte-identical, and a
//!     stale `expected_revision` is refused as `bindings.stale`;
//! -   undo then redo restores the state;
//! -   the live (incrementally laid out) session draws exactly what a session
//!     reopened from its saved package draws, and the reopened state is the
//!     same;
//! -   package bits 14, 15, 16 and 18 are set when their stored form is live
//!     and only when it could have been written;
//! -   replicas that exchange all updates, in any order, reach the same state;
//! -   the whole scenario twice gives byte-identical output.
//!
//! Like the core harness, a scenario is a byte string and decoding is total.

mod commands;
pub mod oracles;

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use reprise::{
    Affinity, AnnotationKind, AnnotationPayload, AnnotationStatus, AssetDeclaration, AssetKind,
    Batch, Block, Caret, Command, Create, DocumentSession, ExportFormat, FontDeclaration,
    FontSettings, FontStyle, GenericFamily, ImportCopy, LayoutOptions, LinkDestination, Open,
    Paste, Payload, Selection, State, TextImport, Transaction, Workspace,
};

use crate::codes;
use crate::ensure;
use crate::facade::{call, documented, layout_equivalence, options, pages, run_job};
use crate::input::{Input, fnv};
use crate::violation::{R, Violation};
use commands::{Gen, Snap, boundary};
use oracles::{Flags, authored};

/// The most ops a foundations scenario runs.
pub const MAX_OPS: usize = 36;
/// The most documents alive at once.
const MAX_DOCS: usize = 5;

const DOCUMENT_ID: &str = "00112233445566778899aabbccddeeff";
const SERIF: &[u8] = include_bytes!("../../../../fixtures/fonts/SourceSerifPro-Regular.otf");
const RED_PNG: &[u8] = include_bytes!("../../../../fixtures/images/red-1x1.png");
const DENSITY_PNG: &[u8] = include_bytes!("../../../../fixtures/images/density.png");

const HTML: [&str; 8] = [
    "<h1>Title</h1><p>one <a href=\"https://example.com/x\"><b>two</b> three</a></p>",
    "<ul><li>a</li><li>b<ul><li>c</li></ul></li></ul>",
    "<ol start=\"7\"><li>x</li><li>y</li></ol><p>after</p>",
    "<p>plain</p><blockquote>quoted</blockquote><pre>code\nmore</pre>",
    "<p>a <img src=x alt=pic> b</p><p>deep <i><b><u>nest</u></b></i></p>",
    "<p>link <a href=\"javascript:alert(1)\">bad</a> and <a href=\"mailto:a@b.c\">mail</a></p>",
    "<p>field <span data-field=\"page-number\">7</span> end \u{FFFC}</p>",
    "<h2>H2</h2><h3>H3</h3><ul><li><a href=\"https://e.com/\">linked item</a></li></ul>",
];

const PLAIN: [&str; 4] = [
    "one\ntwo\nthree",
    "- item\n- another\n  - nested",
    "1. first\n2. second",
    "\u{FFFC}\u{5d0}\u{5d1}\n\n\ttabbed",
];

/// What a finished scenario reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Digests of the states, pages and packages the scenario produced, in
    /// order; two runs of one scenario must agree on them exactly.
    pub transcript: Vec<u64>,
    /// How often each command and feature was reached: `cmd:<Name>` attempts,
    /// `ok:<Name>` successes, `refused:<Name>`, `op:*` and `state:*`.
    pub counts: BTreeMap<String, usize>,
}

/// Runs bytes as a foundations scenario; a panic is a `no-panic` violation.
pub fn run_foundations(data: &[u8]) -> Result<Report, Violation> {
    match catch_unwind(AssertUnwindSafe(|| execute(data))) {
        Ok(result) => result,
        Err(payload) => Err(Violation::new(
            "no-panic",
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "non-string panic".into()),
        )),
    }
}

/// Runs the scenario twice: the whole transcript must be identical.
pub fn run_foundations_twice(data: &[u8]) -> Result<Report, Violation> {
    let first = run_foundations(data)?;
    let second = run_foundations(data)?;
    ensure!(
        first.transcript == second.transcript,
        "foundations-repeatable",
        "the same scenario produced different output on a second run (first differs at {:?})",
        first
            .transcript
            .iter()
            .zip(&second.transcript)
            .position(|(a, b)| a != b)
    );
    Ok(first)
}

/// For libFuzzer and tests: panics with the bytes on a violation.
pub fn check_foundations_bytes(data: &[u8]) -> Report {
    match run_foundations(data) {
        Ok(report) => report,
        Err(v) => panic!("{v}\nfoundations scenario bytes: {}", crate::run::hex(data)),
    }
}

struct Replica {
    s: DocumentSession,
    may: Flags,
    lineage: u8,
    /// Authored states before each of this replica's own undoable steps, and
    /// the states undo has stepped back from. Cleared by anything that
    /// isn't the replica's own step (a merge).
    trail: Vec<State>,
    redo: Vec<State>,
    /// The revision before the last edit: the source of a believable stale one.
    old_revision: Vec<reprise::Clock>,
    inline_ids: std::collections::BTreeSet<String>,
    /// An inline atom was removed or deactivated here at some point.
    retired: bool,
    /// A `MoveBlock` succeeded in this lineage (the shape of fuzz-2
    /// `move-undo-orphan`).
    moved: bool,
}

struct Clip {
    bytes: Vec<u8>,
    may: Flags,
}

struct World<'a> {
    input: Input<'a>,
    ws: Workspace,
    reps: Vec<Replica>,
    opts: LayoutOptions,
    assets: Vec<String>,
    targets: Vec<String>,
    clip: Option<Clip>,
    transcript: Vec<u64>,
    counts: BTreeMap<String, usize>,
    copies: u32,
}

fn violation(oracle: &'static str, detail: impl Into<String>) -> Violation {
    Violation::new(oracle, detail)
}

fn command_name(c: &Command) -> String {
    let debug = format!("{c:?}");
    debug
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .next()
        .unwrap_or("?")
        .to_owned()
}

fn note_flags(flags: &mut Flags, c: &Command) {
    use Command::*;
    match c {
        SetRole { .. }
        | CreateList { .. }
        | UpdateList { .. }
        | DeleteList { .. }
        | SetListItem { .. }
        | RemoveListItem { .. }
        | SetListLevel { .. }
        | IndentListItem { .. }
        | RestartList { .. }
        | ContinueList { .. }
        | Enter { .. }
        | BackspaceAtStart { .. } => flags.semantics = true,
        InsertInline { .. } | UpdateInline { .. } => flags.inline = true,
        AddAnnotation { .. } | UpdateAnnotation { .. } | RemoveAnnotation { .. } => {
            flags.annotations = true;
        }
        DefineStyle { .. }
        | UpdateStyle { .. }
        | RenameStyle { .. }
        | DeleteStyle { .. }
        | ApplyStyle { .. }
        | SetStyle { .. }
        | InsertBlock { .. }
        | FormatText { .. } => flags.styles = true,
        _ => {}
    }
}

fn total_atoms(state: &State) -> usize {
    state.blocks.iter().map(|b| b.inline.len()).sum()
}

fn execute(data: &[u8]) -> R<Report> {
    let mut input = Input::new(data);
    let ws = Workspace::new();
    ws.declare_font(
        &Payload::new(FontDeclaration {
            family: "Source Serif Pro".into(),
            weight: 400,
            style: FontStyle::Normal,
            stretch: 1000,
            face_index: 0,
        }),
        SERIF,
    )
    .map_err(|e| violation("foundations-setup", e.to_string()))?;
    ws.set_font_settings(&Payload::new(FontSettings {
        generics: BTreeMap::from([
            (GenericFamily::Serif, "Source Serif Pro".to_owned()),
            (GenericFamily::SansSerif, "Source Serif Pro".to_owned()),
        ]),
        ..FontSettings::default()
    }))
    .map_err(|e| violation("foundations-setup", e.to_string()))?;
    let opts = match input.below(3) {
        0 => LayoutOptions::default(),
        // Narrow pages, so lists and atoms wrap and paginate.
        1 => LayoutOptions {
            width: 160 * 1024,
            height: 120 * 1024,
            ..LayoutOptions::default()
        },
        _ => LayoutOptions {
            width: 300 * 1024,
            height: 70 * 1024,
            ..LayoutOptions::default()
        },
    };
    let mut world = World {
        input,
        ws,
        reps: Vec::new(),
        opts,
        assets: Vec::new(),
        targets: Vec::new(),
        clip: None,
        transcript: Vec::new(),
        counts: BTreeMap::new(),
        copies: 0,
    };
    world.setup()?;
    let mut ops = 0;
    while !world.input.is_empty() && ops < MAX_OPS {
        ops += 1;
        world.op().map_err(|mut v| {
            v.op = Some(ops);
            v
        })?;
    }
    world.epilogue().map_err(|mut v| {
        v.op = Some(ops + 1);
        v
    })?;
    Ok(Report {
        transcript: world.transcript,
        counts: world.counts,
    })
}

impl World<'_> {
    fn count(&mut self, key: impl Into<String>) {
        *self.counts.entry(key.into()).or_default() += 1;
    }

    fn state(&self, i: usize) -> R<State> {
        self.reps[i]
            .s
            .state()
            .map(|p| p.data)
            .map_err(|e| violation("facade-state", format!("{} ({})", e, e.code())))
    }

    /// Every operation the document holds, as bytes. A snapshot would also
    /// carry the empty root containers that a read (even `state()`) declares
    /// lazily; those are not operations and not the document's content.
    fn bytes(&self, i: usize) -> R<Vec<u8>> {
        self.reps[i]
            .s
            .sync_export(&Payload::new(reprise::SyncRequest {
                since: Some(Vec::new()),
            }))
            .map(|u| u.data.content.bytes)
            .map_err(|e| violation("facade-export-updates", format!("{} ({})", e, e.code())))
    }

    fn register_assets(&mut self, s: &mut DocumentSession) -> R {
        for (id, bytes) in [("red", RED_PNG), ("density", DENSITY_PNG)] {
            match s.register_asset(
                &Payload::new(AssetDeclaration {
                    id: id.into(),
                    kind: AssetKind::Image,
                }),
                bytes,
            ) {
                Ok(hash) => {
                    if !self.assets.contains(&hash.data) {
                        self.assets.push(hash.data);
                    }
                }
                // Already embedded in a package this session was opened from.
                Err(e) => documented(&e)?,
            }
        }
        Ok(())
    }

    fn open(&mut self, bytes: &[u8], peer: &str) -> R<Option<DocumentSession>> {
        let Some(mut s) = call(self.ws.open(
            &Payload::new(Open {
                peer_id: peer.into(),
            }),
            bytes,
        ))?
        else {
            return Ok(None);
        };
        self.register_assets(&mut s)?;
        Ok(Some(s))
    }

    fn push(&mut self, s: DocumentSession, may: Flags, lineage: u8) -> R<usize> {
        let state = s
            .state()
            .map_err(|e| violation("facade-state", e.to_string()))?
            .data;
        self.reps.push(Replica {
            s,
            may,
            lineage,
            trail: Vec::new(),
            redo: Vec::new(),
            old_revision: state.revision.clone(),
            inline_ids: inline_ids(&state),
            retired: false,
            moved: false,
        });
        Ok(self.reps.len() - 1)
    }

    fn setup(&mut self) -> R {
        let mut a = self
            .ws
            .create(&Payload::new(Create {
                document_id: DOCUMENT_ID.into(),
                peer_id: "1".into(),
            }))
            .map_err(|e| violation("foundations-setup", e.to_string()))?;
        self.register_assets(&mut a)?;
        for i in 0..1 + self.input.below(3) {
            let text = commands::TEXTS[1 + self.input.below(commands::TEXTS.len() - 1)]
                .repeat(1 + self.input.below(6));
            a.apply(&Payload::new(Transaction {
                commands: vec![Command::InsertBlock {
                    parent: None,
                    index: i as u32,
                    block_kind: reprise::BlockKind::Paragraph,
                    text: text.replace('\u{FFFC}', "x"),
                    style: reprise::Style::default(),
                }],
            }))
            .map_err(|e| violation("foundations-setup", format!("{e} ({})", e.code())))?;
        }
        let first = self.push(a, Flags::default(), 0)?;
        // The seed paragraphs name styles of their own.
        self.reps[first].may.styles = true;
        // Seed the document with some of the interactions, so the scenario
        // starts in a state with lists, objects and annotations in it.
        for _ in 0..self.input.below(4) {
            self.edit(first, true, true)?;
        }
        let saved = self.reps[first]
            .s
            .save()
            .map_err(|e| violation("foundations-setup", e.to_string()))?
            .data
            .bytes;
        let replicas = 2 + usize::from(self.input.chance(80));
        for n in 2..=replicas {
            let b = self
                .open(&saved, &n.to_string())?
                .ok_or_else(|| violation("foundations-setup", "the seed package didn't open"))?;
            let may = self.reps[first].may;
            self.push(b, may, 0)?;
        }
        Ok(())
    }

    fn pick(&mut self) -> usize {
        self.input.below(self.reps.len())
    }

    fn op(&mut self) -> R {
        let i = self.pick();
        match self.input.u8() {
            0..=89 => self.edit(i, false, false),
            90..=129 => self.edit(i, true, false),
            130..=149 => {
                let batch = self.input.chance(128);
                self.edit(i, batch, true)
            }
            150..=164 => self.undo_redo(i),
            165..=179 => self.sync(),
            180..=199 => self.copy_paste(i),
            200..=209 => self.import_text(i),
            210..=219 => self.reference(i),
            220..=227 => self.import_copy(i),
            228..=237 => {
                self.count("op:layout-budgets");
                let mut transcript = Vec::new();
                layout_equivalence(&mut self.reps[i].s, &mut self.input, &mut transcript)?;
                self.transcript.extend(transcript);
                Ok(())
            }
            238..=247 => self.queries(i),
            _ => self.export_render(i),
        }
    }

    // ------------------------------------------------------------------
    // Editing

    fn stale_revision(&mut self, i: usize, current: &[reprise::Clock]) -> Vec<reprise::Clock> {
        let old = &self.reps[i].old_revision;
        if old != current && self.input.chance(160) {
            return old.clone();
        }
        let mut bumped = current.to_vec();
        match bumped.first_mut() {
            Some(clock) if self.input.chance(160) => {
                clock.counter = clock.counter.saturating_add(1)
            }
            _ => bumped.push(reprise::Clock {
                peer: "999".into(),
                counter: 1,
            }),
        }
        bumped
    }

    fn edit(&mut self, i: usize, batch: bool, recipe: bool) -> R {
        let before = self.state(i)?;
        let before_bytes = self.bytes(i)?;
        let snap = Snap {
            state: before.clone(),
        };
        let mut commands = {
            let mut g = Gen {
                input: &mut self.input,
                snap: &snap,
                assets: &self.assets,
                targets: &self.targets,
            };
            if recipe {
                g.recipe()
            } else if batch {
                let n = 1 + g.input.below(4);
                (0..n).map(|_| g.command()).collect()
            } else {
                vec![g.command()]
            }
        };
        if commands.len() == 1 && matches!(commands[0], Command::InsertImage { .. }) {
            commands.clear();
        }
        for _ in 0..commands::enrol_list_targets(&snap, &mut commands, &mut self.input) {
            self.count("generated:list-enrolment");
        }
        for _ in 0..commands::exclude_known_shapes(&mut commands) {
            self.count("excluded:astra-4-R1");
        }
        let expectation = if batch { self.input.u8() } else { 0 };
        let expected = match expectation {
            0..=119 => None,
            120..=199 => Some(before.revision.clone()),
            _ => Some(self.stale_revision(i, &before.revision)),
        };
        let stale = expectation >= 200;
        let selection = if batch && self.input.chance(100) && !before.blocks.is_empty() {
            let blocks = &before.blocks;
            let b = &blocks[self.input.below(blocks.len())];
            let a = boundary(&b.text, self.input.u16());
            let f = boundary(&b.text, self.input.u16());
            let caret = |offset| Caret {
                node: b.id.clone(),
                offset,
                affinity: Affinity::Downstream,
            };
            Some(Selection {
                anchor: caret(a),
                focus: caret(f),
            })
        } else {
            None
        };
        trace(|| {
            format!(
                "replica {i}: {} {commands:?} expected={expected:?}",
                if batch { "batch" } else { "apply" }
            )
        });
        for c in &commands {
            self.count(format!("cmd:{}", command_name(c)));
        }
        self.count(if !batch {
            "op:apply"
        } else if stale {
            "op:batch-stale"
        } else if expected.is_some() {
            "op:batch-fresh"
        } else {
            "op:batch"
        });
        let result = if batch && !commands.is_empty() {
            self.reps[i]
                .s
                .batch(&Payload::new(Batch {
                    commands: commands.clone(),
                    expected_revision: expected.clone(),
                    selection: selection.clone(),
                }))
                .map(|r| (r.data.applied, r.data.selection))
        } else {
            self.reps[i]
                .s
                .apply(&Payload::new(Transaction {
                    commands: commands.clone(),
                }))
                .map(|r| (r.data, None))
        };
        match result {
            Ok((applied, mapped)) => {
                ensure!(
                    !stale,
                    "stale-revision-refused",
                    "a batch expecting a stale revision was applied"
                );
                for note in &applied.diagnostics {
                    ensure!(
                        codes::is_documented(&note.code),
                        "undocumented-code",
                        "an applied edit reported `{}`",
                        note.code
                    );
                }
                for c in &commands {
                    self.count(format!("ok:{}", command_name(c)));
                    note_flags(&mut self.reps[i].may, c);
                }
                if commands
                    .iter()
                    .any(|c| matches!(c, Command::MoveBlock { .. }))
                {
                    self.reps[i].moved = true;
                }
                // An object made and deleted inside one transaction is never seen
                // in a state, yet leaves a hidden marker (the shape of Astra 4 R4).
                if commands
                    .iter()
                    .any(|c| matches!(c, Command::InsertInline { .. }))
                    && commands.iter().any(|c| {
                        matches!(
                            c,
                            Command::DeleteText { .. }
                                | Command::DeleteBlock { .. }
                                | Command::JoinBlocks { .. }
                        )
                    })
                {
                    self.reps[i].retired = true;
                }
                self.reps[i].old_revision = before.revision.clone();
                let after = self.state(i)?;
                if let Some(sel) = mapped {
                    for caret in [&sel.anchor, &sel.focus] {
                        let ok = after.blocks.iter().any(|b| {
                            b.id == caret.node
                                && b.text.is_char_boundary(caret.offset as usize)
                                && (caret.offset as usize) <= b.text.len()
                        });
                        ensure!(
                            ok,
                            "batch-selection-valid",
                            "the mapped selection names {} at {}, which isn't a position",
                            caret.node,
                            caret.offset
                        );
                    }
                }
                self.after_edit(i, &before, after)?;
            }
            Err(error) => {
                documented(&error)?;
                self.count(format!("refused:{}", error.code()));
                trace(|| format!("  refused: {error} ({})", error.code()));
                if let [only] = commands.as_slice() {
                    self.count(format!(
                        "refused-by:{}:{}",
                        command_name(only),
                        error.code()
                    ));
                }
                if error.code() == "bindings.poisoned" {
                    return Err(violation(
                        "no-panic",
                        format!("a session was poisoned by {commands:?}"),
                    ));
                }
                if error.code() == "bindings.stale" {
                    ensure!(
                        stale,
                        "stale-revision-refused",
                        "a command or batch was refused as stale without a stale expected_revision"
                    );
                }
                ensure!(
                    !stale || error.code() == "bindings.stale",
                    "stale-revision-refused",
                    "a batch expecting a stale revision was refused as `{}`, not stale",
                    error.code()
                );
                self.refusal_clean(i, &before, &before_bytes, &commands, error.code())?;
            }
        }
        Ok(())
    }

    /// A refused call changed nothing: the snapshot bytes, the authored
    /// state, the undo flags and the version vector are exactly as before.
    fn refusal_clean(
        &mut self,
        i: usize,
        before: &State,
        before_bytes: &[u8],
        what: &dyn std::fmt::Debug,
        code: &str,
    ) -> R {
        let after = self.state(i)?;
        let bytes = self.bytes(i)?;
        ensure!(
            authored(before) == authored(&after)
                && before.can_undo == after.can_undo
                && before.can_redo == after.can_redo,
            "refused-leaves-document-identical",
            "a refusal (`{code}`) changed the state; call: {what:?}"
        );
        ensure!(
            bytes == before_bytes && before.revision == after.revision,
            "refused-leaves-document-identical",
            "a refusal (`{code}`) changed the document bytes or revision; call: {what:?}"
        );
        Ok(())
    }

    /// Everything that must hold after a replica's own successful step.
    fn after_edit(&mut self, i: usize, before: &State, after: State) -> R {
        let changed = authored(before) != authored(&after);
        let vector_moved = before.revision != after.revision;
        if vector_moved {
            self.reps[i].trail.push(authored(before));
            self.reps[i].redo.clear();
        }
        self.observe(i, &after)?;
        if changed && vector_moved && self.input.chance(90) {
            self.round_trip(i, before, &after)?;
        }
        Ok(())
    }

    /// Undo restores the state before the step; redo restores the state after.
    fn round_trip(&mut self, i: usize, before: &State, after: &State) -> R {
        self.count("check:undo-redo");
        trace(|| format!("replica {i}: undo then redo"));
        let undone = call(self.reps[i].s.undo())?;
        ensure!(
            undone.is_some_and(|d| d.data),
            "undo-restores-state",
            "undo had nothing to undo right after a step that changed the document"
        );
        let mid = self.state(i)?;
        // Undo deactivates the atoms the step made; they stay in the text as
        // hidden markers (the shape of Astra 4 R4).
        self.note_retired(i, &mid);
        if authored(&mid) != authored(before) {
            return self.restore_mismatch(
                i,
                "undo-restores-state",
                &authored(before),
                &authored(&mid),
            );
        }
        let redone = call(self.reps[i].s.redo())?;
        ensure!(
            redone.is_some_and(|d| d.data),
            "redo-restores-state",
            "redo had nothing to redo right after an undo"
        );
        let end = self.state(i)?;
        if authored(&end) != authored(after) {
            return self.restore_mismatch(
                i,
                "redo-restores-state",
                &authored(after),
                &authored(&end),
            );
        }
        // Undo and redo are steps of their own for the version vector, but
        // leave the own-step trails as they were.
        self.observe(i, &end)
    }

    /// The states differ after undo or redo. Three faults are known and
    /// tracked separately, so the run can go on and find new bugs; anything
    /// else is a violation.
    ///
    /// -   `astra-4-R4`: annotations over hidden atom markers rebind on undo.
    /// -   `fuzz2-annotation-redo`: an annotation created in the same step as
    ///     the text it covers is gone after undo and redo (docs/fuzzing.md).
    /// -   `fuzz2-format-undo`: formatted text that was deleted comes back
    ///     without its formatting after undo (docs/fuzzing.md).
    fn restore_mismatch(&mut self, i: usize, oracle: &'static str, want: &State, got: &State) -> R {
        let (mut a, mut b) = (want.clone(), got.clone());
        let mut known = Vec::new();
        if a.blocks != b.blocks {
            let (mut a2, mut b2) = (a.clone(), b.clone());
            for block in a2.blocks.iter_mut().chain(b2.blocks.iter_mut()) {
                block.formatting = None;
            }
            if a2.blocks == b2.blocks {
                (a, b) = (a2, b2);
                known.push("known:fuzz2-format-undo");
            }
        }
        if a.annotations != b.annotations {
            let lost = b.annotations.len() < a.annotations.len()
                && b.annotations.iter().all(|x| a.annotations.contains(x));
            let resurrected = a.annotations.len() < b.annotations.len()
                && a.annotations.iter().all(|x| b.annotations.contains(x));
            // The same annotations, some Active before and Orphaned after.
            let orphaned = a.annotations.len() == b.annotations.len()
                && a.annotations.iter().zip(&b.annotations).all(|(x, y)| {
                    x == y
                        || (x.id == y.id
                            && x.status == AnnotationStatus::Active
                            && y.status == AnnotationStatus::Orphaned)
                });
            if lost {
                known.push("known:fuzz2-annotation-redo");
                a.annotations = b.annotations.clone();
            } else if resurrected {
                known.push("known:fuzz2-link-resurrected");
                a.annotations = b.annotations.clone();
            } else if self.reps[i].moved && orphaned {
                known.push("known:fuzz2-move-undo-orphan");
                a.annotations = b.annotations.clone();
            } else if self.reps[i].retired {
                known.push("known:astra-4-R4");
                a.annotations = b.annotations.clone();
            }
        }
        if a == b && !known.is_empty() {
            for key in known {
                self.count(key);
            }
            self.reps[i].trail.clear();
            self.reps[i].redo.clear();
            return Ok(());
        }
        Err(violation(oracle, describe_difference(want, got)))
    }

    /// Structural and layout checks on replica `i` in `state`.
    fn observe(&mut self, i: usize, state: &State) -> R {
        oracles::state_is_coherent(state)?;
        oracles::documented_diagnostics(state)?;
        self.note_retired(i, state);
        self.features(state);
        let Some(layout) = run_job(&mut self.reps[i].s, &self.opts, &[])? else {
            return Ok(());
        };
        let rendered = pages(&self.reps[i].s, &layout)?;
        for page in &rendered {
            self.transcript.push(fnv(page.as_bytes()));
        }
        self.transcript
            .push(fnv(format!("{:?}", authored(state)).as_bytes()));
        for d in self.reps[i].s.diagnostics().data {
            ensure!(
                codes::is_documented(&d.code),
                "undocumented-code",
                "diagnostics() reported `{}`",
                d.code
            );
        }
        self.reading_order(i, state)
    }

    fn features(&mut self, state: &State) {
        let blocks = &state.blocks;
        if blocks.iter().any(|b| b.role != reprise::BlockRole::Body) {
            self.count("state:role");
        }
        if blocks.iter().any(|b| b.list.is_some()) {
            self.count("state:list-item");
        }
        if !state.lists.is_empty() {
            self.count("state:list-definition");
        }
        if blocks.iter().any(|b| !b.inline.is_empty()) {
            self.count("state:inline");
        }
        if blocks
            .iter()
            .flat_map(|b| &b.inline)
            .any(|e| e.object.kind == "field")
        {
            self.count("state:inline-field");
        }
        if blocks
            .iter()
            .flat_map(|b| &b.inline)
            .any(|e| e.object.kind == "image")
        {
            self.count("state:inline-image");
        }
        if state
            .annotations
            .iter()
            .any(|a| a.kind == AnnotationKind::Link)
        {
            self.count("state:link");
        }
        if state
            .annotations
            .iter()
            .any(|a| a.kind == AnnotationKind::Comment)
        {
            self.count("state:comment");
        }
        if state
            .annotations
            .iter()
            .any(|a| a.status == AnnotationStatus::Orphaned)
        {
            self.count("state:orphan");
        }
        if state.annotations.iter().any(|a| a.spans.len() > 1) {
            self.count("state:cross-paragraph-annotation");
        }
        if blocks
            .iter()
            .any(|b| b.list.is_some() && !b.inline.is_empty())
        {
            self.count("state:atom-in-list-item");
        }
        if blocks.iter().any(|b| {
            !b.inline.is_empty()
                && state
                    .annotations
                    .iter()
                    .any(|a| a.spans.iter().any(|s| s.node == b.id))
        }) {
            self.count("state:atom-in-annotated-paragraph");
        }
        if blocks
            .iter()
            .any(|b| b.list.is_some() && !b.style.name.is_empty())
        {
            self.count("state:styled-list-item");
        }
        if state.styles.iter().any(|s| !s.builtin) {
            self.count("state:custom-style");
        }
    }

    /// Reading-order steps name real lines whose byte ranges are in their
    /// blocks and on boundaries (docs/plumbing.md).
    fn reading_order(&mut self, i: usize, state: &State) -> R {
        let Some(steps) = call(self.reps[i].s.reading_order())? else {
            return Ok(());
        };
        for step in &steps.data {
            let Some(block) = state.blocks.iter().find(|b| b.id == step.node) else {
                continue;
            };
            ensure!(
                step.start <= step.end
                    && step.end as usize <= block.text.len()
                    && block.text.is_char_boundary(step.start as usize)
                    && block.text.is_char_boundary(step.end as usize),
                "reading-order-ranges",
                "line {} of {} reads bytes {}..{} of {}",
                step.line,
                step.node,
                step.start,
                step.end,
                block.text.len()
            );
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Undo and redo as ops of their own

    fn undo_redo(&mut self, i: usize) -> R {
        self.count("op:undo-redo");
        trace(|| format!("replica {i}: undo/redo op"));
        let before = self.state(i)?;
        let undo = self.input.chance(128);
        let stepped = call(if undo {
            self.reps[i].s.undo()
        } else {
            self.reps[i].s.redo()
        })?;
        let after = self.state(i)?;
        if !stepped.is_some_and(|d| d.data) {
            ensure!(
                authored(&before) == authored(&after),
                "undo-redo-noop",
                "undo/redo reported nothing to do but changed the state"
            );
            return Ok(());
        }
        // The replica's own trail predicts the state.
        let rep = &mut self.reps[i];
        let predicted = if undo {
            rep.trail
                .pop()
                .inspect(|_| rep.redo.push(authored(&before)))
        } else {
            rep.redo
                .pop()
                .inspect(|_| rep.trail.push(authored(&before)))
        };
        if let Some(expected) = predicted
            && authored(&after) != expected
        {
            self.restore_mismatch(
                i,
                if undo {
                    "undo-restores-state"
                } else {
                    "redo-restores-state"
                },
                &expected,
                &authored(&after),
            )?;
        }
        self.observe(i, &after)
    }

    // ------------------------------------------------------------------
    // Sync

    fn sync(&mut self) -> R {
        self.count("op:sync");
        trace(|| "sync".to_owned());
        let lineages: Vec<u8> = {
            let mut l: Vec<u8> = self.reps.iter().map(|r| r.lineage).collect();
            l.sort_unstable();
            l.dedup();
            l
        };
        for lineage in lineages {
            let mut group: Vec<usize> = (0..self.reps.len())
                .filter(|&k| self.reps[k].lineage == lineage)
                .collect();
            if group.len() < 2 {
                continue;
            }
            // Any order: rotate by one choice, reverse by another.
            let rotate = self.input.below(group.len());
            group.rotate_left(rotate);
            if self.input.chance(128) {
                group.reverse();
            }
            self.converge(&group)?;
        }
        Ok(())
    }

    /// Every replica in `group` imports every other's snapshot, in `group`'s
    /// order, until all agree.
    fn converge(&mut self, group: &[usize]) -> R {
        for _ in 0..4 {
            let mut exports = Vec::new();
            for &k in group {
                exports.push((k, self.reps[k].s.export_updates()));
            }
            for (from, update) in exports {
                let update = match update {
                    Ok(update) => update,
                    Err(e) => {
                        documented(&e)?;
                        return Ok(());
                    }
                };
                for &to in group.iter().rev() {
                    if to == from {
                        continue;
                    }
                    if let Err(e) = self.reps[to].s.import_updates(&update) {
                        documented(&e)?;
                        ensure!(
                            e.code() == "bindings.limit",
                            "sync-accepts-replica",
                            "replica {to} refused replica {from}'s snapshot: {} ({})",
                            e,
                            e.code()
                        );
                        return Ok(());
                    }
                }
            }
            let vectors: Vec<_> = group
                .iter()
                .map(|&k| self.reps[k].s.sync_info().data.vector)
                .collect();
            if vectors.windows(2).all(|w| w[0] == w[1]) {
                break;
            }
        }
        let mut may = Flags::default();
        let mut retired = false;
        let mut moved = false;
        for &k in group {
            may.merge(self.reps[k].may);
            retired |= self.reps[k].retired;
            moved |= self.reps[k].moved;
        }
        for &k in group {
            self.reps[k].may = may;
            self.reps[k].retired = retired;
            self.reps[k].moved = moved;
            self.reps[k].trail.clear();
            self.reps[k].redo.clear();
        }
        let first = self.state(group[0])?;
        let first_vector = self.reps[group[0]].s.sync_info().data.vector;
        for &k in &group[1..] {
            let state = self.state(k)?;
            ensure!(
                self.reps[k].s.sync_info().data.vector == first_vector,
                "peers-converge",
                "version vectors differ after exchanging updates"
            );
            ensure!(
                authored(&state) == authored(&first),
                "peers-converge",
                "replicas {} and {k} differ after exchanging all updates: {}",
                group[0],
                describe_difference(&authored(&first), &authored(&state))
            );
        }
        let mut reference: Option<Vec<String>> = None;
        for &k in group {
            let state = self.state(k)?;
            self.observe(k, &state)?;
            let Some(layout) = run_job(&mut self.reps[k].s, &self.opts, &[])? else {
                continue;
            };
            let rendered = pages(&self.reps[k].s, &layout)?;
            match &reference {
                None => reference = Some(rendered),
                Some(first) => ensure!(
                    *first == rendered,
                    "peers-converge-layout",
                    "converged replicas draw differently"
                ),
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Save, reopen, and the full-versus-incremental layout oracle

    /// The live session (laid out incrementally across every edit so far)
    /// against a session reopened from its package (laid out from scratch).
    fn reference(&mut self, i: usize) -> R {
        self.count("op:reference");
        trace(|| format!("replica {i}: save, reopen, compare"));
        let Some(saved) = call(self.reps[i].s.save())? else {
            return Ok(());
        };
        let bytes = saved.data.bytes;
        self.transcript.push(fnv(&bytes));
        let state = self.state(i)?;
        oracles::bits_match(&bytes, &state, self.reps[i].may)?;
        let live = run_job(&mut self.reps[i].s, &self.opts, &[])?;
        let Some(mut reopened) = self.open(&bytes, "8")? else {
            return Ok(());
        };
        let reopened_state = reopened
            .state()
            .map_err(|e| violation("facade-state", e.to_string()))?
            .data;
        ensure!(
            authored(&state) == authored(&reopened_state),
            "reopen-identical",
            "a reopened package differs: {}",
            describe_difference(&authored(&state), &authored(&reopened_state))
        );
        if let Some(live) = live {
            let Some(fresh) = run_job(&mut reopened, &self.opts, &[])? else {
                return Ok(());
            };
            let (a, b) = (pages(&self.reps[i].s, &live)?, pages(&reopened, &fresh)?);
            ensure!(
                live.pages == fresh.pages && a == b,
                "incremental-equals-reference",
                "the live session and a from-scratch layout of its own package draw differently"
            );
        }
        // The reopened package, saved again, declares the same bits.
        if let Some(again) = call(reopened.save())? {
            ensure!(
                oracles::package_bits(&again.data.bytes)? == oracles::package_bits(&bytes)?,
                "package-bits-stable",
                "saving a reopened package changed its feature bits"
            );
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Clipboard

    fn copy_paste(&mut self, i: usize) -> R {
        self.count("op:copy-paste");
        trace(|| format!("replica {i}: copy/paste"));
        let source = self.state(i)?;
        if source.blocks.is_empty() {
            return Ok(());
        }
        let n = source.blocks.len();
        let (x, y) = (self.input.below(n), self.input.below(n));
        let (lo, hi) = (x.min(y), x.max(y));
        let (first, last) = (&source.blocks[lo], &source.blocks[hi]);
        let (mut a, mut b) = (
            boundary(&first.text, self.input.u16()),
            boundary(&last.text, self.input.u16()),
        );
        if lo == hi && a > b {
            std::mem::swap(&mut a, &mut b);
        }
        let caret = |block: &Block, offset| Caret {
            node: block.id.clone(),
            offset,
            affinity: Affinity::Downstream,
        };
        let selection = Selection {
            anchor: caret(first, a),
            focus: caret(last, b),
        };
        // Selections need a complete layout.
        let Some(_) = run_job(&mut self.reps[i].s, &self.opts, &[])? else {
            return Ok(());
        };
        let fresh = self.input.chance(180) || self.clip.is_none();
        let (bytes, may, copied_atoms) = if fresh {
            let Some(copied) = call(self.reps[i].s.copy(&Payload::new(selection)))? else {
                return Ok(());
            };
            let atoms = if lo == hi {
                first.text[a as usize..b as usize]
                    .chars()
                    .filter(|&c| c == '\u{FFFC}')
                    .count()
            } else {
                let mut n = first.text[a as usize..]
                    .chars()
                    .filter(|&c| c == '\u{FFFC}')
                    .count();
                for block in &source.blocks[lo + 1..hi] {
                    n += block.text.chars().filter(|&c| c == '\u{FFFC}').count();
                }
                n + last.text[..b as usize]
                    .chars()
                    .filter(|&c| c == '\u{FFFC}')
                    .count()
            };
            let may = self.reps[i].may;
            self.clip = Some(Clip {
                bytes: copied.data.bytes.clone(),
                may,
            });
            (copied.data.bytes, may, Some(atoms))
        } else {
            let clip = self.clip.as_ref().expect("checked");
            (clip.bytes.clone(), clip.may, None)
        };
        let j = self.pick();
        let target_before = self.state(j)?;
        let target_bytes = self.bytes(j)?;
        let at = if self.input.chance(160) && !target_before.blocks.is_empty() {
            let block = &target_before.blocks[self.input.below(target_before.blocks.len())];
            Some(caret(block, boundary(&block.text, self.input.u16())))
        } else {
            None
        };
        trace(|| {
            format!(
                "  copy {a}..{b} of blocks {lo}..={hi}; paste into {j} at {at:?}\n  source: {:?} {:?}\n  target: {:?} {:?}",
                source
                    .blocks
                    .iter()
                    .map(|b| (&b.id, &b.text))
                    .collect::<Vec<_>>(),
                source.annotations,
                target_before
                    .blocks
                    .iter()
                    .map(|b| (&b.id, &b.text))
                    .collect::<Vec<_>>(),
                target_before.annotations
            )
        });
        self.count(if i == j {
            "paste:same-replica"
        } else if self.reps[i].lineage == self.reps[j].lineage {
            "paste:other-replica"
        } else {
            "paste:other-document"
        });
        match self.reps[j].s.paste(&Payload::new(Paste { at }), &bytes) {
            Ok(applied) => {
                for note in &applied.data.diagnostics {
                    ensure!(
                        codes::is_documented(&note.code),
                        "undocumented-code",
                        "a paste reported `{}`",
                        note.code
                    );
                }
                self.count("ok:paste");
                self.reps[j].may.merge(may);
                self.reps[j].old_revision = target_before.revision.clone();
                let after = self.state(j)?;
                if let Some(copied) = copied_atoms {
                    let before_atoms = total_atoms(&target_before);
                    ensure!(
                        total_atoms(&after) == before_atoms + copied,
                        "paste-keeps-atoms",
                        "pasting a range with {copied} inline objects changed the target's objects from {before_atoms} to {}",
                        total_atoms(&after)
                    );
                    let comments = |s: &State| {
                        s.annotations
                            .iter()
                            .filter(|a| a.kind == AnnotationKind::Comment)
                            .count()
                    };
                    ensure!(
                        comments(&after) == comments(&target_before),
                        "paste-drops-comments",
                        "a clipboard paste changed the comment count from {} to {}",
                        comments(&target_before),
                        comments(&after)
                    );
                    ensure!(
                        after.annotations.len() >= target_before.annotations.len(),
                        "paste-keeps-annotations",
                        "a paste removed annotations"
                    );
                }
                self.after_edit(j, &target_before, after)
            }
            Err(error) => {
                documented(&error)?;
                self.count(format!("refused:{}", error.code()));
                ensure!(
                    error.code() != "bindings.poisoned",
                    "no-panic",
                    "a session was poisoned by a paste"
                );
                self.refusal_clean(j, &target_before, &target_bytes, &"paste", error.code())
            }
        }
    }

    fn import_text(&mut self, i: usize) -> R {
        self.count("op:import-text");
        trace(|| format!("replica {i}: import text"));
        let before = self.state(i)?;
        let before_bytes = self.bytes(i)?;
        let html = self.input.chance(160);
        let text = if html {
            HTML[self.input.below(HTML.len())]
        } else {
            PLAIN[self.input.below(PLAIN.len())]
        };
        let at = if self.input.chance(128) && !before.blocks.is_empty() {
            let block = &before.blocks[self.input.below(before.blocks.len())];
            Some(Caret {
                node: block.id.clone(),
                offset: boundary(&block.text, self.input.u16()),
                affinity: Affinity::Downstream,
            })
        } else {
            None
        };
        match self.reps[i].s.import_text(&Payload::new(TextImport {
            text: text.into(),
            html,
            at,
        })) {
            Ok(applied) => {
                for note in &applied.data.diagnostics {
                    ensure!(
                        codes::is_documented(&note.code),
                        "undocumented-code",
                        "an import reported `{}`",
                        note.code
                    );
                }
                self.count("ok:import-text");
                if html {
                    self.reps[i].may.semantics = true;
                    self.reps[i].may.annotations = true;
                    self.reps[i].may.inline = true;
                }
                self.reps[i].may.styles = true;
                self.reps[i].old_revision = before.revision.clone();
                let after = self.state(i)?;
                self.after_edit(i, &before, after)
            }
            Err(error) => {
                documented(&error)?;
                self.count(format!("refused:{}", error.code()));
                self.refusal_clean(i, &before, &before_bytes, &"import_text", error.code())
            }
        }
    }

    // ------------------------------------------------------------------
    // import_copy

    fn import_copy(&mut self, i: usize) -> R {
        self.count("op:import-copy");
        trace(|| format!("replica {i}: import_copy"));
        let Some(saved) = call(self.reps[i].s.save())? else {
            return Ok(());
        };
        let bytes = saved.data.bytes;
        let source = self.state(i)?;
        let source_bytes = self.bytes(i)?;
        self.copies += 1;
        let id = format!("{:032x}", 0xC0_FFEE_u128 + u128::from(self.copies));
        let Some(mut copy) = call(self.ws.import_copy(
            &Payload::new(ImportCopy {
                document_id: id,
                peer_id: "9".into(),
            }),
            &bytes,
        ))?
        else {
            return Ok(());
        };
        self.register_assets(&mut copy)?;
        let copied = copy
            .state()
            .map_err(|e| violation("facade-state", e.to_string()))?
            .data;
        oracles::state_is_coherent(&copied)?;
        // Same content, new identities.
        let (a, b) = (oracles::shape(&source), oracles::shape(&copied));
        ensure!(
            a == b,
            "import-copy-keeps-content",
            "import_copy changed the content:\n{}",
            first_differences(&a, &b)
        );
        // Annotations inside one paragraph come across exactly; a copy never
        // has more than its source. A link to a block or range that is gone
        // cannot be remapped: the copy drops it and says so
        // (`clipboard.annotation-dropped`), and only then may it be missing.
        let dropped = copy
            .diagnostics()
            .data
            .iter()
            .any(|d| d.code == "clipboard.annotation-dropped");
        let mut internal = BTreeSet::new();
        if dropped {
            for brief in &source.annotations {
                if let Some(Some(full)) =
                    call(self.reps[i].s.annotation(&Payload::new(brief.id.clone())))?
                        .map(|p| p.data)
                    && matches!(
                        full.payload,
                        AnnotationPayload::Link {
                            target: LinkDestination::Node { .. }
                                | LinkDestination::Heading { .. }
                                | LinkDestination::Range { .. }
                        }
                    )
                {
                    internal.insert(brief.id.clone());
                }
            }
        }
        let (mut from_copy, copied_total) = oracles::annotation_lines(&copied, &BTreeSet::new());
        let (from_source, source_total) = oracles::annotation_lines(&source, &internal);
        for line in &from_source {
            let found = from_copy.iter().position(|l| l == line);
            ensure!(
                found.is_some(),
                "import-copy-keeps-annotations",
                "import_copy lost the annotation `{line}`; the copy has {from_copy:?}"
            );
            if let Some(at) = found {
                from_copy.remove(at);
            }
        }
        ensure!(
            copied_total <= source_total,
            "import-copy-keeps-annotations",
            "import_copy made {copied_total} annotations from {source_total}"
        );
        // A source delta can't apply to the copy.
        if let Some(update) = call(self.reps[i].s.export_updates())? {
            ensure!(
                copy.import_updates(&update).is_err(),
                "import-copy-independent",
                "a copy accepted its source's update"
            );
        }
        ensure!(
            self.bytes(i)? == source_bytes,
            "import-copy-leaves-source",
            "import_copy changed the source document"
        );
        let mut may = self.reps[i].may;
        may.styles = true;
        self.copies = self.copies.min(250);
        let lineage = 1 + (self.copies % 250) as u8;
        if self.reps.len() >= MAX_DOCS {
            // Drop the oldest independent copy to make room.
            if let Some(old) = self.reps.iter().position(|r| r.lineage != 0) {
                self.reps.remove(old);
            }
        }
        if self.reps.len() < MAX_DOCS {
            let k = self.push(copy, may, lineage)?;
            let state = self.state(k)?;
            self.observe(k, &state)?;
            // The copy lives on: an edit, then the reference check.
            let batch = self.input.chance(128);
            self.edit(k, batch, false)?;
            self.reference(k)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Queries, exports

    fn queries(&mut self, i: usize) -> R {
        self.count("op:queries");
        let state = self.state(i)?;
        let Some(layout) = run_job(&mut self.reps[i].s, &self.opts, &[])? else {
            return Ok(());
        };
        let s = &self.reps[i].s;
        if let Some(outline) = call(s.outline())? {
            for entry in outline.data {
                ensure!(
                    (1..=6).contains(&entry.level)
                        && state.blocks.iter().any(|b| b.id == entry.node),
                    "outline-valid",
                    "outline entry {:?}",
                    entry
                );
            }
        }
        for &page in layout.pages.iter().take(6) {
            if let Some(links) = call(s.page_links(page))? {
                for link in links.data.links {
                    ensure!(
                        state
                            .annotations
                            .iter()
                            .any(|a| a.id == link.id && a.kind == AnnotationKind::Link),
                        "page-links-are-links",
                        "page {page} shows the link {} that State doesn't list",
                        link.id
                    );
                }
            }
        }
        for brief in state.annotations.iter().take(8) {
            let Some(full) = call(s.annotation(&Payload::new(brief.id.clone())))? else {
                continue;
            };
            if let Some(full) = full.data {
                ensure!(
                    full.spans == brief.spans && full.status == brief.status,
                    "annotation-matches-brief",
                    "annotation {} differs from its brief",
                    brief.id
                );
            }
            let _ = call(s.annotation_geometry(&Payload::new(brief.id.clone())))?;
        }
        for block in state.blocks.iter().take(4) {
            let _ = call(s.annotations_in(&Payload::new(reprise::AnnotationsIn {
                node: block.id.clone(),
                start: 0,
                end: block.text.len() as u32,
            })))?;
            if let Some(entry) = block.inline.first() {
                let _ = call(s.annotations_at(&Payload::new(reprise::AnnotationsAt {
                    node: block.id.clone(),
                    at: entry.at,
                })))?;
                // A page-of field's target is a persistent position.
                if let Some(target) = call(s.field_target(&Payload::new(reprise::FieldTarget {
                    node: block.id.clone(),
                    at: entry.at,
                })))? {
                    if self.targets.len() < 6 {
                        self.targets.push(target.data);
                    }
                }
            }
        }
        // A filtered view is the diagnostics, filtered.
        let all = s.diagnostics().data;
        for prefix in ["annotation.", "layout.", "role.", "list.", "style."] {
            if let Some(list) = call(s.diagnostics_for(&Payload::new(reprise::DiagnosticFilter {
                code_prefix: Some(prefix.into()),
                min_severity: None,
                subject: None,
                limit: None,
            })))? {
                let want: Vec<_> = all
                    .iter()
                    .filter(|d| d.code.starts_with(prefix))
                    .take(list.data.diagnostics.len())
                    .cloned()
                    .collect();
                ensure!(
                    want == list.data.diagnostics,
                    "diagnostics-for-filters",
                    "diagnostics_for({prefix}) isn't diagnostics() filtered"
                );
            }
        }
        Ok(())
    }

    fn export_render(&mut self, i: usize) -> R {
        self.count("op:export");
        let Some(_) = run_job(&mut self.reps[i].s, &self.opts, &[])? else {
            return Ok(());
        };
        let kind = self.input.u8();
        let format = match kind % 4 {
            0 => ExportFormat::PlainText,
            1 => ExportFormat::Html,
            2 => ExportFormat::Native,
            _ => ExportFormat::Pdf,
        };
        let s = &self.reps[i].s;
        let first = call(s.export(&Payload::new(format)))?;
        let second = call(s.export(&Payload::new(format)))?;
        ensure!(
            first.as_ref().map(|e| &e.data.content.bytes)
                == second.as_ref().map(|e| &e.data.content.bytes),
            "export-repeatable",
            "exporting twice gave different bytes"
        );
        if let Some(exported) = first {
            for loss in &exported.data.losses {
                ensure!(
                    codes::is_documented(&loss.code),
                    "undocumented-code",
                    "export reported `{}`",
                    loss.code
                );
            }
            for d in &exported.data.diagnostics {
                ensure!(
                    codes::is_documented(&d.code),
                    "undocumented-code",
                    "export diagnostic `{}`",
                    d.code
                );
            }
            self.transcript.push(fnv(&exported.data.content.bytes));
        }
        if kind & 0x80 != 0 {
            let mut transcript = Vec::new();
            crate::facade::render(&mut self.reps[i].s, &mut transcript)?;
            self.transcript.extend(transcript);
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // The end

    fn epilogue(&mut self) -> R {
        // Everything exchanged both ways must converge, whatever the order.
        self.order_independence()?;
        for i in 0..self.reps.len() {
            self.reference(i)?;
        }
        Ok(())
    }

    /// The same snapshots merged in two different orders give one state.
    fn order_independence(&mut self) -> R {
        let group: Vec<usize> = (0..self.reps.len())
            .filter(|&k| self.reps[k].lineage == 0)
            .collect();
        if group.len() < 2 {
            return Ok(());
        }
        self.count("check:order-independence");
        let mut saves = Vec::new();
        for &k in &group {
            let Some(saved) = call(self.reps[k].s.save())? else {
                return Ok(());
            };
            saves.push(saved.data.bytes);
        }
        let mut results = Vec::new();
        for reverse in [false, true] {
            let mut set = Vec::new();
            for (n, bytes) in saves.iter().enumerate() {
                let Some(s) = self.open(bytes, &format!("{}", 20 + n))? else {
                    return Ok(());
                };
                set.push(s);
            }
            let order: Vec<usize> = if reverse {
                (0..set.len()).rev().collect()
            } else {
                (0..set.len()).collect()
            };
            for _ in 0..2 {
                let mut exports = Vec::new();
                for &k in &order {
                    match set[k].export_updates() {
                        Ok(u) => exports.push((k, u)),
                        Err(e) => {
                            documented(&e)?;
                            return Ok(());
                        }
                    }
                }
                for (from, update) in exports {
                    for &to in order.iter().rev() {
                        if to != from
                            && let Err(e) = set[to].import_updates(&update)
                        {
                            documented(&e)?;
                            return Ok(());
                        }
                    }
                }
            }
            let state = set[0]
                .state()
                .map_err(|e| violation("facade-state", e.to_string()))?
                .data;
            let vector = set[0].sync_info().data.vector;
            results.push((authored(&state), vector));
        }
        ensure!(
            results[0] == results[1],
            "merge-order-independent",
            "merging the same replicas in two orders gave different states: {}",
            describe_difference(&results[0].0, &results[1].0)
        );
        Ok(())
    }
}

/// A short account of how two states differ, for violation details.
pub fn describe_difference(want: &State, got: &State) -> String {
    let mut parts = Vec::new();
    if want.blocks != got.blocks {
        let ids_want: Vec<&str> = want.blocks.iter().map(|b| b.id.as_str()).collect();
        let ids_got: Vec<&str> = got.blocks.iter().map(|b| b.id.as_str()).collect();
        if ids_want != ids_got {
            parts.push(format!("block ids {ids_want:?} vs {ids_got:?}"));
        } else {
            for (w, g) in want.blocks.iter().zip(&got.blocks) {
                if w != g {
                    parts.push(format!("block {}:\n   want {w:?}\n   got  {g:?}", w.id));
                }
            }
        }
    }
    if want.styles != got.styles {
        parts.push(format!(
            "styles\n   want {:?}\n   got  {:?}",
            want.styles, got.styles
        ));
    }
    if want.lists != got.lists {
        parts.push(format!(
            "lists\n   want {:?}\n   got  {:?}",
            want.lists, got.lists
        ));
    }
    if want.annotations != got.annotations {
        parts.push(format!(
            "annotations\n   want {:?}\n   got  {:?}",
            want.annotations, got.annotations
        ));
    }
    if want.resolved != got.resolved {
        parts.push("resolved styles".to_owned());
    }
    if want.page_setup != got.page_setup {
        parts.push("page setup".to_owned());
    }
    if parts.is_empty() {
        parts.push("diagnostics or metadata".to_owned());
    }
    parts.join("\n")
}

/// The options the layout-equivalence op draws, for callers that want them.
pub fn layout_options(input: &mut Input<'_>) -> LayoutOptions {
    options(input)
}

/// Whether `FOUNDATIONS_TRACE` asks for a running account on stderr. It only
/// prints; nothing a scenario does depends on it.
fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FOUNDATIONS_TRACE").is_some())
}

fn trace(what: impl FnOnce() -> String) {
    if tracing() {
        eprintln!("{}", what());
    }
}

/// The first lines where two multi-line texts differ, each cut short.
fn first_differences(a: &str, b: &str) -> String {
    let cut = |s: &str| s.chars().take(400).collect::<String>();
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let mut out = Vec::new();
    for n in 0..la.len().max(lb.len()) {
        let (x, y) = (
            la.get(n).copied().unwrap_or(""),
            lb.get(n).copied().unwrap_or(""),
        );
        if x != y {
            out.push(format!(
                "line {n}:\n  source {}\n  copy   {}",
                cut(x),
                cut(y)
            ));
            if out.len() == 3 {
                break;
            }
        }
    }
    out.join("\n")
}

fn inline_ids(state: &State) -> std::collections::BTreeSet<String> {
    state
        .blocks
        .iter()
        .flat_map(|b| b.inline.iter().map(|e| e.id.clone()))
        .collect()
}

impl World<'_> {
    /// Notes that an inline object went away or was replaced since the last
    /// look at replica `i`: its marker stays in the text as a hidden
    /// U+FFFC, the shape of Astra 4 R4.
    fn note_retired(&mut self, i: usize, state: &State) {
        let ids = inline_ids(state);
        if !self.reps[i].inline_ids.is_subset(&ids) {
            self.reps[i].retired = true;
        }
        self.reps[i].inline_ids = ids;
    }
}
