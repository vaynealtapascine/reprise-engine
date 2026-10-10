# Code map

Each crate lives in `crates/<name>` and is published to the workspace as `reprise-<name>`.
The table is in crate dependency order. File paths are relative to that crate unless they
start with `docs/`, which names repository-level documentation.
[contracts.md](contracts.md) describes the frozen interfaces.

Repository branding lives in `docs/branding/reprise-engine-logo.svg`, the
supplied engine logo displayed by `README.md`.

| Crate | Files | What it does | Decisions |
| --- | --- | --- | --- |
| `diag` | `src/lib.rs` | `Note`, `Severity`, `Code`: diagnostics shared by every library | 37, 39 |
| `geom` | `src/lib.rs` | `Length` (1/1024 pt) and `Fixed` (16.16), saturating and rounding rules, `Matrix` with exact integer direction/angle rotations, mirrors and pivot transforms, typed spaces, `Point`, `Rect`, `Transform` | 19, 20 |
| `text` | `src/lib.rs` | `Text` (byte offsets over Loro), `Anchor`, `Affinity`, `RangePolicy`, `Resolved` | 09, 10, 12 |
| `text` | `src/segment.rs` | Grapheme and word boundaries (ICU4X) | 09, 10 |
| `doc` | `src/lib.rs` | `Document`: content tree, versioned authored ranges (`add_range`, `range_policy`, resolution), styles, relations, revisions, fork and merge; exports authored frame geometry and reading schemas; legacy policies remain distinguishable from unreadable metadata | 05–07, 10, 12, 15, 29, 34, 35 |
| `reprise` | `tests/soak.rs`, `tests/sync_delta.hex` | Structural partition/rejoin/undo soak, deterministic peer fixture and native/WASM packet-byte golden | 29, 34, 38 |
| `doc` | `src/flow.rs`, `src/flow_tests.rs`, `docs/flow.md` | Flow text: paragraphs as U+FDD0 breaks with records in `breaks1`, paragraph views, per-revision cache, staged breaks and embeds, `locate`, placement | 06, 07, 11, 12, 29 |
| `doc` | `src/formatting.rs`, `docs/text-formatting.md` | Anchored character formatting: versioned `format1` range envelopes, per-revision host index, ordered overlap resolution with reset, grapheme-aligned runs, copy/split/join carry-over and read-time limits | 05, 07, 12, 34, 37 |
| `doc` | `src/transfers.rs`, `src/persist.rs` | Retained copy lineage over visible characters, bounded per-revision source-ID index, undo/redo aliases and bounded columnar snapshot counts | 10, 12, 29, 34 |
| `shape`, `layout` | `shape/src/lib.rs`, `layout/src/flow/stage.rs`, `layout/src/display.rs` | Upright vertical shaping, bounded combinations and display compensation | 20, 22, 38 |
| `fixtures` | `tests/vertical.rs` | Both downward modes, combination, frame transitions, storage and export parity | 20, 33, 38 |
| `doc` | `src/relation.rs` | `Relation`, `Target`, `StructuralQuery`, `LayoutQuery`, `SnapshotRef`, `RelationSchema`, `SchemaRegistry`, the built-in schemas (`reprise.follow`, `reprise.reference`, `reprise.reading-order`), `Dependency`, copy planning (`plan_copy`, `CopySet`, `IdMap`) | 13–15, 27, 35 |
| `doc` | `src/structure.rs` | Live tree navigation and structural queries (`evaluate`), inherited deletion through table nesting, succession links (`supersede`, `succession`) | 06, 07, 13, 15, 24 |
| `doc` | `src/history.rs` | `DocumentAt` (a past version), `HistoryCache`, snapshot resolution, `compact_history` | 07, 13 |
| `doc` | `src/resolve.rs` | `resolve_target` / `resolve_relation`: any non-layout target, with `OnTargetDeleted` applied from tombstones; `dead_relations` | 13–15 |
| `doc` | `src/relation_tests.rs` | Structural, historical and shared target/relation resolution, relation schema and copy planning tests | 13–15, 35 |
| `doc` | `src/style.rs` | `Style` with versioned additive family chains, `LengthExpr`, `Authored` (stored forms), the four stages (`Specified`, `Computed`, used via `StyleResolution`), `ComputedStyle`, defaults | 08, 17, 18, 39 |
| `doc` | `src/expr.rs`, `src/expr/tests.rs` | `Expr`: bounded typed expressions with a canonical text form, type check, folding, saturating evaluation and dependency sets | 17, 19, 27 |
| `doc` | `src/function.rs` | `FunctionRegistry`, `PureFunction`, `Signature` and the built-in functions | 17, 36 |
| `doc` | `src/context.rs` | `ResolutionContext`, `Level`, `Basis`, definite, indefinite and unresolved bases | 18 |
| `doc` | `src/codes.rs` | The `style.*` diagnostic codes | 37 |
| `doc` | `src/page.rs` | `PageTemplate`, `FrameTemplate`, `FrameRole`, `Dim` (lengths that may follow the `Medium`), v2 authored transforms, writing modes and spiral paths, v1-compatible Loro storage and the built-in template | 05, 20, 24, 34 |
| `doc` | `src/reading.rs` | Independent `reprise.reading-order` block-precedence schema and authoring helper | 01, 14, 33 |
| `doc` | `src/persist.rs` | Opaque snapshot export/import, explicit peer/history mode, bounded Loro/KV/LZ4 expansion and columnar-count preflight | 07, 09, 29, 34, 37 |
| `doc` | `src/region_schema.rs` | Owned float/note relation schemas and parameter vocabularies | 05, 13, 24 |
| `doc` | `src/table.rs` | Versioned authored table/row/cell topology, column declarations, cell spans and row header setters on the movable tree | 05, 06, 24 |
| `doc` | `src/table_grid.rs` | `resolve_grid` and `Document::table_structure`: the single deterministic interpretation of authored spans and header rows, with issues, shared by layout, export and accessibility | 06, 24, 33, 37 |
| `doc` | `src/ranges.rs` | Versioned authored policy1 envelope, bounded refusal without rewriting unknown data, legacy detection and history/shallow/fork/merge/hostile policy tests | 10, 12, 29, 34, 35, 37 |
| `doc` | `src/edit.rs`, `src/lifecycle.rs`, `tests/lifecycle.rs` | Structural split/join/move operations, excluded staging, soft deletion and identity-preserving per-peer undo/redo; split/join retain original text-container anchors and stored policies | 07, 12, 29 |
| `doc` | `src/style/tests.rs` | Adversarial style resolution and expression storage tests | 08, 17, 18, 37 |
| `doc` | `src/fragment.rs` | Versioned authored subtrees, inherited styles, clipped ranges with authored policies (legacy cursor fallback and affinity Warning), relation copy planning, invisible range/table staging and identity-preserving reanchoring; set_fragment_table_columns supplies metadata needed for imported cell rendering | 05, 07, 12, 24, 34, 35 |
| `doc` | `tests/history_hostile.rs` | Forged revisions resolve or report, never panic | 07, 13, 37 |
| `doc` | `src/image.rs` | Versioned image records, authored updates, opaque preservation and clipboard staging; alt text in the existing text container | 05, 33, 34 |
| `font` | `src/lib.rs`, `src/supply.rs`, `src/supply/tests.rs` | `Face` with a pinned `FaceId`, metrics, glyph outlines and one type-erased adapter-data cache slot; `FontStore`, frontend declarations, CSS-inspired matching and pinned generic defaults | 21, 22, 38 |
| `shape` | `src/lib.rs`, `src/fallback_tests.rs` | `ShapingAdapter` contract, `HarfRust` with per-face data caching, `ShapedText`/`ShapedRun`, `Reshape`, `visual_order` | 22, 38 |
| `shape` | `src/paragraph.rs` | `itemize` / `itemize_families` (legacy and grapheme-preserving generic fallback chains, resolved bidi levels and contextual scripts), `Shaper` (shaping and reshaping a paragraph) | 09, 21, 22 |
| `shape` | `src/unicode.rs`, `src/bidi-character-subset.txt` | ICU4X property adapter for UAX #9 (including N0) and UAX #24 script resolution; pinned Unicode conformance subset | 09, 22, 38 |
| `shape` | `src/line.rs` | Pure L1/L2 line reordering, preserving bidi groups across missing-font gaps | 20, 22, 30 |
| `compose` | `src/lib.rs` | `Composer` and `GeometryProvider` contracts, `Measure`, break opportunities, `LineFragment`, `Explanation` | 23 |
| `compose` | `src/greedy.rs` | The greedy composer, and the first-fit algorithm other composers fall back to | 23 |
| `compose` | `src/optimal.rs` | `Optimal`: Knuth–Plass total fit over variable geometry, ragged or justified; integer demerits, `Limits` | 19, 23, 39 |
| `compose` | `src/authored.rs` | `AuthoredBreak`: verse, lines end at forced breaks, turnovers with a hanging indent | 11, 23 |
| `compose` | `src/polygon.rs` | `Polygon` and `Runaround` geometry providers, integer ellipses | 20, 23, 24 |
| `compose` | `src/para.rs` | The paragraph as composers see it: normalised breaks, prefix widths, fragments | 23 |
| `compose` | `src/walk.rs` | Walking a geometry provider: skips, `End`, stalls | 23 |
| `compose` | `src/testing.rs` | Unit-test shaping with the bundled font | 39 |
| `compose` | `tests/conformance.rs` | Every composer against adversarial geometry and texts, checking every `Composer` guarantee | 23, 37, 39 |
| `display` | `src/lib.rs` | `DisplayList`, `Item` (glyphs, images, paths, groups), `RenderError` | 32 |
| `display` | `src/svg.rs`, `src/png.rs`, `src/pdf.rs` | Backends; PDF ToUnicode, ordered run addresses and extraction spans, cluster ActualText and logical-run fallback for RTL/malformed ranges; `render_tagged` writes the PDF/UA-1 tagged PDF (MCIDs, StructTreeRoot/ParentTree, artifacts, outline, note link annotations, XMP title and `pdfuaid`) and drops the claim with `pdf.ua-not-met` when a requirement cannot be met | 32, 33, 37, 38 |
| `display` | `src/pdf/tags.rs` | The structure a tagged PDF is built from: roles (P, H1–H6, Figure, Note, Reference, Div, Table/THead/TBody/TR/TD/TH), artifact runs, leaves addressing glyph and image items, `CellRole` (TD/TH, scope, spans), codes | 33 |
| `display` | `tests/pdf_tags.rs`, `tests/common/checker.rs` | Structural checker over emitted bytes (MCIDs reachable once, ParentTree, nothing untagged, structure-order text, catalog/XMP, outline, links) and hand-built structures: roles, run splits, depth bound, bad leaf sets, missing-glyph fallback, cross-page order, determinism | 32, 33, 37, 38 |
| `display` | `tests/pdf_text.rs` | Ordered PDF extraction through rotated, mirrored, vertical and spiral layouts; generated-PDF extraction through lopdf plus bfchar/ActualText reader; ligatures, clusters, RTL, malformed ranges, ZWJ and empty runs | 33, 37, 39 |
| `display` | `src/assets.rs`, `src/assets_tests.rs`, `src/image_pixels.rs` | Host image store, bounded integer PNG/JPEG/JFIF/EXIF headers; backend-only bounded pixel decode | 19, 32, 34, 37, 38 |
| `display` | `tests/images.rs` | Image items in SVG, PNG and PDF, transforms and clips, ordered alt-text extraction, gray missing-resource boxes | 20, 32, 33, 37 |
| `layout` | `src/lib.rs` | `Engine` (configuration, including `schemas`, `functions`, `plugins`, `medium` and `FlowSettings`), `Engine::layout`, geometry, plugins and reading module wiring | 20, 24, 26, 33, 36, 38 |
| `layout` | `src/codes.rs` | Diagnostic codes reported by layout and the relation pass | 37 |
| `layout` | `src/snapshot.rs` | `LayoutSnapshot` (pages, frames, blocks), `RelationLayout`, `Resolution`, `Diagnostic`, queries | 05, 13, 37 |
| `layout` | `src/geometry.rs`, `docs/geometry.md` | Exact authored frame transforms, logical writing axes, bounded spiral expansion, inverse and path diagnostics | 19, 20, 37, 38 |
| `layout` | `src/reading.rs` | Snapshot reading-order queries with explicit document input, stable partial-order completion and iterative cycle repair | 01, 33, 37 |
| `layout` | `src/template.rs` | Resolving the document's page template against the medium; falling back to the built-in one; exact comparable templates for incremental caching | 24, 34, 37, 38 |
| `layout` | `src/region.rs` | `Bounded`: any geometry provider, ended at a frame's depth | 23, 24 |
| `layout` | `src/flow.rs` | Pass 1: per-starting-frame style resolution and diagnostics, shaping, line L1/L2 reordering and glyph spacing adjustments, composing and threading paragraphs and indivisible images through the main flow's frames, page after page; owned resumable flow cursor, optional exact-input memo hooks and actual work counters; reference disables reuse | 08, 17, 18, 22–24, 26–28, 30 |
| `layout` | `src/relations/mod.rs` | Schema dispatch, plugin resolve-and-report after the shared resolver, final region relation reporting and references to note lines; explicit relation/reading stages shared with incremental evaluation | 13–15, 26, 36 |
| `layout` | `src/plugins.rs` | Plugin registration helpers, complete ordered reproduction envelope, safe relation acknowledgement adapter | 04, 36–38 |
| `layout` | `src/relations/follow.rs` | `reprise.follow`: placing a block in the margin frame of its target line's page | 13, 15, 24 |
| `layout` | `src/relations/resolve.rs` | `Resolver`: any `Target` to a `TargetLayout`, status and diagnostics; shared by `follow` and other relation behaviours | 13–15 |
| `layout` | `src/query.rs` | Layout queries added by relations (`first_line`, `frame_of`, `answer`, ...) | 13, 16 |
| `layout` | `src/display.rs` | `to_display_list(s)`, `pdf_reading_blocks` (the ordered runs grouped by block, with source bytes and line), `pdf_artifact_runs` (repeated table header copies), image display items, ordered PDF glyph/image addresses and individually selectable debug boxes, baselines, available/used intervals, run boundaries, break symbols, reshaped lines, relation statuses and diagnostic cluster markers | 32, 39 |
| `layout` | `src/solver.rs` | Explicit integer solver domains, bounded water filling and diagnosed fallbacks | 19, 25, 37 |
| `layout` | `src/regions.rs` | Bounded staged feedback, complete input-plan freeze on oscillation and dependency rounds; shared feedback outcome/freeze helpers for full and incremental evaluation with unchanged reference output | 24, 26, 37, 38 |
| `layout` | `src/floats.rs` | Side/edge float allocation, stacking, deferral, runaround exclusions; optional exact-input memo hooks and work counters | 24, 26–28 |
| `layout` | `src/notes.rs` | Anchor-preserving note allocation, continuation, nesting, endnotes and full-page reservations; optional exact-input memo hooks and work counters | 11, 24, 26–28 |
| `layout` | `src/table.rs` | Grid-driven content measurement (column spans raise bounds), declared column allocation, row-group assembly and grid issue diagnostics; optional exact-input memo hooks and work counters | 24–28 |
| `layout` | `src/table_flow.rs`, `src/table_tests.rs` | Row-group fragmentation (pure per-frame fragments, keep-together, rowspan split), derived repeated header copies and their adversarial tests | 24, 26, 33, 37 |
| `layout` | `src/regions/tests.rs` | Adversarial mixed-region, cycles, dependency depth, fanout, pagination and table cursor unit tests | 24–26, 37 |
| `layout` | `src/incremental.rs` | Exact-input preparation/shaping/flow/region/final-pass caches, computed dependency and reverse-inspection graph, revision-gated viewport jobs beside the full-layout reference | 16, 26–28, 39 |
| `layout` | `src/incremental/tests.rs` | Cyclic/missing/deep computed graphs, unified inputs, extreme and reversed viewports | 16, 28, 37, 39 |
| `layout` | `docs/regions.md` | Allocation, fragmentation, cycle and fallback design | 24–26 |
| `layout` | `docs/incremental.md`, `docs/contracts.md` | Exact memo keys and reuse argument, scheduling/partial semantics, limits and Salsa evaluation | 16, 26–28, 39, 41 |
| `layout` | `src/image.rs` | Intrinsic and authored image size, aspect-preserving frame fit and diagnostic placeholders | 19, 24, 37, 38 |
| `plugin` | `Cargo.toml`, `src/lib.rs`, `src/codes.rs` | Content-hash identities, manifests, ordered capabilities, phase grants, explicit limits and reproduction pins; staged editing-kernel interface and stable failures | 04, 19, 36–38 |
| `plugin` | `src/runtime.rs` | Private Runtime boundary and wasmi backend, static preflight/proposal validator, nesting bound, typed import allowlist, fuel, memory/table/stack limits, fresh instances, checked buffers and deterministic host-copy fuel | 36–38 |
| `plugin` | `src/abi.rs`, `docs/plugins.md` | Language-independent ABI v1: integer values, UTF-8 byte offsets, linear-memory records, imports/exports and statuses | 04, 19, 36 |
| `plugin` | `src/function.rs` | Atomic FunctionRegistry registration and typed PureFunction adapter; frozen style failure diagnostic | 17, 36, 37 |
| `plugin` | `src/geometry.rs` | Shape provider with containment/order/progress validation and exact frame-room fallback; wrapper over conforming composers | 20, 23, 36, 37 |
| `plugin` | `tests/common/mod.rs`, `tests/sandbox.rs`, `tests/capabilities.rs`, `tests/geometry.rs` | Runtime/ABI/capability/state/fuel/NaN/adversarial geometry conformance, staged editing and exact WAT/WASM equality | 36–39 |
| `plugin` | `tests/stack.rs` | Fat and indirect recursion under maximum fuel trap on wasmi's own stack, on a 512 KiB host thread | 36, 37 |
| `plugin` | `test-plugins/`, `examples/compile_test_plugins.rs` | Checked-in test sources and deterministic explicit fixture compiler, no build-time toolchain requirement | 36, 38, 39 |
| `edit` | `src/lib.rs`, `src/command.rs`, `src/editor.rs`, `src/plan.rs` | Validated typed commands, staged activation, atomic commit/undo, bounded transaction models and position effects | 02, 05, 07, 12, 29, 37 |
| `edit` | `src/error.rs`, `src/codes.rs` | Typed refusals and stable Error diagnostics for validation, resource bounds and store failures | 37 |
| `edit` | `src/caret.rs`, `src/model.rs`, `src/navigator.rs` | Byte/affinity carets, cached cluster/grapheme cells, atomic combined-run caret units, integer page geometry, overlapping-strip hit testing and snapshot reading-order integration | 09, 20, 22, 30, 33 |
| `edit` | `src/movement.rs`, `src/select.rs` | Logical/page-direction navigation through frame transforms, goal-x line movement, reading order, logical selection ranges, bidi geometry and gesture operations | 20, 30, 31, 33 |
| `edit` | `src/paste.rs` | Validated standalone paste, fresh staged IDs, style collision and dangling-reference handling, relation remapping, returned activated prefix/suffix IDs, identity-preserving host-range reanchoring with authored policy and diagnosed cross-block omissions; atomic commit/undo | 07, 12, 29, 35, 37 |
| `edit` | `tests/audit.rs`, `tests/transactions.rs` | Real-peer identity, atomic deletion, retained history, every command and fixed-seed convergence/undo properties; split/join preserve authored range policy through undo/redo | 07, 12, 29 |
| `edit` | `tests/undo_merge_fuzz.rs` | Undo, redo and merges interleaved on two peers: replicas and layout converge, new IDs never repeat | 07, 29 |
| `edit` | `tests/range_policy_concurrent.rs` | A concurrent split and join over the same ranges converge with their authored policies | 12, 29 |
| `edit` | `tests/document_edits.rs` | General structural edit primitives, table/row/cell subtree deletion with identity-preserving undo, and collaborative text undo | 07, 12, 24, 29 |
| `edit` | `tests/table_headers.rs` | Hits and reading order around repeated header copies | 30, 33 |
| `edit` | `tests/common/mod.rs`, `tests/navigation.rs`, `tests/movement.rs`, `tests/selection.rs` | All-hostile geometric round trips, bidi/zero-width and spiral-strip traversal, authored reading overrides, empty input, transforms and selection coverage | 20, 30, 31, 33, 37 |
| `format` | `src/lib.rs`, `src/container.rs` | Versioned checksummed container, document identity, feature masks, hard bounds, typed errors and codes | 34, 37, 38 |
| `format` | `src/json.rs` | Iterative manifest bounds and canonical metadata | 34, 37, 38 |
| `format` | `src/assets.rs` | Font pins (including versioned frontend declarations), bundled/external assets, hash validation, missing-font list, open-and-restore/store API and preservation of unknown declaration fields | 21, 34 |
| `format` | `src/migration.rs` | Pure checked N -> N+1 migrations, synthetic v0 | 34 |
| `format` | `src/package.rs` | Package save/open with additive used-layout-font and authored-image embedding, read-only newer files, snapshot envelopes and opaque cache tags/validation | 05, 07, 34 |
| `format` | `src/tests.rs`, `tests/roundtrip.rs`, `tests/fonts.rs`, `tests/data/` | Corruption/limits/migrations/golden tests, every hostile fixture and spike persistence/convergence; history and shallow packages preserve all eight authored endpoint policies at Unicode text boundaries | 10, 12, 34, 35, 37–39 |
| `format` | `SPEC.md` | Version 1 wire specification, compatibility, bounds and dependency licenses | 34 |
| `format` | `src/image_assets.rs` | Embed authored image references and restore verified bytes | 34 |
| `format` | `src/features.rs` | Table feature bits declared from live content on save: optional headers, required spans | 24, 34 |
| `clipboard` | `src/lib.rs` | Copy-all, block and kernel-selection entry points, full-table promotion, partial-table flattening diagnostics and resource collection | 24, 33, 35 |
| `clipboard` | `src/native.rs` | Deterministic versioned NativeFragment encoding, validation and SHA-256-addressed font/asset bundles with frontend declarations keyed by face | 21, 34, 35, 38 |
| `clipboard` | `src/html.rs` | Bounded plain/HTML import, malformed tag recovery, attribute/CSS limits, pre-wrap whitespace, proportional nonnested table columns and explicit-direction conflict diagnostics; direction inference uses shaping's pinned ICU Unicode 17 properties | 09, 24, 35, 37, 38 |
| `clipboard` | `src/text_css.rs`, `tests/formatting.rs` | Bounded inline-CSS subset for `<span>` formatting (family, size, language, OpenType features); HTML/native round trips and hostile CSS | 35, 37 |
| `clipboard` | `src/export.rs` | Extensible Exporter trait, per-feature LossReport and plain/HTML/native/PDF exporters; plain reading order retains page-limited tails | 32, 33, 35 |
| `clipboard` | `src/codes.rs` | Stable clipboard and export diagnostic codes | 35, 37 |
| `clipboard` | `tests/paste_undo_walk.rs` | Twenty pastes (end and mid-paragraph) of an encoded and decoded fragment, then each undone and redone exactly, with the same IDs | 07, 29, 35 |
| `clipboard` | `tests/declared_fonts.rs` | Two collection faces declared under aliases travel in one fragment and reinstall as the same faces | 21, 35 |
| `clipboard` | `tests/native.rs` | Unicode cuts, all eight authored endpoint policies including empty points, host reanchoring, native encode/decode/paste/undo/redo, identity collisions, table promotion, style references, concurrent caret pastes, cross-document graph remapping and input bounds | 07, 12, 29, 35, 37 |
| `clipboard` | `src/pdf_tags.rs` | The PDF exporter's structure over `reading_order`: headings from style names, Figure alt, Note IDs and linked References, floats, tables from `Document::table_structure` (THead/TBody, TH scope Column, RowSpan/ColSpan, repeated header copies as artifacts); `cell_role` is the single seam for TH/scope/spans; `PdfMetadata`, `export_pdf` | 32, 33, 35 |
| `clipboard` | `tests/pdf_ua.rs` | Every hostile fixture exported as a checked tagged PDF; notes, floats, tables, headings, metadata, reading override, unmet-requirement reporting | 33, 35, 37, 38 |
| `clipboard` | `tests/roundtrip.rs` | Every hostile fixture: live topology, range policies and page/frame/block/glyph geometry after native copy/paste and undo/redo | 12, 35, 38, 39 |
| `clipboard` | `tests/import_export.rs` | Parser caps, malformed tag soup, resource hashes, reading order and House-of-Leaves loss reports; imported table cell rendering and Unicode direction agreement for isolates, controls, paragraph breaks and RTL characters | 24, 33, 35, 37, 39 |
| `clipboard` | `tests/tables.rs` | HTML th/thead/colspan/rowspan import and export, span bounds, loss-report details and native span preservation | 24, 35, 37 |
| `clipboard` | `src/image_export.rs`, `tests/images.rs` | Image assets through attach/install, asset-aware native and PDF exporters, package/copy/paste/export pipeline tests | 33, 34, 35 |
| `fixtures` | `src/lib.rs`, `src/fonts.rs` | Pinned fonts, engine and peers for tests, three-face fallback text and relocated OTC fixture | 38, 39 |
| `fixtures` | `src/spike.rs` | The spike document | 40 |
| `fixtures` | `src/templates.rs` | Page templates for tests: columns, a margin, responsive sizing | 24 |
| `fixtures` | `src/hostile.rs` | Hostile text/bidi/display-cluster fixtures; relation targets and policies; composers; style expressions, cycles and bases; transformed RTL, writing modes, spirals, reading cycles and degenerate/extreme transforms; editing lifecycle, transaction refusal and empty/zero-width carets; font chains, generics, three-face fallback, corrupt declarations and collection indices; plugin extensions/fuel fallback; persistence_tombstones (Unicode anchors, tombstones, retained history); ten region fixtures; six table fixtures (repeated headers, oversized header, whole-table and malformed spans, 1,000 columns, rowspan break in a rotated vertical frame, concurrent colliding spans); incremental_page_seam (UTF-8 pagination, following note, reading precedence); clipboard_unicode_seams (expanding/fixed/point policies over ligatures, combining text and RTL with note/reference edges); range_policy_endpoints (all eight authored policies, empty-text points, converged replica); pdf_structure_storm (heading style, ligature/overlapping/point/RTL note anchors, note on a note, empty note, float, table) | 07, 09, 10, 12, 17, 18, 20, 21, 24–27, 29, 33–39 |
| `fixtures` | `src/plugins.rs`, `tests/plugins.rs` | Pinned WAT-derived binaries; end-to-end styles, geometry, relation resolution, envelope budgets and real editing kernel atomicity/undo | 04, 29, 36–38 |
| `fixtures` | `tests/hostile.rs`, `tests/snapshots/` | Invariant checks for 91 fixtures, editing undo/refusal/extreme-hit and region-role checks; paired layout/content goldens for all fixtures including plugins and authored endpoints; content-preserving debug overlay families and explainability geometry snapshots | 10, 12, 35–39 |
| `fixtures` | `tests/relations.rs` | Relation targets, queries and deletion policies through layout | 13–15 |
| `fixtures` | `tests/fonts.rs` | Versioned font chain storage, legacy overrides, unknown versions, collection declarations and incremental parity including fallback-semantics cache invalidation | 21, 22, 34, 38 |
| `fixtures` | `tests/styles_concurrent.rs` | Concurrent style expressions converge and keep unreadable values | 08, 17, 29, 34 |
| `fixtures` | `tests/flow.rs` | Columns, pagination, fragmentation, annotations on later pages, responsive templates | 24, 34 |
| `fixtures` | `tests/geometry.rs` | Transform/caret round trips, bounds, rotated follow, spiral reflow, semantic and overridden reading order; SVG/PNG visual exports | 20, 33, 39 |
| `fixtures` | `tests/regions_geometry.rs` | Floats and nested notes inside rotated, mirrored and vertical frames: determinism, roles, backends, reading order | 20, 24, 33 |
| `fixtures` | `tests/fonts_damaged.rs` | Truncated and byte-flipped default faces register or refuse, and lay out, draw and subset without panicking | 21, 37 |
| `fixtures` | `tests/incremental.rs` | Seeded text/style/template/relation/split/join/concurrent edits of every hostile fixture and spike; exact equivalence, counters, budgets, cancellation, revision/identity rejection, identical-byte anchor changes, line-height-only shaping reuse, affected-page counters and 100,000-paragraph viewport test | 27, 38, 39 |
| `fixtures` | `tests/incremental_fuzz.rs` | Seeded random edit walks (deleting pointed-at blocks, owner edits, frame-relative styles then template changes, concurrent deletes, new notes, floats and follows); every step equals `Engine::layout` | 16, 27, 38 |
| `fixtures` | `tests/images.rs`, `tests/images_damaged.rs` | Image pipeline tests; damaged PNG/JPEG bytes never panic from header to pixels | 19, 37, 38 |
| `cli` | `src/lib.rs` | `write_outputs`: layout JSON, and display list JSON, SVG and PNG for every page, plus a PDF | 32 |
| `cli` | `src/main.rs` | The `reprise spike [OUT_DIR]` command | 40 |
| `cli` | `tests/spike.rs`, `tests/snapshots/` | End-to-end spike tests and JSON fixtures | 38, 40 |
| `reprise` | `src/lib.rs`, `dto.rs`, `error.rs`, `convert.rs`, `session.rs`, `typescript.rs`; `API.txt`, `tests/`, `examples/` | Versioned session facade, owned jobs, editor lifecycle, boundary validation, TypeScript generation and native display golden | 02, 04, 28, 29, 38, 41 |
| `reprise-wasm` | `src/lib.rs`, `ts/`, `package.json` | Typed JavaScript objects, Uint8Array resources and explicit worker job steps over the same facade | 04, 28, 36, 38, 41 |
| `fuzz-harness` | `src/input.rs`, `src/scenario.rs`, `src/pool.rs` | Total byte-to-scenario decoder (every byte string is valid), seeded byte generator, and the fixed pools (texts, styles, fonts, images, HTML) scenarios draw from | 39 |
| `fuzz-harness` | `src/start.rs`, `src/engine.rs` | Starting documents (synthetic, or a hostile fixture through a saved package, once per peer) and the engine configuration as data, rebuilt when an op changes it | 34, 38, 39 |
| `fuzz-harness` | `src/run.rs`, `src/oracle.rs`, `src/authored.rs`, `src/codes.rs`, `src/violation.rs` | The executor over two peers (edit, undo/redo, sync, copy/paste, import/export, styles/templates, layout through `Engine::layout` and budgeted jobs, save/reopen, render, navigation) and the oracles checked after every op: no panic, incremental equals reference, convergence, undo/redo restores state, reopen identical, copy/paste keeps text, reading order covers each line once, byte-identical output, documented diagnostic codes | 07, 16, 27–29, 33–35, 37–39 |
| `fuzz-harness` | `src/minimize.rs`, `examples/explore.rs`, `examples/corpus_to_bin.rs` | Delta-debugging shrinker for failing byte strings, a seeded explorer that prints minimised failures, and the hex-corpus-to-libFuzzer converter | 39 |
| `fuzz-harness` | `tests/cross_crate.rs`, `tests/regressions.rs`, `corpus/` | The bounded stable target (corpus twice, seeded random scenarios, feature-coverage guard), one explicit regression per bug found, and the checked-in minimised corpus (`corpus/regress/*.hex`) | 37, 39 |
| `fuzz` | `fuzz/Cargo.toml`, `fuzz/fuzz_targets/` | cargo-fuzz (nightly, libFuzzer) targets over the same harness; not in the workspace | 39 |

