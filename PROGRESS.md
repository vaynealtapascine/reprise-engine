# Progress

The hand-off log. Newest first.

## 2026-10-09: anchored character formatting; glyph outlines for web renderers

Completed Astra's uncommitted formatting slice (`docs/text-formatting.md`) after
review. Formatting actions are `format1` envelopes on persistent ranges; overlaps
resolve by action order, field by field, with reset; runs are grapheme-aligned.
They flow through the kernel (`FormatText`), layout style runs and cache keys,
native fragments, HTML `<span>` export/import, the `REQUIRED_TEXT_FORMATTING`
package bit, and the facade (`Command::FormatText`, `State.blocks[].formatting`,
omitted when a block has none, so existing JSON is unchanged).

**Review fixes:** resolution scanned the whole range tree for every paragraph,
and incremental layout's input capture called it for every node on every
revision (O(paragraphs × ranges) per layout). A per-revision index built in one
pass now serves `text_formats` and `text_format_capacity`, following the lineage
cache. Paste indexed its ID map with `[]`; a missing block is now a typed error.

**Glyph outlines:** `glyph_outlines(Payload<GlyphRequest>)` returns SVG path
data in font units per glyph (≤ 4,096 per request) so a canvas renderer can draw
display lists without the SVG backend. New code `bindings.missing-font`.

**New hostile fixture:** `format_overlap_storm`: 300 overlapping size, language
and feature actions, some starting inside graphemes, a reset, a concurrent peer's
action and a corrupted envelope from a hostile peer, then a split. The replica
converges; the garbage is reported (`style.format-unreadable`) and never applied.
Two new snapshots; nothing else changed.

**Not yet supported:** weight, slant, decoration and colour (refused, not
ignored); per-line metrics for mixed sizes (the strut grows to 1.2× the largest
inline size). Workstream brief: `F:/reprise-wt/briefs/ws-emphasis.md`.

## 2026-10-09: flow integration verified after the Opus checkpoint

Worktree: `G:/reprise-wt/flow`, branch `flow/breaks`, based on the document-layer
checkpoint `abc8ff5`. The parent application's unfinished scaffold is separate.

The editing kernel now uses flow splits, copy moves, flow-aware paste and stable
caret lookup. Follow-up fixes validate reserved markers and break parents before
writing, handle split-then-move transactions, and permit copy moves within the
source flow. Copy lineage maps visible characters rather than counting hidden
markers, and lives at the shared-text host so pre-split anchors can follow later
copies. Random transaction refusal and complete undo/redo regressions caught
partial-write bugs in both paths. Copying non-flow splits can read their staged
destination for lineage. Clipboard expectations now check retained host identity
and spanning ranges. A split after head deletion cannot insert before its own
paragraph marker: splits inside inherited concurrent prefix text use copy/lineage;
the concurrency limitation of that exceptional path is explicit in `flow.md`.

Lineage is indexed per revision and source ID, capped at 4,096 records per host
and 16 candidates per lookup. Canonical numeric keys must match their cursors.
Undo recovery uses the same validated index. Excess state is retained verbatim;
normalization is read-only. Editing-step history discards an entire group if the
16,384-item CRDT history cap evicts part of it, rather than partly undoing it.

`range_extent` exposes both paragraph-local endpoints, including ranges whose
first slice is empty. Copying a split tail includes ranges authored before the
split. Excessive embed nesting is reported by `collab.flow-depth`. Native/WASM
DTOs include the `moved` effect and accept compound paragraph IDs with large peer
IDs. The design note now describes the actual root break-record map and grouped
undo implementation.

Verification completed on Windows: `cargo test --workspace -j 2 --no-fail-fast`
passed 874 tests across 97 target results, with only the existing opt-in geometry
preview exporter ignored. Hostile fixtures and layout snapshots passed unchanged.
`cargo fmt --all --check`, workspace/all-target Clippy with warnings denied, and
the required WASM target checks passed. Rebuilt WASM bindings passed native JSON
parity, editor/sync/image/hostile-object smoke and worker smoke tests. Strict
TypeScript checking and generated web declaration parity passed too. The completed
integration is local to `flow/breaks`; it has not been merged into main or the app.

**Remaining engine work from the original brief:** formatting as overlapping
anchored ranges, resolved into mixed-style shaping/composition and exported through
clipboard/PDF; true inline objects with metrics and caret geometry (the current
embeds place blocks between paragraphs); delta-maintained boundary indexing for
very long shared texts (the current derived flow cache rebuilds once per revision).
Cross-host joins and explicit copy moves still have the documented concurrent
copy-intent limitation, as do splits inside text inherited before a surviving
paragraph's marker after head deletion. Ordinary Enter/Backspace retain character
identity; a representation allowing boundary relocation is still needed to remove
that exceptional split-copy path.

## 2026-10-08: tree positions checked, panics contained

**Found in review:** a remote packet whose tree move carried an empty, all-`00` or
all-`FF` fractional index passed every preflight check and imported. The receiver's
next local block insertion beside it then panicked inside `loro_fractional_index`
and poisoned Loro's store. Because the index is CRDT state, it would also be saved
and spread to every replica. The hostile fuzz never produced one, because it
writes through a real `LoroDoc`. In addition, Loro's JSON reader slices
`fractional_index` strings by byte offset, so a non-ASCII one panicked inside
`serde_json` before any check ran.

