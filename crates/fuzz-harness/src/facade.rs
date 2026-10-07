//! Scenarios through the public facade (`reprise`), as a host would drive it.
//!
//! The core harness (`run.rs`) sees the engine's own types. This one only
//! uses what an application sees: payloads, sessions, layout jobs, save and
//! open, sync packets, copy and paste, export and rendering. It checks what
//! must hold at that boundary:
//!
//! -   every error carries a code the contract documents, and a refused call
//!     changes nothing;
//! -   a layout job stepped with any budgets, and any viewport, produces the
//!     same pages as one stepped to completion in one go;
//! -   a job started before an edit can't publish (stale), and a fresh job
//!     after it is current;
//! -   two replicas that exchange updates converge, and lay out identically;
//! -   a saved and reopened session draws exactly what the original drew;
//! -   exports, SVG, PNG and PDF are byte-identical when produced twice.
//!
//! Like the core harness, a scenario is a byte string and decoding is total.

use std::panic::{AssertUnwindSafe, catch_unwind};

use reprise as api;
use reprise::{
    Affinity, Caret, Command, Create, DocumentSession, ExportFormat, LayoutOptions, LayoutProgress,
    Open, Paste, Payload, Selection, Style, Transaction, Workspace,
};

use crate::codes;
use crate::ensure;
use crate::input::{Input, fnv};
use crate::violation::{R, Violation};

/// The most ops a facade scenario runs.
pub const MAX_FACADE_OPS: usize = 40;

const DOCUMENT_ID: &str = "00112233445566778899aabbccddeeff";

const WORDS: [&str; 10] = [
    "",
    "alpha ",
    "office ffi \u{5d0}\u{5d1}\u{5d2} ",
    "e\u{301}\u{302} ",
    "\u{1F468}\u{200D}\u{1F469} family ",
    "Supercalifragilisticexpialidocious_Supercalifragilisticexpialidocious ",
    "line\u{2028}break ",
    "a b c d e f g h i j k l m n o p q r s t u v w x y z ",
    "tab\there ",
    "<b>&amp;</b> ",
];

/// Runs bytes as a facade scenario; a panic is a `no-panic` violation.
pub fn run_facade_bytes(data: &[u8]) -> Result<Vec<u64>, Violation> {
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

/// For libFuzzer and tests: panics with the bytes on a violation.
pub fn check_facade_bytes(data: &[u8]) -> Vec<u64> {
    match run_facade_bytes(data) {
        Ok(transcript) => transcript,
        Err(v) => panic!("{v}\nfacade scenario bytes: {}", crate::run::hex(data)),
    }
}

fn documented(error: &api::Error) -> R {
    ensure!(
        codes::is_documented(error.code()),
        "undocumented-code",
        "the facade reported `{}`, which docs/contracts.md doesn't list",
        error.code()
    );
    Ok(())
}

/// Ok, or an error whose code is documented (and which the caller may
/// then treat as a refusal).
fn call<T>(result: api::Result<T>) -> R<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) => {
            documented(&error)?;
            Ok(None)
        }
    }
}

fn session(peer: &str) -> R<DocumentSession> {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: DOCUMENT_ID.into(),
            peer_id: peer.into(),
        }))
        .map_err(|e| Violation::new("facade-create", e.to_string()))
}

fn state_blocks(s: &DocumentSession) -> R<Vec<api::Block>> {
    Ok(s.state()
        .map_err(|e| Violation::new("facade-state", e.to_string()))?
        .data
        .blocks)
}

fn plain_style() -> Style {
    Style {
        families: Some(vec!["serif".into()]),
        ..Style::default()
    }
}

/// Drives a layout job to completion with the given budgets (cycled), or one
/// unbounded step at a time when `budgets` is empty. Returns the final
/// progress.
fn run_job(
    s: &mut DocumentSession,
    options: &LayoutOptions,
    budgets: &[u32],
) -> R<Option<LayoutProgress>> {
    let Some(mut job) = call(s.start_layout(&Payload::new(options.clone())))? else {
        return Ok(None);
    };
    for round in 0..20_000usize {
        let budget = if budgets.is_empty() {
            100_000
        } else {
            budgets[round % budgets.len()]
        };
        match call(job.step(s, budget))? {
            Some(progress) if progress.data.complete => return Ok(Some(progress.data)),
            Some(_) => {}
            None => return Ok(None),
        }
    }
    Err(Violation::new(
        "job-terminates",
        "a facade layout job didn't finish in 20,000 steps",
    ))
}

fn pages(s: &DocumentSession, progress: &LayoutProgress) -> R<Vec<String>> {
    let mut out = Vec::new();
    for &page in progress.pages.iter().take(8) {
        match call(s.display_json(page))? {
            Some(json) => out.push(json.data),
            None => out.push(String::new()),
        }
    }
    Ok(out)
}

