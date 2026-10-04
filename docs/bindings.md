# Reprise application bindings, API v1

`reprise` is the one Rust dependency for a Tauri backend. `reprise-wasm` wraps
that same facade for browser workers. Both crates start at **0.1.0**. Neither
owns a UI, filesystem, font discovery, network transport or collaboration server.
The external contract is also recorded in [contracts.md](contracts.md).

## Object and payload model

A `Workspace` creates or opens independent `DocumentSession` replicas. Each
session owns an editor, engine configuration, resources, incremental memos and
its last complete layout. A native `LayoutJob` is an owned continuation that
is stepped against its originating session. A loaded `Plugin` is another
opaque native object. WASM exposes `Workspace` and `DocumentSession` classes;
jobs/plugins have monotone session-local numeric handles and explicit release.
Call `free()` when disposing the WASM classes.

All standalone data objects use `Payload<T> { version: 1, data: T }`, including
errors, state, clipboard buffers, sync updates and display results. Nested
records inherit that version. Raw byte arguments are opaque channels paired
with versioned metadata; they are not JSON arrays in JavaScript. `Bytes.bytes`
is `Vec<u8>` in Rust and **Uint8Array** in JS. All returned arrays are owned
copies; a JS client may transfer their buffers without detaching engine memory.
Native JSON IPC may encode those byte buffers as arrays; a Tauri host should
prefer its binary IPC channel for large resources.

Document IDs are host-assigned 128-bit identities written as 32 lowercase hex
digits. Preserve them across saves and replicas, and choose a new identity for
an independent copy. Peer IDs are canonical decimal strings in `0..u64::MAX`
(the upper endpoint is excluded because the CRDT reserves it). Concurrent
replicas of one document must have distinct peers. Node/range/relation IDs are
opaque canonical strings; never derive meaning from their apparent spelling.
Revisions/frontiers and sorted version vectors contain peer strings and signed
32-bit counters, without JS precision loss. A frontier is not a version vector.

Geometry uses signed integer units of **1/1024 point**. Display matrices use
signed integer **16.16** coefficients and integer length translations. Caret
positions and glyph source ranges are **UTF-8 byte offsets**, not UTF-16 indexes.
Convert JS string offsets explicitly, for example with `TextEncoder`. Movement
uses the core's grapheme/word segmentation, bidi ordering and transforms.

## Native editor loop

```rust
use reprise::{Command, LayoutOptions, Open, Payload, Transaction, Workspace};

fn example(bytes: &[u8]) -> reprise::Result<()> {
    let workspace = Workspace::new();
    let mut document = workspace.open(&Payload::new(Open { peer_id: "2".into() }), bytes)?;
    let mut job = document.start_layout(&Payload::new(LayoutOptions::default()))?;
    loop {
        let progress = job.step(&mut document, 64)?.data;
        for page in progress.pages {
            let drawing = job.display_page(&document, page)?;
            // Send drawing with its revision/generation and settled/complete flags.
            let _ = drawing;
        }
        if progress.complete { break; }
    }
    let state = document.state()?.data;
    if let Some(block) = state.blocks.first() {
        document.apply(&Payload::new(Transaction { commands: vec![Command::InsertText {
            node: block.id.clone(), at: 0, text: "office אבג".into(),
        }] }))?;
    }
    let mut job = document.start_layout(&Payload::new(LayoutOptions::default()))?;
    while !job.step(&mut document, 64)?.data.complete {}
    let package_bytes = document.save()?.data.bytes;
    // The host writes package_bytes to its chosen destination.
    let _ = package_bytes;
    Ok(())
}
```

The normal sequence is **open/create → supply resources → viewport job → edit →
incremental job → save**. The engine remains deterministic if hosts supply
identical fonts, plugin pins/grants/budgets, authored state and layout options.
Font registration and plugin installation change configuration and drop old
memos. Edits keep exact-input memos, so unaffected paragraphs can be reused.
`state()` includes tree parents, logical text, undo/redo availability, the current
revision and diagnostics; `Applied` reports created IDs and ordered position
effects. Rejected transactions preserve authored state and undo history.

A job charges one core work unit at a time: a paragraph/table or a staged pass.
A unit can be substantial; `step(64)` is a count budget, not a millisecond promise.
Zero budget is a checked no-op; budgets above 100,000 are rejected. The job may
yield early as soon as viewport demand is covered. `LayoutProgress` reports the
producing pass, covered pages, complete/settled flags, outside-document demand,
and shape/composition/reuse counters. Provisional pages may move during region
feedback. Only a complete current layout supports navigation, reading order,
PDF, SVG and PNG. Empty/negative/extreme media remain diagnosed core inputs;
page caps and viewport bounds limit work rather than using floats or a clock.

