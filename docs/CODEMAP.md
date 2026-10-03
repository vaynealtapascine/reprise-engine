# Code map

Each crate lives in `crates/<name>` and is published to the workspace as `reprise-<name>`.
They are listed in dependency order.

| Crate | Files | What it does | Decisions |
| --- | --- | --- | --- |
| `geom` | `src/lib.rs` | `Length` (fixed point, 1/1024 pt), typed spaces (`PageSpace`, `FrameSpace`, `LineSpace`), `Point`, `Rect`, `Transform` | 19, 20 |
| `text` | `src/lib.rs` | `Text` (a Loro text, byte offsets), `Anchor`, `Affinity`, `RangePolicy`, `resolve` | 09, 10, 12 |
| `doc` | `src/lib.rs` | `Document`: content tree, named ranges, relations, styles, revisions, fork and merge | 05–08, 13–15, 29 |
| `font` | `src/lib.rs` | `Face` with a pinned `FaceId`, metrics, glyph outlines; `FontStore` | 21 |
| `shape` | `src/lib.rs` | `ShapingAdapter` contract, `HarfRust` default adapter, `ShapedText` | 22 |
| `compose` | `src/lib.rs` | `Composer` (`Greedy`), `GeometryProvider` (`Measure`), break opportunities (ICU4X), `LineFragment` | 23 |
| `display` | `src/lib.rs` | `DisplayList`, `Item`, `RenderError` | 32 |
|  | `src/svg.rs`, `src/png.rs`, `src/pdf.rs` | Backends | 32 |
| `layout` | `src/lib.rs` | `Engine` (configuration), `PageSettings`, `Engine::layout` running the passes in order | 24, 26 |
|  | `src/snapshot.rs` | `LayoutSnapshot`, `BlockLayout`, `LineLayout`, `RelationLayout`, `Diagnostic`, queries | 05, 13, 37 |
|  | `src/flow.rs` | Pass 1: shaping, composition and stacking blocks in the main column | 24 |
|  | `src/relations.rs` | Pass 2: resolving relations and placing the blocks they position | 13, 15, 26 |
|  | `src/display.rs` | `to_display_list` and the debug overlay | 32, 39 |
| `cli` | `src/lib.rs` | The spike fixture document and `write_outputs` | 40 |
|  | `src/main.rs` | The `reprise spike [OUT_DIR]` command | |
|  | `tests/spike.rs`, `tests/snapshots/` | End-to-end tests and JSON fixtures | 38, 39 |

Other files:

-   `fixtures/fonts/`: the bundled test font (Source Serif Pro, OFL) and its license.
-   `.github/workflows/ci.yml`: CI.

## Recipes

-   **Add a relation kind or target query:**
    1.  Add the variant to `RelationKind` or `Target` in `doc`.
    2.  Resolve it in `layout/src/relations.rs`.
    3.  Cover it in `cli/tests/spike.rs`.
-   **Add a shaping adapter:** implement `ShapingAdapter` in `shape`, or in a new crate if it
    pulls in platform code. Set `platform_independent` honestly.
-   **Add a composer or geometry provider:** implement `Composer` or `GeometryProvider` in
    `compose`. `Engine.composer` selects which composer layout uses.
-   **Add a display item:**
    1.  Add the variant to `Item` in `display`.
    2.  Draw it in all three backends.
    3.  Emit it from `LayoutSnapshot::to_display_list` in `layout/src/display.rs`.
-   **Add a style property:**
    1.  Add the field to `Style` (with `read` and `write`) and `ComputedStyle` in `doc`.
    2.  Resolve it in `Document::computed_style`, recording its source in `explain`.
