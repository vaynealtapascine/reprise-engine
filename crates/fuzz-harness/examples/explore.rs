//! Runs seeded random scenarios, shrinks every failure and prints it.
//!
//! cargo run -p reprise-fuzz-harness --release --example explore -- FIRST COUNT [LEN]
//!
//! A failure prints the oracle, the op that tripped it and the minimised
//! bytes in hex, ready to be saved under `corpus/` (see docs/fuzzing.md).
//!
//! `NO_MINIMIZE=1` prints failures unshrunk (faster). `FACADE=1` drives the
//! scenarios through the public `reprise` facade instead of the core crates.
//! `FOUNDATIONS=1` drives the wave-1 foundations scenario (styles, lists,
//! inline objects, annotations, batch, copies) through the facade;
//! `TWICE=1` also runs each of those scenarios a second time and compares.

use reprise_fuzz_harness::facade::run_facade_bytes;
use reprise_fuzz_harness::foundations::{run_foundations, run_foundations_twice};
use reprise_fuzz_harness::input::seed_bytes;
use reprise_fuzz_harness::minimize::minimize;
use reprise_fuzz_harness::{Violation, hex, run_bytes};
use std::time::Instant;

#[derive(Clone, Copy)]
enum Mode {
    Core,
    Facade,
    Foundations { twice: bool },
}

fn run(mode: Mode, bytes: &[u8]) -> Option<Violation> {
    match mode {
        Mode::Core => run_bytes(bytes).err(),
        Mode::Facade => run_facade_bytes(bytes).err(),
        Mode::Foundations { twice: false } => run_foundations(bytes).err(),
        Mode::Foundations { twice: true } => run_foundations_twice(bytes).err(),
    }
}

fn shown(violation: &Violation) -> String {
    violation.to_string().chars().take(1800).collect()
}

fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let first = args.first().copied().unwrap_or(0);
    let count = args.get(1).copied().unwrap_or(100);
    let len = args.get(2).copied().unwrap_or(300) as usize;
    let mode = if std::env::var_os("FOUNDATIONS").is_some() {
        Mode::Foundations {
            twice: std::env::var_os("TWICE").is_some(),
        }
    } else if std::env::var_os("FACADE").is_some() {
        Mode::Facade
    } else {
        Mode::Core
    };
    // `HEX=<bytes>` runs one scenario as given (use with FOUNDATIONS_TRACE=1).
    if let Ok(text) = std::env::var("HEX") {
        let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
        let bytes: Vec<u8> = digits
            .chunks(2)
            .filter_map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
            .collect();
        match run(mode, &bytes) {
            Some(v) => println!("{v}"),
            None => println!("passes"),
        }
        return;
    }
    let mut failures = 0;
    let began = Instant::now();
    for seed in first..first + count {
        // A soak ends at its timeout; the last of these lines says how far it got.
        if seed > first && (seed - first).is_multiple_of(100) {
            println!(
                "progress: {} scenarios, {failures} failing, {:?}",
                seed - first,
                began.elapsed()
            );
        }
        let bytes = seed_bytes(seed, len);
        let started = Instant::now();
        let failure = run(mode, &bytes);
        let took = started.elapsed();
        let Some(violation) = failure else {
            if took.as_secs() >= 2 {
                println!("seed {seed}: slow, {took:?}");
            }
            continue;
        };
        failures += 1;
        if std::env::var_os("NO_MINIMIZE").is_some() {
            println!(
                "seed {seed} ({took:?}): {}\n  bytes: {}\n",
                shown(&violation),
                hex(&bytes)
            );
            continue;
        }
        let small = minimize(&bytes, violation.oracle, |b| run(mode, b));
        println!(
            "seed {seed} ({took:?}): {}\n  minimised ({} bytes): {}\n  -> {}\n",
            shown(&violation),
            small.len(),
            hex(&small),
            run(mode, &small).map_or("passes?!".into(), |v| shown(&v))
        );
    }
    println!("{failures} failing of {count} in {:?}", began.elapsed());
}