**Fixed:** preflight refuses tree positions that lack Loro's `0x80` terminator or
exceed `MAX_TREE_POSITION_BYTES` (16 KiB). A non-allocating walk refuses
non-ASCII position strings before Loro parses the body. `Document::import`
refuses saved packages and v1 snapshots with such positions. Regression tests
cover the crashing values, several unusual but valid ones followed by local
insertion, escaped keys, and a crafted saved document. The saved-document test
fails without the check.

**Containment:** `DocumentSession::contain` and `reprise::contain` turn a panic into
`bindings.poisoned`, and poison the session so that the host reopens it. WASM gains
`setPanicHandler`, which reports the panic message before a `panic=abort` trap. The
reference worker answers with `bindings.poisoned` from then on. See
`docs/bindings.md`, "Panics". The native Tauri host should route its dispatch
through `contain`.

## 2026-10-07: remaining robustness limits completed on main

Recovered and integrated the vertical worktree's remaining geometry/template
changes. Version 3 adds actual downward vertical-lr; old templates retain upward
sideways-lr behavior. Upright runs use UAX #50 and vertical OpenType shaping;
short combinations fit one em with integer horizontal compression. Shared display
groups compensate the frame reflection/rotation across SVG, PNG and PDF. Cache
identity and paragraph continuation track the actual frame mode. The new vertical
suite pins native/stepped parity, storage and reading-order addresses; the PNGs
were visually checked. `all` combinations are bounded to four scalars per item.

Binary snapshot preflight checks internal columnar counts and sums retained
counter lengths across change blocks after bounding KV expansion. Regression
cases refuse oversized/change-count inconsistencies at document import before
Loro sees the block. The known unbounded n_changes allocation path is closed.
Binary compatibility remains separate from preflighted format-2 peer transport.

Split/join operations now save compact character-ID lineage spans.
Excluded staging commits retain identity history across split undo; undo/redo
captures explicit aliases for recreated IDs. Fresh carets retain live identities;
old carets follow those aliases through subsequent ordinary text edits. Stable
carets follow recreated text through chained structural edits and block cycles;
end carets anchor to the last character. Exact lineage keeps the original stable
identity across merges. Anchored insertion boundaries and SHA-256 span digests
recover redo copies under fresh IDs. Malformed metadata is ignored, competing
records sort deterministically, and breadth-first traversal visits at most 256
distinct identities. Atomic combined-digit caret geometry is covered by pointer
hits and caret-rectangle round trips. Existing authored
range block policies and concurrent split/join intent anomalies remain explicit.

The final worktree audit recovered later collaboration commits: unchanged-text
sync validation optimization, structural partition/rejoin soak, packet-byte parity
and the scripted malformed-peer fixture. The soak now fails on every generated
edit/undo error. Native clipboard/plain-text tests assert typed refusal of unreadable
blocks; tagged PDF still renders and structurally checks the readable remainder.
Main keeps the stronger worker responses and fuzz oracles instead of overwriting
them with the older worktree versions. Original worktrees/recovery patches remain.

Verification: the complete workspace passed 828 tests (one ignored visual-artifact
writer), including the widened 2,100-step sync soak (143.79 s) and 2,000-edit
structural soak (405.92 s). Formatting and strict all-target Clippy passed. All
required WASM crates check, the actual WASM build and JavaScript smoke pass,
including native packet bytes; the compiled reference worker passes its effect,
created-ID, split-selection and image checks. Strict TypeScript compilation and
generated web declaration parity passed. Vertical PNGs were visually inspected.
The three existing vertical snapshots add only writing-mode metadata; geometry
is unchanged. The malformed-peer fixture adds diagnosed readable-remainder
snapshots rather than normalizing away hostile authored data.

Astra-medium reviewed the completed source and all responses to earlier findings:
no unresolved concrete multiplayer correctness blocker. This was a read-only
correctness review, with validation performed by the primary agent. Main reconciles
the later collab/fuzz branch ancestry after recovering their unique work; original
worktrees and uncommitted scratch/recovery changes remain intact.

Host-owned transport, authentication, peer identity, resource distribution and
presence expiry remain explicit integration responsibilities. Concurrent structural
intent anomalies converge but are not semantically merged; authored ranges retain
their block policy. Binary compatibility remains trusted, and bounded format 2 is
the peer transport. These contracts are documented in `docs/collaboration.md`.

## 2026-10-07: multiplayer hardening consolidated for Reprise

**Consolidation:** `hardening/collab`, `hardening/pdfua`, `hardening/fuzz`,
`hardening/incremental` and the committed `hardening/vertical` foundations are
merged into main. Tables were already merged. The uncommitted facade fuzzer and
incremental work counters/tests were recovered into main. Original worktrees and
their changes remain intact. The fuzz scratch driver requiring `SEED` is kept in
its worktree, not added to the suite. Incomplete vertical geometry edits remain
in `F:/reprise-wt/vertical`: they require the missing shaping/display implementation
and must not publish a reflected vertical-lr glyph transform on their own.

**Multiplayer boundary:** Opus's delta sync, causal text preflight, invariant audit,
change reports, stable selections, presence and per-user undo are now exposed
through both native and WASM facades and the reference worker. New first joins
use `snapshot-json`, with the same preflight as deltas. Binary v2 join packets are
refused before decoding; trusted v1 and package compatibility remain readable.
Compacted replicas refuse requests for trimmed history with a typed error.
Tree preflight checks creation order; corrupt presence cursor bytes and foreign
text containers cannot fabricate remote carets. Native facade layout jobs can
select scoped workers without changing output, steps or counters.

