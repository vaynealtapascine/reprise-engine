//! As `foundations`, and the scenario runs twice: states, pages, exports and
//! saved packages must be byte-identical both times.
#![no_main]

use libfuzzer_sys::fuzz_target;
use reprise_fuzz_harness::foundations::run_foundations_twice;

fuzz_target!(|data: &[u8]| {
    if let Err(violation) = run_foundations_twice(data) {
        panic!(
            "{violation}\nfoundations scenario bytes: {}",
            reprise_fuzz_harness::hex(data)
        );
    }
});