| `doc` | `src/sync.rs`, `src/sync_text.rs`, `src/sync_tests.rs` | Bounded JSON deltas and full joins, negotiation, causal preflight and atomic refusals | 29, 34, 37 |
| `doc` | `src/changes.rs`, `src/invariants.rs`, `src/position.rs` | Change reports, read-time invariant audit and stable-caret fallback | 07, 10, 12, 27, 30 |
| `doc` | `src/hostile.rs`, `src/hostile_tests.rs` | Bounded seeded raw-peer mutations and malformed packet regressions | 37, 39 |
| `edit` | `src/stable.rs`, `tests/stable.rs`, `tests/concurrency.rs` | Anchored selections, per-user undo and the conflicting-command matrix | 07, 10, 12, 29, 30 |
| `reprise` | `src/session/collab.rs`, `tests/collab.rs`, `tests/hostile_peer.rs` | Native multiplayer API, presence geometry, partition/rejoin soak and hostile-session pipeline | 29, 30, 37, 39 |
| `reprise-wasm` | `src/lib.rs`, `ts/worker.ts`, `ts/smoke.cjs`, `ts/worker-smoke.cjs` | Browser multiplayer API, worker selection effects and native/WASM behavior parity | 29, 30, 38 |
| `layout` | `src/shaping.rs`, `src/workers.rs`, `src/incremental.rs` | Fine layout units, scoped workers and deterministic staged preparation | 27, 28, 38 |
| `fixtures` | `tests/incremental_steps.rs` | Budget bounds, worker order parity, stale jobs and remote memo reuse | 27, 28, 38 |
| `fuzz-harness` | `src/facade.rs` | Public session, job, sync, save/reopen, clipboard and rendering oracles | 29, 34, 37, 39 |

