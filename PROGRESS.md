# Progress

The hand-off log. Newest first.

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