Every result has a `LayoutToken` (document identity, revision and session
generation). Native publication additionally checks object identity. New jobs
supersede older jobs, even at the same revision; edits/sync invalidate old
revisions; font/asset/plugin changes invalidate configuration; cancellation is
terminal. Already posted worker messages must also be gated by the host's active
job/token. Matching a revision alone is insufficient. Dropping a native job
releases it; WASM clients call `release_job` after completion/cancellation.

## API coverage

| Area | Native `DocumentSession` methods (WASM uses the same names) |
| --- | --- |
| Authored state | `state`, `apply`, `undo`, `redo` |
| Resources | `declare_font`, `register_asset`, `resources`, `resource_bytes` |
| Layout | `start_layout`; native job `step`, `cancel`, `display_page`; WASM `step`, `cancel`, `partial_page`, `release_job` |
| Navigation | `move_cursor`, `caret_rect`, `hit_test`, `selection_rects` |
| Clipboard | `copy` (native fragment bytes), `copy_as` (plain/HTML selection with loss reports), `paste`, `import_text` |
| Export/persistence | `save`, `export` (plain/HTML/native/PDF) |
| Presentation | `display_page`, `display_json`, `svg`, `png`, `reading_order`, `diagnostics` |
| Extensions | `load_plugin`, `install_plugin`, `run_plugin_edit`; WASM `release_plugin` |
| Collaboration | `sync_info`, `export_updates`, `import_updates`, `awareness`, `validate_awareness` |

Transactions expose the kernel's text/block/style/relation commands. Native
paste and plain/HTML import use the kernel's standalone paste transaction.
Copies/promotions of tables and relation edges keep the clipboard's policies and
loss diagnostics. `copy_as` uses an isolated fragment projection, so it does not
mutate the original document or pollute its undo history. `Cursor.goal_x` survives
line movement; selections retain anchor/focus and upstream/downstream affinities.
Invalid/unlaid-out caret endpoints are typed errors rather than empty success.

Fonts use frontend family aliases, integer weight/style/stretch descriptors and
collection indices. `save` finishes incremental layout if necessary and embeds
precisely used faces. It preserves opaque package sections/extensions, untouched
metadata, IDs, history and registered assets. Refreshed font/asset manifests are
canonicalized with unknown fields retained. It never compacts automatically.
`resources()` describes bundled/missing resources, hashes, font pins and optional
external locations. `resource_bytes()` returns only verified bundles; the engine
never resolves a path or URL. Generic defaults remain those of the core. Image
registration is deliberately opaque; placement and image display variants must
be connected after the parallel images workstream is integrated.

Plugins require a SHA-256 pin plus declared imports, independent grants, phase,
function signatures and explicit budgets. Installation supports functions,
geometry (wrapping the default Greedy composer) and relation schemas. Schema
installation preserves editor undo history. Editing plugins receive caller-
assigned handles for explicit live nodes and commit staged text insertions as
one kernel transaction. No ambient document/resource/transport authority exists.
The same deterministic wasmi interpreter runs natively and inside this WASM
module. Keep the engine's **WASM stack at least 512 KiB**; the commands below use
1 MiB. See [plugins.md](plugins.md) for ABI and sandbox details.

Sync v1 deliberately uses **self-contained history snapshots as update packets**.
This gives bounded, preflighted import and preserves per-peer undo/convergence
without exposing Loro or accepting unchecked delta encodings. `sync_info` returns
a sorted vector; a transport may suppress sending when vectors agree. Import
checks document scope, declared distinct peers and exact vector agreement before
merging. Updates include authored state, not fonts, package assets, plugins or
layout settings; hosts supply matching resources/configuration separately.
Awareness is at most 64 KiB of opaque bytes, is scoped to document/peer, and is
never interpreted or stored. These primitives do not authenticate senders.
Compact deltas and sync-format negotiation are a follow-up, not v1 behavior.

## Worker protocol

[`worker.ts`](../crates/reprise-wasm/ts/worker.ts) is a dependency-free reference,
with checked TypeScript request/response unions. Deploy its emitted JavaScript
beside wasm-bindgen's web `reprise_wasm.js` and `.wasm`. One worker owns one
current session; all incoming requests are serialized, including initialization.
The main thread drives explicit steps, leaving room for edits/cancellation between
messages. There are no hidden threads, timers or unbounded pump loops.

