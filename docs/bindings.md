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

Flow paragraph IDs include both host and break identity, for example `12@1/34@2`.
Splitting returns this new paragraph ID. Moving a paragraph of a shared flow copies
it and returns `{ kind: "moved", node, new }` in `Applied.effects`; hosts must map
positions from `node` to `new`. Whole-tree moves retain their ID. This additive
effect is declared in the Rust DTO and generated TypeScript union.

Character formatting is authored with `Command::FormatText { node, start, end,
style }` (byte offsets; empty ranges are refused). `TextStyle` fields are
optional patches: `families`, `size` (1/1024 pt, positive), `language`, and
OpenType `features` (`{ tag, value }`, four ASCII characters). `reset: true`
clears earlier formatting under the range before applying its own fields. Each
action is a separate undoable range whose boundaries expand when typing at
either end; overlaps resolve by action order, field by field (see
`docs/text-formatting.md`). `State.blocks[].formatting`, present only when some
action covers the block, lists the resolved disjoint runs over the whole text;
a run's absent fields inherit the paragraph style. Weight, slant, decoration and
paint are supported as `weight` (1..1000), `slant` (normal/italic/oblique),
`decoration: { underline?: boolean, strike?: boolean }`, and `color: [r,g,b,a]`
with byte channels. These also exist on paragraph `Style`. Decoration members
compose independently; null/absent inherits and false removes that line.
Missing faces use CSS matching and report `font.nearest` Warning.

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
| Authored state | `state`, `apply`, `insert_image`, `undo`, `redo` |
| Resources | `declare_font`, `register_asset`, `resources`, `resource_bytes` |
| Layout | `start_layout`; native job `step`, `cancel`, `display_page`; WASM `step`, `cancel`, `partial_page`, `release_job` |
| Navigation | `move_cursor`, `caret_rect`, `hit_test`, `selection_rects` |
| Clipboard | `copy` (native fragment bytes), `copy_as` (plain/HTML selection with loss reports), `paste`, `import_text` |
| Export/persistence | `save`, `export` (plain/HTML/native/PDF) |
| Presentation | `display_page`, `display_json`, `svg`, `png`, `glyph_outlines`, `reading_order`, `diagnostics` |
| Extensions | `load_plugin`, `install_plugin`, `run_plugin_edit`; WASM `release_plugin` |
| Collaboration | `sync_info`, `export_updates`, `import_updates`, `awareness`, `validate_awareness` |

`glyph_outlines(Payload<GlyphRequest>)` returns, for one face and at most 4,096
glyph IDs, each glyph's outline as absolute SVG path data in font design units
with y up, plus `units_per_em`. A canvas or WebGL renderer caches these (for
example as `Path2D`) and draws display-list glyph runs by scaling with
`size / units_per_em` and flipping y. IDs the face lacks give empty paths; an
unknown face is `bindings.missing-font`. Outlines are for rendering only.

Transactions expose the kernel's text/block/style/relation commands. Native
paste and plain/HTML import use the kernel's standalone paste transaction.
Copies/promotions of tables and relation edges keep the clipboard's policies and
loss diagnostics. `copy_as` uses an isolated fragment projection, so it does not
mutate the original document or pollute its undo history. `Cursor.goal_x` survives
line movement; selections retain anchor/focus and upstream/downstream affinities.
Absent node IDs give `bindings.id`; valid unplaced blocks give
`bindings.layout-required`. Invalid byte/cluster offsets give `bindings.invalid`.
New sessions define the empty-name base style with `serif`, 10 pt size and 1.2 em
line height. Opening a package adds it only if absent; authored definitions survive.
Default-styled text needs no frontend font registration. The base intentionally
selects the legacy `serif` generic, so it has no substitution warning; explicit
family chains retain per-grapheme coverage diagnostics. Legacy named families
that are unavailable use the serif generic default with `font.fallback` (Warning).

Fonts use frontend family aliases, integer weight/style/stretch descriptors and
collection indices. `save` finishes incremental layout if necessary and embeds
precisely used faces. It preserves opaque package sections/extensions, untouched
metadata, IDs, history and registered assets. Refreshed font/asset manifests are
canonicalized with unknown fields retained. It never compacts automatically.
`resources()` describes bundled/missing resources, hashes, font pins and optional
external locations. `resource_bytes()` returns only verified bundles; the engine
never resolves a path or URL. Image registration installs immutable bytes in
`Engine.assets`, keyed by the returned SHA-256. `insert_image(Payload<ImageInsert>)`
and `Command::InsertImage { image }` accept that hash, collaborative alt text,
optional integer physical width/height, style and insertion caret (`None` appends).
The image command must be alone in its transaction, because it uses the kernel's
atomic fragment paste; `InsertBlock` with image kind is refused without a record.
Undo/redo covers the insertion. Missing/corrupt resources remain diagnosed
placeholders. Display DTOs include image items with a renderer-shaped `DisplayRect`
(`origin`, `width`, `height`), distinct from flat navigation rectangles. SVG/PNG/PDF
use the engine asset store; package save embeds every live image reference and
open restores verified bundles. Native copy/export attaches selected image bytes;
paste preflights fonts/assets before committing and installs them atomically.

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
correlation string. Request kinds are `create`, `open`, `font`, `asset`, `image`, `edit`, `start`,
`step`, `cancel`, `save`, `sync-export`, `sync-import`, and `close`. Responses are
`state`, `asset`, `started`, `layout`, `saved`, `sync`, `ack`, or `error`.
An `asset` request carries declaration and bytes; its `asset` response returns
the content hash for the following `image` insertion request.

