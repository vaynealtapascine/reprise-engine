# Frozen contracts

These are the interfaces that parallel workstreams build against (decision 40, step 2),
frozen on 2026-10-03. Each section says where the contract lives, what it guarantees, and
which parts are deliberately left open for a workstream to fill in.

## Changing a contract

A frozen contract changes only in a dedicated commit (`feat(contracts): …`) that:

-   says why the change is needed and which workstreams it affects
-   updates this document
-   keeps every fixture in `crates/fixtures` passing

A workstream that needs a change asks the orchestrator. It doesn't make the change in its
own branch. Additions that the extension points below allow aren't contract changes:

-   a new `Code`
-   a new `LayoutQuery` or `Target` variant for a reserved class
-   a new display item drawn by every backend

## Determinism (applies to every contract)

-   Everything that feeds layout is integer arithmetic on `Length` (1/1024 pt) and `Fixed`
    (16.16). Every division rounds half away from zero. Arithmetic saturates at
    `Length::MIN` and `Length::MAX`; it never wraps or panics. Division by zero saturates.
-   Every trait method below is a pure function of its inputs: no time, randomness, I/O, or
    output that depends on hidden state. Caches are allowed if they can't change output.
-   Nothing order-dependent reaches output from a `HashMap` or `HashSet`. Use `BTreeMap`,
    or sort.
-   Positions are UTF-8 byte offsets everywhere outside `reprise-text`.

## Diagnostics: `reprise-diag`

-   Libraries below layout report a `Note`: a `Severity` (`Info` < `Warning` < `Error`),
    a stable `Code`, a message and optionally source bytes. Layout attaches a `Subject`
    (`Document`, `Node`, `Relation` or `Range`) and publishes it as a `Diagnostic`.
-   Codes are dotted, and the prefix names the reporting library. **Codes are public:
    renaming one is a contract change.** Tests match on codes, never on messages.
-   Severity rules:
    -   `Info`: the output is what the author asked for.
    -   `Warning`: the output differs from what the author asked for.
    -   `Error`: something was left out of the output.
-   Nothing panics on document content (37). Layout reports what it couldn't do, and the
    rest still lays out.

Codes in use:

| Library | Codes |
| --- | --- |
| font / shape | `font.fallback`, `font.missing`, `shape.bad-style-run` |
| compose | `compose.overflow`, `compose.geometry-stalled` |
| layout | `layout.malformed-block`, `layout.style`, `layout.style-clamped`, `layout.text-unplaced`, `layout.frame-overflow`, `layout.unplaced`, `layout.template-unreadable`, `layout.template-unusable`, `layout.degenerate-frame`, `layout.page-limit` |
| relations | `relation.unreadable`, `relation.unknown-schema`, `relation.not-applied`, `relation.missing-target`, `relation.rebound`, `relation.bad-target`, `relation.owner-not-placeable`, `relation.owner-deleted`, `relation.no-match`, `relation.pushed`, `relation.no-frame` |

## Text store: `reprise-text`

-   **Positions:** three kinds (09).
    -   Storage offsets are bytes.
    -   Navigation positions are grapheme and word boundaries, from ICU4X (`segment`).
    -   Persistent positions are `Anchor`s.
-   **`Text`:** has `len`, `slice`, `insert`, `delete`, `anchor` and `resolve`. It also has
    grapheme and word navigation. An offset that isn't on a character boundary, or a
    reversed range, is a `TextError`, never a panic.
-   **Anchors:**
    -   An anchor has an `Affinity`: `Before` sticks to the previous character, `After` to
        the next.
    -   It resolves to `Resolved::Live(offset)`, or to `Tombstoned(offset)` when its
        character was deleted.
-   **`RangePolicy`:** start and end affinity, plus what an emptied range does
    (`Empty::Missing` or `Keep`). The presets are `EXPANDING`, `FIXED` and `POINT`.
-   **Loro stays inside.** `Text::from_loro` and `Text::loro` exist only for `reprise-doc`.
    No other crate touches Loro types.
-   **Open:** words are derived at query time and never stored (10). Author-marked ranges
    are the durable mechanism.

## Relations: `reprise-doc::relation`

-   **A `Relation` is authored data:**
    -   a `SchemaId`
    -   an optional owner
    -   targets grouped by role (`BTreeMap<String, Vec<Target>>`)
    -   typed `Param`s
