#![allow(dead_code)]
use reprise_plugin::{Limits, Manifest, Phase, Plugin};
use std::collections::BTreeSet;

pub fn bytes(body: &str, extra: &str) -> Vec<u8> {
    wat::parse_str(format!(r#"(module
      (memory (export "memory") 2 16)
      (global $heap (mut i32) (i32.const 1024))
      (func (export "reprise_abi_version") (result i32) i32.const 1)
      (func (export "reprise_alloc") (param $len i32) (result i32) (local $ptr i32)
        global.get $heap local.tee $ptr local.get $len i32.add global.set $heap local.get $ptr)
      {extra}
      (func $call (export "reprise_call") (param $op i32) (param $input i32) (param $len i32) (param $out i32) (param $cap i32) (result i32)
        {body}))"#)).unwrap()
}

pub fn load(bytes: &[u8]) -> Plugin {
    load_limits(bytes, Limits::default())
}

pub fn load_limits(bytes: &[u8], limits: Limits) -> Plugin {
    Plugin::load(
        bytes,
        Manifest::new("test", "1", bytes),
        Phase::Layout,
        BTreeSet::new(),
        limits,
    )
    .unwrap()
}
