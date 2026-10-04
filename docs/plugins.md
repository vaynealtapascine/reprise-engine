# Sandboxed plugins: ABI v1

Decisions 04, 19, 36, 37 and 38. The only backend is **wasmi 2.0.0**, pinned
in Cargo.lock, using eager compilation, portable dispatch and its deterministic
floating-point profile. There is no WASI, clock, randomness, filesystem, network,
thread or ambient document access. SIMD (including relaxed SIMD), threads,
shared memory, memory64, custom pages, multiple memories and start functions
are refused. Ordinary internal f32/f64 arithmetic is supported; wasmi
canonicalises arithmetic NaNs. Literal/reinterpreted NaN bits are deterministic
authored constants. No float type crosses the layout ABI.

## Identity and configuration

The host supplies `Manifest`: descriptive UTF-8 name and version, ABI version 1,
SHA-256 of the exact module bytes, declared imports and function signatures.
Loading recomputes and verifies the hash before any code executes. Names and
versions are nonempty and at most 256 bytes. Host declarations are pinned
configuration, not self-authenticating claims by a module.

`Envelope` records ABI version, identity, runtime version, phase, grants, declared imports,
limits, and each function's name, operation and complete signature. Use
`Engine::install_plugin_functions`, `install_plugin_geometry` and
`install_plugin_relation` to retain these pins in `Engine.plugins.envelope()`.
Hosts include this envelope alongside existing schemas, composer settings and
engine configuration in package cache tags. Frozen LayoutSnapshot and package
formats are unchanged; the envelope is additive derived configuration.

## Required core-WASM exports

Exactly one ordinary 32-bit linear memory, exported as `memory`, with explicit
maximum. Optional single table also requires an explicit maximum. All declared
resources, including unexported ones, are inspected before compilation. A module
must export these function types:

```text
reprise_abi_version() -> i32                         // must return 1
reprise_alloc(len: i32) -> i32                       // pointer, or negative failure
reprise_call(op: i32, input: i32, input_len: i32,
             output: i32, output_capacity: i32) -> i32
```

Each call creates a fresh store, instance, globals, table and memory. The module
is compiled once; instances are never reused. ABI version, both allocations and
the operation share one fuel budget. Allocations remain live until the call
ends; no free is needed. Input and output must be disjoint in-bounds regions,
with nonnegative pointers. The host initializes output to zero and copies input
after both allocations. Buffers have byte alignment 1. The caller supplies the
full configured output capacity. The returned nonnegative length must fit that
capacity; only those bytes are interpreted.

Status returns: `-1` unsupported operation; `-2` invalid input; `-3` resource or
allocation refusal; `-4` capability refusal. Other negative returns are malformed.
The first two and other negatives produce `plugin.result`; `-3` produces
`plugin.limit`, and `-4` produces `plugin.capability`. Any operation can trap;
traps never escape as Rust panics. WASM memory/table growth beyond the module's
maximum normally returns -1 inside WASM; authors propagate allocation failure
as status -3. A handled growth failure is valid execution, and a loop that
ignores failures still runs out of fuel. Fuel exhaustion cannot be caught by WASM.

## Integer records and style functions

All words are little-endian 32-bit. A Length is signed 1/1024 pt; Fixed is
signed 16.16. Offsets and lengths are UTF-8 bytes. u32 operation IDs and handles
use the i32 bit pattern; pointers and byte counts must be nonnegative.

A value is exactly 12 bytes: `(tag, a, b)`, three signed words:

| Tag | Value | a | b |
| --- | --- | --- | --- |
| 1 | Length | raw Length | 0 |
| 2 | Number | raw Fixed | 0 |
| 3 | Percentage | raw Fixed percent points (50% = 50 × 65536) | 0 |
| 4 | Ratio | raw Fixed numerator | nonzero raw Fixed denominator |

Other tags (including attempted float/NaN records), nonzero reserved fields,
zero ratio denominators, extra/truncated bytes and wrong dimensions are refused.
Every i32 Length/Fixed raw value is representable; plugin integer overflow
wraps under WASM semantics, so plugin authors must implement saturating/checked
arithmetic when calculating lengths. The host never converts layout floats.

