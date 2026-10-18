//! The bounded, stable target for the wave-1 foundations fuzz
//! (`docs/fuzzing.md`, "The wave-1 foundations").
//!
//! -   every entry of `corpus/foundations/` runs twice, with identical output;
//! -   a fixed range of seeded scenarios runs, some of them twice;
//! -   a coverage test checks that those scenarios reach every command, every
//!     stored root's feature and every interaction the generators are built
//!     around, so the target can't rot into testing nothing.
//!
//! The default budget stays under a minute in a debug build. Widen it:
//!
//! ```sh
//! REPRISE_FOUNDATIONS_SEEDS=2000 REPRISE_FOUNDATIONS_FIRST=100000 \
//!     cargo test -p reprise-fuzz-harness --test foundations --release
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use reprise_fuzz_harness::foundations::{Report, run_foundations, run_foundations_twice};
use reprise_fuzz_harness::hex;
use reprise_fuzz_harness::input::seed_bytes;

/// Seeded scenarios per run. Each is 300 bytes, up to 36 ops.
const DEFAULT_SEEDS: u64 = 16;
const SEED_BYTES: usize = 300;

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .join("foundations")
}

fn parse_entry(text: &str) -> Vec<u8> {
    let digits: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .flat_map(|l| l.chars().filter(char::is_ascii_hexdigit))
        .collect();
    (0..digits.len() / 2)
        .filter_map(|i| u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    if let Ok(read) = fs::read_dir(corpus_dir()) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "hex")
                && let Ok(text) = fs::read_to_string(&path)
            {
                entries.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    parse_entry(&text),
                ));
            }
        }
    }
    entries.sort();
    entries
}

#[test]
fn every_corpus_scenario_passes_and_repeats() {
    let mut failures = Vec::new();
    for (name, bytes) in corpus() {
        if let Err(violation) = run_foundations_twice(&bytes) {
            failures.push(format!("{name}: {violation}\n  bytes: {}", hex(&bytes)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn random_run(first: u64, count: u64) -> (Vec<String>, BTreeMap<String, usize>) {
    let mut failures = Vec::new();
    let mut totals = BTreeMap::new();
    for seed in first..first + count {
        let bytes = seed_bytes(seed, SEED_BYTES);
        // Every sixth scenario runs twice, for byte-identical output.
        let result: Result<Report, _> = if seed % 6 == 0 {
            run_foundations_twice(&bytes)
        } else {
            run_foundations(&bytes)
        };
        match result {
            Ok(report) => {
                for (key, n) in report.counts {
                    *totals.entry(key).or_default() += n;
                }
            }
            Err(violation) => failures.push(format!(
                "seed {seed} ({SEED_BYTES} bytes): {violation}\n  bytes: {}",
                hex(&bytes)
            )),
        }
    }
    (failures, totals)
}

type Totals = BTreeMap<String, usize>;

/// The default seeds, run once for the whole test binary: the seeded test and
/// the coverage guard both read it, so the scenarios run a single time.
fn default_run() -> &'static (Vec<String>, Totals) {
    static RUN: OnceLock<(Vec<String>, Totals)> = OnceLock::new();
    RUN.get_or_init(|| random_run(0, DEFAULT_SEEDS))
}

#[test]
fn seeded_scenarios_hold_every_oracle() {
    // `REPRISE_FOUNDATIONS_*` widen or move the range; without them this is
    // the default run the coverage guard shares.
    let (failures, _) = if std::env::var_os("REPRISE_FOUNDATIONS_SEEDS").is_some()
        || std::env::var_os("REPRISE_FOUNDATIONS_FIRST").is_some()
    {
        random_run(
            env_number("REPRISE_FOUNDATIONS_FIRST", 0),
            env_number("REPRISE_FOUNDATIONS_SEEDS", DEFAULT_SEEDS),
        )
    } else {
        default_run().clone()
    };
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Commands that must be attempted and must succeed at least once over the
/// default seeds: the whole wave-1 vocabulary, as the facade names it.
const COMMANDS: [&str; 32] = [
    "InsertText",
    "DeleteText",
    "SplitBlock",
    "JoinBlocks",
    "InsertBlock",
    "DeleteBlock",
    "SetStyle",
    "FormatText",
    "DefineStyle",
    "UpdateStyle",
    "RenameStyle",
    "ApplyStyle",
    "SetRole",
    "CreateList",
    "UpdateList",
    "SetListItem",
    "SetListLevel",
    "IndentListItem",
    "RestartList",
    "Enter",
    "BackspaceAtStart",
    "InsertInline",
    "UpdateInline",
    "AddAnnotation",
    "UpdateAnnotation",
    "RemoveAnnotation",
    "SetAlignment",
    "MoveBlock",
    "DeleteStyle",
    "DeleteList",
    "RemoveListItem",
    "ContinueList",
];

#[test]
fn the_seeded_scenarios_reach_the_whole_vocabulary_and_its_interactions() {
    // The coverage guard runs the default seeds (not the widened ones) so it
    // is a fixed bar.
    let (failures, totals) = default_run();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    let n = |key: &str| totals.get(key).copied().unwrap_or(0);
    let mut missing = Vec::new();
    for command in COMMANDS {
        if n(&format!("cmd:{command}")) == 0 {
            missing.push(format!("never attempted: {command}"));
        }
    }
    for key in [
        // The success path of every wave-1 command, not just its refusals.
        "ok:SetRole",
        "ok:CreateList",
        "ok:UpdateList",
        "ok:DeleteList",
        "ok:SetListItem",
        "ok:RemoveListItem",
        "ok:SetListLevel",
        "ok:IndentListItem",
        "ok:RestartList",
        "ok:ContinueList",
        "ok:Enter",
        "ok:BackspaceAtStart",
        "ok:InsertInline",
        "ok:UpdateInline",
        "ok:AddAnnotation",
        "ok:UpdateAnnotation",
        "ok:RemoveAnnotation",
        "ok:DefineStyle",
        "ok:UpdateStyle",
        "ok:RenameStyle",
        "ok:DeleteStyle",
        "ok:ApplyStyle",
        "ok:SetStyle",
        "ok:SplitBlock",
        "ok:JoinBlocks",
        "ok:DeleteText",
        "ok:FormatText",
        "ok:MoveBlock",
        // The ops.
        "op:apply",
        "op:batch",
        "op:batch-fresh",
        "op:batch-stale",
        "op:undo-redo",
        "op:sync",
        "op:copy-paste",
        "op:import-text",
        "op:reference",
        "op:import-copy",
        "op:queries",
        "op:export",
        "check:undo-redo",
        "check:order-independence",
        "ok:paste",
        // What the documents reached.
        "state:role",
        "state:list-item",
        "state:list-definition",
        "state:inline",
        "state:inline-field",
        "state:inline-image",
        "state:link",
        "state:comment",
        "state:custom-style",
        // The interactions the generators lean on.
        "state:atom-in-list-item",
        "state:atom-in-annotated-paragraph",
        "state:cross-paragraph-annotation",
        "state:styled-list-item",
        "paste:same-replica",
        "paste:other-replica",
        "paste:other-document",
    ] {
        if n(key) == 0 {
            missing.push(format!("never reached: {key}"));
        }
    }
    println!("{totals:#?}");
    assert!(missing.is_empty(), "{}\n{totals:#?}", missing.join("\n"));
}