-   **A `RelationSchema` declares:**
    -   roles: the target classes each accepts, and how many
    -   params
    -   `Ownership` (`Owned` or `Independent`)
    -   `OnTargetDeleted` (`Rebind`, `KeepMissing` or `Delete`)
    -   a `CopyPolicy`
-   **The registry:** schemas live in a `SchemaRegistry`, which is engine configuration.
    `SchemaRegistry::builtin()` registers `reprise.follow`. `validate` checks a relation
    against its schema; `Document::add_relation` validates before storing.
-   **Layout behaviour is not part of the schema.** Layout dispatches on `SchemaId`. A
    relation whose schema isn't registered is kept in the document and reported with
    `relation.unknown-schema` (34). A registered schema with no layout behaviour is
    reported with `relation.not-applied`.
-   **An owned relation goes with its owner.** If the owner is deleted, layout reports
    `relation.owner-deleted` (`Info`) and status `OwnerDeleted` (07, 14).
-   **Open:**
    -   `TargetClass::Structural` and `TargetClass::Snapshot` are reserved; their `Target`
        variants come with the relations workstream.
    -   `LayoutQuery` has only `LineContaining` so far.
    -   `Ambiguous` exists in `RelationStatus` but nothing produces it yet.

## Styles: `reprise-doc::style`

-   **Resolution:** a `Style` has optional `parent`, `family`, `size` and `line_height`.
    Resolution goes engine defaults, then the named style chain (parents first), then
    direct overrides.
-   **`LengthExpr`:** `Pt(Length)` or `Em(permille)`.
    -   An em size is relative to the inherited size.
    -   Line height resolves against the final size.
-   **`ComputedStyle`:** holds used values (08).
    -   `explain` names the layer that set each property (39).
    -   **Used lengths are never negative.** Negative values are clamped to zero and listed
        in `clamped`, and layout reports each with `layout.style-clamped`.
-   **Open:** typed expressions with registered pure functions (17), and resolution
    contexts beyond em (18), come with the relations-and-document workstream.

## Shaping: `reprise-shape`

-   **Three steps:**
    1.  `itemize(ParagraphInput, &FontStore)` splits the paragraph into `Item`s. Each item
        has one face, size, bidi level and script. Fallback chains pick the first available
        family and report `font.fallback`. Text with no available face is left out and
        reported with `font.missing`.
    2.  `Shaper::shape()` shapes every item with the configured adapter.
    3.  `Reshape::reshape(range)` shapes part of the paragraph again as a line on its own.
-   **`ShapingAdapter::shape(&ShapeRequest) -> Vec<ShapedGlyph>`:**
    -   It is pure.
    -   Glyphs come back in visual order for the run's direction.
    -   Every cluster is a paragraph byte offset inside `request.range`, on a character
        boundary, and never splits a grapheme.
    -   `context` is the text the shaper may look at around the range.
    -   It never panics. Unknown characters give `.notdef` (glyph 0).
-   **`AdapterInfo`:** the adapter's `name` and `version`, plus `platform_independent`.
    That flag decides whether the cross-platform guarantee covers the adapter (22, 38).
-   **Bidi:** `visual_order(levels)` implements UAX #9 rule L2.
-   **Open:** the bidi algorithm and script itemisation. Every item currently takes the
    paragraph's base level, and its script is left to the adapter.

## Composition: `reprise-compose`

-   **Space:** everything is in a frame's logical space. The inline axis runs along a line;
    the block axis runs from one line to the next. Writing mode, rotation and mirroring
    live in the frame's transform, never here (20).
-   **`GeometryProvider::available(&LineQuery) -> Available`:**
    -   It is pure.
    -   The query has the line index, its block offset, the line height and the previous
        fragments.
    -   It answers with one of:
        -   `Room(intervals)`: several intervals per line are allowed, for runarounds.
        -   `Skip { next }`: move down to `next`, which must move down.
        -   `End`: the region is full.
-   **`Composer::compose(&ComposeRequest) -> Composition`.** Every composer guarantees:
    -   Fragments cover the text from `start` contiguously, up to `rest` or the end.
    -   Empty text still gets one empty line, so a caret has somewhere to go.
    -   Lines break only at `breaks`, except an overflowing unbreakable run, which is
        reported with `compose.overflow`.
    -   Forced breaks always end a line.
    -   A fragment whose start or end isn't safe to break is reshaped.
    -   Composition always terminates. `MAX_CONSECUTIVE_SKIPS` stops a geometry provider
        that keeps answering `Skip`, and the stall is reported with
        `compose.geometry-stalled`.
