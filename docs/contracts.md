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
| font / shape | `font.fallback`, `font.missing`, `shape.bad-style-run`, `shape.script-depth`, `shape.bad-line` |
| font / shape | `font.fallback`, `font.missing`, `shape.bad-style-run` |
| compose | `compose.overflow`, `compose.geometry-stalled`, `compose.fallback` |
| layout | `layout.malformed-block`, `layout.style`, `layout.style-clamped`, `layout.text-unplaced`, `layout.frame-overflow`, `layout.unplaced`, `layout.template-unreadable`, `layout.template-unusable`, `layout.degenerate-frame`, `layout.page-limit`, `layout.solver-infeasible`, `layout.solver-underconstrained`, `layout.solver-limit`, `layout.region-cycle`, `layout.region-limit`, `layout.region-parameter`, `layout.float-deferred`, `layout.float-unplaceable`, `layout.note-continued`, `layout.note-depth`, `layout.table-invalid`, `layout.table-limit` |
| relations | `relation.unreadable`, `relation.unknown-schema`, `relation.not-applied`, `relation.missing-target`, `relation.rebound`, `relation.bad-target`, `relation.owner-not-placeable`, `relation.owner-deleted`, `relation.no-match`, `relation.pushed`, `relation.ambiguous`, `relation.target-deleted`, `relation.snapshot-unavailable`, `relation.self-reference`, `relation.rebind-limit`, `relation.no-frame` |
| style | `style.unparsed`, `style.expr-limit`, `style.type-error`, `style.unknown-function`, `style.function-failed`, `style.basis-unresolved`, `style.basis-indefinite`, `style.cycle`, `style.saturated`, `style.divide-by-zero`, `style.parent-cycle`, `style.parent-missing`, `style.chain-too-long` |
| format | `format.invalid`, `format.limit`, `format.cache-dropped`, `format.cache-ignored`, `format.asset-hash`, `format.font-hash`, `format.font-unreadable`, `format.asset-missing`, `format.migrated`, `format.read-only` |

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
-   **Targets** (13):
    -   `Target::Structural(StructuralQuery)`: siblings, children, the nth child or the
        parent of a block (the document root when `of` is `None`), optionally filtered by
        `BlockKind`. Only `Children` can match several.
    -   `Target::Snapshot(SnapshotRef)`: a node or range as it was at a `Revision`. An
        unreadable version is `relation.snapshot-unavailable`. A subject deleted since is
        still `Valid`, because a snapshot reference is meant not to change.
    -   `LayoutQuery`: `LineContaining`, `PreviousLine`, `NextLine`, `FirstLine`,
        `LastLine`, `LinesIn`, `FrameContaining` and `PageContaining`. Only `LinesIn` can
        match several.
    -   Zero matches is `Missing` with `relation.no-match`. A role that takes one target
        but gets several candidates is `Ambiguous` (`relation.ambiguous`): none is chosen,
        and all are listed.
-   **Deletion policies** (14, 15) are evaluated at resolution time from tombstones, never
    by mutating the document, so a deletion merged from another peer behaves exactly like
    a local one.
    -   `Rebind` follows succession links (`Document::supersede`) to the nearest
        generation of live successors. Several equally near is `Ambiguous`, and no
        successor is `Missing`.
    -   `KeepMissing` reports the target `Missing`.
    -   `Delete` takes the whole relation out of effect: status `Deleted`, with
        `relation.target-deleted` (`Info`).
-   **Copying:** `plan_copy` decides, per relation, whether a copy duplicates it with
    remapped IDs, keeps it pointing outside, or drops it, following its `CopyPolicy` (35).
-   **Dependencies:** `Relation::dependencies()` lists what resolving it reads, in order,
    for incremental evaluation (27).