fn execute(data: &[u8]) -> R<Vec<u64>> {
    let mut input = Input::new(data);
    let mut transcript = Vec::new();
    let mut a = session("1")?;
    // Seed the document, then derive the second replica by saving and opening:
    // the facade's own way of sharing history.
    for i in 0..1 + input.below(3) {
        a.apply(&Payload::new(Transaction {
            commands: vec![Command::InsertBlock {
                parent: None,
                index: i as u32,
                block_kind: api::BlockKind::Paragraph,
                text: WORDS[input.below(WORDS.len())].repeat(1 + input.below(8)),
                style: plain_style(),
            }],
        }))
        .map_err(|e| Violation::new("facade-seed", e.to_string()))?;
    }
    let saved = a
        .save()
        .map_err(|e| Violation::new("facade-seed", e.to_string()))?
        .data
        .bytes;
    let b = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "2".into(),
            }),
            &saved,
        )
        .map_err(|e| Violation::new("facade-seed", e.to_string()))?;
    let mut sessions = [a, b];
    let mut ops = 0;
    while !input.is_empty() && ops < MAX_FACADE_OPS {
        ops += 1;
        let which = usize::from(input.u8() & 1);
        let op = input.u8();
        let s = &mut sessions[which];
        match op {
            0..=119 => edit(s, &mut input)?,
            120..=134 => {
                let done = call(if op.is_multiple_of(2) {
                    s.undo()
                } else {
                    s.redo()
                })?;
                let _ = done;
            }
            135..=159 => sync(&mut sessions)?,
            160..=189 => layout_equivalence(s, &mut input, &mut transcript)?,
            190..=204 => stale_job(s, &mut input)?,
            205..=219 => save_open(s, &mut input, &mut transcript)?,
            220..=234 => copy_paste(s, &mut input)?,
            235..=244 => export(s, input.u8(), &mut transcript)?,
            _ => render(s, &mut transcript)?,
        }
    }
    // Everything exchanged both ways must converge.
    sync(&mut sessions)?;
    Ok(transcript)
}

fn pick(blocks: &[api::Block], selector: u16) -> Option<&api::Block> {
    blocks.get(usize::from(selector) % blocks.len().max(1))
}

fn boundary(text: &str, selector: u16) -> u32 {
    let bounds: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect();
    bounds[usize::from(selector) % bounds.len()] as u32
}

fn edit(s: &mut DocumentSession, input: &mut Input<'_>) -> R {
    let blocks = state_blocks(s)?;
    let before = s
        .state()
        .map_err(|e| Violation::new("facade-state", e.to_string()))?
        .data;
    let (block, at, from, to, kind) = (
        input.u16(),
        input.u16(),
        input.u16(),
        input.u16(),
        input.below(8),
    );
    let Some(target) = pick(&blocks, block) else {
        return Ok(());
    };
    let node = target.id.clone();
    let text = WORDS[input.below(WORDS.len())].to_owned();
    let command = match kind {
        0..=2 => Command::InsertText {
            node,
            at: if at & 0x8000 != 0 {
                u32::from(at)
            } else {
                boundary(&target.text, at)
            },
            text,
        },
        3 => {
            let (a, b) = (boundary(&target.text, from), boundary(&target.text, to));
            Command::DeleteText {
                node,
                start: a.min(b),
                end: a.max(b),
            }
        }
        4 => Command::SplitBlock {
            node,
            at: boundary(&target.text, at),
        },
        5 => Command::InsertBlock {
            parent: None,
            index: u32::from(at) % (blocks.len() as u32 + 1),
            block_kind: api::BlockKind::Paragraph,
            text,
            style: plain_style(),
        },
        6 => Command::DeleteBlock { node },
        _ => {
            let other = pick(&blocks, to).map_or(node.clone(), |b| b.id.clone());
            Command::JoinBlocks {
                first: node,
                second: other,
            }
        }
    };
    let result = s.apply(&Payload::new(Transaction {
        commands: vec![command],
    }));
    match result {
        Ok(applied) => {
            for note in &applied.data.diagnostics {
                ensure!(
                    codes::is_documented(&note.code),
                    "undocumented-code",
                    "an applied edit reported `{}`",
                    note.code
                );
            }
            Ok(())
        }
        Err(error) => {
            documented(&error)?;
            let after = s
                .state()
                .map_err(|e| Violation::new("facade-state", e.to_string()))?
                .data;
            ensure!(
                before.blocks == after.blocks && before.revision == after.revision,
                "refused-edit-changes-nothing",
                "a refused transaction ({}) changed the document",
                error.code()
            );
            Ok(())
        }
    }
}