**Verification additions:** a seeded three-peer partition/rejoin soak with replay,
explicit missing predecessors, local undo/redo, selection parity, convergence,
layout, reopen and bounded keystroke packets; actual WASM/JavaScript multiplayer
smoke coverage; native/WASM and TypeScript checks in CI. The recovered facade
harness now uses valid step budgets and does not undo a preceding action after a
no-op paste. Its seeded scenarios pass. The incremental table oracle now accounts
for multiple cell compositions per placement unit, as documented. Large paragraph
coverage defaults to 10,000 words; `REPRISE_LONG_PARAGRAPH_WORDS=100000` widens it.
`REPRISE_COLLAB_STEPS=2100` widens the default 300-step soak.

**Remaining scope:** binary package/v1 decoding still has the upstream columnar
allocation risk documented in `docs/collaboration.md`; Reprise's untrusted
multiplayer transport must use format 2. Resources, authentication, authorization,
peer identity allocation, presence expiry and transport acknowledgements belong
to the host. Upright vertical shaping is unfinished; the retained foundation
properties do not yet affect glyph orientation. Table borders and nested tables
remain feature gaps rather than multiplayer invariant failures.

**Final checks and Astra-medium review:** full workspace tests passed (809),
including all snapshot, hostile-document, cross-crate and recovered incremental
checks. Strict Clippy, formatting, all-crate WASM checks, the actual WASM build
and JavaScript smoke, and strict TypeScript compilation passed. The added remote
split fallback regression passes, bringing the test total to 810. Astra-medium
found one worker integration defect: local edits discarded `Applied`. Fixed
`edit`/`image` responses to return effects and created IDs with state, and verified
the compiled worker against real WASM bindings. No further ordinary multiplayer
correctness blocker was found. Remote split caret fallback is now documented
and pinned; the review was a correctness review, not a security certification.
The widened 2,100-step three-peer partition/rejoin soak also passed (129.64 s).


## 2026-10-06: tables merged (hardening)

**Done** (Sonnet 5.5): **header rows that repeat, and row and column spans** (06, 24, 26, 33,
34, 35).

-   `CellInfo` stores authored `colspan` and `rowspan`. `doc/src/table_grid.rs::resolve_grid`
    is the one pure interpretation, shared by layout, export and accessibility:
    -   cells resolve in document order, and the first claim wins;
    -   a zero span counts as one;
    -   spans are cut to the edge, to free cells, and to the header rows they start in;
    -   a cell is dropped when its origin is taken or its column is outside the table.

    Concurrent span edits from two peers therefore resolve identically on every replica.
    The read-only API is `Document::table_structure` and `table_cell_structure`.
-   Rows joined by a row span form a group. A group is kept together when a fresh frame fits
    it; otherwise it splits with `layout.table-rowspan-split`.
-   The leading header rows repeat on every continuation frame as derived copies in
    `LayoutSnapshot.repeated_headers`, never in `blocks`. That makes them not navigable: a
    hit resolves to the nearest authored line. When a copy can't fit, the warning is
    `layout.table-header-unrepeated`.
-   **Package features:** optional bit 8 marks header rows and required bit 8 marks spans,
    so older readers refuse span documents instead of misreading them. Both bits are
    recomputed on save.
-   **HTML clipboard:** `th`, `thead`, `colspan` and `rowspan` are read and written, within
    bounds.
-   **Orchestrator:**
    -   Approved the additive `repeated_headers` snapshot field and the feature bits.
    -   Added `tables_orchestrator.rs`: 48 seeded hostile-span tables in tiny to roomy
        frames, checked for determinism, incremental equality and no double-claimed grid
        positions, with counters proving that repeats, issues and splits occur. Also 12
        rounds of concurrent span and header edits that converge both ways.

**Gaps:** borders; nested tables; a header that itself spans frames never repeats (it only
warns).

**Hand-offs:**

-   **pdfua:** tag `THead`/`TH` (scope Column) and `RowSpan`/`ColSpan` from
    `table_structure`, and mark `repeated_headers` copies as Artifacts.
-   **incremental:** tables added two one-liners in `incremental.rs`: the snapshot field,
    and `partial()` dropping header copies of a cut frame.

## 2026-10-05: hardening pass under way

Five workstreams started at 08:00 UTC on Claude (Sonnet 5.5 and Opus 5.5). All stopped at
08:14 on the account's usage limit, and Sol's usage is exhausted until 2026-10-10. Worktrees
moved from D: to `F:/reprise-wt/<name>` (branches `hardening/<name>`), with uncommitted work
carried over. Briefs are in `F:/reprise-wt/briefs/hardening-*.md`.

| Workstream | State at the stop |
| --- | --- |
| **tables:** repeated header rows, row and column spans | `doc` commit: `CellInfo` spans, `set_table_row_header`, and a shared `resolve_grid` that clamps or drops malformed spans; uncommitted `layout/src/table_flow.rs` |
| **pdfua:** tagged ordered PDF aiming at PDF/UA-1 | uncommitted, never compiled `pdf.rs` rewrite plus `pdf/tags.rs` |
| **fuzz:** cross-crate scenario harness and oracles | uncommitted `crates/fuzz-harness` skeleton |
| **incremental:** finer steps, native workers | research only |
| **vertical:** `text-orientation`, vertical shaping, tate-chū-yoko | research only |

**Resumed at 12:50 UTC.** All five agents picked up again, now on the F: worktrees.

