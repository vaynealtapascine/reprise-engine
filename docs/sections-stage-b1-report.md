# Sections Stage B1 final report

Worktree: `F:/reprise-wt/eng-sections2`, branch `ws/sections2`, implementation
base `0cc69f9`. Scope is S1–S5 only. Final gate results are appended below when
validation finishes.

## 1. Summary by decision and slice

- **S1 — 05–07, 29, 34, 37:** explicit body/story traversal and the clippy
  guard; root stories with retained orphan recovery; flat `starts1` records
  with operation lifetimes; retained `geometry1`; aggregated read-only audit;
  required package/sync bit 17 including inactive, malformed and tombstoned
  authored state. Reads and rejected/empty commands never create these roots.
- **S2 — 06, 07, 11, 20:** transactional breaks, section patches, independent
  story creation/deletion, binding changes and deep region copy with fresh
  identities. Cross-domain ranges are refused before mutation. List counters
  reset in each domain within one aggregate cache; list marker rendering is
  unchanged. Section and story authored state is exposed through additive
  facade State fields, without S11 command bindings.
- **S3 — 20, 24, 26–28, 37, 38:** the sole page/frame allocator, stable
  Main/Other/Running addresses, complete frame provenance, guarded feedback
  entries and memo replay, physical page accounting and exhausted one-frame
  story source. Every pre-existing golden is unchanged.
- **S4 — 20, 24, 26, 28, 34, 37, 38:** frozen section resolution, bounded merged
  geometry, exact checked-integer column apportionment, RTL/vertical threading,
  template preservation, owners, manual breaks, parity fillers, decimal
  counters, mode fallback and derived physical rule paths. Note continuation
  uses the carried page's owner, including shared continuous-section anchors.
- **S5 — 20, 24, 26, 27, 37, 38:** continuous bands and a resumable integer
  bisection with 24 trials and 32 bands per page. Recorded `fit(H)` must pass;
  a rejected reference preserves ordinary output/diagnostics and starts the
  incoming section on a new page. Successful trial output is retained and
  committed directly. Full and incremental continuous layout use the same
  charged machine; table groups yield after each fragment or move. Physical
  note/float source boxes retain full provenance and project into every current
  band, including scratch depths. Spatial main depths are preserved and clipped,
  while generated columns use the owner's remaining depth.

## 2. Commits and per-slice validation

- S1: `0983a59`; domain/liveness, lifetime, retained-root/bit-17, refusal,
  orphan and facade state tests; complete S1 gate log
  `G:/reprise-target/eng-sections/s1-current-gate.log` ends with GATE DONE.
- S2: `0b6791f`; transactional commands, undo/redo, hostile peers, deep-copy
  identities, cross-domain refusal, story list reset and shared-cache tests;
  complete S2 gate log `s2-full-gate.log` ends with GATE DONE.
- S3: source checkpoint `4778c39`, validation `b22d5a4`; five layout tests
  cover allocation limits, index remapping, every provenance field, stale
  placements and long one-frame story exhaustion. All native tests, unchanged
  goldens, static checks and WASM smoke passed. A stalled rustdoc target was
  interrupted and retried successfully; see the validation note for exact logs.
- S4: `afe66d4`, `5a5773c`, `07d7fd0`, `0434bcf`; 85 layout tests and geometry
  backend fixtures, all native workspace tests and unchanged goldens passed.
  The bounded outer gate exited 124 at rustdoc; the bounded workspace doc-test
  retry passed. This is combined validation, not a claim that that outer gate
  exited zero. The fixture coverage split was explicitly approved.
- S5: drafts `e76d719`, `90cf233`, `e7a0897`; integrated implementation
  `5555f9b`, acceptance/goldens/parity `e1d98db`, spatial preservation and
  suffix-fit scan `a0a4a52`. All 103 layout tests pass (3.63 s). The unchanged
  incremental_steps suite passes all five tests in 41.90 s, close to the
  orchestrator's approximately 40 s baseline. Workspace/all-targets clippy
  passes. Final full-gate validation is recorded in item 10.

Every implementation commit has the requested GPT-6.1-Sol co-author footer.
The immediate WIP checkpoints follow the orchestrator's explicit instruction
to preserve unfinished work. The complete `main..HEAD` log is appended below.

## 3. Files touched and scope exceptions

Core changes are in doc domain/section storage and lists; edit planning and
commands; format feature detection; layout pages, sections, flow, tables,
regions, snapshots and display; facade State/TypeScript; fixtures and tests.

