# Code map

Each crate lives in `crates/<name>` and is published to the workspace as `reprise-<name>`.
They are listed in dependency order. [contracts.md](contracts.md) describes the frozen
interfaces.

| Crate | Files | What it does | Decisions |
| --- | --- | --- | --- |
| `diag` | `src/lib.rs` | `Note`, `Severity`, `Code`: diagnostics shared by every library | 37, 39 |
| `geom` | `src/lib.rs` | `Length` (1/1024 pt) and `Fixed` (16.16), saturating and rounding rules, `Matrix` with exact integer direction/angle rotations, mirrors and pivot transforms, typed spaces, `Point`, `Rect`, `Transform` | 19, 20 |
| `text` | `src/lib.rs` | `Text` (byte offsets over Loro), `Anchor`, `Affinity`, `RangePolicy`, `Resolved` | 09, 10, 12 |
|  | `src/segment.rs` | Grapheme and word boundaries (ICU4X) | 09, 10 |
| `doc` | `src/lib.rs` | `Document`: content tree, ranges, styles, relations, revisions, fork and merge; exports authored frame geometry and reading schemas | 05–07, 15, 29 |
|  | `src/relation.rs` | `Relation`, `Target`, `StructuralQuery`, `LayoutQuery`, `SnapshotRef`, `RelationSchema`, `SchemaRegistry`, the built-in schemas (`reprise.follow`, `reprise.reference`, `reprise.reading-order`), `Dependency`, copy planning (`plan_copy`, `CopySet`, `IdMap`) | 13, 14, 15, 27, 35 |
|  | `src/structure.rs` | Live tree navigation and structural queries (`evaluate`), inherited deletion through table nesting, succession links (`supersede`, `succession`) | 06, 07, 13, 15, 24 |
|  | `src/history.rs` | `DocumentAt` (a past version), `HistoryCache`, snapshot resolution, `compact_history` | 07, 13 |
|  | `src/resolve.rs` | `resolve_target` / `resolve_relation`: any non-layout target, with `OnTargetDeleted` applied from tombstones; `dead_relations` | 13, 14, 15 |
|  | `src/relation_tests.rs` | Tests for the three files above and for relation.rs | 13, 14, 15, 35 |
|  | `tests/history_hostile.rs` | Forged revisions resolve or report, never panic | 07, 13, 37 |
|  | `src/style.rs` | `Style` with versioned additive family chains, `LengthExpr`, `Authored` (stored forms), the four stages (`Specified`, `Computed`, used via `StyleResolution`), `ComputedStyle`, defaults | 08, 17, 18, 39 |
|  | `src/expr.rs` (`src/expr/tests.rs`) | `Expr`: bounded typed expressions with a canonical text form, type check, folding, saturating evaluation and dependency sets | 17, 19, 27 |
|  | `src/function.rs` | `FunctionRegistry`, `PureFunction`, `Signature` and the built-in functions | 17, 36 |
|  | `src/context.rs` | `ResolutionContext`, `Level`, `Basis`, definite, indefinite and unresolved bases | 18 |
|  | `src/codes.rs` | The `style.*` diagnostic codes | 37 |
|  | `src/page.rs` | `PageTemplate`, `FrameTemplate`, `FrameRole`, `Dim` (lengths that may follow the `Medium`), v2 authored transforms, writing modes and spiral paths, v1-compatible Loro storage and the built-in template | 05, 20, 24, 34 |
|  | `src/reading.rs` | Independent `reprise.reading-order` block-precedence schema and authoring helper | 01, 14, 33 |
| `font` | `src/lib.rs`, `src/supply.rs`, `src/supply/tests.rs` | `Face` with a pinned `FaceId`, metrics, glyph outlines and one type-erased adapter-data cache slot; `FontStore`, frontend declarations, CSS-inspired matching and pinned generic defaults | 21, 22, 38 |
| `shape` | `src/lib.rs`, `src/fallback_tests.rs` | `ShapingAdapter` contract, `HarfRust` with per-face data caching, `ShapedText`/`ShapedRun`, `Reshape`, `visual_order` | 22, 38 |
|  | `src/paragraph.rs` | `itemize` / `itemize_families` (legacy and grapheme-preserving generic fallback chains, resolved bidi levels and contextual scripts), `Shaper` (shaping and reshaping a paragraph) | 09, 21, 22 |
|  | `src/unicode.rs`, `src/bidi-character-subset.txt` | ICU4X property adapter for UAX #9 (including N0) and UAX #24 script resolution; pinned Unicode conformance subset | 09, 22, 38 |
|  | `src/line.rs` | Pure L1/L2 line reordering, preserving bidi groups across missing-font gaps | 20, 22, 30 |
| `compose` | `src/lib.rs` | `Composer` and `GeometryProvider` contracts, `Measure`, break opportunities, `LineFragment`, `Explanation` | 23 |
|  | `src/greedy.rs` | The greedy composer, and the first-fit algorithm other composers fall back to | 23 |
|  | `src/optimal.rs` | `Optimal`: Knuth–Plass total fit over variable geometry, ragged or justified; integer demerits, `Limits` | 19, 23, 39 |
|  | `src/authored.rs` | `AuthoredBreak`: verse, lines end at forced breaks, turnovers with a hanging indent | 11, 23 |
|  | `src/polygon.rs` | `Polygon` and `Runaround` geometry providers, integer ellipses | 20, 23, 24 |
|  | `src/para.rs` | The paragraph as composers see it: normalised breaks, prefix widths, fragments | 23 |
|  | `src/walk.rs` | Walking a geometry provider: skips, `End`, stalls | 23 |
|  | `src/testing.rs` | Unit-test shaping with the bundled font | 39 |
|  | `tests/conformance.rs` | Every composer against adversarial geometry and texts, checking every `Composer` guarantee | 23, 37, 39 |
| `display` | `src/lib.rs` | `DisplayList`, `Item` (glyphs, paths, groups), `RenderError` | 32 |
|  | `src/svg.rs`, `src/png.rs`, `src/pdf.rs` | Backends; PDF ToUnicode, ordered run addresses and extraction spans, cluster ActualText and logical-run fallback for RTL/malformed ranges | 32, 33 |
|  | `tests/pdf_text.rs` | Ordered PDF extraction through rotated, mirrored, vertical and spiral layouts; generated-PDF extraction through lopdf plus bfchar/ActualText reader; ligatures, clusters, RTL, malformed ranges, ZWJ and empty runs | 33, 37, 39 |
| `layout` | `src/lib.rs` | `Engine` (configuration, including `schemas`, `functions`, `medium` and `FlowSettings`), `Engine::layout`, geometry and reading module wiring | 20, 24, 26, 33, 38 |
|  | `src/codes.rs` | Diagnostic codes reported by layout and the relation pass | 37 |
|  | `src/snapshot.rs` | `LayoutSnapshot` (pages, frames, blocks), `RelationLayout`, `Resolution`, `Diagnostic`, queries | 05, 13, 37 |
|  | `src/geometry.rs`, `docs/geometry.md` | Exact authored frame transforms, logical writing axes, bounded spiral expansion, inverse and path diagnostics | 19, 20, 37, 38 |
|  | `src/reading.rs` | Snapshot reading-order queries with explicit document input, stable partial-order completion and iterative cycle repair | 01, 33, 37 |
|  | `src/template.rs` | Resolving the document's page template against the medium; falling back to the built-in one | 24, 34, 37, 38 |
|  | `src/region.rs` | `Bounded`: any geometry provider, ended at a frame's depth | 23, 24 |
|  | `src/flow.rs` | Pass 1: per-starting-frame style resolution and diagnostics, shaping, line L1/L2 reordering and glyph spacing adjustments, composing and threading paragraphs through the main flow's frames, page after page | 08, 17, 18, 22, 23, 24, 30 |
|  | `src/relations/mod.rs` | Pass 2: dispatching relations on their schema | 13–15, 26 |
|  | `src/relations/follow.rs` | `reprise.follow`: placing a block in the margin frame of its target line's page | 13, 15, 24 |
|  | `src/relations/resolve.rs` | `Resolver`: any `Target` to a `TargetLayout`, status and diagnostics; shared by `follow` and other relation behaviours | 13, 14, 15 |
|  | `src/query.rs` | Layout queries added by relations (`first_line`, `frame_of`, `answer`, ...) | 13, 16 |
|  | `src/display.rs` | `to_display_list(s)`, ordered PDF run addresses and individually selectable debug boxes, baselines, available/used intervals, run boundaries, break symbols, reshaped lines, relation statuses and diagnostic cluster markers | 32, 39 |
| `fixtures` | `src/lib.rs`, `src/fonts.rs` | Pinned fonts, engine and peers for tests, three-face fallback text and relocated OTC fixture | 38, 39 |
|  | `src/spike.rs` | The spike document | 40 |
|  | `src/templates.rs` | Page templates for tests: columns, a margin, responsive sizing | 24 |
|  | `src/hostile.rs` | Hostile fixtures every workstream must keep passing: text and bidi, display clusters, relation targets and policies, composers, style expressions, cycles and bases; transformed RTL, writing modes, spirals, reading cycles and degenerate/extreme transforms; editing lifecycle, transaction refusal and empty/zero-width carets; font chains, generic defaults, three-face fallback, corrupt declarations and out-of-range collection indices | 17, 18, 20, 33, 37, 39 |
|  | `tests/hostile.rs`, `tests/snapshots/` | Invariant checks, editing undo/refusal/extreme-hit tests, content-preservation goldens and debug explainability geometry snapshots | 38, 39 |
|  | `tests/relations.rs` | Relation targets, queries and deletion policies through layout | 13, 14, 15 |
|  | `tests/fonts.rs` | Versioned font chain storage, legacy overrides, unknown versions, collection declarations and incremental parity including fallback-semantics cache invalidation | 21, 22, 34, 38 |
|  | `tests/styles_concurrent.rs` | Concurrent style expressions converge and keep unreadable values | 08, 17, 29, 34 |
|  | `tests/flow.rs` | Columns, pagination, fragmentation, annotations on later pages, responsive templates | 24, 34 |
|  | `tests/geometry.rs` | Transform/caret round trips, bounds, rotated follow, spiral reflow, semantic and overridden reading order; SVG/PNG visual exports | 20, 33, 39 |
|  | `tests/regions_geometry.rs` | Floats and nested notes inside rotated, mirrored and vertical frames: determinism, roles, backends, reading order | 20, 24, 33 |
| `edit` | `tests/audit.rs`, `tests/transactions.rs` | Real-peer identity, atomic deletion, retained history, every command and fixed-seed convergence/undo properties | 07, 29 |
|  | `tests/undo_merge_fuzz.rs` | Undo, redo and merges interleaved on two peers: replicas and layout converge, new IDs never repeat | 07, 29 |
|  | `src/lib.rs`, `src/command.rs`, `src/editor.rs`, `src/plan.rs` | Validated typed commands, staged activation, atomic commit/undo, bounded transaction models and position effects | 02, 05, 07, 12, 29, 37 |
|  | `src/error.rs`, `src/codes.rs` | Typed refusals and stable Error diagnostics for validation, resource bounds and store failures | 37 |
|  | `tests/document_edits.rs` | General structural edit primitives, table/row/cell subtree deletion with identity-preserving undo, and collaborative text undo | 07, 12, 24, 29 |
|  | `src/caret.rs`, `src/model.rs`, `src/navigator.rs` | Byte/affinity carets, cached cluster/grapheme cells, integer page geometry, overlapping-strip hit testing and snapshot reading-order integration | 09, 20, 22, 30, 33 |
|  | `src/movement.rs`, `src/select.rs` | Logical/page-direction navigation through frame transforms, goal-x line movement, reading order, logical selection ranges, bidi geometry and gesture operations | 20, 30, 31, 33 |
|  | `tests/common/mod.rs`, `tests/navigation.rs`, `tests/movement.rs`, `tests/selection.rs` | All-hostile geometric round trips, bidi/zero-width and spiral-strip traversal, authored reading overrides, empty input, transforms and selection coverage | 20, 30, 31, 33, 37 |
| `cli` | `src/lib.rs` | `write_outputs`: layout JSON, and display list JSON, SVG and PNG for every page, plus a PDF | 32 |
|  | `src/main.rs` | The `reprise spike [OUT_DIR]` command | 40 |
|  | `tests/spike.rs`, `tests/snapshots/` | End-to-end spike tests and JSON fixtures | 38, 40 |