Every message is `{ version: 1, data: { id, kind, ... } }`. `id` is a caller
correlation string. Request kinds are `create`, `open`, `font`, `edit`, `start`,
`step`, `cancel`, `save`, `sync-export`, `sync-import`, and `close`. Responses are
`state`, `started`, `layout`, `saved`, `sync`, `ack`, or `error`.

1. `open` carries `Payload<Open>` and package `Uint8Array` (or use `create`).
2. `start` carries `Payload<LayoutOptions>` and returns a monotone `job` handle.
3. `step` carries that handle and budget; `layout` returns progress and covered
   display pages. If unfinished, post another `step` after consuming the result.
4. `edit` carries `Payload<Transaction>`; its successful state response supplies
   a new revision. Start a new job and discard queued results for the old handle.
5. `save` finishes current layout if needed and returns package bytes. `saved`
   and `sync` transfer owned output buffers; the sender no longer owns them.
6. `cancel` acknowledges and releases the active job. `close` frees the session.

Failed open/edit/font/sync requests preserve the existing valid session/job.
The example sends all covered pages on each step for clarity; a production host
may choose dirty-page transport. It releases superseded/complete handles. The
WASM class permits at most eight retained jobs and 64 loaded plugin handles;
installed plugins remain pinned in engine configuration after their lookup handle
is released. Dropping a WASM session releases all its resources.

## Versioning and errors

API version 1 identifies the wire grammar and semantics; crate semver identifies
the Rust/WASM package. Unsupported envelope versions are rejected **before**
interpreting data. Unknown command variants/fields are refused rather than
silently ignored. A breaking wire change requires a new API version. Breaking
Rust API changes require a semver-breaking release (a minor bump while 0.x, a
major bump from 1.0). Compatible fixes preserve this wire version and diagnostic
codes; new capabilities can use new methods/types. Hosts pin crate/npm versions
and exchange an agreed protocol version; v1 has no automatic negotiation.

All boundary data is owned by the facade: **no internal types are re-exported**.
The display DTO mirrors the frozen glyph/path/group wire schema, with an explicit
conversion, and its native/WASM golden pins that conversion. New image display
operations need an additive facade DTO/adapter when the image workstream lands.
Generated `types.d.ts` is checked against Rust DTOs by a native test. `API.txt`
pins native public signatures; `reprise_wasm.d.ts` is generated by wasm-bindgen.
Run the regeneration commands and inspect their diffs when making API changes.

Native methods return one non-exhaustive typed `Error`. `Error::payload()` gives
`Payload<ErrorPayload>`; WASM throws that same object. Match codes and the optional
transaction command index, not messages. Core errors/diagnostics retain their
stable codes and severity; facade errors add the following codes, all **Error**
because a requested operation/result was omitted. Explicit successful cancellation
returns an acknowledgement rather than an error.

| Code | Meaning |
| --- | --- |
| `bindings.version` | Unsupported object-payload version |
| `bindings.invalid` | Malformed payload, cyclic/throwing JS object, inconsistent sync metadata, or invalid operation |
| `bindings.limit` | Explicit input/work/retention/resource bound exceeded |
| `bindings.id` | Noncanonical, reserved, absent or foreign-scoped identity |
| `bindings.stale` | Job/result no longer matches session, revision or generation |
| `bindings.cancelled` | Further access to a cancelled job |
| `bindings.layout-required` | Current complete layout is required |
| `bindings.read-only` | Compatible newer package cannot be exposed as editable |
| `bindings.store` | Document/store error without a more specific core note |
| `bindings.render` | Backend could not render the requested page |

Important v1 bounds: JSON 20 MiB/depth 64; JS object graph 100,000 values,
depth 64, 10,000 array entries, 1,024 own keys/object and 20 Mi UTF-16 units;
aggregate embedded JS byte buffers 128 MiB; fonts 32 MiB; assets 16 MiB;
packages 128 MiB; clipboard buffers 96 MiB; sync snapshots 64 MiB/vector 4,096
entries; awareness 64 KiB; plugin modules 1 MiB; pages/viewport 10,000;
step budget 100,000; document-state enumeration 100,000 nodes/depth 1,024;
raster output 16 million pixels and scale 1..16,000 permille. The core's stricter
format, fragment, editing, HTML, font and plugin limits also apply. Repeated
references in acyclic JS data are allowed; getters are read into a bounded,
prototype-free snapshot before serde runs. Rust does not catch panics to turn
bad content into success; the boundary validates content and uses fallible core
APIs. Both facade crates forbid unsafe code.

## Host ownership and supported scope

