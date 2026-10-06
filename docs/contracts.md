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
| font / shape | `font.fallback`, `font.missing`, `font.unreadable`, `font.nearest`, `font.chain-limit`, `shape.bad-style-run`, `shape.script-depth`, `shape.bad-line` |
| font / shape | `font.fallback`, `font.missing`, `shape.bad-style-run` |
| compose | `compose.overflow`, `compose.geometry-stalled`, `compose.fallback` |
| layout | `layout.malformed-block`, `layout.style`, `layout.style-clamped`, `layout.text-unplaced`, `layout.frame-overflow`, `layout.unplaced`, `layout.template-unreadable`, `layout.template-unusable`, `layout.degenerate-frame`, `layout.page-limit`, `layout.transform-unusable`, `layout.path-invalid`, `layout.path-limit`, `layout.reading-cycle`, `layout.reading-conflict`, `layout.reading-missing`, `layout.reading-partial`, `layout.reading-limit`, `layout.reading-revision`, `layout.solver-infeasible`, `layout.solver-underconstrained`, `layout.solver-limit`, `layout.region-cycle`, `layout.region-limit`, `layout.region-parameter`, `layout.float-deferred`, `layout.float-unplaceable`, `layout.note-continued`, `layout.note-depth`, `layout.table-invalid`, `layout.table-limit`, `layout.table-span`, `layout.table-rowspan-split`, `layout.table-header-unrepeated`, `layout.image-record`, `layout.image-missing`, `layout.image-header`, `layout.image-limit`, `layout.image-size` |
| relations | `relation.unreadable`, `relation.unknown-schema`, `relation.not-applied`, `relation.missing-target`, `relation.rebound`, `relation.bad-target`, `relation.owner-not-placeable`, `relation.owner-deleted`, `relation.no-match`, `relation.pushed`, `relation.ambiguous`, `relation.target-deleted`, `relation.snapshot-unavailable`, `relation.self-reference`, `relation.rebind-limit`, `relation.no-frame` |
| style | `style.unparsed`, `style.expr-limit`, `style.type-error`, `style.unknown-function`, `style.function-failed`, `style.basis-unresolved`, `style.basis-indefinite`, `style.cycle`, `style.saturated`, `style.divide-by-zero`, `style.parent-cycle`, `style.parent-missing`, `style.chain-too-long` |
| format | `format.invalid`, `format.limit`, `format.cache-dropped`, `format.cache-ignored`, `format.asset-hash`, `format.font-hash`, `format.font-unreadable`, `format.font-missing`, `format.asset-missing`, `format.migrated`, `format.read-only` |
| edit | `edit.limit`, `edit.invalid-command`, `edit.store` |
| plugin | `plugin.invalid`, `plugin.abi`, `plugin.hash`, `plugin.capability`, `plugin.limit`, `plugin.fuel`, `plugin.trap`, `plugin.result`, `plugin.unavailable` |
| clipboard | `clipboard.invalid`, `clipboard.limit`, `clipboard.version`, `clipboard.html-approximated`, `clipboard.html-dropped`, `clipboard.resource-missing`, `clipboard.resource-hash`, `clipboard.relation-dropped`, `clipboard.style-clash`, `clipboard.range-affinity`, `clipboard.selection-table`, `clipboard.host-range-dropped` |
| export | `export.relations`, `export.reading-order`, `export.transforms`, `export.notes-floats`, `export.tables`, `export.styles`, `export.bidi`, `export.fonts`, `export.assets`, `export.editing-structure` |
| bindings | `bindings.version`, `bindings.invalid`, `bindings.limit`, `bindings.id`, `bindings.stale`, `bindings.cancelled`, `bindings.layout-required`, `bindings.read-only`, `bindings.store`, `bindings.render` |

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
-   **Authored range policies (10, 12, 35):** Ranges persist authored start/end
    affinities and empty policy alongside anchors. Clipboard preserves authored
    policy at text endpoints. Legacy ranges without versioned policy use observable
    cursor behavior and report `clipboard.range-affinity` Warning where the authored
    affinity is ambiguous. `Document::add_range` keeps its signature;
    `Document::range_policy(id) -> Result<Option<RangePolicy>, DocError>` returns
    `None` only for a legacy range. New creation, staging and reanchoring write a
    `policy1` JSON envelope with `version: 1`, `start`, `end` and `empty`. Unsupported
    versions or malformed envelopes are refused and retained verbatim (34); range
    resolution treats them as missing and copy refuses affected ranges. The legacy
    `empty` field is read only when `policy1` is absent. Split/join retain their
    existing anchor lifecycle semantics and preserve stored policies.
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