Other files:

-   `fixtures/fonts/`: four bundled generic defaults (Source Serif Pro, Source Sans 3, Source Code Pro, Dancing Script), their OFL licenses and pinned provenance in README.md.
-   `.github/workflows/ci.yml`: CI.

## Recipes

-   **Add a relation type:**
    1.  Define its `RelationSchema` in `doc/src/relation.rs`. Add it to `builtin::all()`
        if it is built in.
    2.  Give it layout behaviour in its own file under `layout/src/relations/`, and dispatch
        to it on its `SchemaId` in `relations/mod.rs`.
    3.  Cover it, including its deleted-target cases, in `fixtures`.
-   **Add a target or layout query:**
    1.  Add the variant to `Target` or `LayoutQuery` in `doc/src/relation.rs`, with its
        `TargetClass`.
    2.  Answer it from the `LayoutSnapshot` queries.
-   **Add a shaping adapter:** implement `ShapingAdapter` in `shape`, or in a new crate if it
    pulls in platform code. Set `platform_independent` honestly.
-   **Add a composer or geometry provider:** implement `Composer` or `GeometryProvider` in
    `compose`, following the guarantees on the trait. `Engine.composer` selects which
    composer layout uses.
-   **Add a display item:**
    1.  Add the variant to `Item` in `display`.
    2.  Draw it in all three backends.
    3.  Emit it from `layout/src/display.rs`.