Function input: u32 argument count, then count value records, at most 256.
Output: one value record matching the declarative return dimension. Signatures
declare fixed parameters and an optional variadic dimension. Registry installation
is atomic. An absent/refused plugin is not registered: `style.unknown-function`
skips the style layer. An installed function that fails reports the frozen
`style.function-failed`, with the underlying stable `plugin.*` code in its
message, and skips the layer. `PluginFunction::evaluate` also exposes the typed
plugin Note directly. No mutable diagnostic side channel is used.

## Geometry

Geometry runs through `PluginGeometry` and `GeometryComposer`, wrapping an
existing conforming composer. Input words:

```text
u32 line; i32 block_offset; i32 line_height; u32 room_count;
room_count × (i32 start, i32 end);
u32 previous_count;
previous_count × (u32 line, i32 block_offset, i32 height,
                  i32 available_start, i32 available_end,
                  u32 source_start_byte, u32 source_end_byte);
```

At most 64 room intervals and 4096 previous fragments. Requests are also bounded
by buffer_bytes. Previous source ranges and query positions are paragraph-local.
The frame's existing Skip/End answers take precedence and do not invoke the plugin.

Output: `(tag, n)` followed by records only for Room:

| Tag | Meaning | n | Following bytes |
| --- | --- | --- | --- |
| 0 | Room | 0..64 count | count × (signed start, signed end) |
| 1 | Skip | signed next block offset, strictly greater than query offset | none |
| 2 | End | 0 | none |

Room intervals must have positive width, be ordered, not overlap, and each be
contained in one existing room interval. They cannot bridge exclusions. An empty
Room means skip one line height under the composer contract. Invalid bytes,
traps, unavailable modules and exhausted budgets return the exact original frame
room, with one Note per distinct failure per composition. The host's existing
composer bounds nonprogress and endless skips.

## Relation resolve-and-report

`RelationBinding` associates a declared SchemaId with an operation and optional
loaded plugin. The existing resolver handles every target class, tombstones,
rebinding, cardinality, ambiguity and historical snapshots first. Deleted or
owner-deleted relations do not invoke the plugin.

Input: u32 target count, then each `(u32 role_byte_len, UTF-8 role bytes,
u32 status, u32 resolution_kind, u32 subject_count)`, with no padding. At most
256 targets, 256 bytes per role and the configured buffer limit. Roles arrive
in resolver order. Status: Valid=0, Rebound=1, Ambiguous=2, Missing=3,
OwnerDeleted=4, Deleted=5. Resolution: absent=0, Node=1, Range=2, Line=3,
Nodes=4, Lines=5, Frame=6, Page=7, Snapshot=8. Single resolutions count 1;
collections give their count; absent counts 0. Actual engine IDs are not exposed.

Output: exactly one word, 0 (decline) or 1 (acknowledge). Acknowledgement only
applies if every target is Valid or Rebound. This behaviour resolves/reports;
it cannot place blocks or mutate the document. Missing/refused/trapping/budgeted
plugins leave targets reported, `applied=false`, a `plugin.*` Warning and the
existing `relation.not-applied` Info. A valid decline reports not-applied Info.

## Capabilities and editing

Imports must be declared **and** independently granted. Unknown modules/imports,
wrong function types, imported memories/tables/globals and undeclared imports
are refused before execution. Unused declared capabilities are allowed. Layout
grants support only ReadText; InsertText requires Editing phase and cannot be
installed as a pure function or geometry provider. There are no I/O grants in v1.

```text
reprise_v1.read_text(handle: i32, dest: i32, capacity: i32) -> i32
reprise_v1.insert_text(handle: i32, at_byte: i32, src: i32, len: i32) -> i32
```

ReadText exposes only immutable UTF-8 strings in CallContext.texts, keyed by
caller-assigned u32 handles. Returns copied byte length, -1 unknown handle, or
-2 insufficient capacity. Invalid pointer traps with plugin.result. Text is
copied exactly; no terminator is written. Handles have no ambient meaning.

InsertText stages UTF-8 insertion requests on explicitly granted editable
handles, returns 0, or traps on invalid authority, pointer, offset, UTF-8 or
limits. At most 256 insertions, with total bytes bounded by buffer_bytes.
Offsets are checked against the real document by the editing kernel, in command
order. The successful editing operation must return zero bytes. `Plugin::edit`
then calls `EditKernel::apply` once: the host implements it using one
`reprise_edit::Transaction` and `Editor::apply`. This keeps kernel validation,
undo, identity and collaborative rules authoritative without creating an
edit → layout → plugin → edit dependency cycle. A trap, negative status,
nonempty result or kernel rejection applies nothing. `Plugin::call` exposes
staged requests for inspection only, and never changes authored state.

