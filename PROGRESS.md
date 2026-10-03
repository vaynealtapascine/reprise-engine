# Progress

The hand-off log. Newest first.

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