The necessary cross-workstream edits are narrow: clipboard callers explicitly
select Body (section transport itself is deferred); formatting/image asset and
fuzz callers select all authored domains; existing tests route body calls
explicitly to satisfy the clippy guard. Facade compatibility aliases keep their
public names and are allowed locally. Generated declaration files reflect the
additive State fields. `clippy.toml`, CODEMAP, validation/report notes, and gate
tools support these changes. The exact implementation file inventory is
appended below. No PROGRESS, frozen contract, inline checkout, dependency,
CI configuration or other worktree was edited. No implementation commit was
pushed, merged or rebased.

## 4. Design choices and deviations

The approved contract and design remain unchanged. Lifetimes are excluded
operation writes, identities are never recycled, unknown authored keys are
retained, and interpretation never repairs the document. Whole transactions
validate on an isolated fork before staging. Geometry/column arithmetic uses
checked i64 intermediates and bounded Length conversion; no layout floats.

The **approved coverage deviation** is the fixture split until S9: the shared
`all()` suite registers a retained/inactive hostile section fixture, while
effective geometry and band fixtures run dedicated layout/backend checks.
The existing clipboard roundtrip test and every old fixture/assertion remain
unchanged. Effective section transport is not added prematurely.

Balancing follows the best-tested-fit guarantee, not a global minimum claim
outside the documented monotone subset. The checkpoint owns geometry and active
continuation state; completed prefix text stays in the ordinary snapshot.
Reference fit checks scan only the suffix since C. Trial sources cannot allocate
a page. Source provenance checks remain exact; page-owned physical float planes
make constraints available before any later band is allocated. Unaffected legacy
flow/memo paths remain exact; continuous-ending sections conservatively bypass
flow transition memo replay so it cannot skip a band checkpoint.

The gate disables the compiler wrapper only in its own process environment,
after the S3/S4 rustdoc stalls. Machine cargo configuration is untouched. Every
test/gate run after the timeout rule is bounded and logged on G:.

## 5. Public API and diagnostics

Implemented approved additions: Domain and explicit traversal methods;
SectionId/BreakKind, start records, section views/bindings, story/mode storage
and audit; edit InsertBreak/RemoveBreak/SetSection/CreateStory/SetRegion/
CreateRegion/DeleteStory plus BreakExists/CrossDomain reasons;
LayoutSnapshot.sections and its ownership/band/rule records; facade
State.sections/State.stories/BlockState.starts. The resolved State summary is
read-only. Page/frame sources, balancing and physical plans remain crate-private.

Implemented approved diagnostics: Warning `section.property`, `section.geometry`,
`section.template`, `section.columns-limit`, `section.columns-fit`,
`section.column-narrow`, `section.continuous-new-page`, `section.band-limit`,
`section.page-counter`, `section.region-slot`, `section.mode-unsupported`,
`section.orphan-start`, `story.nested`; Info `section.balance-approx`,
`section.inactive`, `story.orphan`. Warnings identify changed requested rendering
or ignored invalid authored data; Info reports a verified bounded result or
recoverable retained state. Existing layout overflow/keep/page-limit/unplaced
codes retain their meanings. No frozen signature or code was changed.

## 6. Snapshots

No existing snapshot changed. Six new snapshots were hand-reviewed:

- `hostile__sections_geometry_hostile.snap`: 612x792 pt portrait, counted blank
  filler, weighted RTL section, then 792x612 pt landscape; counters 1,2,-2,-1;
  64 capped columns at 7,488 units, then 144/0/288 pt columns after two 18 pt
  gaps; rules at x=386.5/368.5 pt and body depth 648 pt. Expected fallback
  diagnostics were checked.
- `hostile__content_sections_geometry_hostile.snap`: blank filler content,
  complete glyph runs and the two physical black rule paths.
- `hostile__sections_retained_hostile.snap`: ordinary 420x300 pt page with a
  220x228 pt Main at (36,36), two full paragraph lines, no section metadata or
  diagnostics. Hostile inactive generations remain inert.
- `hostile__content_sections_retained_hostile.snap`: the same two ordinary
  paragraph runs with no added section paths.
- `hostile__sections_bands_hostile.snap`: one 200x200 pt page, owner first,
  counter 1; 1/2/1 Main frames with offsets 0/12/36 pt and depths 12/24/144 pt.
  First two bands balanced, last ragged. Weighted RTL widths 40/80 pt at
  x=120/30 pt, y=22 pt; incoming block margins ignored; restart -7 pending.
- `hostile__content_sections_bands_hostile.snap`: six complete glyph lines and
  one 1 pt rule from (114.5,22) to y=46 pt; no diagnostics or extra artifacts.