Other files:

-   `fixtures/images/`: original CC0 PNG and JPEG inputs, an oversized metadata-only input and a reproducible generator.
-   `fixtures/fonts/`: four bundled generic defaults (Source Serif Pro, Source Sans 3, Source Code Pro, Dancing Script), their OFL licenses and pinned provenance in README.md.
-   `.github/workflows/ci.yml`: CI, including clipboard in native and WASM checks and the wider cross-crate fuzz job.
-   `docs/fuzzing.md`: how to run the cross-crate fuzz target, shrink a failure and add a regression.
-   `Cargo.toml`, `Cargo.lock`: shared dependencies, including pinned plugin runtime and WAT test compiler.

Every crate has a `Cargo.toml` manifest; shared dependencies live in the root manifest.

| Crate | Files | Purpose | Decisions |
| --- | --- | --- | --- |
| `doc` | `src/marks.rs`, `src/style.rs`, `src/relation.rs`, `src/lib.rs`, `src/changes.rs` | LF/TAB characters, merge-safe marks1 property patches, inherited alignment/tabs1, staged POINT-based defaults and pins, feature detection and change reporting | 05–07, 11–15, 19, 29, 34 |
| `compose` | `src/marks.rs`, `src/para.rs`, `src/greedy.rs`, `src/lib.rs`, `Cargo.toml` | Bounded first-fit tab-field fitting, invisible advance cells, unreachable-stop/work diagnostics and empty-tail continuation; additive request entry point | 11, 19, 20, 23, 37 |
| `layout` | `src/marks.rs`, `src/relations/alignment.rs`, `src/relations/mod.rs`, `src/flow.rs`, `src/snapshot.rs`, `src/display.rs`, `src/codes.rs`, `src/lib.rs` | Read-only page marks, bounded line-translation domain, alignment and leader positioning, unpainted control advances; snapshots retain derived interval/guide data | 05, 13–15, 20, 24–28, 30, 37 |
| `edit` | `src/command.rs`, `src/plan.rs`, `src/editor.rs`, `src/error.rs` | Atomic marks commands, property-only style patches and staged anchor/default identity through undo | 07, 12, 29, 31, 37 |
| `format` | `src/features.rs`, `src/tests.rs`, `SPEC.md` | Required bit 11 refuses older readers for retained authored marks; unknown-bit refusal uses an unallocated high bit | 34 |
| `clipboard` | `src/html.rs`, `src/export.rs`, `tests/pdf_ua.rs` | HTML paragraph alignment, plain/BR/TAB semantics and explicit marks fidelity loss; invisible note-reference regression | 35, 37 |
| `display` | `src/pdf.rs` | Retain a break-only reference's Link annotation in the structure tree, preventing an untagged-annotation panic | 32, 37 |
| `reprise`, `reprise-wasm` | `reprise/src/dto.rs`, `reprise/src/convert.rs`, `reprise/src/session.rs`, `reprise/src/typescript.rs`, `reprise/tests/marks.rs`, `reprise/API.txt`, `reprise-wasm/src/lib.rs`, `reprise-wasm/ts/{types.d.ts,reprise_wasm.d.ts,worker.ts,smoke.cjs,worker-smoke.cjs}` | Owned marks/command DTOs, current-layout marks query and worker response, generated declarations and actual WASM smokes | 02, 04, 28–31, 38 |
| `fixtures` | `src/marks.rs`, `src/hostile.rs`, `src/lib.rs`, `tests/marks.rs`, `tests/hostile.rs`, `tests/geometry.rs`, `examples/marks_poem.rs` | Poem and cyclic/unreachable-tab goldens, concurrency, empty tails, geometric hits, reflow, RTL/vertical and storm attacks; reproducible visual export | 11–15, 20, 29, 30, 37–39 |
| docs | `docs/marks.md`, `docs/bindings.md`, `docs/contracts.md`, `docs/CODEMAP.md` | Authored/query vocabulary, bounded fallback rules and target fidelity | 05, 11–15, 34, 37–39 |

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
    1.  Append a function in `fixtures/src/hostile.rs` and append it to `all()`.
        Pin peers/fonts and declare every expected Warning or Error code.
    2.  Append a test in `fixtures/tests/hostile.rs` and bump the fixture count.
    3.  Record its geometry snapshot with `INSTA_UPDATE=always cargo test -p
        reprise-fixtures --test hostile <name>`. Run the
        `debug_families_preserve_content_for_every_fixture` test with the same update
        setting to record the paired `hostile__content_<name>.snap` golden.
    4.  Read both diffs; explain each change. Existing goldens must keep passing,
        and content must remain identical with every debug overlay family.