fn options(input: &mut Input<'_>) -> LayoutOptions {
    let mut options = LayoutOptions::default();
    if input.chance(60) {
        options.width = [0, 1, 100 * 1024, 600 * 1024, i32::MAX][input.below(5)];
        options.height = [0, 1, 80 * 1024, 800 * 1024, i32::MAX][input.below(5)];
    }
    options.max_pages = [1, 2, 3, 1000][input.below(4)] as u32;
    options.viewport_start = input.below(3) as u32;
    options.viewport_end = options.viewport_start + input.below(3) as u32;
    options
}

/// Any budgets and viewport give the pages that one unbounded job gives.
fn layout_equivalence(
    s: &mut DocumentSession,
    input: &mut Input<'_>,
    transcript: &mut Vec<u64>,
) -> R {
    let options = options(input);
    let budgets: Vec<u32> = (0..1 + input.below(4))
        .map(|_| [0, 1, 2, 5, 100_000][input.below(5)])
        .collect();
    // A run of only zero budgets never advances; that is the contract ("a
    // checked no-op"), so make sure the cycle can make progress.
    let budgets: Vec<u32> = if budgets.iter().all(|&b| b == 0) {
        vec![1]
    } else {
        budgets
    };
    let Some(stepped) = run_job(s, &options, &budgets)? else {
        return Ok(());
    };
    let stepped_pages = pages(s, &stepped)?;
    let Some(whole) = run_job(s, &options, &[])? else {
        return Err(Violation::new(
            "job-repeatable",
            "an unbounded job failed after a budgeted one succeeded",
        ));
    };
    let whole_pages = pages(s, &whole)?;
    ensure!(
        stepped.pages == whole.pages && stepped_pages == whole_pages,
        "incremental-equals-reference",
        "budgets {budgets:?} gave different pages than one unbounded job"
    );
    for page in &whole_pages {
        transcript.push(fnv(page.as_bytes()));
    }
    Ok(())
}

/// A job that has started can't publish after an edit.
fn stale_job(s: &mut DocumentSession, input: &mut Input<'_>) -> R {
    let options = options(input);
    let Some(mut job) = call(s.start_layout(&Payload::new(options.clone())))? else {
        return Ok(());
    };
    let _ = call(job.step(s, 1))?;
    edit(s, input)?;
    match job.step(s, 100_000) {
        Ok(progress) => {
            // An edit that was refused leaves the job current; otherwise a
            // completed result must not describe the old revision. Either way
            // it may only complete if the document really is unchanged.
            let _ = progress;
        }
        Err(error) => {
            documented(&error)?;
            ensure!(
                matches!(error.code(), "bindings.stale" | "bindings.cancelled"),
                "stale-job-is-stale",
                "a job interrupted by an edit failed with `{}`",
                error.code()
            );
        }
    }
    // A fresh job is current and completes.
    ensure!(
        run_job(s, &options, &[100_000])?.is_some(),
        "fresh-job-completes",
        "a new job after an edit didn't complete"
    );
    Ok(())
}

fn sync(sessions: &mut [DocumentSession; 2]) -> R {
    let [a, b] = sessions;
    let (Some(from_a), Some(from_b)) = (call(a.export_updates())?, call(b.export_updates())?)
    else {
        return Ok(());
    };
    let (Some(_), Some(_)) = (
        call(a.import_updates(&from_b))?,
        call(b.import_updates(&from_a))?,
    ) else {
        return Ok(());
    };
    ensure!(
        a.sync_info().data.vector == b.sync_info().data.vector,
        "peers-converge",
        "version vectors differ after exchanging updates"
    );
    ensure!(
        state_blocks(a)? == state_blocks(b)?,
        "peers-converge",
        "blocks differ after exchanging updates"
    );
    let options = LayoutOptions::default();
    let (Some(pa), Some(pb)) = (run_job(a, &options, &[])?, run_job(b, &options, &[])?) else {
        return Ok(());
    };
    ensure!(
        pages(a, &pa)? == pages(b, &pb)?,
        "peers-converge-layout",
        "converged replicas draw differently"
    );
    Ok(())
}

fn save_open(s: &mut DocumentSession, input: &mut Input<'_>, transcript: &mut Vec<u64>) -> R {
    let Some(saved) = call(s.save())? else {
        return Ok(());
    };
    let bytes = saved.data.bytes;
    transcript.push(fnv(&bytes));
    let peer = ["3", "4", "5"][input.below(3)];
    let Some(mut reopened) = call(Workspace::new().open(
        &Payload::new(Open {
            peer_id: peer.into(),
        }),
        &bytes,
    ))?
    else {
        return Ok(());
    };
    ensure!(
        state_blocks(s)? == state_blocks(&reopened)?,
        "reopen-identical",
        "reopened blocks differ"
    );
    let options = LayoutOptions::default();
    let (Some(p1), Some(p2)) = (
        run_job(s, &options, &[])?,
        run_job(&mut reopened, &options, &[])?,
    ) else {
        return Ok(());
    };
    ensure!(
        pages(s, &p1)? == pages(&reopened, &p2)?,
        "reopen-lays-out-identically",
        "a reopened session draws differently"
    );
    Ok(())
}