**Added: collab** (Opus 5.5, `hardening/collab`). A review for multiplayer readiness found
gaps that no other workstream covers:

-   Every exchange sends a full history snapshot.
-   Carets and selections are byte offsets, so they go stale when a remote merge lands.
-   There is no remote presence.
-   Peer updates bypass kernel validation, with no defined post-merge invariants.
-   `import_updates` doesn't report what changed.

The brief (`briefs/hardening-collab.md`) covers delta sync with negotiation, a trust boundary
and a concurrency matrix, a raw-op hostile-peer fuzz, stable carets and presence, change
reports, per-user undo proofs, and a three-peer soak test.

**Review of the tables commit.** The design is sound: spans are interpreted in one pure
function shared by layout, export and accessibility, and repeated headers stay derived.
Spans live in the existing version-1 cell record, which has `deny_unknown_fields`, so an
older reader drops a spanned cell instead of misreading it. The package feature declaration
is therefore required, so older readers refuse the whole document instead.

## 2026-10-05: bindings merged

**Done** (Sol): **bindings (11)**, the external contract the Reprise app is built on.

-   **`crates/reprise`:** the native facade for the Tauri backend. A `Workspace` opens and
    creates `DocumentSession`s that cover editing, undo, navigation, clipboard, exports with
    loss reports, budgeted viewport-first layout jobs, display lists, reading order, fonts,
    image assets, plugins and sync primitives. Every payload is versioned, the public API is
    pinned in `API.txt`, and errors are one typed enum with stable codes.
-   **`crates/reprise-wasm`:** the same facade over `wasm-bindgen`, with generated TypeScript
    declarations and a dependency-free reference worker in `ts/`. Native and WASM give
    byte-identical display JSON for text and images.
-   `docs/bindings.md` covers the editor loop, worker protocol, versioning, error codes and
    the split between the Tauri backend and the engine.
-   **Review fixes:**
    -   Create and open define a base style when a document has none, so default-styled
        text lays out without registering fonts.
    -   A missing legacy family falls back to the serif default with `font.fallback`
        instead of dropping the text (`font_legacy_missing` fixture). An authored generic
        name resolves directly, with no warning.
    -   Unplaced selections return `bindings.layout-required`, not `InvalidId`.
-   **Orchestrator tests:** a boundary fuzz over every byte-taking entry point; two peers
    opening the same legacy package write the base style concurrently and still converge.

**Gaps:**

-   **Sync:** v1 updates are bounded, self-contained history snapshots; compact deltas are a
    follow-up.
-   Packages from a newer version are refused rather than opened read-only.
-   Rich-image HTML.

**Next:** hardening (finer incremental steps, native workers, PDF/UA tags, table headers and
spans, upright vertical text, cross-crate fuzzing, a performance pass), and the Reprise UI
on top of the bindings.

## 2026-10-05: range policies, consolidated CODEMAP and images merged; bindings under review

**Done** (Sol):

-   **Range policies (contract change).** Ranges persist their authored `RangePolicy` in a
    versioned `policy1` envelope, read back with `Document::range_policy`. Clipboard copies
    policies exactly; `clipboard.range-affinity` now applies only to legacy ranges.
    Operation counts and every snapshot are unchanged. `docs/CODEMAP.md` is one table again.
    **Orchestrator test:** a concurrent split and join keep authored policies on both
    replicas.
-   **Images (contract additions: `BlockKind::Image`, `BlockLayout.image`).**
    -   Image blocks have collaborative alt text, and are sized from PNG and JPEG headers,
        density-aware and integer-only. Layout never decodes pixels.
    -   Images are placed in flow, floats, notes and tables, with placeholders and warnings
        when an image is missing or corrupt.
    -   `Item::Image` is drawn in SVG, PNG and PDF from a host `AssetStore`. Alt text goes
        into ordered PDF and text export.
    -   Packages and the clipboard carry images.
    -   **Orchestrator test:** 256 damaged images never panic, from header to pixels.

**Under review: bindings (11).** The facade (`crates/reprise`, versioned DTOs, a pinned API,
typed errors) and the WASM/TypeScript layer (`crates/reprise-wasm`, a reference worker, and
byte-identical display JSON from native and WASM) are strong. The orchestrator's boundary
fuzz test found that **default-styled text in a new document is never laid out**:

-   no base style is defined;
-   a legacy single family missing from the store drops the text instead of falling back to
    the serif default, as the font policy requires;
-   `copy` then reports a misleading `InvalidId`.

These went back to the agent with images wiring.

**Gaps:**

-   **Images:** EXIF orientation, colour profiles, rich-image HTML, DOCX and EPUB;
    editor-level image split and join policies.

## 2026-10-05: font supply, plugins, and clipboard and exporters merged

All three ran on Sol. They were cut off by Sol's usage limit and resumed after it reset.

-   **Font supply (21).**
    -   Frontends declare fonts (bytes, family, descriptors, collection index), and faces are
        matched the way CSS does it.
    -   Explicit family chains end in a generic class, and characters fall back per grapheme.
    -   Bundled OFL defaults: Source Serif, Source Sans 3, Source Code Pro and Dancing Script,
        about 586 KiB.
    -   Packages embed the faces a layout used, and restore them on open, with a list of
        missing fonts.
    -   **Orchestrator:** fixed PDF embedding the wrong face from a collection. Added a test
        that damages every default face and checks layout, SVG, PNG and PDF survive.
