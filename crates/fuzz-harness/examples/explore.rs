//! Runs seeded random scenarios, shrinks every failure and prints it.
//!
//! cargo run -p reprise-fuzz-harness --release --example explore -- FIRST COUNT [LEN]
//!
//! A failure prints the oracle, the op that tripped it and the minimised
//! bytes in hex, ready to be saved under `corpus/` (see docs/fuzzing.md).
//!
//! `NO_MINIMIZE=1` prints failures unshrunk (faster). `FACADE=1` drives the
//! scenarios through the public `reprise` facade instead of the core crates.

use reprise_fuzz_harness::facade::run_facade_bytes;
use reprise_fuzz_harness::input::seed_bytes;
use reprise_fuzz_harness::minimize::minimize;
use reprise_fuzz_harness::{Violation, hex, run_bytes};
use std::time::Instant;

fn run(facade: bool, bytes: &[u8]) -> Option<Violation> {
    if facade {
        run_facade_bytes(bytes).err()
    } else {
        run_bytes(bytes).err()
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
    let facade = std::env::var_os("FACADE").is_some();
    let mut failures = 0;
    for seed in first..first + count {
        let bytes = seed_bytes(seed, len);
        let started = Instant::now();
        let failure = run(facade, &bytes);
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
        let small = minimize(&bytes, violation.oracle, |b| run(facade, b));
        println!(
            "seed {seed} ({took:?}): {}\n  minimised ({} bytes): {}\n  -> {}\n",
            shown(&violation),
            small.len(),
            hex(&small),
            run(facade, &small).map_or("passes?!".into(), |v| shown(&v))
        );
    }
    println!("{failures} failing of {count}");
}