-   **Add a plugin extension:**
    1.  Follow the versioned ABI in `docs/plugins.md`; declare the content hash,
        capability grants, phase and explicit fuel/memory/table/stack limits.
    2.  Use `plugin/src/function.rs` for pure style functions or
        `plugin/src/geometry.rs` for bounded geometry. Register layout adapters through
        `layout/src/plugins.rs`; relation schemas use the shared target resolver.
        Editing extensions stage commands through `EditKernel` for one atomic step.
    3.  Add checked-in WAT/WASM fixtures and capability/failure tests. Verify exact
        compilation with `plugin/examples/compile_test_plugins.rs`; include
        `Engine.plugins.envelope()` in host reproduction/cache tags.
-   **Add an exporter:**
    1.  Implement `Exporter` in `clipboard/src/export.rs` or a new module and return
        bytes plus a `LossReport` for every relevant feature. Define stable codes for
        newly reported losses in `codes.rs` and `docs/contracts.md`.
    2.  Follow semantic or explicit `reading_order`; report unplaced content and
        unsupported relations, frames, notes, transforms, fonts and assets honestly.
    3.  Cover Unicode, page-limited tails and House-of-Leaves features in
        `clipboard/tests/import_export.rs`; test output with a format-aware reader.
-   **Add a bundled font default:**
    1.  Add a GPLv3-compatible licensed font under `fixtures/fonts/` with its license,
        pinned source and content hash in that directory's `README.md`.
    2.  Register the bytes and generic-family mapping in `font/src/supply.rs`; cover
        matching and deterministic fallback in `src/supply/tests.rs` and
        `shape/src/fallback_tests.rs`.
    3.  Update fixture font helpers and persistence/clipboard coverage as needed;
        verify package embedding/restoration, corrupt-font refusal and backends.
        Explain any affected geometry or content goldens.


