//! The wave-1 foundations through the facade: styles, lists, inline objects,
//! annotations, batch and copies. Every byte string is a scenario; a violation
//! panics with the bytes' hex (`docs/fuzzing.md`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    reprise_fuzz_harness::foundations::check_foundations_bytes(data);
});
