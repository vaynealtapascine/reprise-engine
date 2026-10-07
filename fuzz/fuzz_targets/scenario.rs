//! Every byte string is a scenario; every oracle is checked after every op.
//! A violation panics with the bytes' hex, the decoded scenario and the op.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    reprise_fuzz_harness::check_bytes(data);
});