-   **Add a style property:**
    1.  Add the field to `Style` and `ComputedStyle` in `doc/src/style.rs`.
    2.  Add it to `Property` and resolve it in `Computed` (`style.rs`), recording its source in `explain` and
        clamping it if negative values make no sense.
-   **Add a diagnostic:** add a `Code` constant to the reporting crate's `codes` module
    (`layout/src/codes.rs` for layout) and to the table in `contracts.md`.
-   **Add a page template feature (a frame role, a frame property):**
    1.  Add it to `FrameTemplate` or `FrameRole` in `doc/src/page.rs`. Unknown fields make a
        stored template unreadable on purpose, so an older engine reports it.
    2.  Resolve it in `layout/src/template.rs` and give it behaviour in `flow.rs`.
-   **Add a hostile fixture:**
    1.  Add a function in `fixtures/src/hostile.rs` and add it to `all()`.
    2.  Add a test in `fixtures/tests/hostile.rs`.
    3.  Record its snapshot with `INSTA_UPDATE=always`, then read it.

## Persistence files

| Crate | Files | What it does | Decisions |
| --- | --- | --- | --- |
| `doc` | `src/persist.rs` | Opaque snapshot export/import, explicit peer/history mode, allocation-free Loro/LZ4 expansion preflight | 07, 09, 29, 34, 37 |
| `format` | `src/lib.rs`, `src/container.rs` | Versioned checksummed container, document identity, feature masks, hard bounds, typed errors and codes | 34, 37, 38 |
| | `src/json.rs` | Iterative manifest bounds and canonical metadata | 34, 37, 38 |
| | `src/assets.rs` | Font pins (including versioned frontend declarations), bundled/external assets, hash validation, missing-font list, open-and-restore/store API and preservation of unknown declaration fields | 21, 34 |
| | `src/migration.rs` | Pure checked N -> N+1 migrations, synthetic v0 | 34 |
| | `src/package.rs` | Package save/open with additive used-layout-font embedding, read-only newer files, snapshot envelopes and opaque cache tags/validation | 05, 07, 34 |
| | `src/tests.rs`, `tests/roundtrip.rs`, `tests/fonts.rs`, `tests/data/` | Corruption/limits/migrations/golden tests, every hostile fixture and spike persistence/convergence | 37, 38, 39 |
|  | `tests/fonts_damaged.rs` | Truncated and byte-flipped default faces register or refuse, and lay out, draw and subset without panicking | 21, 37 |
| | `SPEC.md` | Version 1 wire specification, compatibility, bounds and dependency licenses | 34 |
| `fixtures` | `src/hostile.rs`, `tests/hostile.rs`, `tests/snapshots/` | Added persistence_tombstones: Unicode anchors, tombstones and retained historical references | 07, 09, 29, 34 |