-   **Additive family chains (21):** `Style.families` replaces `family` in the
    same layer, stored as `families1:<JSON string array>`. Unreadable versions
    are retained in `unparsed_families` and reported with `style.unparsed`.
    A later single `family` clears an inherited explicit chain. Legacy documents
    keep their exact output form; empty `ComputedStyle.families` denotes legacy.


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

-   **Additive explicit-chain entry point:** `itemize_families` appends serif
    when no terminal generic is authored. Generic defaults are pinned bundled
    faces; `FontStore::set_generic` supplies configuration overrides. It searches
    at most 64 named families (`font.chain-limit`, Warning), preserving the
    authored terminal generic. The first face covering a whole grapheme wins,
    so per-character fallback never cuts a cluster. Uncovered graphemes remain
    in the generic default with `.notdef` (`font.missing`, Warning). Each later
    family selection reports `font.fallback` (Warning). Legacy `itemize` and
    single-family documents keep availability-only selection when a named face
    exists; if none exists they use the serif generic default and report
    `font.fallback` (Warning), retaining all text. An authored legacy generic
    selects its engine default directly without a substitution diagnostic.
    Explicit-chain runs use a grapheme's base scalar script/level even when
    a combining mark has its own script, preserving adapter cluster boundaries.

-   **Frontend supply:** `FontDeclaration` declares a family alias, weight 1..1000,
    style and positive stretch in permille, plus a collection index. `FontStore::register`
    rejects unreadable imports with `font.unreadable` (Error); font bytes are bounded
    to 32 MiB. `match_family` searches stretch, style, then weight in CSS-inspired
    order, breaking ties by `FaceId` and reporting nearest matches with `font.nearest`
    (Warning). Versions and collection descriptors supplement the frozen `FaceId`;
    package pins retain them. No synthetic outlines, variable-axis instancing,
    system discovery, license checks or embedding-flag checks are performed.

-   **Three steps:**
    1.  `itemize(ParagraphInput, &FontStore)` splits the paragraph into `Item`s. Each item
        has one face, size, bidi level and script. Fallback chains pick the first available
        family and report `font.fallback`. When no named face is available, the serif
        generic default is used with `font.fallback` (Warning); text is not omitted.
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
-   **Base direction:** each `BlockLayout` records its paragraph's resolved bidi
    `base_level` (0 left to right, 1 right to left; left out of JSON when 0). Carets,
    visual movement and alignment use it instead of re-deriving it.
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

## Tables: header rows and spans (06, 24, 33, 34, 37)

- **Authored (additive):** `CellInfo` gains `colspan` and `rowspan` (default 1,
  omitted from the stored `table1` record when 1, so spanless tables keep their
  bytes). `RowInfo.header` already existed. New setters:
  `Document::append_table_cell_spanned`, `set_table_cell_span`,
  `set_table_row_header`. Values are stored as authored (zero and huge included).
- **One interpretation:** `Document::table_structure(table) -> Result<Option<TableGrid>>`
  resolves spans deterministically from the merged document (first claim wins in
  document order; zero is one; spans are cut to the table edge, to free cells, and
  to the header rows they start in; a cell whose origin is taken or whose column is
  outside the table is dropped). `TableGrid` lists `rows`, placed `cells`
  (`GridCell { node, row, column, colspan, rowspan, header }`),
  `repeating_header_rows` (the leading run of header rows) and `issues`
  (`GridIssue { node, kind }`). `Document::table_cell_structure(cell)` answers
  "is this cell a header, and what does it span?" for one cell. Layout, HTML
  export and PDF/UA tagging must all read this, never the raw record. `None` means
  0 or more than 256 columns.
- **Layout:** rows linked by row spans form a group, composed together. A group
  that does not fit the rest of a frame but fits a fresh one starts on the fresh
  frame; one that fits neither splits with `layout.table-rowspan-split` (Warning).
  Column spans use the sum of the solved widths and raise content min/max bounds.
  Clamped spans report `layout.table-span` (Warning); dropped cells keep
  `layout.table-invalid` (Error).