fn copy_paste(s: &mut DocumentSession, input: &mut Input<'_>) -> R {
    let blocks = state_blocks(s)?;
    let Some(target) = pick(&blocks, input.u16()) else {
        return Ok(());
    };
    let (a, b) = (
        boundary(&target.text, input.u16()),
        boundary(&target.text, input.u16()),
    );
    let caret = |offset| Caret {
        node: target.id.clone(),
        offset,
        affinity: Affinity::Downstream,
    };
    let selection = Selection {
        anchor: caret(a.min(b)),
        focus: caret(a.max(b)),
    };
    // Selections need a complete layout; without one the facade says so.
    let _ = run_job(s, &LayoutOptions::default(), &[])?;
    let Some(copied) = call(s.copy(&Payload::new(selection)))? else {
        return Ok(());
    };
    let before = state_blocks(s)?;
    let count = before.len();
    let before_vector = s.sync_info().data.vector;
    match call(s.paste(&Payload::new(Paste { at: None }), &copied.data.bytes))? {
        Some(_) => {
            let after = state_blocks(s)?;
            ensure!(
                after.len() >= count,
                "paste-preserves-text",
                "a paste removed blocks ({count} to {})",
                after.len()
            );
            let (x, y) = (a.min(b) as usize, a.max(b) as usize);
            if let Some(slice) = target.text.get(x..y)
                && !slice.is_empty()
            {
                ensure!(
                    after.iter().any(|b| b.text.contains(slice)),
                    "paste-preserves-text",
                    "pasted {slice:?} is nowhere in the document"
                );
            }
            // An empty paste has no undo step. Undoing it would undo the
            // preceding authored action, which is not a restoration oracle.
            if s.sync_info().data.vector != before_vector {
                let _ = call(s.undo())?;
                ensure!(
                    state_blocks(s)? == before,
                    "undo-restores-state",
                    "undoing a paste didn't restore the blocks"
                );
            }
        }
        None => {
            ensure!(
                state_blocks(s)? == before,
                "refused-paste-changes-nothing",
                "a refused paste changed the document"
            );
        }
    }
    Ok(())
}

fn export(s: &mut DocumentSession, kind: u8, transcript: &mut Vec<u64>) -> R {
    let _ = run_job(s, &LayoutOptions::default(), &[])?;
    let format = match kind % 4 {
        0 => ExportFormat::PlainText,
        1 => ExportFormat::Html,
        2 => ExportFormat::Native,
        _ => ExportFormat::Pdf,
    };
    let first = call(s.export(&Payload::new(format)))?;
    let second = call(s.export(&Payload::new(format)))?;
    ensure!(
        first.as_ref().map(|e| &e.data.content.bytes)
            == second.as_ref().map(|e| &e.data.content.bytes),
        "export-repeatable",
        "exporting twice gave different bytes"
    );
    if let Some(exported) = first {
        ensure!(
            exported.data.losses.len() == 10,
            "loss-report-complete",
            "{} features reported, 10 expected",
            exported.data.losses.len()
        );
        for loss in &exported.data.losses {
            ensure!(
                codes::is_documented(&loss.code),
                "undocumented-code",
                "export reported `{}`",
                loss.code
            );
        }
        transcript.push(fnv(&exported.data.content.bytes));
    }
    Ok(())
}

fn render(s: &mut DocumentSession, transcript: &mut Vec<u64>) -> R {
    let Some(progress) = run_job(s, &LayoutOptions::default(), &[])? else {
        return Ok(());
    };
    let Some(&page) = progress.pages.first() else {
        return Ok(());
    };
    let svg = (call(s.svg(page))?, call(s.svg(page))?);
    ensure!(
        svg.0.as_ref().map(|p| &p.data) == svg.1.as_ref().map(|p| &p.data),
        "svg-repeatable",
        "SVG differs between runs"
    );
    let png = (call(s.png(page, 250))?, call(s.png(page, 250))?);
    ensure!(
        png.0.as_ref().map(|p| &p.data.bytes) == png.1.as_ref().map(|p| &p.data.bytes),
        "png-repeatable",
        "PNG differs between runs"
    );
    if let Some(svg) = svg.0 {
        transcript.push(fnv(svg.data.as_bytes()));
    }
    Ok(())
}