-   **Plugins (10).**
    -   `reprise-plugin` runs plugins on wasmi 2.0 with fuel, limits, capability grants, no
        SIMD or threads, canonical NaNs and a fresh instance per call.
    -   Its versioned core-WASM ABI is in `docs/plugins.md`.
    -   Plugins can supply style functions, wrap a composer with geometry, back relation
        schemas, and stage edits through the kernel.
    -   **Orchestrator:** recursion never uses the host stack. Hosts need about 384 KiB of
        stack in debug builds and 256 KiB in release; `docs/plugins.md` says to keep 512 KiB.
-   **Clipboard and exporters (9b).**
    -   `reprise-clipboard` copies native fragments through `plan_copy` and pastes them as
        one transaction with fresh IDs.
    -   Plain text follows reading order. HTML goes in and out, bounded.
    -   Plain, HTML, native and PDF exporters return a `LossReport`.
    -   **Orchestrator:** twenty pastes undo and redo exactly. Fixed in the merge: fonts are
        now identified by declaration, so fragments carry each `FontDeclaration` and key
        fonts by face.

**Pending contract proposal (clipboard), approved:** persist each range's authored
`RangePolicy` beside its anchors. Endpoint affinity isn't always recoverable from cursors.
It is scheduled next.

**Gaps:**

-   **Fonts:** authored weight, style and stretch; variable axes; emoji and broad-script
    defaults.
-   **Plugins:** relation placement and sandboxed composers; hosts must put
    `Engine.plugins.envelope()` in cache tags.
-   **Clipboard:** a host range spanning several pasted blocks goes missing (undo restores
    it); no nested tables in HTML; DOCX and EPUB exporters.

**Next:**

-   Images.
-   The `RangePolicy` persistence contract.
-   A CODEMAP tidy: workstreams appended their own tables.
-   11: bindings.

## 2026-10-04: workstream 7, the editing kernel, merged; plugin and font choices recorded

**Done.** Workstream 7 started on Sonnet and was finished by Sol.

-   **`reprise-edit`:**
    -   Typed commands and transactions that are validated before anything is written. A
        rejected transaction changes nothing.
    -   Per-peer undo and redo that keep collaborators' edits.
    -   Carets with affinity, hit testing and integer caret geometry.
    -   Logical and visual movement through bidi text, ligatures and transformed, vertical
        and spiral frames.
    -   Selections, with discontiguous geometry where directions mix.
    -   Movement across blocks follows `reading_order`.
    -   The "Editing kernel" contract in `contracts.md` is frozen.
-   **Identity (07): soft deletion.**
    -   A delete writes a `deleted` flag on the node. Undo restores it in place, with the
        same ID.
    -   Descendants inherit deletion, including tables, rows and cells.
    -   A concurrent delete and restore resolve last-writer-wins.
    -   Inserted blocks are staged invisibly and activated in the transaction's own commit,
        so redo brings back the same IDs.
-   **Orchestrator test:** undo, redo and merges interleaved on two peers converge, and no
    new ID repeats.

**Decisions recorded in `architecture.md`** (implementation choices under decision 41):

-   **Collaboration transport:** the engine provides sync primitives only.
-   **Compaction:** opt-in and explicit.
-   **Formal model:** deferred.
-   **API versioning:** stable types at the bindings boundary.
-   **Plugins:** wasmi everywhere, open to wasmtime later. Floats are allowed inside
    plugins, with NaN canonicalisation and no relaxed SIMD. Layout-time host APIs are
    integer-only.
-   **Fonts:**
    -   Frontends supply font data.
    -   Used fonts are embedded in the package.
    -   A missing font is reported and the user is asked.
    -   CSS-like fallback chains end in a generic class with an engine default face.
    -   Licensing is the user's responsibility.

The open questions in `architecture.md` are all resolved.

**Next, on Sol, in parallel:**

-   9b: clipboard and exporters.
-   10: plugins on wasmi.
-   Font supply: generic fallback classes, default faces, embedding fonts on use.

After those: images, then 11, the bindings.

## 2026-10-04: workstream 6, incremental layout and scheduling, merged

**Done** (Sol). The design and its verification are described in `docs/incremental.md`.

-   **`LayoutSession`** re-lays out only what changed. It sits beside the frozen
    `Engine::layout`, which stays the reference.
    -   Its caches are keyed on complete typed inputs: text, fonts, adapter, composer settings,
        schemas, functions, medium and flow settings.
    -   An edit to one paragraph of 1,000 recomputes that paragraph and touches one page.
-   **`LayoutJob`:** budgeted, resumable and cancellable, viewport-first. It returns a
    `PartialLayout` that states what it covers. Results are rejected when they are stale (an
    older revision, or another document). It is single-threaded, so it works on WASM.
-   **`DependencyGraph`:** one dependency vocabulary over styles and relations, with
    forward and reverse inspection and the reason each unit was recomputed (39).
-   **Explicit passes:** flow, region feedback, relations and reading order, each with its
    existing cycle bound.
-   **No dependencies added.** Salsa was evaluated and rejected: its fixed-point recovery
    assumes monotone computations, and region feedback can oscillate.
-   **Verification:** every hostile fixture and the spike take 32 seeded edits, each checked
    against `Engine::layout`. The orchestrator added seeded random walks: 600 edits over ten
    documents, including deleting blocks that others point at, frame-relative styles before
    a template change, and a concurrent peer's delete.

**Gaps:**

-   Work units are whole paragraphs, tables or passes.
-   Region and relation views are provisional until settled.
-   Native worker threads aren't used yet.
-   The debug overlay doesn't show dependencies yet.

