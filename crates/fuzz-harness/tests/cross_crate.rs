//! The stable, bounded cross-crate fuzz target (decision 39).
//!
//! -   every scenario in `corpus/` (a minimised regression for each bug the
//!     fuzzer found, plus hand-picked seeds) runs twice, with identical output;
//! -   a fixed range of seeded random scenarios runs, a few of them twice;
//! -   a coverage test checks that the random scenarios actually reach the
//!     features they are meant to exercise, so the target can't rot into
//!     testing nothing.
//!
//! The default budget keeps this under a minute in a debug build. Widen it with
//! `REPRISE_FUZZ_SEEDS=<count>` and `REPRISE_FUZZ_FIRST=<first seed>`; the
//! `explore` example shrinks any failure to a corpus entry.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use reprise_fuzz_harness::input::seed_bytes;
use reprise_fuzz_harness::{Report, Scenario, hex, run_bytes, run_twice};

/// Default number of seeded scenarios. Each is 300 bytes, about 40 ops.
const DEFAULT_SEEDS: u64 = 40;
const SEED_BYTES: usize = 300;

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

/// Corpus entries are hex text so they diff and review well. Lines starting
/// with `#` are comments: the oracle that failed and where it was found.
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
    let mut dirs = vec![corpus_dir()];
    while let Some(dir) = dirs.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "hex") {
                let name = path
                    .strip_prefix(corpus_dir())
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                if let Ok(text) = fs::read_to_string(&path) {
                    entries.push((name, parse_entry(&text)));
                }
            }
        }
    }
    entries.sort();
    entries
}

#[test]
fn corpus_entries_decode_and_are_not_empty() {
    let entries = corpus();
    assert!(!entries.is_empty(), "the regression corpus is missing");
    for (name, bytes) in entries {
        assert!(!bytes.is_empty(), "{name} is empty");
        // Total decoding: nothing in the corpus can be malformed.
        let _ = Scenario::decode(&bytes);
    }
}

#[test]
fn every_corpus_scenario_passes_and_repeats() {
    let mut failures = Vec::new();
    for (name, bytes) in corpus() {
        if let Err(violation) = run_twice(&Scenario::decode(&bytes)) {
            failures.push(format!("{name}: {violation}\n  bytes: {}", hex(&bytes)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn random_run(first: u64, count: u64) -> (Vec<String>, BTreeMap<&'static str, usize>) {
    let mut failures = Vec::new();
    let mut totals = BTreeMap::new();
    for seed in first..first + count {
        let bytes = seed_bytes(seed, SEED_BYTES);
        // Every fifth scenario also runs twice for byte-identical output.
        let result: Result<Report, _> = if seed % 5 == 0 {
            run_twice(&Scenario::decode(&bytes))
        } else {
            run_bytes(&bytes)
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

#[test]
fn seeded_random_scenarios_hold_every_oracle() {
    let count = env_number("REPRISE_FUZZ_SEEDS", DEFAULT_SEEDS);
    let first = env_number("REPRISE_FUZZ_FIRST", 0);
    let (failures, _) = random_run(first, count);
    assert!(
        failures.is_empty(),
        "{} of {count} scenarios failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn random_scenarios_reach_the_features_they_exist_to_test() {
    // A different range from the test above, so the two run different work.
    let (failures, totals) = random_run(10_000, 30);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    for key in [
        "edit.applied",
        "edit.refused",
        "undo",
        "redo",
        "sync",
        "copy.ok",
        "paste.ok",
        "import.ok",
        "layout",
        "render",
        "export",
        "save",
        "navigate",
        "template",
        "define-style",
        "engine",
        "epilogue",
    ] {
        assert!(
            totals.get(key).copied().unwrap_or(0) > 0,
            "no random scenario exercised {key}: {totals:?}"
        );
    }
}

/// The same idea through the public facade: sessions, jobs, sync packets,
/// save/open, copy/paste, export and rendering. See `facade.rs`.
#[test]
fn facade_scenarios_hold_every_oracle() {
    let count = env_number("REPRISE_FUZZ_FACADE_SEEDS", 24);
    let mut failures = Vec::new();
    for seed in 0..count {
        let bytes = seed_bytes(50_000 + seed, 120);
        if let Err(violation) = reprise_fuzz_harness::facade::run_facade_bytes(&bytes) {
            failures.push(format!(
                "facade seed {seed}: {violation}
  bytes: {}",
                hex(&bytes)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{}",
        failures.join(
            "

"
        )
    );
}
