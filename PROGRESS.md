# Progress

The hand-off log. Newest first.

## 2026-10-03: phase 2 started (decision 40, step 3)

**In flight: wave 1.** Each workstream has its own worktree under `D:/!!Self/dev/reprise-wt/`
and its own `ws/*` branch. The shared rules every agent follows are in
`reprise-wt/BRIEF-COMMON.md`.

| Branch | Workstream | Decisions | Model |
| --- | --- | --- | --- |
| `ws/shape` | 1: UAX #9 bidi and UAX #24 scripts in `itemize`, line reordering helper (L1/L2), shaping-data cache | 21, 22 | GPT-6.1-Sol |
| `ws/compose` | 2: Knuth–Plass composer, authored-break (verse) composer, polygon and runaround geometry, composer conformance suite | 11, 23 | Opus 5.5 |
| `ws/flow` | 3a: authored page templates (`doc/src/page.rs`) replacing `PageSettings`, a responsive medium, frame threading, pagination and fragmentation | 24, 25 | Sonnet 5.5 |
| `ws/relations` | 4a: `Structural` and `Snapshot` targets, more layout queries, `Ambiguous`, `OnTargetDeleted`, copy planning | 13–16 | Sonnet 5.5 |
| `ws/styles` | 4b: typed bounded expressions, registered pure functions, resolution contexts, explicit value stages | 08, 17, 18 | Sonnet 5.5 |
| `ws/display` | 8: PDF ToUnicode and ActualText, a richer debug overlay | 32, 39 | GPT-6.1-Sol |

**Split to avoid contention:** workstream 3 became 3a (core flow, now) and 3b (floats,
tables, notes and solver domains, after 3a and the geometry providers of 2). Workstream 4
became 4a (relations) and 4b (styles), which use different files. Layout's `codes` and the
`follow` behaviour moved into their own files first (`eabf5d0`).

**Integration the orchestrator does after merging** (each workstream was told not to touch
the other's files):

-   Wire workstream 1's L1/L2 line reordering into `layout/src/flow.rs`.
-   Switch `relations/follow.rs` to workstream 4a's shared target resolver.
-   Pass frame and page resolution contexts from flow into workstream 4b's styles.
-   Apply `Adjustment` (justified Knuth–Plass) to glyph positions in layout.

**Wave 2, queued:**

-   5: rotated and mirrored frames, writing modes, and reading-order overrides (after 3a and 8).
-   3b: floats, tables, notes and solver domains (after 2 and 3a).
-   7: the editing kernel (after 1 and 3a).
-   9: persistence and clipboard (after 4a).

**Later:** workstream 6 (incremental evaluation and scheduling), once 1–5 stabilise, then 10
(plugins) and 11 (bindings).

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
