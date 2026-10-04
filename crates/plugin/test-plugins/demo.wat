;; Reproducible ABI v1 example: double a length, taper a paragraph, acknowledge a relation.
(module
  (memory (export "memory") 2 16)
  (global $heap (mut i32) (i32.const 1024))
  (func (export "reprise_abi_version") (result i32) i32.const 1)
  (func (export "reprise_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap local.tee $ptr local.get $len i32.add global.set $heap
    local.get $ptr)
  (func (export "reprise_call")
    (param $op i32) (param $input i32) (param $len i32) (param $out i32) (param $cap i32)
    (result i32) (local $start i32) (local $end i32) (local $inset i32)
    local.get $op i32.const 1 i32.eq
    if
      local.get $out i32.const 1 i32.store
      local.get $out local.get $input i32.load offset=8 i32.const 2 i32.mul i32.store offset=4
      local.get $out i32.const 0 i32.store offset=8
      i32.const 12 return
    end
    local.get $op i32.const 2 i32.eq
    if
      local.get $input i32.load offset=16 local.set $start
      local.get $input i32.load offset=20 local.set $end
      ;; Three steps of inset; all arithmetic stays inside the frame's integer room.
      local.get $end local.get $start i32.sub i32.const 8 i32.div_s
      local.get $input i32.load i32.const 3 i32.rem_u i32.mul local.set $inset
      local.get $out i32.const 0 i32.store
      local.get $out i32.const 1 i32.store offset=4
      local.get $out local.get $start local.get $inset i32.add i32.store offset=8
      local.get $out local.get $end local.get $inset i32.sub i32.store offset=12
      i32.const 16 return
    end
    local.get $op i32.const 3 i32.eq
    if
      local.get $out i32.const 1 i32.store
      i32.const 4 return
    end
    i32.const -1))
