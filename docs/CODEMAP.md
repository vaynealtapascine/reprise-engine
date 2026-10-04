# Code map

Each crate lives in `crates/<name>` and is published to the workspace as `reprise-<name>`.
They are listed in dependency order. [contracts.md](contracts.md) describes the frozen
interfaces.

| Crate | Files | What it does | Decisions |
| --- | --- | --- | --- |
| `diag` | `src/lib.rs` | `Note`, `Severity`, `Code`: diagnostics shared by every library | 37, 39 |
| `geom` | `src/lib.rs` | `Length` (1/1024 pt) and `Fixed` (16.16), saturating and rounding rules, `Matrix`, typed spaces, `Point`, `Rect`, `Transform` | 19, 20 |
| `text` | `src/lib.rs` | `Text` (byte offsets over Loro), `Anchor`, `Affinity`, `RangePolicy`, `Resolved` | 09, 10, 12 |
|  | `src/segment.rs` | Grapheme and word boundaries (ICU4X) | 09, 10 |
| `doc` | `src/lib.rs` | `Document`: content tree, ranges, styles, relations, revisions, fork and merge | 05–07, 15, 29 |
|  | `src/relation.rs` | `Relation`, `Target`, `StructuralQuery`, `LayoutQuery`, `SnapshotRef`, `RelationSchema`, `SchemaRegistry`, the built-in schemas (`reprise.follow`, `reprise.reference`), `Dependency`, copy planning (`plan_copy`, `CopySet`, `IdMap`) | 13, 14, 15, 27, 35 |
|  | `src/structure.rs` | Tree navigation, structural queries (`evaluate`), succession links (`supersede`, `succession`) | 06, 13, 15 |
|  | `src/history.rs` | `DocumentAt` (a past version), `HistoryCache`, snapshot resolution, `compact_history` | 07, 13 |
|  | `src/resolve.rs` | `resolve_target` / `resolve_relation`: any non-layout target, with `OnTargetDeleted` applied from tombstones; `dead_relations` | 13, 14, 15 |
|  | `src/relation_tests.rs` | Tests for the three files above and for relation.rs | 13, 14, 15, 35 |
|  | `tests/history_hostile.rs` | Forged revisions resolve or report, never panic | 07, 13, 37 |
|  | `src/style.rs` | `Style`, `LengthExpr`, `ComputedStyle`, defaults | 08, 17, 18 |
| `font` | `src/lib.rs` | `Face` with a pinned `FaceId`, metrics, glyph outlines and one type-erased adapter-data cache slot; `FontStore` | 21, 22, 38 |
| `shape` | `src/lib.rs` | `ShapingAdapter` contract, `HarfRust` with per-face data caching, `ShapedText`/`ShapedRun`, `Reshape`, `visual_order` | 22, 38 |
|  | `src/paragraph.rs` | `itemize` (fallback chains, resolved bidi levels and contextual scripts), `Shaper` (shaping and reshaping a paragraph) | 09, 21, 22 |
|  | `src/unicode.rs`, `src/bidi-character-subset.txt` | ICU4X property adapter for UAX #9 (including N0) and UAX #24 script resolution; pinned Unicode conformance subset | 09, 22, 38 |
|  | `src/line.rs` | Pure L1/L2 line reordering, preserving bidi groups across missing-font gaps | 20, 22, 30 |
|  | `HANDOFF.md` | Unicode/version/cache bounds and instructions for layout's line-helper integration | 21, 22, 38 |
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
|  | `src/svg.rs`, `src/png.rs`, `src/pdf.rs` | Backends; PDF ToUnicode, cluster ActualText and logical-run fallback for RTL/malformed ranges | 32, 33 |
|  | `tests/pdf_text.rs` | Generated-PDF extraction through lopdf plus bfchar/ActualText reader; ligatures, clusters, RTL, malformed ranges, ZWJ and empty runs | 33, 37, 39 |
| `layout` | `src/lib.rs` | `Engine` (configuration, including `schemas`), `PageSettings`, `Engine::layout` | 24, 26 |
|  | `src/codes.rs` | Diagnostic codes reported by layout and the relation pass | 37 |
|  | `src/snapshot.rs` | `LayoutSnapshot` (pages, frames, blocks), `RelationLayout`, `Resolution`, `Diagnostic`, queries | 05, 13, 37 |
|  | `src/flow.rs` | Pass 1: shaping, composition and stacking blocks in the main frame | 24 |
|  | `src/relations/mod.rs` | Pass 2: dispatching relations on their schema | 13–15, 26 |
|  | `src/relations/follow.rs` | `reprise.follow`: placing a block beside the line it follows | 13, 15 |
|  | `src/relations/resolve.rs` | `Resolver`: any `Target` to a `TargetLayout`, status and diagnostics; how `follow` switches over is documented at the top | 13, 14, 15 |
|  | `src/query.rs` | Layout queries added by relations (`first_line`, `frame_of`, `answer`, ...) | 13, 16 |
|  | `src/display.rs` | `to_display_list(s)` and individually selectable debug boxes, baselines, available/used intervals, run boundaries, break symbols, reshaped lines, relation statuses and diagnostic cluster markers | 32, 39 |
| `fixtures` | `src/lib.rs` | Pinned fonts, engine and peers for tests | 38, 39 |
|  | `src/spike.rs` | The spike document | 40 |
|  | `src/hostile.rs` | Hostile fixtures every workstream must keep passing, including `display_text_clusters` (ligature, stacked marks, RLO and ZWJ) | 37, 39 |
|  | `tests/hostile.rs`, `tests/snapshots/` | Invariant checks, content-preservation goldens and debug explainability geometry snapshots | 38, 39 |
|  | `tests/relations.rs` | Relation targets, queries and deletion policies through layout | 13, 14, 15 |
| `cli` | `src/lib.rs` | `write_outputs`: every output format for a snapshot | 32 |
|  | `src/main.rs` | The `reprise spike [OUT_DIR]` command | 40 |
|  | `tests/spike.rs`, `tests/snapshots/` | End-to-end spike tests and JSON fixtures | 38, 40 |

Other files:

-   `fixtures/fonts/`: the bundled test font (Source Serif Pro, OFL) and its license.
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
    2.  Resolve it in `Document::computed_style`, recording its source in `explain` and
        clamping it if negative values make no sense.
-   **Add a diagnostic:** add a `Code` constant to the reporting crate's `codes` module
    (`layout/src/codes.rs` for layout) and to the table in `contracts.md`.
-   **Add a hostile fixture:**
    1.  Add a function in `fixtures/src/hostile.rs` and add it to `all()`.
    2.  Add a test in `fixtures/tests/hostile.rs`.
    3.  Record its snapshot with `INSTA_UPDATE=always`, then read it.