## 2026-10-04: geometry, regions and base direction merged; kernel in progress

**Done:**

-   **Workstream 5, geometry and reading order.** It went to Sol after Claude rate limits.
    -   Exact frame rotation (quarter turns, rational directions, angles in thousandths of a
        degree, raw matrices) and mirroring.
    -   `vertical-rl`, plus a sideways `vertical-lr`.
    -   Spiral paths, expanded into threaded chord frames (at most 1,024 strips).
    -   `reprise.reading-order` overrides, with diagnostics for cycles, conflicts and
        missing targets; `LayoutSnapshot::reading_order(&doc)`.
    -   `pdf::render_ordered`.
    -   The design note is in `docs/geometry.md`. Orchestrator test: extreme authored matrices
        and spirals stay total.
-   **Workstream 3b, floats, notes, tables and solver domains** (Sol).
    -   Floats with runaround, stacking and deferral.
    -   Footnotes and endnotes that continue across pages, nest, and can take over pages.
    -   Tables (`doc/src/table.rs`) with rows fragmenting across pages.
    -   Integer solver domains for column widths.
    -   Region feedback runs at most 16 passes and freezes on oscillation.
    -   `docs/regions.md` answers the open question of where allocation, fragmentation and
        backtracking live. `architecture.md` still lists it as open: the user may want to
        close it by pointing there.
    -   Orchestrator test: floats and nested notes in rotated, mirrored and vertical frames,
        with reading order covering every line.
-   **Contract change:** `BlockLayout.base_level`, so the editing kernel doesn't re-derive
    paragraph direction.

**In flight:**

-   **Workstream 7, the editing kernel** (Sol, `D:/!!Self/dev/reprise-wt/edit`).
    -   Its first trash-root identity scheme was proven unsound (ID collisions with a
        concurrent real peer, split undo steps, lost undo history). It was replaced by
        **soft deletion**: a `deleted` flag in the node's metadata, undone in place with the
        same ID.
    -   It must merge `main`, cover tables and reading order, and check carets in transformed
        frames.

**Gaps noted by these workstreams:**

-   **Vertical text:** upright CJK and downward `vertical-lr` need vertical shaping.
-   **Paths:** straight-strip approximation, with no collision avoidance.
-   **Reading order:** overrides work on whole blocks. The ordered PDF has no PDF/UA
    structure tree.
-   **Floats and notes:** floats don't fragment, and notes use the first notes frame.
-   **Tables:** no repeated headers, spanning cells, borders or nested tables yet.
-   **File format:** needs feature declarations for table metadata and the new schemas.

**Next:** workstream 6, incremental evaluation and scheduling (Sol), now that 1–5 have
landed. Then 9b, the clipboard, after the kernel merges.

## 2026-10-04: integration and the file format merged; wave 2b started

**Done.**

-   **Integration** (`ws/integrate`):
    -   Layout applies bidi rules L1 and L2 per line with `reorder_line`. `shape/HANDOFF.md` is
        gone.
    -   `follow` uses the shared resolver, so it gets every layout query and the deletion
        policies.
    -   Styles resolve against the frame a block starts in, and `style.*` notes are published
        as diagnostics. `Engine.functions` is new.
    -   Justified `Optimal` lines now render justified: word and letter spacing reach the glyph
        advances.
    -   Snapshot changes: a trailing space at the paragraph level in `bidi_stray_controls`; the
        style fixtures now publish their notes; `style_bases` has real frame bases; and three
        relation messages are reworded, with the same codes.
    -   Orchestrator test: positioned runs tile their lines in every fixture.
-   **Workstream 9a, the file format** (`reprise-format`):
    -   A versioned container with a document ID, checksummed sections and canonical JSON
        metadata. The spec is `crates/format/SPEC.md`.
    -   Bundled and external assets, checked against their hashes, and pinned fonts.
    -   Migrations, and tagged derived caches that are ignored when they don't match.
    -   Unknown data is kept byte for byte, and parsing is bounded.
    -   Loro snapshots are preflighted before import.
    -   Orchestrator tests: a pin on the Loro version that the preflight reads, and rejection
        of Loro blobs that aren't documents.

**In flight:**

-   **7, the editing kernel** (`ws/edit`, `D:/!!Self/dev/reprise-wt/edit`).
-   **5, frame transforms, writing modes, spiral text and reading order** (`ws/geometry`,
    `F:/reprise-wt/geometry`, Opus 5.5). It owns `template.rs` `to_page`, `display.rs` and
    the `FrameTemplate` transform fields.
-   **3b, floats, notes, tables and solver domains** (`ws/regions`, `F:/reprise-wt/regions`,
    Sonnet 5.5). It owns `flow.rs`, plus the new frame roles. It also writes
    `docs/regions.md`, on where allocation, fragmentation and backtracking live.

**Build disk.** At the user's request, checkouts and builds now go on **F:**, an SSD. New
worktrees are in `F:/reprise-wt/`, with build output in `F:/reprise-target/<name>`.

**Follow-ups from this round:**

-   **Format:**
    -   Hosts must resolve external assets and canonicalise engine configuration for cache
        tags.
    -   Serialising a layout cache is still layout's job.
    -   Opening a newer file read-only needs a layout entry point that takes `DocumentAt`.
-   **Layout:** a `follow` to a frame or page target resolves but can't place a note
    without a unique line. Letter spacing inside a ligature would need shaping support.