## Region subsystems

| Crate | Files | What it does | Decisions |
| --- | --- | --- | --- |
| `doc` | `src/region_schema.rs`, registration in `lib.rs` and `relation.rs` | Owned float/note relation schemas and parameter vocabularies | 05, 13, 24 |
| `doc` | `src/table.rs` | Versioned authored table/row/cell topology and column declarations on the movable tree | 05, 06, 24 |
| `layout` | `src/solver.rs` | Explicit integer solver domains, bounded water filling and diagnosed fallbacks | 19, 25, 37 |
| `layout` | `src/regions.rs`, entry in `lib.rs` | Bounded staged feedback, complete input-plan freeze on oscillation, dependency rounds | 24, 26, 37 |
| `layout` | `src/floats.rs` | Side/edge float allocation, stacking, deferral, runaround exclusions | 24 |
| `layout` | `src/notes.rs` | Anchor-preserving note allocation, continuation, nesting, endnotes and full-page reservations | 11, 24, 26 |
| `layout` | `src/table.rs`, hooks in `flow.rs` | Content measurements, declared column allocation and synchronous row fragmentation | 24, 25 |
| `layout` | `src/relations/mod.rs` | Final region relation reporting and references to note lines | 13, 26 |
| - | `docs/regions.md` | Allocation, fragmentation, cycle and fallback design | 24–26 |