-   **Layout behaviour:** `reprise.follow` places its owner; `reprise.reference` only
    resolves and reports. Every other registered schema has its targets resolved and
    reported, plus `relation.not-applied`.

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
-   **Expressions (17):** `reprise-doc::expr`. Bounded typed expressions, like `calc()`.
    -   **Types:** length, number, percentage and ratio, checked before evaluation.
        `length * length` is a type error. Units are `pt`, `em`, `lh` and `%`, plus
        geometric references (`medium-`, `page-`, `frame-`, `block-`, `line-` `width` and
        `height`, `frame-width("name")`, `nearest-width("level")`). Operators are
        `+ - * /`, with `min`, `max` and `clamp`.
    -   **Bounds:** at most 256 nodes, 32 deep and 2048 bytes of text, enforced when an
        expression is parsed or built. Nothing recurses past them.
    -   **Numbers:** integer arithmetic only. Every operation saturates (`style.saturated`),
        every division rounds half away from zero, and division by zero saturates by the
        sign of the numerator (`style.divide-by-zero`).
    -   **Dependencies (27):** `Expr::dependencies` returns an ordered set of `Dependency`
        values: `Em`, `Lh`, `PercentBasis`, a `Basis`, or a `Function`.
    -   **Stored form:** one string per property: `pt:N` or `em:N` (the original form),
        or `expr1:<text>`. The text form is canonical and round-trips. A value this engine
        can't read is **kept verbatim and reported** (`style.unparsed`), never dropped (34).
    -   **Functions (17, 36):** a `FunctionRegistry` is engine configuration, like
        `SchemaRegistry`. Functions are pure and deterministic, with a declarative
        `Signature`. An unknown function is kept and reported (`style.unknown-function`).
        `ratio`, `scale` and `round-to` are built in.
-   **Resolution contexts (18):** `reprise-doc::context`. A `ResolutionContext` has a
    medium, page, frame (current and named), block and line, each with a width and height
    that is `Definite`, `Indefinite` (depends on its own content, like an auto-height
    frame) or `Unresolved`. The default context resolves nothing.
    -   `em` is the element's font size, except in `size`, where it is the inherited size.
    -   `lh` is the used line height. It is a cycle (`style.cycle`) in `size` and
        `line-height`.
    -   `%` is of the property's declared basis: the inherited size for `size`, the
        element's size for `line-height`. `50% * frame-width("main")` names another basis.
    -   A basis that is unresolved or indefinite counts as zero (`Warning`), and a style
        layer that depends on one is skipped: the property keeps the value it inherited.
-   **The four stages (08):** `Specified` (the layers), then inherited and `Computed`
    (symbolic where it depends on a context), then used (`Computed::used`, giving a
    `StyleResolution`). `Document::computed_style` is the default context.
    `computed_style_in` and `computed_style_with` take a context and a function registry.
    `resolve_style` also explains all four stages for each property.
-   **`ComputedStyle.notes`:** the `style.*` problems met while resolving. `bases` says what
    percentages and references were resolved against. Both are skipped when empty, so
    styles that use only `pt` and `em` serialise as they always did.
-   **Named-style chains:** a parent cycle, a missing parent and a chain over 32 styles
    cut the chain at the problem and report it (`style.parent-cycle`,
    `style.parent-missing`, `style.chain-too-long`). What was collected stays in use.
-   **Open:** `Param::Length` still holds a `LengthExpr`. Letting relation parameters hold
    an `Expr` is a follow-up.

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
-   **Bidi and scripts:** `itemize` resolves UAX #9 levels (paragraph rules P2/P3 when no
    direction is given) and UAX #24 scripts from ICU4X data, whose Unicode version
    (`UNICODE_VERSION`) is part of the reproducibility envelope. Items split wherever the
    face, size, level or script changes. `Itemized.levels` holds the resolved level of
    every byte.
-   **Lines:** `reorder_line` applies rules L1 and L2 to one composed line and returns its
    runs in visual order.

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
-   **Composers:** `Greedy`; `Optimal` (Knuth–Plass, ragged by default or justified, each
    line's demerits in its score); and `AuthoredBreak` (verse: lines end at forced breaks,
    and long lines turn over with a hanging indent). An optimising composer that can't stay
    optimal (geometry that depends on earlier lines, or its search limits) sets the rest
    first-fit with `score: None` and reports `compose.fallback` (`Info`).
-   **Geometry:** `Measure`, `Polygon` (exact band intersection, several intervals for
    concave shapes) and `Runaround` (a provider minus exclusions, with a margin and a
    minimum width).