1. `open` carries `Payload<Open>` and package `Uint8Array` (or use `create`).
2. `start` carries `Payload<LayoutOptions>` and returns a monotone `job` handle.
3. `step` carries that handle and budget; `layout` returns progress and covered
   display pages. If unfinished, post another `step` after consuming the result.
4. `edit` carries `Payload<Transaction>`; its successful state response supplies
   a new revision. Start a new job and discard queued results for the old handle.
5. `save` finishes current layout if needed and returns package bytes. `saved`
   and `sync` transfer owned output buffers; the sender no longer owns them.
6. `cancel` acknowledges and releases the active job. `close` frees the session.

Failed open/edit/font/asset/image/sync requests preserve the existing valid session/job.
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
The display DTO mirrors the frozen glyph/path/group/image wire schema, with an
explicit conversion. Native/WASM goldens pin both text and image display JSON;
image editor loops verify asset-aware rendering, clipboard and package round trips.
Regenerate the image golden with `cargo run -p reprise --example image_golden`.
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
| `bindings.missing-font` | `glyph_outlines` named a face the session doesn't have |
| `bindings.poisoned` | The engine panicked; discard the session and reopen it (see below) |

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

### Panics

A panic is an engine bug, but the host must survive one. Native hosts route
every session call through `DocumentSession::contain(|s| ...)`, and calls with no
session, such as `Workspace::open`, through `reprise::contain`. A contained panic
returns `bindings.poisoned` with the panic message. A panic can leave the store
half-updated or its locks poisoned, so the session refuses every later contained
call with the same code. The host drops the session, reopens the last saved
package and resynchronises with its peers: the CRDT makes that recovery lossless
for everything that was saved or sent.

WASM builds use `panic=abort` on stable Rust, so a panic traps the instance and
nothing can catch it. Call `setPanicHandler(message => ...)` once after `init`:
the hook calls it with the message and location just before the trap. The
reference worker records the message, answers that request and every later one
with `bindings.poisoned`, and the host should terminate the worker and start a
new one as above. Unwinding WASM (nightly `-Zbuild-std`, `-Cpanic=unwind` and the
exception-handling proposal) would let wasm-bindgen 0.2.129 turn panics into JS
exceptions; the wrappers would then also need to route through `contain`.

Recovery only helps when the panic came from a request. A panic caused by
document state recurs after reopening. For that reason the boundary refuses such
state on import (for example, invalid tree positions), and does not rely on
containment.

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

### Layout workers