## 7. Dependencies

None added or changed. No license changes.

## 8. Contract proposals

None. `docs/contracts.md` has no implementation diff from `0cc69f9`.

## 9. Gaps and follow-ups for S6–S12

This work stops after S5. S6a connects the plain one-frame story runner,
running-slot registry, physical reservations, clipping and derived instances;
running content is not composed yet. S7a adds provenance/read-set scheduling
and selective prefix reuse, then later field-aware cache keys after inline lands.
Re-run S3 byte identity after merging inline's changes to the same machinery.
S8b adds running FieldInstance adapters, non-decimal labels and shared field/
region convergence. Known numbering formats are retained now; rendered labels
remain decimal in this part. S9 must carry section boundaries, geometry mode
and remapped stories through fragment/clipboard/import-copy/HTML paths, then
move the effective fixtures into the shared suite. S10 adds export losses and
running artifact semantics. S11 exposes commands and page/section/region/field
queries through the facade/WASM. S12 pageless layout remains gated; authored
pageless mode is retained with the approved warning and rendered paged here.

### How later features plug in

Header/footer editing uses independent Story domains, transactional SetRegion
and deep-copy unlink, with State.stories for orphan recovery. The app keeps the
page instance as view state when S6/S11 navigation arrives. A TOC uses future
page-of field instances and page labels, not authored page numbers. Per-section
footnote numbering can use the domain/section identities; one notes area and
shared carried-page ownership are already established, while section-specific
numbering/endnote policies remain future work. Print/PDF consumers use actual
PageLayout sizes and PageDescriptor ownership; S10 supplies running artifact
tags. Facing pages use physical parity and pending restarts, then running
first/even selection. Spatial House-of-Leaves templates keep their authored
main transforms/depths when columns are absent; generated columns explicitly
replace Main while preserving non-main frames.

### App seam

Available now: the editing kernel commands named in item 5, existing commands
targeting story NodeIds, raw/resolved State.sections, story text/bound_by in
State.stories and BlockState.starts. Existing hit/caret/selection/reading-order
APIs remain body-only. Planned facade insert_break/set_section/set_region and
page_sections/section_pages/page_label/page_regions/region_instances/page_fields
belong to S11; running navigation belongs to S6a. Required feature bit is
`REQUIRED_SECTIONS = 1 << 17` in package and sync compatibility checks.

## 10. Verification

Final full-gate log: `G:/reprise-target/eng-sections/s5-full-gate-2.log`.
The command tails, exit status and timing checks are appended after completion.

## Branch log before the final report commits

~~~text
a0a4a52 fix(layout): preserve spatial main depths in continuous bands
e1d98db test(sections): add reviewed band goldens and native WASM parity cases
5555f9b feat(layout): integrate resumable continuous bands and bounded balancing
e7a0897 wip(layout): checkpoint physical float spaces and continuation tests
0434bcf test(sections): record S4 native and doc-test validation
90cf233 wip(layout): preserve S5 band acceptance tests and formatted drafts
e76d719 wip(layout): checkpoint S5 resumable band and constraint drafts
07d7fd0 fix(fixtures): separate sections geometry from pending clipboard transport
5a5773c chore(sections): bound full gates and workspace tests
afe66d4 wip(sections): preserve S4 geometry integration
b22d5a4 test(layout): verify S3 page factory compatibility
4778c39 wip(sections): S3/S4 checkpoint
0b6791f feat(edit): add transactional breaks and independent story editing
0983a59 feat(doc): add retained section storage and explicit story domains
0cc69f9 docs(contracts): orchestrator review of the sections additions
f671ebb feat(contracts): draft the sections additions, for review before S1
68bc3e8 docs(sections): answer the second design review (A.3, N1-N8)
096ba8f Merge engine main (95b74ef, lists) into ws/sections2
06d0896 docs(sections): revise the design after the advisor review (A.2)
fdfe2b4 docs(sections): design note for sections, breaks and running regions
~~~

## Implementation file inventory (relative to 0cc69f9)

