# Reprise engine architecture

Decisions for **reprise-engine**, the relational document and layout engine behind
**Reprise**, the WYSIWYG editor. They come from the architecture decision workbook
(40 decisions in 10 groups), answered on 2026-10-03. Decision 41, the implementation language,
was added afterwards.

## North star

> It should be possible to write a responsive *House of Leaves* with this engine.

That means:

-   Text that rotates, mirrors, spirals, runs backwards, or is boxed and struck through, and
    still reflows as the viewport changes.
-   Footnotes that nest, cross-reference and take over whole pages.
-   Very sparse pages, where a few words are placed in space.
-   A reading order that stays well defined and accessible however strange the geometry.

When a decision below is unclear, choose the reading that keeps this possible.

## Repositories

| Repo | Contents |
| --- | --- |
| [`vaynealtapascine/reprise-engine`](https://github.com/vaynealtapascine/reprise-engine) | Headless engine libraries, the editing kernel, and this document. |
| [`vaynealtapascine/Reprise`](https://github.com/vaynealtapascine/Reprise) | The WYSIWYG editor UI. It includes reprise-engine as a git submodule at `engine/`. |

The submodule pins the engine commit that Reprise builds against. After cloning Reprise, run
`git submodule update --init`. To move to a newer engine, commit the new submodule pointer in Reprise.

The editing kernel (decision 02) lives in reprise-engine as its own library, so any UI can reuse it.
Decisions about the editor UI itself belong in Reprise.

## Summary

| # | Decision | Choice |
| --- | --- | --- |
| 01 | Engine purpose | Implicit document flow by default; spatial composition and reading order can be made explicit |
| 02 | Engine / editor separation | Headless engine + reusable editing kernel + separate UIs |
| 03 | Module boundaries | Several libraries in one workspace |
| 04 | Extension contracts | Typed interfaces + sandboxed scripts/WASM; "native" means built-in first-party libraries |
| 05 | Authored vs derived state | Separate document, layout and display representations |
| 06 | Semantic structure | Content tree + separate relation and style graphs |
| 07 | Identity and lifecycle | Persistent opaque IDs, never reused, with tombstones |
| 08 | Styles | Hybrid: named styles with inheritance + direct overrides, explicit value stages |
| 09 | Text store and positions | Collaborative sequence (CRDT) |
| 10 | Word references | Persistent anchors with derived word queries, plus author-marked ranges |
| 11 | Authored vs visual lines | Both authored line entities and break markers; visual lines are derived |
| 12 | Anchor edit behaviour | Custom policies per range type, with presets |
| 13 | Relation targets | IDs and ranges, structural queries, layout queries, snapshot references |
| 14 | Relation types | Registered relation schemas |
| 15 | Invalid targets | Automatic rebinding with diagnostics |
| 16 | Graph separation | Separate identity, authored-relation and computed-dependency graphs |
| 17 | Expression language | Bounded typed expressions + registered pure functions |
| 18 | Resolution context | Hierarchical by default; explicit declaration allowed |
| 19 | Numeric policy | Fixed-point layout units |
| 20 | Coordinates | Typed spaces with transforms; logical and physical axes |
| 21 | Fonts | Pinned face identities, bundled fonts, explicit fallback chains |
| 22 | Shaping | Interchangeable adapters behind one contract; the adapter is part of engine configuration |
| 23 | Line composition | Pluggable composers + geometry providers |
| 24 | Block and region flow | All in scope: ordered flow, frames/columns, pagination, floats/tables/notes |
| 25 | Solvers | Hybrid: specialised algorithms + explicit constraint-solver domains |
| 26 | Cycles | Staged passes + bounded iteration |
| 27 | Invalidation | Dependency-driven incremental evaluation |
| 28 | Scheduling | Viewport-first layout; background workers complete the full pass |
| 29 | Mutation and collaboration | Collaborative operations; collaboration is in scope |
| 30 | Selection and navigation | Editing kernel over layout mappings |
| 31 | Gestures | Mode-dependent, with user-selectable policies |
| 32 | Display boundary | Backend-neutral display list |
| 33 | Reading order | Semantic order with explicit overrides |
| 34 | File format | Versioned structured file; packaged and external assets; optional caches |
| 35 | Clipboard and export | Multiple formats; exports report what they lose |
| 36 | Plugin execution | Sandboxed WASM/scripts for all loadable extensions |
| 37 | Failure behaviour | Partial snapshot + diagnostics |
| 38 | Determinism | Same inputs, including the adapter, give the same output on every platform |
| 39 | Provenance and verification | Explanations, reverse dependencies, debug renderer, reproducible fixtures |
| 40 | Development approach | End-to-end spike, then contract-first parallel modules |
| 41 | Implementation language | Rust; Bend2 possibly later as a proven model of core rules |

Every decision here is **Decided (pre-spike)**. A decision changes by editing this document in
a commit that says why. The **Revisit if** lines record what evidence would reopen a decision.

---

## 1. Project boundaries

### 01 Engine purpose

**Choice:** Document flow by default. Content with no placement rules flows and gets its reading
order from that flow. Any part of a document can opt into explicit spatial composition, with
an explicit reading order (see 33).

**Rationale:** Flow covers most writing for free. The north star needs spatial freedom anywhere,
without losing order or accessibility.

**Related:** 20, 24, 33. **Revisit if:** the spike shows that switching between flow and spatial
needs a separate composition domain, not a per-subtree opt-in.

### 02 Engine and editor separation

**Choice:** A headless engine, a reusable editing kernel (selection, commands, undo,
collaboration ops) and separate UIs. Reprise is the first UI.

**Rationale:** Every client should be able to do every document operation without a particular
UI. Editors can then expose different capabilities over the same kernel.

**Related:** 29, 30, 31.

### 03 Module boundaries

**Choice:** Several libraries in one workspace in reprise-engine, for example text store,
document model, styles, shaping adapters, composition, layout, display list and editing kernel.

**Rationale:** Explicit interfaces between libraries let several agents work in parallel (40),
at the cost of extra interface and packaging work. Process or service boundaries are not needed.

**Revisit if:** the cost of keeping interfaces in sync between libraries is greater than the
benefit of working on them in isolation.

### 04 Public extension contracts

**Choice:** Several mechanisms:

-   Registered typed interfaces for composers, relation schemas, pure functions, geometry
    providers and shaping adapters.
-   Sandboxed scripts or WASM for third-party code (36).
-   "Native extensions" means first-party libraries compiled into the engine. There are no
    loadable native plugins.

Contracts are versioned before any external client depends on them.

**Related:** 14, 17, 22, 23, 36.

---

## 2. Authoritative state and document structure

### 05 Authored and derived representations

**Choice:** Three separate representations:

-   **Document:** authored. It is saved, undoable and collaborative.
-   **Layout:** derived. Fragments and geometry can be thrown away and recomputed.
-   **Display:** derived. A display list for each backend (32).

**Rationale:** Saved and undoable state stays separate from state that is recomputed. Derived
caches may be stored, but only as optional caches (34).

### 06 Semantic structure

**Choice:** A content tree for containment, plus separate relation and style graphs. Headings,
verses, stanzas and notes are tree nodes. Discontinuous groups, references and notes are
relations.

**Rationale:** The tree carries meaning without fixing appearance, and graphs carry anything that
cuts across the tree.

**Related:** 14, 16.

### 07 Stable identity and lifecycle

**Choice:** Persistent opaque IDs that are never reused. Deleting an item leaves a tombstone, so
undo, collaboration and relations can restore it or report it missing. Copying creates new IDs
and remaps them (35). Moving keeps the ID.

Split/join copies retain bidirectional character lineage. Like staged block IDs,
identity history survives undo; successful undo/redo records aliases for recreated
character IDs before later edits. Stable carets prefer live identities and search
at most 256 distinct lineage identities. Authored ranges retain block-local policy.

**Rationale:** This fits the CRDT text store (09) and automatic rebinding (15), which both need
to know what an ID used to be.

**Revisit if:** tombstones grow too much and need a compaction policy. A compaction policy is
needed at some point anyway.

### 08 Styles and property precedence

**Choice:** A hybrid. Named styles with inheritance, plus direct overrides on nodes. The value
stages are explicit:

1.  specified
2.  inherited
3.  computed
4.  used

Percentage bases are part of the resolution context (18).

**Rationale:** Named styles are familiar in an editor, direct overrides allow one-off edits, and
explicit stages make every value explainable (39).

---

## 3. Text storage and persistent addressing

### 09 Text store and canonical positions

**Choice:** A collaborative sequence (CRDT), so every character has an identity. Storage
offsets, grapheme navigation and persistent identity are separate kinds of position, with
defined conversions between them.

**Rationale:** Collaboration is in scope (29), and per-character identity gives persistent anchors
(10, 12) a natural basis.

**Revisit if:** the memory or performance overhead is too high for long documents. In that case,
consider a rope with a CRDT layer kept separately.

### 10 Persistent ranges and word references

**Choice:** Two mechanisms together:

-   **Persistent anchors with derived word queries:** "this word" is resolved by segmentation at
    query time.
-   **Author-marked ranges:** some writing systems have no word boundaries, or segment them in
    ways the author disagrees with, so the author can always mark a unit explicitly.

**Rationale:** Words are not universal, so the engine must not depend on segmentation for
anything durable.

**Related:** 12, 13.

### 11 Authored and visual lines

**Choice:** Both. Authored line entities are used where line structure is meaningful, as in
verse. Break markers are used inside ordinary paragraphs. Visual lines are always derived
fragments.

The following are separate concepts:

-   soft wrap
-   forced line break
-   stanza break
-   paragraph break

**Rationale:** For poetry, an authored line must survive reflow as something that can be
addressed, even when it wraps.

### 12 Anchor edit behavior

**Choice:** A policy for each range type. A policy sets start and end affinity separately and
says what replacing, deleting and moving do to the range. Presets such as *expanding* and
*fixed-content* cover the common cases.

**Rationale:** A single affinity flag is not a complete policy, as the workbook notes.

---

## 4. Relations and query semantics

### 13 Relation targets

**Choice:** A relation can target:

-   concrete IDs and persistent ranges
-   structural queries, such as "the next stanza"
-   layout queries, such as `LineContaining` and `PreviousLine`
-   explicit snapshot references

Layout queries are re-evaluated after reflow. Each query defines what happens with zero matches
and with several matches.

**Related:** 15, 16, 26. Layout queries can create cycles.

### 14 Relation types and ownership

**Choice:** Registered relation schemas. The built-in types are alignment, spacing, order,
grouping, breaks and references. Extensions can register more. Each schema declares:

-   whether it is owned by a node or exists independently
-   what happens to it when its targets are deleted or copied

### 15 Invalid targets and rebinding

**Choice:** Automatic rebinding with diagnostics. A relation is always in one of four states,
which the editor can see:

-   **valid**
-   **rebound**
-   **ambiguous**
-   **missing**

Tombstones (07) and CRDT identity (09) are the evidence for choosing a new target.

**Revisit if:** automatic rebinding surprises authors. Schemas could then opt out (see 14).

### 16 Graph separation and public queries

**Choice:** Separate graphs:

-   identity
-   authored relations
-   computed dependencies

Public queries such as "all relations touching this range" and "all layout fragments containing
this stanza" work across them.

**Rationale:** Authored edges are saved and undoable, and computed edges can be thrown away (05).
Mixing them would blur that line.

---

## 5. Values, geometry and coordinate systems

### 17 Symbolic expression language

**Choice:** Bounded typed expressions, like `calc()`, with dimensions checked by type. Registered
pure functions can be added. Every expression reports its dependencies (27). Symbolic values
become numbers at the computed or used stage (08).

### 18 Resolution context

**Choice:** Contexts are hierarchical by default: page, frame, block, line. A property may
declare its basis explicitly instead, for example a percentage of a named frame.

`em`, `lh`, percentages and reference geometry have precise definitions, including what happens
when the basis is unresolved or indefinite.

### 19 Resolved units and numeric policy

**Choice:** Fixed-point layout units, an integer number of sub-units per point, chosen at
implementation time. Rounding, overflow and comparison rules are specified.

**Rationale:** Fixed point is needed for the cross-platform determinism guarantee (38), but is not
enough on its own. Transforms (20) may use floating point at render time only.

### 20 Coordinates and writing modes

**Choice:** Typed spaces, each with its own coordinate type and explicit transforms between them:

-   page
-   frame
-   line
-   glyph
-   viewport
-   device

Both logical axes (inline and block) and physical axes exist. Direction, writing mode, rotation
and mirroring are first-class.

Vertical columns progress downward in both `vertical-rl` and `vertical-lr`;
`sideways-lr` preserves the earlier upward geometry. Upright shaping and short
horizontal combinations use integer metrics, with glyph orientation compensated
inside the shared display list. Combined groups are atomic visual caret units.
Frame continuations prepare text for the destination mode while freezing the
starting percentage context and table column allocation.

**Rationale:** The north star needs this: rotated, mirrored and spiralling text.

---

## 6. Typography and composition

### 21 Fonts and reproducibility

**Choice:** Documents reference pinned face identities: family, version and content hash.
Fonts can be bundled in the document package (34). Each style has an explicit fallback chain.
Every substitution is reported.

The reproducibility envelope is:

-   fonts
-   the shaping adapter and its version
-   dictionaries
-   engine settings

### 22 Shaping and Unicode mappings

**Choice:** Interchangeable shaping adapters behind one contract. The contract keeps the
mappings from source text to clusters to glyphs, and the bidi information. It allows text to be
reshaped when a break changes its boundaries.

The adapter and its version are part of the engine configuration, and so part of the input
set that the determinism guarantee covers (21, 38). Each adapter declares whether it is
**platform-independent**, meaning its output depends only on its declared inputs:

-   **Platform-independent:** for example a pinned HarfBuzz or rustybuzz build, possibly compiled
    to WASM. It gives the same output on every platform.
-   **Platform-dependent:** an adapter that wraps the OS or browser shaper, such as CoreText,
    DirectWrite or a browser text engine. It is deterministic on one platform, but the platform
    shaper's version is a hidden input, so its output can differ between platforms.

The engine ships a default adapter that is platform-independent.

### 23 Line composition and geometry

**Choice:** Pluggable composers: greedy, optimal (Knuth–Plass style) and authored-break.
Geometry providers return the available intervals for each line, which handles shapes,
runarounds and non-rectangular measures. The available geometry may depend on line height and on
earlier composition.

A fragment records its adjustments, the reason for its break and its score (39).

### 24 Block flow and region composition

**Choice:** All of these are in scope:

-   ordered flow
-   frames and columns that text threads through
-   pagination and fragmentation
-   floats
-   tables
-   notes

Floats, tables and notes are each their own subsystem.

**Note:** This is a large scope. The spike (40) covers only ordered flow plus one annotation.
The others followed as parallel modules once the contracts were frozen.

**Resolved (2026-10-04):** where allocation, fragmentation and backtracking live. See
[regions.md](regions.md).

-   **Allocation** lives in each subsystem: flow owns frames and pages, floats their
    placement and exclusions, notes their areas and body reservations, and tables their
    columns, through a declared solver domain (25).
-   **Fragmentation** advances monotonic cursors within a pass.
-   **Backtracking** is a complete reflow, run by a coordinator with a bounded number of
    passes (26). Line breaking stays inside composers.

---

## 7. Dependencies and layout evaluation

### 25 Algorithm and solver boundaries

**Choice:** A hybrid. Specialised algorithms handle text breaking and block flow. A constraint
solver is used only inside declared solver domains, such as alignment relations and region
allocation.

### 26 Passes, cycles and convergence

**Choice:** Staged passes. Where a cycle is allowed, it iterates up to a limit. Each cycle class
is documented. Oscillation and non-convergence produce diagnostics, not hangs (37).

### 27 Invalidation and caching

**Choice:** Dependency-driven incremental evaluation over the computed-dependency graph (16).
Cache keys include:

-   text revision
-   font identity
-   style
-   query results
-   relations

### 28 Scheduling and revision consistency

**Choice:** Layout is demand-driven and viewport first. The visible region is laid out promptly.
When a full pass is nontrivial, background workers finish it in parallel.

Every result is tagged with the document revision it was computed from. A stale result is
rejected or published only as stale. Jobs can be cancelled.

**Related:** 05 (snapshots per revision), 37.

---

## 8. Editing and rendering contracts

### 29 Mutation, undo and collaboration

**Choice:** Edits are collaborative operations on the CRDT (09). Collaboration is in scope. The
editing kernel defines:

-   atomic groups of operations
-   validation
-   per-user undo

### 30 Selection, hit testing and navigation

**Choice:** The editing kernel implements caret, selection, hit testing and logical versus visual
movement, using mappings that layout exposes between source, clusters and geometry. It handles
caret affinity, ligatures, bidi boundaries and range geometry.

### 31 Gesture interpretation

**Choice:** Editor modes decide what a drag authors, for example moving something versus
creating a relation. Users can pick a policy. A gesture never creates a relationship, such as a
proportional one, without an explicit rule.

**Note:** This is mostly a Reprise decision. The kernel only provides the operations.

### 32 Display and backend boundaries

**Choice:** A backend-neutral display list of glyph, path, image and clip operations, with
resource ownership defined. Renderers (canvas, DOM) and exporters (SVG, PDF, HTML) consume it.

---

## 9. Semantics, persistence and extension safety

### 33 Reading order and accessibility

**Choice:** Reading order follows the semantic tree by default, with explicit overrides wherever
the spatial layout diverges from it. Accessible structure, annotation attachment and text
extraction all follow this order.

### 34 File format, assets and migrations

**Choice:** A schema-versioned structured file with migrations. Unknown extension data is kept,
not dropped. Documents can be packaged with their fonts and images, or reference external
assets. Derived layout caches can be stored, but can always be thrown away.

Pinned font identities and layout settings are saved (21).

### 35 Clipboard and export fidelity

**Choice:** The clipboard carries several formats:

-   native fragments that include relations, with IDs remapped
-   rich text
-   plain text

The native format defines what happens to relations that cross the copied boundary. Each
exporter reports what it preserved and what it lost.

### 36 Plugin execution and capabilities

**Choice:** All loadable extensions run sandboxed, as WASM or scripts. The host grants each one
capabilities, such as access to document operations, resources and I/O, and an execution budget.
Layout-time plugins get no time, randomness or I/O (38). Built-in first-party code is not a
plugin (see 04).

When a plugin is unavailable, it degrades gracefully as described in 37.

---

## 10. Failure behaviour, observability and delivery

### 37 Partial layout and fallback

**Choice:** Layout always produces a partial snapshot with diagnostics, never a failure of the
whole job. This covers missing fonts, impossible constraints, plugin errors and non-convergence
(26). Cached geometry is never presented as valid output.

### 38 Determinism contract

**Choice:** A cross-platform guarantee. Given the same inputs, composition is identical on
every platform. The inputs are:

-   the document
-   pinned fonts
-   dictionaries
-   engine settings and engine version
-   engine configuration, including the shaping adapter and its version

The guarantee covers any configuration whose adapters declare themselves platform-independent
(22). A platform-dependent adapter still gives repeatable output on one platform, and output
records which kind of adapter produced it. Fixed-point units (19) support this guarantee.
Layout-time plugins must be pure.

**Revisit if:** no platform-independent adapter can run on a target platform, such as some
browser environment.

### 39 Provenance and verification

**Choice:** All of these:

-   explanations of why a property has its value and why a line broke where it did
-   reverse dependency inspection
-   a full debug renderer with overlays for fragments, spaces and relations
-   reproducible semantic, geometry, visual and fuzz test fixtures that keep enough context to
    reproduce a failure

### 40 Integration and parallel development

**Choice:**

1.  A small end-to-end spike: anchored text, reflow, a following annotation and headless output.
2.  Freeze the library interfaces (03) and the shared test fixtures (39).
3.  Build the modules in parallel, with several agents working against the frozen contracts.

**Rationale:** Freezing contracts before any code exists tends to fix the wrong interfaces. A
spike tests them on real code first, then parallel work can scale without churn.

### 41 Implementation language

**Choice:** Rust, for the engine and the editing kernel.

**Rationale:**

-   **Ecosystem.** Rust already has the hard pieces: shaping (rustybuzz, HarfBuzz bindings),
    fonts (the fontations crates), Unicode bidi and segmentation (unicode-bidi, ICU4X),
    collaborative text (Loro, yrs, Automerge), incremental computation (salsa) and WASM
    sandboxing (wasmtime).
-   **Targets.** It compiles to WASM, which a browser editor and sandboxed plugins (36) need.
-   **Fit.** Traits suit the typed extension interfaces (04). Integer types suit fixed-point
    layout units (19).

**Alternative considered:** Bend2. Its dependent types and built-in proofs are attractive for
code written by many agents. As of 2026-10, it was rejected as the engine language because:

-   It describes itself as young and unaudited, and its proof checker isn't verified.
-   It doesn't support Windows, has no WASM target, and has no visible way to call code in other
    languages.
-   Its affine values (no shared arrays or closures) clash with the engine's shared, changing
    structures.
-   Its automatic parallelism helps little in layout, which is mostly sequential.

**Possible later use:** Bend2 could hold a proven model of the core rules: IDs are never reused
(07), the text store converges (09, 29), anchors follow their policies (12) and rebinding ends
in a defined state (15). The Rust code would then be tested against that model. Verus and Kani,
which prove properties of Rust code directly, are alternatives. Decide after the spike.

**Revisit if:** Bend2 matures, gains WASM output and a way to call other code, and the
verification benefit outweighs the ecosystem cost.

#### Implementation choices

These were chosen on 2026-10-03, before the spike.

-   **Platform:** Reprise is a Tauri app with one web UI for desktop and the web. On desktop the
    engine runs natively in the Tauri backend; on the web it runs as WASM. So the engine needs a
    native API and a WASM/TypeScript API over the same core.
-   **Collaborative text store (09, 29):** Loro, wrapped behind the engine's own text-store
    interface so the rest of the engine doesn't depend on it directly. Loro's movable tree is a
    candidate for the content tree (06), and its stable cursors for anchors (12).
-   **Text stack:**
    -   Our own composer on low-level crates, not a ready-made layout library such as parley or
        cosmic-text. That keeps fixed-point units (19), geometry providers (23) and break
        explanations (39) under our control.
    -   Fonts: fontations (skrifa, read-fonts).
    -   Shaping: harfrust, HarfBuzz's own Rust port, as the default platform-independent
        adapter (22). It was chosen in the spike over rustybuzz because it shares the
        read-fonts stack with skrifa.
    -   Segmentation and bidi: ICU4X.
-   **License:** AGPL-3.0-or-later for reprise-engine and Reprise. Collaboration is in scope, so
    the network clause also covers hosted editors and collaboration servers. Every dependency
    must be compatible with GPLv3: permissive licenses and Apache-2.0 are fine, GPLv2-only is
    not. Crates declare `license = "AGPL-3.0-or-later"`.
-   **Collaboration transport (29), decided 2026-10-04:** the engine exposes sync primitives
    only: exporting and importing updates, version information, and awareness data. How
    peers exchange them (a sync server, peer to peer, or the host's channel) belongs to
    Reprise or a server, not the engine.
-   **Tombstone compaction (07), decided 2026-10-04:** compaction is opt-in and explicit, for
    example on "save compact" or export, and never automatic. A snapshot target (13) that
    names a compacted version reports `relation.snapshot-unavailable`.
-   **Formal model of core rules (41), decided 2026-10-04:** deferred. Seeded property and
    equivalence tests cover IDs, convergence, anchors and rebinding for now. Kani proofs on
    `reprise-doc` remain a candidate later workstream.
-   **API versioning (04), decided 2026-10-04:** only stable API types cross the bindings
    boundary, every serialised payload carries a version, and the binding crates follow
    semver. Internal crates may change freely behind them.
-   **Plugin runtime (36), decided 2026-10-04:** `wasmi`, a pure-Rust WebAssembly
    interpreter (MIT/Apache), is the one plugin backend, natively and inside the engine's
    own WASM build.
    -   Budgets are counted in fuel, so a plugin that runs out stops at the same instruction
        everywhere.
    -   The host sits behind one internal interface, so wasmtime can be added later as a
        native accelerator that passes the same conformance suite.
    -   Plugins may use floats internally. NaNs are canonicalised and relaxed SIMD is
        disabled.
    -   Layout-time host APIs exchange only integer `Length` and `Fixed` values.
-   **Font supply (21), decided 2026-10-04:**
    -   **Sources:** fonts come from the frontend, for example user imports declared like
        CSS `@font-face` or font files in a project. The engine accepts font data and never
        discovers system fonts.
    -   **Pin and embed:** every font a document uses is embedded as a file in its package
        (34), pinned by its `FaceId`.
    -   **Opening elsewhere:** a missing font is substituted and reported, and the frontend
        asks the user what to do.
    -   **Fallback:** a CSS-like chain that ends in a required generic class (for example
        serif, sans-serif or script). Each class resolves to an engine default face, so the
        engine ships one default per class.
    -   **Licensing:** the engine doesn't check font licences or embedding flags. Licensing
        fonts correctly is the user's responsibility.
-   **Spike output:** four headless backends for the display list (32):
    -   JSON snapshots of the layout and display list, for test fixtures and diffs
    -   SVG, with debug overlays
    -   PNG, through a rasterizer, for visual regression tests
    -   PDF

---

## Cross-decision notes

-   **CRDT, tombstones, anchors and rebinding (07, 09, 12, 15).** These rely on each other.
    Per-character identity and tombstones are what make automatic rebinding defensible. A
    compaction policy for tombstones is needed eventually.
-   **Determinism (19, 21, 22, 36, 38).** The guarantee holds only inside the reproducibility
    envelope: pinned fonts, the configured adapters (when they are platform-independent), pure plugins and
    fixed-point units.
-   **Layout queries and cycles (13, 26, 27).** Relations that target layout queries are the main
    source of cycles. The spike should include at least one.
-   **Scope (24).** Choosing every flow feature makes the contract-first step in 40 especially
    important.

## Open questions

Resolved:

-   **Tombstone compaction (07):** opt-in and explicit; see the implementation choices
    under decision 41.

-   **Numbers fixed during implementation (2026-10-04):**
    -   Fixed-point resolution (19): `Length` is 1/1024 pt and `Fixed` is 16.16, saturating
        and rounding half away from zero.
    -   Iteration limits and cycle classes (26): these are documented in
        [regions.md](regions.md) and [incremental.md](incremental.md), and include 16
        region-feedback passes and a note depth of 32.
-   **Where allocation, fragmentation and backtracking live (24):** see
    [regions.md](regions.md) and the note under decision 24.