**Next:** 9b, the clipboard, after the kernel's insert API lands. Workstream 6 comes once 3b
and 5 settle.

## 2026-10-04: phase 2, wave 1 merged

**Done.** All six wave 1 workstreams are reviewed, merged and pushed. Each review reran fmt,
clippy, the tests and the WASM check, and added at least one hostile test the author hadn't
written.

| Workstream | What landed | Orchestrator's added test |
| --- | --- | --- |
| 1: text and shaping | UAX #9 bidi (ICU4X data, Unicode 17) and UAX #24 scripts in `itemize`; `Itemized.levels`; `reorder_line` (L1/L2); a per-face shaping-data cache | Every line split of hostile mixed-direction text keeps every glyph exactly once |
| 2: composition | `Optimal` (Knuth–Plass, ragged or justified), `AuthoredBreak` (verse turnovers), `Polygon` and `Runaround` geometry, a conformance suite; two `Greedy` bugs fixed | Every composer survives shaping from a stale text |
| 3a: flow and regions | Authored page templates (`doc/src/page.rs`) replacing `PageSettings`; a responsive `Medium`; frame threading, fragmentation and pagination with a page limit; `follow` on any page | Responsive templates under degenerate media |
| 4a: relations | `Structural` and `Snapshot` targets, seven layout queries, `Ambiguous`, deletion policies from tombstones, copy planning, `reprise.reference` | Forged revisions never panic Loro |
| 4b: styles | Bounded typed expressions, a `FunctionRegistry`, `ResolutionContext`, explicit value stages; unreadable values kept | Concurrent style expressions converge |
| 8: display | PDF ToUnicode and ActualText; a debug overlay with selectable families | Text in transformed groups and at tiny sizes extracts |

**Contract change:** `RelationStatus::Deleted`, for relations a `Delete` policy has taken out
of effect (`d0e0893`). `contracts.md` now describes what each workstream built.

**Snapshot changes in wave 1**, each explained in its merge commit:

-   Runs split at bidi levels (`rtl_mixed`), and an RLO span shapes right to left.
-   The snapshot's `page` was replaced by `medium`, `settings` and `template`, and frames gained a
    `role`.
-   In `extreme_lengths`, a paragraph that sat at `Length::MAX` now starts page 2.

No line moved in the spike.

**Build note.** D: is a slow laptop disk, and seven build directories on it stalled every
build. Each checkout has a git-ignored `.cargo/config.toml` (listed in `info/exclude`) that
puts build output on `G:/reprise-target/<name>`, with less debug info.

**Integration still to do** (next, one agent):

-   **L1/L2:** call `reprise_shape::reorder_line` from `layout/src/flow.rs` instead of
    `visual_order`. See `crates/shape/HANDOFF.md`, which can be deleted once this lands.
-   **Follow:** switch `relations/follow.rs` to the shared `relations/resolve.rs` resolver,
    which the top of that file documents.
-   **Styles in flow:** build a `ResolutionContext` per frame in flow, call
    `computed_style_with`, and publish `ComputedStyle.notes`. Add `Engine.functions`, and
    replace `Dim` in templates with expressions when it fits.
-   **Justification:** apply `Adjustment.word_spacing` to glyph positions, so justified
    `Optimal` lines render justified.

**Decisions taken by the orchestrator, open to the user:**

-   **Undo and identity (07).** Loro's undo restores a deleted block under a *new* ID. To keep
    IDs stable, as decision 07 intends, deletion should become a move into a trash parent, so
    undo moves the block back with the same ID. The editing kernel will do this.
-   **Snapshot references.** A `Revision` names no document. The file format (34) should
    record a document ID beside any snapshot reference.

**Known gaps, from the hand-offs:**

-   **PDF:** right-to-left runs and malformed ranges extract as whole-run ActualText, with
    no glyph-level text mapping.
-   **Relations:** `Dependency` exists twice, for styles (`reprise_doc::Dependency`) and for
    relations (`relation::Dependency`); workstream 6 should unify them. Rebinding is
    O(nodes) per rebinding relation.
-   **Composition:** layout passes `Measure` only, so polygon and runaround geometry isn't
    used yet. There are no verse-line entities in the document yet (11). `Greedy` reports
    `Overflow` where the optimal composers report `Forced` for an overflowing line that ends
    at a forced break.
-   **Flow:** one template serves every page, and there are no first, left or right rules.
    `follow` uses only the first margin frame on a page. Continuation across frames is
    quadratic in paragraph length. There are no widows or orphans, and overlapping or
    off-page frames aren't checked.
-   **Styles:** relation `Param::Length` can't hold an expression yet. A stored property that
    isn't a string is still dropped on read.

**Next: wave 2.** It runs alongside the integration where files don't overlap:

-   7: the editing kernel, including trash-based deletion.
-   9: persistence and clipboard.

After the integration lands:

-   5: frame transforms, writing modes and reading order.
-   3b: floats, notes, tables and solver domains.

Workstream 6 comes once those settle.

## 2026-10-03: contracts frozen (decision 40, step 2)

**Done.** The interfaces that parallel workstreams build against are frozen and documented
in [docs/contracts.md](docs/contracts.md):

-   `reprise-diag`: diagnostics with stable codes.
-   `reprise-text`: navigation positions in `segment`, plus a `POINT` range policy.
-   `reprise-doc::relation`: a `SchemaRegistry` of registered `RelationSchema`s, replacing
    the old enums.
