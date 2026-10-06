//! Runs seeded random scenarios, shrinks every failure and prints it.
//!
//! cargo run -p reprise-fuzz-harness --release --example explore -- FIRST COUNT [LEN]
//!
//! A failure prints the oracle, the op that tripped it and the minimised
//! bytes in hex, ready to be saved under `corpus/` (see docs/fuzzing.md).

use reprise_fuzz_harness::input::seed_bytes;
use reprise_fuzz_harness::minimize::minimize;
use reprise_fuzz_harness::{hex, run_bytes};
use std::time::Instant;

fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let first = args.first().copied().unwrap_or(0);
    let count = args.get(1).copied().unwrap_or(100);
    let len = args.get(2).copied().unwrap_or(300) as usize;
    let mut failures = 0;
    for seed in first..first + count {
        let bytes = seed_bytes(seed, len);
        let started = Instant::now();
        let result = run_bytes(&bytes);
        let took = started.elapsed();
        if let Err(violation) = result {
            failures += 1;
            let shown: String = violation.to_string().chars().take(1800).collect();
            if std::env::var_os("NO_MINIMIZE").is_some() {
                println!(
                    "seed {seed} ({took:?}): {shown}
  bytes: {}
",
                    hex(&bytes)
                );
                continue;
            }
            let small = minimize(&bytes, violation.oracle, |b| run_bytes(b).err());
            let small_violation = run_bytes(&small).err();
            println!(
                "seed {seed} ({took:?}): {violation}\n  minimised ({} bytes): {}\n  -> {}\n",
                small.len(),
                hex(&small),
                small_violation.map_or("passes?!".into(), |v| v
                    .to_string()
                    .chars()
                    .take(1800)
                    .collect::<String>())
            );
        } else if took.as_secs() >= 2 {
            println!("seed {seed}: slow, {took:?}");
        }
    }
    println!("{failures} failing of {count}");
}