| `doc` | `src/emphasis.rs`, `src/formatting.rs`, `src/style.rs`, `src/lib.rs` | Weight/slant/RGBA and independent decoration patches; paragraph inheritance, bounded resolution, retained emphasis detection and old-reader refusal tests | 08, 21, 29, 34, 37 |
| `font` / `shape` | `font/src/lib.rs`, `font/src/supply.rs`, `shape/src/paragraph.rs`, `shape/src/lib.rs` | Integer decoration metrics; CSS matching against generic variants; additive descriptor-aware itemisation preserving frozen StyleRun | 21, 22, 38 |
| `layout` | `src/flow.rs`, `src/flow/stage.rs`, `src/snapshot.rs`, `src/display.rs`, `src/incremental.rs` | Paint-aware shaping keys, selected-face decoration geometry, RTL/vertical paths and matching PDF addresses | 19, 20, 27, 32 |
| `format` | `src/features.rs`, `src/tests.rs`, `tests/emphasis.rs`, `SPEC.md` | Required emphasis bit 10, retained/tombstone and named-style detection; unknown-bit refusal | 34 |
| `clipboard` | `src/text_css.rs`, `src/html.rs`, `src/export.rs`, `tests/formatting.rs` | Bounded emphasis CSS/tag parsing, exact RGBA spans, independent decoration and native round trips | 35, 37 |
| `fixtures` | `tests/emphasis.rs`, `src/hostile.rs`, `tests/hostile.rs`, `tests/snapshots/hostile__emphasis_overlap.snap`, `tests/snapshots/hostile__content_emphasis_overlap.snap` | CSS face matching, paint memo invalidation, vertical/RTL paths, zero-width/combining/extreme inputs; concurrent emphasis hostile pair | 08, 19–22, 27, 29, 32, 37–39 |
| `reprise` / `reprise-wasm` | `reprise/src/dto.rs`, `convert.rs`, `typescript.rs`, `tests/formatting_glyphs.rs`, `API.txt`; `reprise-wasm/ts/types.d.ts`, `reprise_wasm.d.ts`, `smoke.cjs`, `worker-smoke.cjs` | Facade-owned emphasis fields, validation, generated declarations and native/WASM/worker coverage | 02, 29, 34, 35, 38 |