-   `reprise-doc::style`: clamping of negative used lengths.
-   `reprise-shape`: itemisation with fallback chains, shaping, reshaping and visual order.
-   `reprise-compose`: geometry providers that can return several intervals, skip or end.
    Composers now carry explanations.
-   `reprise-display`: groups with transforms and clips, paths, and source text on glyph runs.
-   `reprise-layout`: a snapshot with pages and frames, frame-relative lines, and a query API.
-   `geom`: saturating fixed-point arithmetic, `Fixed` and `Matrix`.
-   `reprise-fixtures`: the spike document and nine hostile fixtures, with an invariant suite
    run on every platform.

The layout crate is split into `flow`, `relations`, `snapshot` and `display` modules.

**Picked up from an earlier session.** That session wrote most of the code but stopped
before the hostile suite passed. Finishing it found and fixed:

-   **Upward layout:** negative sizes and line heights were laid out upwards, which put the
    following paragraph about 2 million points above the page with no diagnostic. Used
    lengths are now clamped and reported with `layout.style-clamped`. The suite also checks
    that no line sits above its frame.
-   **Deleted owners:** a relation whose owner note was deleted was reported as an `Error`
    (`owner-not-placeable`). Owned relations now go with their owner: status `OwnerDeleted`,
    with an `Info` diagnostic `relation.owner-deleted`. `owner-not-placeable` is still
    covered, by a relation that tries to place a paragraph.
-   **Lint:** clippy errors, which the earlier session hadn't run.

**The spike snapshots changed only in structure.** Every line break and position is the
same. Lines are now relative to their frame, which sits at the 36pt top margin.

**Still open inside the contracts** (each is an extension point, not a contract change):

-   No bidi algorithm or script itemisation yet.
-   Only the `Greedy` composer.
-   Only `LineContaining` as a layout query.
-   Nothing produces `Ambiguous` yet.
-   No `Structural` or `Snapshot` targets.
-   The PDF backend doesn't pass source text on (ToUnicode).
-   No expressions beyond `pt` and `em`.

**Next.** Phase 2: parallel workstreams. The orchestrator prompt lists them.

## 2026-10-03: end-to-end spike (decision 40, step 1)

**Done.** The spike from decision 40 works end to end:

-   A document with two paragraphs and two annotations, stored in Loro.
-   Each annotation has a `Follow` relation to a `LineContaining` query on a named range.
-   An edit reflows the text, and the annotation follows its line.
-   Output in every format: layout JSON, display list JSON, SVG, PNG and PDF.

Run it with `cargo run -p reprise-cli -- spike out`.

`crates/cli/tests/spike.rs` covers:

-   reflow and following
-   determinism, through JSON snapshot fixtures
-   a deleted target: the relation is reported missing and the rest still lays out
-   two replicas editing concurrently, merging, and producing identical layout
-   all backends

**Choices made in the spike:**

-   **Shaper:** harfrust (0.13), not rustybuzz. It is HarfBuzz's own Rust port and shares
    read-fonts with skrifa, so fonts are parsed by one stack. skrifa is pinned to 0.46 to
    match. krilla (the PDF library) brings its own older font crates; it only receives the
    font bytes.
-   **Content tree:** the content tree is a Loro movable tree. Ranges, relations and styles
    are Loro maps. Relations are stored as JSON strings for now.
-   **Anchors:** anchors are Loro cursors. `Affinity::Before` attaches the cursor to the
    previous character with `Side::Right`. Loro reports the character's index, so `resolve`
    adds one.
-   **Ranges:** a range whose anchored character was deleted resolves as `Rebound`. A range
    that collapses to nothing resolves as `Missing`.

**Known gaps.** Each is deliberately out of the spike's scope:

-   **Reshaping:** text isn't reshaped at breaks marked unsafe-to-break. The composer only
    reports a diagnostic.
-   **Bidi:** there is no bidi algorithm yet. Direction is passed to the shaper, and glyphs
    are assumed to be in logical order.
-   **Page and flow:** one page, one main column and one margin column, set by
    `PageSettings`. There are no frames, columns, pagination, floats, tables or notes yet (24).
-   **Relations:** `RelationKind` and `Target` are enums, not registered schemas (14). Only
    `Follow` and `LineContaining` exist.
-   **Rebinding:** there is no automatic rebinding beyond Loro's tombstones, and no
    `Ambiguous` state yet.
-   **Recomputation:** every layout is a full recompute. There is no dependency graph,
    incremental evaluation, workers or viewport-first scheduling yet (27, 28).
-   **Styles:** only `family`, `size` and `line-height`. Expressions are `pt` and `em` literals.
-   **Transforms:** translation only; no rotation or mirroring yet (20).
-   **PDF text:** the PDF draws real font glyphs, but has no ToUnicode text, because the
    display list carries no source text. Copy and paste won't work yet.
-   **Caching:** `HarfRust` rebuilds `ShaperData` on every call; there is no cache.
-   **Fonts:** there are no fallback chains. A missing family produces a diagnostic and the
    block is skipped.

**Next: decision 40, step 2.** Freeze the contracts the parallel modules will build against,
then split the work. Candidates to freeze:

-   `ShapingAdapter`, including reshaping
-   `Composer` and `GeometryProvider`
-   the `DisplayList` item set
-   a `RelationSchema` registry, to replace the enums
-   the `LayoutSnapshot` query API (`line_containing` and friends)
-   the text-store surface of `reprise-text`

Freeze the test fixtures at the same time.
