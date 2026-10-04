(module
  (memory (export "memory") 2 16)
  (global $heap (mut i32) (i32.const 1024))
  (func (export "reprise_abi_version") (result i32) i32.const 1)
  (func (export "reprise_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap local.tee $ptr local.get $len i32.add global.set $heap
    local.get $ptr)
  (func (export "reprise_call") (param i32 i32 i32 i32 i32) (result i32)
    (loop $forever br $forever)
    i32.const 0))