- **Repeated headers:** after the table continues on a new frame, the leading
  header rows are composed again at its top. Copies are derived only:
  `LayoutSnapshot.repeated_headers: Vec<RepeatedHeader { table, frame, blocks }>`
  (additive snapshot field, omitted from JSON when empty). Copies are not in
  `blocks`, so block queries, reading order and the editing kernel never see them;
  their blocks carry the authored node IDs and byte ranges. The display list draws
  them after the authored blocks of each frame, so authored display-item indices do
  not move. They are artifacts for accessibility. A copy that does not fit, or
  would leave the body no room, is skipped with `layout.table-header-unrepeated`
  (Warning), as is a header that itself spans frames.
- **Editing kernel:** copies are not navigable. A point over a copy resolves to the
  nearest authored line (never an authored header on another page); reading order
  and `Navigator::semantic` visit each authored header block once.
- **File format:** `format::features` declares optional bit 8 `TABLE_HEADERS` and
  required bit 8 `TABLE_SPANS` from live content on every save; readers that know no
  required bit refuse span files with `RequiredFeatures`.
- **Clipboard/export:** HTML import reads `th`, `thead`, `colspan`, `rowspan`
  (at most 128 columns, 4096 rows, 65,536 claimed grid positions; clamps noted with
  `clipboard.html-approximated`); HTML export writes `th` and the resolved spans;
  `export.tables` details say what headers and spans become in HTML and plain text.

## Authored and positioned images (05, 24, 33, 34)

- `BlockKind::Image` is an additive, pre-approved block kind. The block's
  collaborative text is its alt text; empty alt text denotes a decorative image.
- `doc::image::ImageData` names a lowercase SHA-256 asset hash and optional
  `LengthExpr` width and height. Its version-1 JSON lives in the existing block
  envelope under `image1`. Unreadable records remain stored verbatim.
- `FragmentBlock.image: Option<String>` carries that raw record on copy/paste,
  defaults to absent on old fragments, and is omitted from JSON when absent.
- `BlockLayout.image: Option<ImageLayout>` is the additive snapshot extension
  approved in the images brief. It is omitted when absent, preserving all existing
  text goldens. `ImageLayout` records asset, alt, frame, logical rectangle and
  placeholder status. An image has one line-like entry for queries/reading order.
- Images may own float and note relations. Host resources are derived engine
  inputs; pixels and intrinsic size are never written into authored state.

## Display list: `reprise-display`

-   **One `DisplayList` per page.** Items are:
    -   `Glyphs(GlyphRun)`
    -   `Image`, with SHA-256 asset hash, destination rectangle, alt text and layer
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
-   **Images:** `AssetStore` verifies SHA-256 bytes supplied by the host. Each backend
    adds `render_with_assets`; PDF also adds `render_ordered_with_assets`. Original
    signatures use an empty store. Missing/unreadable raster data draw a gray box;
    SVG embeds header-readable bytes as data URIs, or draws the same missing box.
    PNG/PDF pixel decoding is capped at 16,777,216 pixels and 64 MiB source bytes.
    Layout scans at most 1 MiB and 512 header parts, stopping at IDAT/SOS.
    PNG pHYs and JPEG JFIF/primary-IFD EXIF densities set physical size; absence uses 96 DPI.
    Width-only/height-only sizes preserve the unrounded physical aspect even
    when intrinsic lengths saturate or round to zero; excess inline size
    scales both dimensions. A tall image advances before overflowing an empty
    frame with `layout.frame-overflow`; page limits still bound placement.
    Image ActualText follows ordered PDF run addresses, including placeholders.
    The computed text style is retained; image box height does not overwrite
    authored/computed line-height.
-   **Open:** Full PDF/UA structure tagging and cross-page reading overrides remain follow-ups; ordered extraction is available through the new `pdf::render_ordered` function.

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

## Incremental layout (additive API)

`reprise_layout::incremental` adds `LayoutSession`, `LayoutJob`, `Viewport`,
`PartialLayout`, `Coverage`, `Pass`, `Step`, `JobError`, `WorkCounters`,
`Computation`, `Dependency` and `DependencyGraph`. The frozen `Engine::layout`,
`LayoutSnapshot` structure and existing queries are unchanged.