Each host import costs 32 fuel for lookup/validation, then another 32 plus one
fuel per copied byte if a copy occurs. The immutable context contains at most 256 texts,
256 editable handles and buffer_bytes total text bytes. All execution authority
and the immutable context are explicit inputs.

## Bounds, diagnostics and reproduction

Defaults: 100,000 fuel/call; 16 memory pages (64 KiB each); 1024 table elements;
65,536 bytes per input/output/context/staged-edit buffer. Hard host maxima:
100,000,000 fuel, 256 pages, 65,536 table elements, 1 MiB buffers/module,
4096 types/functions/globals/exports and locals per function, 256 structured
block nesting depth, 128 call depth,
65,536 operand stack slots. There is one memory/table/instance. Eager validation
and the module byte limit bound preparation; execution is fuel-metered.
Proposal rejection is also checked by an explicit wasmparser validator, so a
downstream crate enabling extra wasmi features cannot broaden the sandbox policy.

| Code | Severity | Fallback |
| --- | --- | --- |
| plugin.invalid | Warning | refuse invalid/unsupported module |
| plugin.abi | Warning | refuse incompatible exports/version/start function |
| plugin.hash | Warning | refuse changed pinned bytes |
| plugin.capability | Warning | refuse ungranted authority |
| plugin.limit | Warning | skip/substitute extension exceeding resource bounds |
| plugin.fuel | Warning | skip/substitute extension exceeding call fuel |
| plugin.trap | Warning | skip/substitute trapping extension |
| plugin.result | Warning | skip/substitute malformed output |
| plugin.unavailable | Warning | substitute missing geometry/relation plugin |

Warning means a style layer or requested behaviour differs while ordinary text
continues to lay out. Existing unknown-function and not-applied codes remain
unchanged. Kernel errors retain their existing Error severity and Note.

`Runtime` is private and implemented only by WasmiRuntime. A future wasmtime
backend must pass the same ABI/capability/resource/NaN/state-isolation conformance
tests and agree on fuel semantics before it is usable for reproducible layout.
No plugin composer is implemented; the geometry adapter delegates to existing
composers. Shaping remains native first-party code because it is too hot a path
for v1 sandbox calls.

Test plugins live in `crates/plugin/test-plugins`: WAT source plus checked-in
deterministically generated WASM. Regenerate explicitly with
`cargo run -p reprise-plugin --example compile_test_plugins`; tests compare exact
bytes. CI needs no extra WASM toolchain. Hostile tests cover malformed modules,
loops, recursion, memory/table bombs, imports, SIMD/threads, NaNs, invalid
geometry/value records, budgets, instance isolation and editing atomicity.

Implementation references: [wasmi source and license](https://github.com/wasmi-labs/wasmi),
[pinned config source](https://docs.rs/crate/wasmi/2.0.0/source/src/engine/config.rs).

## Added dependency licenses

Checked against the downloaded packages' Cargo.toml license declarations.
MIT alternatives are selected where offered. No incompatible license is added.

| Added package | Version | Available license |
| --- | --- | --- |
| wasmi (direct) | 2.0.0 | MIT / Apache-2.0 |
| wasmparser (direct preflight) | 0.228.0 | MIT OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception |
| wat (direct, development only) | 1.261.0 | MIT OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception |
| wasmi_collections, wasmi_core, wasmi_ir | 2.0.0 each | MIT / Apache-2.0 |
| string-interner | 0.19.0 | MIT / Apache-2.0 |
| hashbrown | 0.15.5 | MIT OR Apache-2.0 |
| foldhash | 0.1.5 | Zlib |
| leb128fmt | 0.1.0 | MIT OR Apache-2.0 |
| unicode-width | 0.2.2 | MIT OR Apache-2.0 |
| wasm-encoder, wasmparser (WAT transitive) | 0.261.0 each | MIT OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception |
| wast | 261.0.0 | MIT OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception |
