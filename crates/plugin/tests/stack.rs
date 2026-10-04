//! Orchestrator review: with the default fuel, runaway recursion is caught
//! by fuel or the call-stack limit early. Here fuel is at its hard ceiling (100 million),
//! frames are fat (2,000 locals each), and recursion goes through a function
//! table. The host thread gets only 512 KiB of stack, half of a typical WASM
//! or browser worker stack. wasmi must trap on its own stack limit rather
//! than recursing on the host's, and do so the same way every time.
//!
//! Measured during review: recursion depth doesn't change the host stack
//! wasmi needs, so it never recurses on the host stack. Loading and calling
//! a plugin has a fixed host-stack cost under 384 KiB in an unoptimised debug
//! build, and works in 256 KiB in release.

mod common;

use common::{bytes, load_limits};
use reprise_plugin::{CallContext, Limits};

#[test]
fn deep_fat_and_indirect_recursion_trap_without_touching_the_host_stack() {
    let locals = "(local ".to_string() + &"i64 ".repeat(2000) + ")";
    let extra = format!(
        r#"(type $t (func (param i32) (result i32)))
        (table 2 2 funcref)
        (elem (i32.const 0) $a $b)
        (func $a (type $t) (param $n i32) (result i32) {locals}
          local.get $n i32.const 1 i32.add i32.const 1 call_indirect (type $t))
        (func $b (type $t) (param $n i32) (result i32) {locals}
          local.get $n i32.const 1 i32.add i32.const 0 call_indirect (type $t))"#
    );
    let module = bytes("i32.const 0 call $a", &extra);
    let limits = Limits {
        // The hard ceiling: far more than recursion needs to hit the stack limit.
        fuel: 100_000_000,
        ..Limits::default()
    };
    let outcome = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            let plugin = load_limits(&module, limits);
            let first = plugin.call(0, &[], &CallContext::default()).unwrap_err();
            for _ in 0..3 {
                assert_eq!(
                    plugin.call(0, &[], &CallContext::default()).unwrap_err(),
                    first
                );
            }
            first.code.to_string()
        })
        .unwrap()
        .join()
        .expect("the host thread must not overflow its stack");
    assert!(
        outcome == "plugin.limit" || outcome == "plugin.trap",
        "unexpected code {outcome}"
    );
}