-   **Conformance:** `compose/tests/conformance.rs` runs every composer against adversarial
    geometry, breaks, shaping and texts. A new composer must be added to it.

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
    -   `RelationStatus` is `Valid`, `Rebound`, `Ambiguous`, `Missing`, `OwnerDeleted` or
        `Deleted`, in increasing order of severity. The overall status is the worst
        target's.
    -   `Resolution` is `Node`, `Range`, `Line`, `Nodes`, `Lines`, `Frame`, `Page` or
        `Snapshot`.
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
-   **Source text in PDF:** the PDF backend maps glyphs to their source text with ToUnicode,
    and uses ActualText where one glyph can't carry it: shared clusters, right-to-left runs
    and malformed ranges. Text extracts in logical order within each run.
-   **Open:** image items come with document assets (34). A reading order across runs
    (33) needs the reading-order workstream.

## Fixtures: `reprise-fixtures`

-   **Pinned inputs:** `fonts()` and `engine()` use only the bundled font. Peer IDs are 1,
    and 2 for second replicas.
-   **`spike`:** the end-to-end spike document.
-   **`hostile`:** documents built to break things, one per function in `all()`:
    -   `empty_text`
    -   `combining_marks`
    -   `emoji_zwj`
    -   `rtl_mixed`
    -   `overlong_word`
    -   `zero_width_measure`
    -   `deleted_targets`
    -   `concurrent_edits`
    -   `extreme_lengths`
    -   `bidi_stray_controls`
    -   `bidi_override_ligature`
    -   `scripts_common_inherited`
    -   `display_text_clusters`
    -   `structural_matches`
    -   `snapshot_targets`
    -   `snapshot_compacted`
    -   `concurrent_policy_deletion`
    -   `self_reference`
    -   `optimal_paragraph`
    -   `verse_turnover`
    -   `optimal_extreme_lengths`
    -   `style_expressions`
    -   `style_cycles`
    -   `style_bases`
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

## File format: `reprise-format`

- `Package` is one versioned container with a checksummed magic/version header,
  persistent host-assigned `DocumentId`, required and optional feature masks,
  and ordered checksummed sections. [The v1 spec](../crates/format/SPEC.md) pins
  the wire encoding, hard bounds, compatibility and diagnostic severity rules.
- `Document::export(PersistenceMode) -> Vec<u8>`, `try_export(...) ->
  Result<Vec<u8>, DocError>` and `import(bytes, peer) -> Result<Document, DocError>`
  live in `doc/src/persist.rs`. Only doc sees Loro; format carries its snapshot
  opaquely. History is retained unless Shallow is explicitly requested. The
  infallible convenience returns an empty invalid blob on encoding failure;
  package APIs always use try_export. Import validates self-contained snapshots,
  bounds embedded LZ4 expansion before Loro runs, and never chooses a random peer.
- Unknown optional features, unknown sections (including flags/codec/payload),
  extension bytes and untouched manifest fields round-trip verbatim. Unknown
  required features/sections refuse with typed `FormatError`s. Corrupt authored
  data or ambiguous framing refuses; safely framed corrupt caches are discarded
  with Info `format.cache-dropped`. All parsing is bounded and length-checked.
- `FontPin` records FaceId family/hash plus an explicit host-supplied version.
  `Asset` records bundled bytes or external path/URL and a full content hash.
  `AssetAvailability` returns needed, verified bundled, missing, usable fonts
  and notes. Fonts with mismatched bytes or identities are reported and excluded.
  External resources are never fetched; the host resolves them.
- `MigrationRegistry` applies pure N -> N+1 container migrations on open and
  reports each step. Saves write the current version. Compatible newer files
  expose `DocumentAt` read-only; editable access and saving return `ReadOnly`.
  Container migrations never duplicate authored schema migrations inside Loro.
- `CacheTags` binds opaque derived bytes to document ID, revision, engine
  version, complete AdapterInfo and canonical engine-configuration hash. Bytes
  are exposed only through `usable_cache` with a matching context. Stale caches
  are ignored (Info `format.cache-ignored`); caches never enter Loro. Producing
  and interpreting layout caches is layout/host work.
- A saved snapshot reference must have a document identity: `SnapshotReference`
  pairs `DocumentId` with `SnapshotRef`. Existing in-document references are
  scoped by their containing document. A Revision alone identifies no document.