- A session borrows one immutable engine configuration; reconfiguration requires
  a new session. Cached computation output is reusable only for equal complete
  input keys. Completed evaluation equals the uncached reference, including
  diagnostic order.
- Jobs use explicit count budgets and can resume or be cancelled without threads.
  Results are tagged and checked against the originating document revision.
  Publication rejects stale or cancelled partial wrappers.
- `PartialLayout` is explicitly wrapped with producing pass, covered pages,
  completion and settled geometry flags. Provisional region/relation views may
  still move or gain content. Only completed jobs return a complete snapshot.
- Computed dependencies stay derived. Ordered forward/reverse inspection and
  recomputation reasons are separate from authored relations and identities.
- Existing feedback bounds and fallback diagnostics are identical on both paths.

See [incremental.md](incremental.md) for exact keys, reuse proof, counter meanings,
partial semantics and conservative memo boundaries. No diagnostic codes are added.

## Editing kernel: `reprise-edit`

Frozen on 2026-10-04, after orchestrator review. It changes like every other contract
here: through a dedicated `feat(contracts)` commit.

- **Authored operations (02, 05, 07, 09, 29):** `Command` is a pure typed description
  of insert/delete text, split/join blocks, insert/delete/move blocks, replacement
  style overrides, and add/remove relations. All text offsets/ranges are UTF-8
  bytes on character boundaries. Block indices count live siblings; a move's
  index counts siblings excluding the moved block. Commands reference existing
  IDs; `Applied.blocks` and `Applied.relations` return new IDs in command order.
- **Transactions:** `Editor::apply(&Transaction)` validates every command against
  the state left by earlier commands before writing. Invalid ranges, offsets,
  tree moves, missing/deleted targets and schema violations reject the whole
  transaction with `EditError { command, reason }`; authored state, revision and
  undo history are unchanged. Relations pass `SchemaRegistry::validate` and
  target liveness checks, including range targets. An empty/no-op transaction
  produces no undo step. Callers route mutations through the editor while it owns
  the document; independent handles must not mutate it during apply.
- **Commit and identity:** newly inserted blocks/relations are staged flagged and
  invisible in excluded `reprise:stage` commits before the authored transaction.
  Their activation and all other transaction commands form one `reprise:step`
  commit and one undo step. Staging is never undone; undo/redo of insertion retains
  the created ID. `Document::delete_block` writes `deleted = true` on the original
  node without moving or committing it. Undo restores its previous flag and
  position; `restore_block(id)` clears its flag in place. Ancestor flags hide the
  entire subtree. Restoring a parent does not clear independently deleted children.
  This includes table, row and cell containers and their ordinary text descendants;
  structural queries and layout see only the live portion of that topology.
  Loro physical tree tombstones also remain deleted. Relations and ranges observe
  both kinds of tombstone under their existing deletion policies.
- **Collaboration and undo:** `Editor::merge` imports peer operations;
  `undo`/`redo` apply this peer's inverse operations while retaining concurrent
  collaborators' edits. Text edits in a deleted node are retained without revival.
  Concurrent flag delete/restore uses Loro map last-writer-wins, ordered by Lamport
  timestamp then peer ID. Moves do not clear deletion. All identities use the real
  peer. `UndoStack` defaults to 1,000 steps, with commit-based grouping and merge
  interval zero; its limit and history can be adjusted/cleared. Physical purging
  of flagged nodes is deferred to a reference-aware replica compaction policy.
- **Structural primitives (12):** `NewBlock`, `insert_block_at`, `move_block`,
  `split_block`, `split_block_into`, `join_blocks`, staging and activation are
  reusable document operations for the kernel and future paste. Split copies the
  tail into a new text container; join appends the second block's text and records
  succession before flagging it. Blocks joined must have equal kinds, and the
  second must have no live children. Persistent anchors/ranges remain attached to
  their original text container and follow `RangePolicy`; they are not rehomed
  across split/join. Node targets follow succession only under `Rebind` policy.
- **Position effects:** `Applied.effects` and `map_position(node, offset, Bias)`
  map UI byte positions through text edits, splits, joins and deletions. `Bias`
  chooses before/after inserted text. Descendants of a deleted block must also
  be checked against document liveness. Persistent positions use document anchors.