~~~text
clippy.toml
crates/clipboard/src/export.rs
crates/clipboard/src/image_export.rs
crates/clipboard/src/native.rs
crates/clipboard/tests/annotation_storm.rs
crates/clipboard/tests/annotations.rs
crates/clipboard/tests/formatting.rs
crates/clipboard/tests/images.rs
crates/clipboard/tests/import_export.rs
crates/clipboard/tests/native.rs
crates/clipboard/tests/paste_undo_walk.rs
crates/clipboard/tests/roles_lists.rs
crates/clipboard/tests/roundtrip.rs
crates/clipboard/tests/tables.rs
crates/doc/src/annotation_tests.rs
crates/doc/src/domain.rs
crates/doc/src/edit.rs
crates/doc/src/flow.rs
crates/doc/src/flow_tests.rs
crates/doc/src/fragment.rs
crates/doc/src/history.rs
crates/doc/src/hostile_tests.rs
crates/doc/src/invariants.rs
crates/doc/src/lib.rs
crates/doc/src/lifecycle.rs
crates/doc/src/lists.rs
crates/doc/src/position.rs
crates/doc/src/relation_tests.rs
crates/doc/src/roles.rs
crates/doc/src/sections.rs
crates/doc/src/sections/tests.rs
crates/doc/src/sections/views.rs
crates/doc/src/structure.rs
crates/doc/src/sync.rs
crates/doc/src/sync_tests.rs
crates/doc/src/table.rs
crates/doc/src/table_grid.rs
crates/doc/tests/lifecycle.rs
crates/edit/src/command.rs
crates/edit/src/editor.rs
crates/edit/src/error.rs
crates/edit/src/lib.rs
crates/edit/src/paste.rs
crates/edit/src/plan.rs
crates/edit/src/plan/sections.rs
crates/edit/src/plan/semantic.rs
crates/edit/src/replace.rs
crates/edit/src/stable.rs
crates/edit/tests/audit.rs
crates/edit/tests/concurrency.rs
crates/edit/tests/document_edits.rs
crates/edit/tests/lists.rs
crates/edit/tests/movement.rs
crates/edit/tests/page_setup_paste.rs
crates/edit/tests/replace.rs
crates/edit/tests/sections.rs
crates/edit/tests/stable.rs
crates/edit/tests/transactions.rs
crates/edit/tests/undo_merge_fuzz.rs
crates/fixtures/src/hostile.rs
crates/fixtures/tests/hostile.rs
crates/fixtures/tests/images.rs
crates/fixtures/tests/incremental.rs
crates/fixtures/tests/incremental_fuzz.rs
crates/fixtures/tests/incremental_steps.rs
crates/fixtures/tests/paragraph_styles.rs
crates/fixtures/tests/roles_lists.rs
crates/fixtures/tests/section_domains.rs
crates/fixtures/tests/snapshots/hostile__content_sections_bands_hostile.snap
crates/fixtures/tests/snapshots/hostile__content_sections_geometry_hostile.snap
crates/fixtures/tests/snapshots/hostile__content_sections_retained_hostile.snap
crates/fixtures/tests/snapshots/hostile__sections_bands_hostile.snap
crates/fixtures/tests/snapshots/hostile__sections_geometry_hostile.snap
crates/fixtures/tests/snapshots/hostile__sections_retained_hostile.snap
crates/format/src/container.rs
crates/format/src/features.rs
crates/format/src/image_assets.rs
crates/format/src/tests.rs
crates/fuzz-harness/src/authored.rs
crates/fuzz-harness/src/run.rs
crates/layout/src/display.rs
crates/layout/src/floats.rs
crates/layout/src/flow.rs
crates/layout/src/flow/bands.rs
crates/layout/src/flow/keeps.rs
crates/layout/src/flow/paragraph.rs
crates/layout/src/incremental.rs
crates/layout/src/lib.rs
crates/layout/src/notes.rs
crates/layout/src/pages.rs
crates/layout/src/pages/bands.rs
crates/layout/src/pages/tests.rs
crates/layout/src/reading.rs
crates/layout/src/regions.rs
crates/layout/src/regions/physical.rs
crates/layout/src/regions/tests.rs
crates/layout/src/sections/geometry.rs
crates/layout/src/sections/mod.rs
crates/layout/src/sections/spaces.rs
crates/layout/src/sections/tests.rs
crates/layout/src/sections/tests/bands.rs
crates/layout/src/snapshot.rs
crates/layout/src/table.rs
crates/layout/src/table_flow.rs
crates/layout/src/table_flow/step.rs
crates/layout/src/table_tests.rs
crates/layout/src/template.rs
crates/reprise-wasm/ts/reprise_wasm.d.ts
crates/reprise-wasm/ts/types.d.ts
crates/reprise/examples/sections_parity.rs
crates/reprise/src/dto.rs
crates/reprise/src/session.rs
crates/reprise/src/session/collab.rs
crates/reprise/src/typescript.rs
crates/reprise/tests/section_domains.rs
docs/CODEMAP.md
docs/sections-stage-b1-validation.md
tools/check-sections.sh
tools/sections-parity.cjs
docs/sections-stage-b1-report.md
~~~