The Tauri backend owns IPC registration/envelopes, paths and atomic file writes,
permission UI, font discovery/import UI, resource fetching, plugin consent,
transport/authentication, native thread scheduling and presentation tokens.
The engine owns document interpretation, transactional edits, undo, navigation,
bounded layout, display data, exporters, package preservation and sync primitives.
There is no Tauri dependency in the facade. Sessions retain single-threaded
incremental caches; create/drive native sessions **inside a dedicated engine
thread/actor** and forward commands to it, instead of moving a live session
between Tauri command threads. No thread is required for correctness.

The necessary additive internal accessors are `Document::version_vector`,
`Package::with_document`, `Editor::register_schema`, and the incremental
`LayoutCache`/`LayoutContinuation` suspend/resume accessors. The editor is boxed
to keep its core document address stable when the facade object moves. Opaque
cache storage stays with that one session and is dropped on reconfiguration.
These accessors add no CRDT type exposure and change no frozen signature.

The first release fixes the core HarfRust adapter, Greedy composer and generic
font defaults, with plugin geometry wrapping Greedy. It opens existing page,
range and table structures and imports supported HTML tables, but does not add
new standalone authoring commands for named styles/templates/ranges/tables that
the frozen editing kernel itself does not expose. Broad-script default coverage,
DOCX/EPUB and PDF/UA remain the core's documented follow-ups. Compatible future
packages are safely refused rather than exposed through an editable session;
a future read-only facade needs a core `DocumentAt` layout entry point. None of
these require modifying an existing frozen contract in this workstream.

## Build, packaging and verification

Required CI needs only Rust with the WASM target. Linux, Windows and macOS run
the workspace tests; the WASM job includes both facade crates. CI does not fetch
wasm-pack, wasm-bindgen CLI, Node test runners or npm dependencies at test time.
For npm packaging, use wasm-bindgen CLI **matching Cargo.lock's wasm-bindgen
version** (currently 0.2.129) and a stack of at least 512 KiB:

```powershell
cargo run -p reprise --example generate_types
$env:RUSTFLAGS = '-C link-arg=-zstack-size=1048576'
cargo build --release --target wasm32-unknown-unknown -p reprise-wasm
# Use cargo metadata --no-deps to find target_directory if it is overridden.
wasm-bindgen <target_directory>/wasm32-unknown-unknown/release/reprise_wasm.wasm --target web --typescript --out-dir crates/reprise-wasm/pkg
Copy-Item crates/reprise-wasm/pkg/reprise_wasm.d.ts crates/reprise-wasm/ts/reprise_wasm.d.ts
tsc -p crates/reprise-wasm/ts/tsconfig.json --noEmit false --outDir crates/reprise-wasm/pkg
npm pack ./crates/reprise-wasm
```

The checked-in package manifest has no JS dependencies. Browser consumers call
the default async `init` before constructing `Workspace`. To emit the worker,
override `noEmit` and put the result next to the generated module:
`tsc -p crates/reprise-wasm/ts/tsconfig.json --noEmit false --outDir crates/reprise-wasm/pkg`.
On Bash, set `RUSTFLAGS='-C link-arg=-zstack-size=1048576'` for the cargo build.
Leave the worktree's `.cargo/config.toml` untouched. Restore/unset `RUSTFLAGS`
after packaging; it is a build setting, not authored state or layout input.

When Node and the matching CLI are available, run an actual native/WASM parity
and hostile-object smoke without wasm-bindgen-test or npm dependencies:

```text
wasm-bindgen <target_directory>/wasm32-unknown-unknown/debug/reprise_wasm.wasm --target nodejs --typescript --out-dir <target_directory>/pkg-node
node crates/reprise-wasm/ts/smoke.cjs <target_directory>/pkg-node
```

Build that debug module with the same stack flag first. The smoke compares
byte-identical display JSON with `crates/reprise/tests/display.json`, then tests
copy/paste, undo, save/reopen, sync, typed Uint8Array channels, stale jobs, cyclic
and throwing objects, getter snapshotting and plugins executing through wasmi
inside WASM. The native facade-only editor test additionally covers RTL/ligature
text, font declarations, visual movement, selections, PDF/plain loss reports and
concurrent-peer convergence. Owned hostile fixtures live in the facade tests;
the shared fixture list and every existing snapshot remain unchanged, respecting
this workstream's file ownership.

New dependency licenses were checked in downloaded Cargo manifests: ts-rs and
ts-rs-macros 12.0.1 and serde-wasm-bindgen 0.6.5 are MIT; termcolor 1.4.1 and
winapi-util 0.1.11 offer MIT (chosen over Unlicense). wasm-bindgen 0.2.129 and
js-sys 0.3.106 are promoted existing dependencies under MIT OR Apache-2.0.
All are GPLv3-compatible; the facade/package license is AGPL-3.0-or-later.