-   **`LineFragment`:** carries its `Explanation`: the `BreakReason`, an optional score,
    an `Adjustment` (word and letter spacing), and whether it was reshaped (39).
-   **Breaks:** `break_opportunities` gives UAX #14 breaks (`Allowed` or `Forced`, each with
    a penalty). The end of the text is always the last one.
-   **Open:** only `Greedy` exists. The optimal and authored-break composers, and
    non-rectangular providers, come with the composition workstream.

## Layout snapshot: `reprise-layout::snapshot`

-   **Structure:** a `LayoutSnapshot` has `pages`, `frames` and `blocks`, plus `relations`
    and `diagnostics`. It also records the `medium`, the flow `settings` and the page
    `template` it was made from (38). Pages are made from the template as the flow needs
    them, up to the engine's page limit; every frame has a `role` (a flow, or margin).
    -   Each frame has a page and a `to_page` transform.
    -   Line geometry is in its frame's logical `FrameSpace`.
    -   Use `line_to_page` or `line_bounds` to get page coordinates.
-   **Provenance:** the snapshot records its `revision`, the adapter's `AdapterInfo` and
    the composer's name, so every result says which inputs produced it (28, 38).
-   **Queries:**
    -   `block`, `line`, `frame`, `relation`
    -   `line_containing(node, at)`: a position at a line's end belongs to that line.
    -   `previous_line` and `next_line`
    -   `lines_in(node, bytes)`
    -   `line_to_page` and `line_bounds`
    -   `diagnostics_with(code)`

    These queries are the API that relations and the editing kernel use.
-   **Relation results:** every relation gets a `RelationLayout`, with its overall status,
    whether it was `applied`, and each target's status and `Resolution`.
-   **Invariants**, checked on every hostile fixture:
    -   Every block has at least one line.
    -   Lines cover the text contiguously and start on grapheme boundaries.
    -   No line sits above its frame's top.
    -   Every glyph belongs to its own run.
    -   Every query agrees with itself.

## Display list: `reprise-display`

-   **One `DisplayList` per page.** Items are:
    -   `Glyphs(GlyphRun)`
    -   `Path`, with an optional fill and stroke
    -   `Group`, with a `Matrix` transform, an optional clip and children

    A rotated or mirrored frame is one group (20).
-   **Source text:** every `GlyphRun` carries the `text` it draws, and each `Glyph` carries
    the byte range of that text it came from. That is what lets exporters make text
    selectable and accessible.
-   **Layers:** `Content` and `Debug`. `content_only()` drops the debug overlay.
-   **Resources:** faces are named by `FaceId` and belong to the backend's `FontStore`, not
    to the list.
-   **Backends:**

    | Function | Pages |
    | --- | --- |
    | `svg::render(&list, &fonts)` | one |
    | `png::render(&list, &fonts, pixels_per_pt)` | one |
    | `pdf::render(&[list], &fonts)` | all |

    All three draw every item. Adding an item kind means drawing it in all three.
-   **Open:** the PDF backend doesn't pass the source text on yet (ToUnicode and
    ActualText). Image items come with document assets (34).

## Fixtures: `reprise-fixtures`

-   **Pinned inputs:** `fonts()` and `engine()` use only the bundled font. Peer IDs are 1,
    and 2 for second replicas.
-   **`spike`:** the end-to-end spike document.
-   **`hostile`:** eighteen documents built to break things:
    -   `empty_text`
    -   `combining_marks`
    -   `emoji_zwj`
    -   `rtl_mixed`
    -   `overlong_word`
    -   `zero_width_measure`
    -   `deleted_targets`
    -   `concurrent_edits`
    -   `extreme_lengths`
    -   `frame_shorter_than_a_line`
    -   `no_main_flow`
    -   `negative_page_size`
    -   `zero_sized_frames`
    -   `page_limit`
    -   `unreadable_template`
    -   `column_storm`
    -   `concurrent_templates`
    -   `no_margin_frame`
-   **`tests/hostile.rs`** runs every hostile fixture and checks:
    -   determinism, and that replicas converge to the same layout
    -   the expected diagnostic codes, and no unexpected Warning or Error
    -   the line and query invariants above
    -   that every frame is on a page that exists, every line is inside its frame unless
        its block carries `layout.frame-overflow`, and text follows the order frames thread in
    -   every backend
    -   a JSON snapshot
-   **Rules for workstreams:**
    -   Every merge keeps every hostile fixture passing.
    -   A snapshot changes only with a stated reason.
    -   New hostile cases are welcome. Removing one is a contract change.
