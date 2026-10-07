//! As `scenario`, and the scenario runs twice: display JSON, SVG, PNG, PDF,
//! exports and saved packages must be byte-identical both times.
#![no_main]

use libfuzzer_sys::fuzz_target;
use reprise_fuzz_harness::{Scenario, run_twice};

fuzz_target!(|data: &[u8]| {
    let scenario = Scenario::decode(data);
    if let Err(violation) = run_twice(&scenario) {
        panic!("{violation}\nscenario: {scenario:#?}");
    }
});