- **Bounds and diagnostics (37, 38):** `MAX_COMMANDS = 10_000`,
  `MAX_INSERT_BYTES = 16 MiB`, `MAX_TRANSACTION_BYTES = 16 MiB` of aggregate
  inserted/copied text, and `MAX_ANCESTORS = 1_024` ancestor checks per command.
  Exceeding a bound is a typed refusal. `EditError::note()` emits `edit.limit`,
  `edit.invalid-command`, or `edit.store`, all `Error` because the edit is omitted.
  `Store` identifies an unexpected document/store failure rather than invalid
  author input. No clock, randomness or unordered iteration chooses editing output.
- **Caret and geometry (20, 22, 30):** `Caret` contains `NodeId`, grapheme-boundary
  byte offset and `Affinity::{Upstream,Downstream}`. Upstream attaches to preceding
  text, downstream to following text; soft line breaks and bidi boundaries may
  give two visual positions. `Navigator` borrows one immutable `LayoutSnapshot`.
  `normalize` preserves logical offsets and canonicalizes equivalent affinities.
  `caret_rect` returns integer page-space geometry; `hit(page, point)` finds the
  nearest text caret, including outside frames and between lines. Glyph clusters
  are subdivided proportionally by grapheme count, with integer half-away rounding.
  Missing glyph spans retain zero-width logical stops. Run levels and visual order
  govern bidi cells. Frame transforms map caret and selection geometry to pages.
- **Hit equivalence:** rect-hit-rect preserves page and rectangle for drawable,
  invertible geometry. Exact normalized identity is required where that visual
  position is unique. Coincident offsets, zero-width cells and coincident lines
  have a deterministic hit representative. A page without text uses the logical
  end before it, or the logical start after it. An invalid page or a document
  without laid-out text returns `None`. Invalid carets are rejected without panic.
- **Movement (30, 33):** logical movement uses ICU4X grapheme/word segmentation;
  visual left/right maps page-horizontal direction through the inverse frame
  transform to its dominant axis (inline on ties). Inline steps traverse bidi
  and zero-width grapheme cells; mirrors reverse the step. Quarter-turns step
  between logical lines. Singular transforms leave visual arrows unchanged.
  Line steps preserve `Cursor.goal_x` in frame space; inline and other movements
  clear it. Page-left/right line edges use the frame's first/last inline edge
  when their page x coincides. `InlineForward`/`InlineBackward` traverse visual
  cells toward increasing/decreasing frame inline x, independently of page
  orientation; `LineInlineStart`/`LineInlineEnd` identify their end junctions.
  These operations traverse turned lines and successive spiral strips.
  Logical and visual line edges, block
  edges and document edges are explicit operations. Cross-block movement follows
  `LayoutSnapshot::reading_order(&doc)` through `Navigator::semantic`, including
  authored `reprise.reading-order` overrides. Pass the layout's document revision;
  the snapshot query defines fallback for a mismatched revision.
  A caller-supplied order uses `Navigator::new(snapshot, order)`. Repeated/unknown IDs are
  ignored; omitted laid-out blocks follow in snapshot order. At a document edge
  a valid caret remains unchanged. Unlaid-out content has no caret geometry.
- **Selections and gestures (30, 31):** `Selection { anchor, focus }` produces
  per-block logical `BlockRange`s independent of drag direction. A collapsed
  selection produces one empty range and no rectangles. Invalid endpoints produce
  no ranges/geometry. `selection_rects` coalesces adjacent selected grapheme cells
  but preserves discontiguous bidi geometry, in block/line/visual order. Hard
  breaks and zero-width cells contribute no area. `select_word`, `select_line`,
  `select_block` and `select_all` are policy-free operations; a word-end caret
  selects that word and a gap caret selects the gap. They do not mutate content.
- **Snapshot base direction:** the kernel uses `BlockLayout.base_level`, supplied
  by layout and omitted from JSON when zero. It does not infer paragraph direction
  from source text. The resolved base level controls transitions between visual
  lines and the inline edge of an empty line; run levels control intra-line cells.

## Plugins (additive ABI v1)

`reprise-plugin` adds pinned core-WASM modules and adapters; existing extension
traits, snapshot fields and diagnostic codes are unchanged. The complete ABI is
[plugins.md](plugins.md), including exports, exact little-endian memory records,
UTF-8 byte offsets, statuses, capabilities, limits and fallbacks.

- A module is identified by SHA-256 of its bytes plus declared name/version.
  Manifest signatures/imports, grants, fuel, memory/table/buffer limits, phase and
  pinned runtime version are explicit reproduction inputs (`Envelope`). The
  layout adapter exposes a sorted `PluginEnvelope`; hosts include it in cache tags.
- The only backend is wasmi 2.0.0, with deterministic NaN arithmetic and portable
  dispatch. SIMD, relaxed SIMD, shared memory, threads and ambient imports are
  refused. Layout API values are integer Length/Fixed records, never floats.
- Required exports: memory, reprise_abi_version () -> i32,
  reprise_alloc (i32) -> i32, reprise_call (i32,i32,i32,i32,i32) -> i32.
  ABI version is 1; input/output buffers are checked and disjoint. Instances and
  mutable state are fresh per invocation. All exports share one fuel budget.
- PureFunction signatures are declared without executing code. Unknown/refused
  functions remain unknown; runtime failure uses existing style.function-failed
  with the underlying plugin code in its message and skips the style layer.
- Geometry narrows existing frame room only. Invalid/missing/exhausted geometry
  falls back to that exact room with a Warning. Existing composer bounds apply.
- Plugin schema bindings resolve targets through the existing resolver, then
  acknowledge or decline. They cannot place blocks. Failure leaves resolved
  targets reported with applied=false, a plugin Warning and relation.not-applied.
- ReadText sees only explicit immutable handle texts. Editing-only InsertText
  stages bounded UTF-8 commands; EditKernel commits one validated atomic kernel
  transaction after successful execution. Refusal/trap applies no commands.
  There are no time, randomness or I/O imports, including in Editing phase v1.

## Clipboard and export: `reprise-clipboard` (35)

- `Fragment` is authored data in `reprise-doc::fragment`, version 1. Source IDs are
  labels. `copy_fragment` accepts whole subtrees or explicit UTF-8 byte ranges;
  intersecting persistent ranges are clipped with their authored policies (legacy ranges use observable cursor behavior).
  Styles include inherited parents. Copy-all also carries raw authored page setup. Kernel selections promote fully selected tables to subtrees; partial table selections flatten cell text with `clipboard.selection-table` Warning. Collapsed selections copy nothing.
- Hosts supply a nonempty source/target namespace (normally the package DocumentId).
  Only equal namespaces authorize external relation targets. Peer equality does not.
  `plan_copy` is authoritative at both copy and paste; cross-document crossing relations
  are dropped with `clipboard.relation-dropped` (Error: the edge was omitted), even
  when their numeric IDs exist in the target. Snapshots remain source history references.
- `NativeFragment` wraps the fragment with SHA-256-addressed resources and copy notes.
  Available layout-used fonts carry FaceId and verified bytes; without layout the
  current computed family is used. Missing resources produce `clipboard.resource-missing`
  (Warning: reproduction may differ). Hosts may attach assets explicitly until an image
  usage graph exists. `install_fonts` installs verified bundles into the host FontStore;
  resource bytes do not enter Loro. Encoding uses sorted maps and source-ID order.
- `Command::Paste` is an additive standalone command, rejected if mixed with other
  commands. `Editor::paste` returns `Pasted` with the frozen node/range `IdMap`, a separate
  relation map, position effects and notes. The entire paste is validated before staging,
  then activated in one commit/undo step. Invisible staged IDs survive undo and redo.
  Matching source IDs in independent documents using the same peer are skipped.
- At a paragraph caret the prefix/first pasted paragraph and last paragraph/suffix join.
  The pasted blocks retain new identities; the original host is tombstoned with a
  succession link. Tables and annotation boundaries keep separate prefix/suffix blocks.
  Existing host ranges migrate with the prefix/suffix while retaining IDs. A range that would span several pasted blocks remains missing, reported with `clipboard.host-range-dropped` Error; undo restores it.
  `at=None` appends; page setup is imported only into an empty document.
- Missing styles are defined. If any named style clashes by content, the entire imported
  style graph is renamed deterministically to available `name (paste N)` names, retaining
  inheritance, and `clipboard.style-clash` reports Warning. Dangling source style references are also renamed when necessary to prevent accidental target binding. Original styles stay intact.
- Native fragment limits: 4,096 blocks, 8,192 ranges/relations, 1,024 styles/templates,
  depth 64, 16 MiB payload and resources each, 96 MiB JSON envelope. Future versions are
  rejected with `clipboard.version` Error; malformed data with `clipboard.invalid` Error;
  resource and parsing limits with `clipboard.limit` Error. Hash/identity mismatches use `clipboard.resource-hash` Error. `clipboard.range-affinity` is Warning only for legacy ranges whose authored affinity cannot be recovered at a text boundary. No partial paste occurs on
  validation failure. An unexpected store failure still uses the kernel's Store semantics.
- Plain import normalizes CRLF/CR and splits paragraphs at two LFs; individual LF stays a
  forced break. Plain export uses logical Unicode bytes, two LF between paragraphs, tab
  between cells and LF between rows. Soft wraps do not add characters. With a current
  layout it follows `reading_order`; unplaced text follows in semantic order.
- HTML export emits semantic paragraphs, paragraph `dir`, computed CSS family/point size/
  line height, whitespace preservation, `br`, and table/row/cell tags. Notes/floats become paragraphs; transforms,
  symbolic styles, column constraints and relation graphs are reported as losses.
- The handwritten HTML reader accepts paragraphs/divisions, breaks, nonnested tables,
  basic point CSS, pre-wrap whitespace and common/numeric entities. Tables infer equal proportional columns (at most 128) from their rows. Direction inference uses shaping's pinned ICU Unicode 17 properties. Unsupported markup retains text with
  `clipboard.html-approximated` Warning. Active/non-content HTML is omitted with
  `clipboard.html-dropped` Error. It fetches nothing. Explicit caps: 8 MiB input,
  100,000 tokens, depth 64, 4,096 blocks, 128 attributes/tag and CSS declarations/block; exceeding a cap rejects the whole import. Conflicting explicit HTML dir values are reported as approximated: the authored model infers direction from Unicode.
- `Exporter::export(doc, layout?, options) -> Result<ExportResult, ClipboardError>` is the
  extension point for DOCX/EPUB. Results contain bytes and `LossReport`: exactly one stable
  feature code for relations, reading order, transforms, notes/floats, tables, styles,
  bidi, fonts, assets and editing structure, with Preserved/Approximated/Dropped and detail.
  Feature outcomes are not diagnostic severities; notes use the normal severity rules.
  PlainText, Html, Native and Pdf implement it. PDF requires current layout/fonts, wraps
  ordered rendering, and explicitly reports editable structure and relation graph loss,
  lack of PDF/UA structure, viewer-dependent text extraction, and layout omissions.

## Image resource additions (34, 35)

- `Package::new_with_resources`, `embed_document_images`,
  `open_with_resources` and `OpenedFile::save_with_resources` add image resource
  capture/restoration beside the existing font paths. All live authored image
  references are captured, including images omitted by a page limit. Missing
  bundles remain declared; unrelated/unknown assets survive.
- `AssetAvailability::restore_images` installs only hash-verified bundled images.
  External resources remain host needs; the library performs no I/O.
- `NativeFragment::attach_images` finds selected image hashes and uses
  `attach_asset`; `install_assets` verifies the fragment before modifying the
  host store. Missing bytes retain their reference and report
  `clipboard.resource-missing` (Warning).
- `NativeWithAssets` and `PdfWithAssets` implement the existing `Exporter` trait
  with a supplied `AssetStore`, leaving frozen `ExportOptions` unchanged.
  `export.assets` reports actual image preservation; plain text preserves alt
  text and reports pixel data dropped. Ordered PDF uses image ActualText.


## Bindings: external API v1 (`reprise`, `reprise-wasm`)

The facade crates are version 0.1.0 and are the external application contract
(02, 04, 28, 29, 38, 41). Core types remain private: all crossing records, enums,
IDs, diagnostics, display operations and errors are facade-owned. No core type is
re-exported. `crates/reprise/API.txt` pins native signatures; generated
`crates/reprise-wasm/ts/types.d.ts` pins wire shapes. Changes to either require
explicit review. See [bindings.md](bindings.md) for the full method overview,
worker protocol, bounds, error meanings, packaging and host responsibilities.

- Every standalone object payload is `Payload<T> { version: 1, data: T }`.
  Nested records inherit the envelope version. Raw resource bytes are opaque
  byte channels paired with versioned metadata, never JSON number-array APIs in
  JavaScript. Native callers receive owned `Vec<u8>`; WASM uses `Uint8Array`.
  Opaque WASM objects are local handles and are not serializable payloads.
- Unsupported versions are refused before interpreting data. Native `decode`
  bounds JSON bytes/depth; WASM snapshots and bounds a plain JS object tree
  before deserializing it, rejects cycles and catches throwing property reads.
  Decimal strings carry peer IDs and other 64-bit counters. Node/range/relation
  IDs are canonical opaque strings; document IDs are 32 lowercase hex digits.
  Geometry is integer 1/1024 pt; matrix coefficients are integer 16.16.
- `Workspace` creates/opens independent `DocumentSession` replicas. Hosts supply
  persistent document identity and distinct peers. All editing transactions,
  paste and plugin edits use the editing kernel, including atomic validation,
  position effects and per-peer undo/redo. Adding plugin schemas retains undo.
  Create/open define the empty-name base style (serif, 10 pt, 1.2 em) only when
  absent; authored definitions are preserved. The base directly selects the legacy
  serif generic without substitution warnings. Default text needs no font import.
  Valid unplaced selection endpoints return `bindings.layout-required`, absent
  node IDs return `bindings.id`, and invalid offsets return `bindings.invalid`.
  Compatible newer packages return `bindings.read-only`; the facade currently
  cannot lay out the core's `DocumentAt` view and never exposes it as editable.
- Layout jobs are owned, single-threaded continuations of the same incremental
  coordinator. Each `step(session, budget)` charges at most its explicit budget,
  may stop early for the viewport and returns pass, coverage, completeness,
  settled/outside-document flags and work counters. Zero is a checked no-op.
  Partial pages are available only inside advertised coverage. Navigation,
  reading order, PDF and convenience rendering require complete current layout.
- Every job/result is bound to native session identity, revision and generation.
  New jobs/configuration changes supersede earlier jobs. Edits and sync invalidate
  revision-bound results; cancellation is terminal. A stale/cancelled job cannot
  publish. Hosts also gate messages already queued outside the engine against
  their currently accepted token. Caches are isolated per session/configuration.
- Saving finishes layout when needed, embeds used fonts and preserves document
  ID, unknown sections, raw untouched manifests, opaque extensions and assets.
  Saving never compacts history implicitly. SVG/PNG/PDF render the same display
  operations; display JSON uses one canonical serializer natively and in WASM.
- Clipboard supports versioned native fragments, plain/HTML selection projection
  with loss reports, and bounded plain/HTML/native paste. Export supports plain,
  HTML, native and PDF, always returning feature losses. Resources are declared,
  enumerated and retrieved without I/O. Image registration installs bytes in
  `Engine.assets`. `ImageInsert` and standalone `Command::InsertImage` insert a
  content hash, alt text, optional physical dimensions and style through atomic
  kernel paste. Image-kind `InsertBlock` without a record is refused. Display
  image DTOs mirror the core's origin-based rectangles. Asset-aware SVG/PNG/PDF,
  package save/open and native clipboard/export preserve available image bytes;
  missing/corrupt references use the core's diagnosed placeholders.
- Plugins are loaded against explicit content pins, manifests, grants and limits;
  functions, geometry and relation schemas can be installed. Editing plugins
  receive only explicit node handles and commit staged insertions atomically.
  Modules share the core's deterministic wasmi sandbox on both platforms.
- Sync v1 exchanges sorted version vectors and self-contained retained-history
  snapshots as update packets. Import verifies document scope, distinct declared
  peers and agreement of vector with bytes before merging. Compact delta packets
  are a future version; no transport/server/authentication is inferred. Fonts,
  assets and engine configuration use separate host-owned resource channels.
  Awareness is bounded opaque bytes with document/peer metadata and no persistence.
- The typed `Error` enum maps core notes without changing stable codes/severities.
  Boundary refusal codes above have Error severity because the requested result
  or operation is omitted. Successful explicit cancellation itself returns an
  acknowledgement. Messages are explanatory and are not matched as contracts.
- Neither facade uses unsafe code, required threads, clocks, random identity,
  filesystem or network I/O. Sessions retain the core's single-threaded caches;
  native hosts create and drive them on a dedicated engine thread/actor rather
  than moving live jobs between arbitrary Tauri command threads.