| Crate | Files | What it does | Decisions |
| --- | --- | --- | --- |
| `layout` | `src/regions/tests.rs` | Adversarial mixed-region, cycles, dependency depth, fanout, pagination and table cursor unit tests | 24–26, 37 |
| `fixtures` | `src/hostile.rs`, `tests/hostile.rs`, `tests/snapshots/hostile__*.snap` | Ten hostile region fixtures, original invariant checks retained, region-role checks and paired layout/content goldens | 24–26, 37–39 |

## Incremental evaluation

| Crate | Files | What it does | Decisions |
| --- | --- | --- | --- |
| `layout` | `src/incremental.rs`, entry in `src/lib.rs` | Exact-input preparation/shaping/flow/region/final-pass caches, computed dependency and reverse-inspection graph, revision-gated viewport jobs beside the full-layout reference | 16, 26-28, 39 |
| `fixtures` | `tests/incremental.rs` | Seeded text/style/template/relation/split/join/concurrent edits of every hostile fixture and spike; exact equivalence, counters, budgets, cancellation, revision/identity rejection, identical-byte anchor changes, line-height-only shaping reuse, affected-page counters and 100,000-paragraph viewport test | 27, 38, 39 |
|  | `tests/incremental_fuzz.rs` | Seeded random edit walks (deleting pointed-at blocks, owner edits, frame-relative styles then template changes, concurrent deletes, new notes, floats and follows); every step equals `Engine::layout` | 16, 27, 38 |

| Crate | Files | What it does | Decisions |
| --- | --- | --- | --- |
| `layout` | `src/incremental/tests.rs` | Cyclic/missing/deep computed graphs, unified inputs, extreme and reversed viewports | 16, 28, 37, 39 |
| `layout` | `src/flow.rs`, `src/table.rs`, `src/notes.rs`, `src/floats.rs` | Owned resumable flow cursor, optional exact-input memo hooks and actual work counters; reference disables reuse | 26-28 |
| `layout` | `src/regions.rs`, `src/regions/tests.rs`, `src/relations/mod.rs`, `src/template.rs` | Shared bounded-feedback outcome/freeze helpers, exact comparable templates, explicit relation/reading stages; unchanged reference output | 26, 37, 38 |
| `fixtures` | `src/hostile.rs`, `tests/hostile.rs`, paired `incremental_page_seam` snapshots | UTF-8 pagination seam with a following annotation and explicit reading precedence | 27, 38, 39 |
| - | `docs/incremental.md`, additive section in `docs/contracts.md` | Exact memo keys and reuse argument, scheduling/partial semantics, limits and Salsa evaluation | 16, 26-28, 39, 41 |