With the default `native-workers` feature on a native target,
`DocumentSession::set_layout_threads(threads)` lends each layout step up to
`threads` scoped threads (the engine thread included) for pure preparation
tasks: shaping requests and short-paragraph itemisation, shaping and break
analysis. It returns the count actually used. That is `1` on WASM, without the
feature, or for `threads <= 1`, and it is capped at 256. The default is `1`.
Worker threads exist only inside a step and are joined before it returns, so
the session stays single-threaded between calls and still belongs on the
engine thread/actor. The thread count never changes output, budget charging,
partial coverage or work counters; it only changes elapsed time. The WASM
facade has no such method: browser layout runs these tasks serially inside the
one worker that owns the session. See [incremental.md](incremental.md#native-workers).

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
byte-identical display JSON with `crates/reprise/tests/display.json` and
`crates/reprise/tests/images.json`, then tests
copy/paste, undo, save/reopen, sync, default text and images, typed Uint8Array
channels, stale jobs, cyclic
and throwing objects, getter snapshotting and plugins executing through wasmi
inside WASM. The native facade-only editor test additionally covers RTL/ligature
text, font declarations, visual movement, selections, PDF/plain loss reports and
concurrent-peer convergence. Boundary hostile tests live in the facade. The review-authorized shared
`font_legacy_missing` fixture adds layout/content goldens for serif substitution;
every existing snapshot remains unchanged. The orchestrator's `boundary_fuzz.rs`
is retained unchanged.

New dependency licenses were checked in downloaded Cargo manifests: ts-rs and
ts-rs-macros 12.0.1 and serde-wasm-bindgen 0.6.5 are MIT; termcolor 1.4.1 and
winapi-util 0.1.11 offer MIT (chosen over Unlicense). wasm-bindgen 0.2.129 and
js-sys 0.3.106 are promoted existing dependencies under MIT OR Apache-2.0.
The facade image tests also use the existing workspace lopdf 0.38.0 dependency
(MIT) to verify embedded PDF image objects; no new package is added for it.
All are GPLv3-compatible; the facade/package license is AGPL-3.0-or-later.

### Multiplayer worker messages (format 2)

`sync-export` with a `Payload<SyncRequest>` returns `sync-packet`; `since: null`
requests a complete JSON join, and a vector requests a delta. Omitting the request
retains the trusted legacy v1 response. `sync-packet-import` returns `synced`,
including the change report, transformed local selection and current state.
`sync-info` supplies the vector for acknowledgement and anti-entropy. Failed
imports retain the active layout job; successful document changes release it.

`selection` sets or clears the anchored local selection and returns its stable
form. `resolve-selection` resolves an opaque stable selection. `presence` creates
awareness bytes; `resolve-presence` returns the current geometry and metadata.
`undo`/`redo` return `history` with the edit report and current state.
`edit`/`image` return `edited` with `Applied` and current state. Use its created
IDs and effects to transform the UI selection after local split/join/delete
commands, then send `selection` to anchor the resulting caret.
See [collaboration.md](collaboration.md) for the transport recovery rules and
trust boundary. Resources travel over the host's separate resource channel.

### Authored marks (W8)

`DocumentSession::marks(page)` / WASM `marks(page)` returns a versioned
`MarksPage` with a current layout token and integer page-space geometry. It
requires complete layout at the current revision. It is a separate read-only
query; display JSON, layout counters and document state do not change. The
reference worker accepts a `marks` request with `page` and returns `marks` with
`page: Payload<MarksPage>`. Commands add line breaks, tabs, paragraph/individual
line alignment, tab stops and persistent pins (`remove-anchor` tombstones the
relation). Geometry, relation states, bounds, HTML fidelity and the poem fixture
are documented in [marks.md](marks.md). Package required bit 11 prevents older
readers silently losing authored positions. Regenerate both declarations using
the packaging commands above after any DTO change; Node and worker smokes cover
the real marks query and command path.
For the Node worker smoke, compile its CommonJS copy with both
`--module commonjs --moduleResolution node`, then run
`node crates/reprise-wasm/ts/worker-smoke.cjs <target_directory>/pkg-node <worker-output>/worker.js`.


### Page setup

Use `session.set_page_setup({version: 1, data: {width, height, top, right,
bottom, left}})` natively or in WASM; every property is optional (null also
means unchanged). The equivalent edit command is `{kind: "set-page-setup",
setup: {...}}`. `{kind: "swap-page-orientation"}` swaps width/height while
keeping physical margins. Standalone calls are individually undoable. Commands
may share a transaction with text edits, with normal atomic validation.

All values are integer 1/1024 pt. Letter is 626688 by 811008 units; ISO A4 is
609562 by 862095 units, rounded from 210 by 297 mm. Landscape swaps these pairs.
Dimensions must be positive and at most 14745600 units (200 inches); margins
must be nonnegative and leave at least one unit of width and height. Invalid
requests return `edit.page-setup-invalid` Error without changing the document,
revision, or undo history. The app chooses its new-document defaults and should
send all six fields when choosing a paper preset.

`state().data.page_setup` reports effective size, `margins: {top, right, bottom,
left}`, orientation, template name/source and `patched`, before any layout job.
Patches change the active template's first main frame and preserve other frames
and authored transforms/writing modes. Unspecified properties inherit that base
frame. Without a patch, the built-in 420 by 300 pt page remains unchanged; its
main frame has margins 36/164/36/36 pt in top/right/bottom/left order. A size-only
patch retains those base margins.

Flat `page-setup1` scalar keys merge independently, including first edits on
both peers. Invalid merged combinations retain authored data but use the base
page with `layout.page-setup-invalid` Warning in state and layout diagnostics.
Required package bit 12 makes old readers refuse page-setup history; it stays
set after undo because retained operations can restore those properties.
The existing worker `edit` request accepts both commands and returns current
state, so no separate worker message is required.

Native copy-all carries the raw page patch along with templates and template
choice. Paste into an empty document restores it in the same undo step as the
content; a nonempty target keeps its page. Partial copies omit page setup.
The optional native-fragment field is absent from older fragments, and strict
older readers refuse new fragments that contain it. It carries authored
properties rather than geometry resolved for a particular viewport.
