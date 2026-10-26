# Sections Stage B2 final report

Worktree `F:/reprise-wt/eng-sections2`, branch `ws/sections2`. Part 1 ended at
`73c8142` (S1–S5). This part merged engine main `4131ec8`, built S6, S7, S8a and
S8b, fixed the two blockers of the Astra review of part 1, then merged main
`0d8d296` and extended the isolated preflight to every section command. The head
the full check ran on is `71adaba`; this report is the only commit after it.
Bit 17 (`REQUIRED_SECTIONS`) is unchanged. Nothing was pushed or merged.

## 0. Read this first: what is narrower than the design

1. **S7 scheduling units are not built.** `docs/sections.md` §11 says a running
   instance is composed in resumable units, "per block, then commit", charged to
   `step(budget)`. Here the regions of a page are composed when the page is
   made, inside the unit that makes it (a paragraph's move to the next frame, a
   section start, or the job's first page). That unit can therefore contain
   up to three story compositions. They are bounded: 4,096 blocks and 1 MiB of
   text read per story, composition stops at the frame's depth, and a memo hit
   costs nothing. But a budget of 1 does not bound the work of that one unit,
   which the design requires. Every other S7 unit stays as before, a budget of 1
   still makes progress, and the equality and chunking tests pass. The fix is a
   look-ahead state machine in `Cursor`, analogous to S5's `Balance`: precompute
   the next page's regions into the running memo one story block per unit, so
   page creation only ever hits the memo. Estimated at a few hundred lines. I
   did not start it because the last instruction was to run the check and write
   this report. Decide whether to commission it or to narrow §11.
2. The job's **first page** is composed in `LayoutJob::new`, before any step.
   The same look-ahead would move it.
3. Everything else in the S6, S7, S8a and S8b build lists is built and tested;
   see §1.

## 1. Summary by decision and slice

- **Merge of main 4131ec8 (01–38):** committed on its own as `7fb8d70`. Lists
  (bit 14) and inline (bit 15) run through `PageFactory`/`FrameAddr`: inline's
  `FieldEnv`/`FieldKey`, per-line extents, fields in notes and floats, and
  caller-supplied table-body placement are all kept, and the resumable
  `GroupStep` takes the caller's instance. Every pre-existing snapshot is
  byte-identical to main; native equals WASM. `KNOWN_REQUIRED` has bits 13–17.
- **Blocker 1, body isolation (`b852f9c`, 06, 27, 33):** body region allocation
  keeps only relations whose live owner is in `Domain::Body`, before the
  4,096-relation limit and before owner registration. A story's note or float no
  longer enters `LayoutSnapshot.blocks`, the reading order or the plain-text
  export; each excluded relation reports Error `relation.owner-not-placeable`.
  Regressions, for orphan and for bound stories, notes and floats:
  `regions::tests::{an_unbound_stories_note_or_float_never_enters_body_layout,
  a_bound_stories_note_or_float_never_enters_body_layout,
  story_relations_do_not_consume_the_body_relation_limit}` and
  `clipboard/tests/story_isolation.rs` (reading order and plain text). The
  reviewer's story probe was the starting point; the limit test and the export
  test fail without the fix. S6's own unsupported-content handling is separate.
- **Blocker 2, balancing in band coordinates (`b4d530f`, 24, 26):** the band has
  a block axis and a start edge from the page owner. Its full depth is the
  farthest frame end from that edge; a trial depth clips each frame past where
  it starts; `fit` tests each line's physical reach along the axis. A main frame
  rotated, scaled or flipped against the axis cannot be measured: the band is
  not balanced, keeps its full depth, and the next section starts on a new page
  with Warning `section.continuous-new-page`. Regressions:
  `sections::tests::bands::{staggered_spatial_frames_balance_in_band_coordinates_and_never_overlap_the_next_band,
  spatial_frames_off_the_block_axis_are_not_balanced}`. The first uses the
  reviewer's two staggered 100x100 pt frames and asserts pairwise
  non-intersecting physical line rectangles for 1, 4 and 10 lines; both fail on
  the old code. (With the fix, four 12 pt lines balance to 48 pt in the first
  frame, not 2 + 2 at 24 pt, because the second frame starts 50 pt down.)
- **S6, running regions (09, 20, 24, 26, 28, 37, 38):**
  - `running.rs`: the ordered slot registry (header, footer, background), each
    with a box, reservation side, paint order, hit priority and artifact type;
    bindings resolved in one forward scan (Inherit, None, Story, Missing); the
    variant rule (`first`, then `even`, then `default`, with explicit None
    stopping fallback); the integer reservation budget
    `top = min(max(T,h), H−B−keep)`, `bottom = min(max(B,f), H−top−keep)`.
  - `running/compose.rs`: a story is composed by the ordinary flow on a scratch
    snapshot holding only the running frame, with the one-frame source, so it
    stops at the frame end and never makes a page. At most 4,096 blocks and
    1 MiB of text are read. The result is memoised by the story's block inputs,
    the box, and the field values it read.
  - `running/attach.rs`: the page factory composes the page's regions, cuts the
    owner's main frames to make room, adds the region frames (role
    `Flow("reprise.running.<slot>")`, address `FrameAddr::Running`) and the
    `RunningInstance`s. A filler shows regions only when the requesting section
    says `filler.regions = running`.
  - Display paints a background below the body and headers and footers above it,
    each clipped to its frame. Region text is a PDF artifact run.
  - `reprise-edit`: `Navigator::{hit_placed, caret_in, selection_rects_in}`,
    `PlacedHit`, `RegionInstance`, `HitMode`. A relayout falls back to the
    nearest page that still shows the story in that slot, lower on ties.
  - Diagnostics `layout.running-missing`, `layout.running-clipped`,
    `layout.story-unsupported`, `section.region-writing-mode`.
- **S7, incremental machinery (11, 27):** `reads.rs`. A flow transition records
  the plan entries it read, each with its frame's address and provenance, and a
  memo hit needs every read unchanged. This replaces `FlowKey`'s clone of the
  whole `Plan`. The regions of pages a transition made are one read whose value
  is what they came to (reservation, frame boxes, which story is where), so
  editing a header's text without changing its height reuses every body
  transition, and the instances are made again from the running memo when a
  transition is replayed. Running-memo counters are in `WorkCounters`.
  `partial()` never shows a page without its three instances.
- **S8a, field adapters (10):** `FieldEnv` gains `labels` and `placed`. A placed
  composition (body flow, running instance, table cell in either) renders
  `page-number` from its page's label directly; an annotation composed before its
  page is known keeps reading the previous pass, which `resolve` corrects. Table
  cells already inherited the instance from the caller (inline `52f7b4c`), so a
  story bound to two slots resolves separately in each. Running blocks are in
  both the assumed and the resolved enumeration. A `page-of` whose target cannot
  be found (in a story, in an orphan or deleted story, or formerly resolved and
  now unplaced) shows `?`.
- **S8b, labels (10, 24):** a page's label is its counter in the owner's
  `numbering.format` through `NumberFormat::render_or_decimal`. A counter the
  format cannot show falls back to decimal with one Warning
  `section.page-number-range` per section. `page-of` renders the target's label.
  The flow memo key gained the section's format; the test that proves it fails
  without it.
- **Merge of main 0d8d296 and preflight (`d65e5f5`, `b9e14ca`):** bit 18 (style
  identities) and sync bits 3–5 are both known and supported. Every section,
  break, story and region command is in `Plan::needs_trial` (`Plan::sections`),
  and `validation_fork` is now the preflight fork, so a refused batch publishes
  no pending raw write. The test `edit/tests/section_preflight.rs` runs all
  seven commands, plus a story copy, alone and after an ordinary edit, with a
  native caller's pending raw write; it fails without the flag and, for the
  copy, without the `validation_fork` change. That second case was a real
  hazard in this workstream's own planning: a plain fork commits what is
  pending. Bit-17 presence checks (`has_sections`, `geometry_mode_record`) use
  `get_by_str_path` lookups; nothing in this workstream calls `get_value()` or
  `get_deep_value`.
- **Frames contract request (non-exhaustive enums):** `PlacedHit` and `PageKind`
  are now `#[non_exhaustive]`, so the frames design can add `PlacedHit::Chain`
  and `PageKind::CanvasTile` without breaking a match. The frames note
  names `PlacedHit` (§10, §16.3), `PageKind` (§16.5), `FieldInstance` (§16.2)
  and new `Reason` variants (§16.10); the last two already were
  non-exhaustive, and so is `Command`. I marked no other enum, because the note
  extends none, and built nothing else for frames.

## 2. Commits

First-parent, oldest first (`git log --first-parent --oneline 73c8142..HEAD`):

```text
7fb8d70 merge: engine main (4131ec8, lists and inline) into ws/sections2
e3246be wip(layout): running regions - registry, composition, page attachment, display and navigation
a6a457f wip(layout): checkpoint S6/S7 running regions, read-set patches and S7a test draft
7d7e847 style(layout): cargo fmt the S6/S7 checkpoint
b852f9c fix(layout): scope body region allocation to the body domain
b4d530f fix(layout): balance bands in band coordinates, not frame-local depths
c28fe3b test(layout): S7a acceptance for running regions under incremental layout
bf25ebe feat(layout): S8a field adapters for running regions and page labels
341ef60 feat(layout): S8b page labels in the owner's number format
d65e5f5 merge: engine main (0d8d296, review fixes 1 and docs sync) into ws/sections2
b9e14ca fix(edit): section, break, story and region commands join the isolated preflight
71adaba fix: clippy findings after the merge; explicit domains in main's new code
```

The two `wip` commits are checkpoints taken when a usage limit cut the session
off; `a6a457f` was committed with its S7a test draft not yet wired in, and
`c28fe3b` wired it. S6 and S7 were not committed as separate finished slices:
`e3246be`/`a6a457f` hold S6 and the S7 read-set work, `c28fe3b` the S7a tests.

## 3. Files touched

New: `layout/src/{running.rs, reads.rs}`, `layout/src/running/{attach, compose,
tests, layout_tests, accept_tests, combo_tests, incremental_tests,
field_tests}.rs`, `edit/src/region.rs`, `edit/tests/{regions, section_preflight}.rs`,
`clipboard/tests/story_isolation.rs`.

Changed in layout (the workstream's own area): `flow.rs`, `flow/{bands,
paragraph}.rs`, `pages.rs`, `pages/bands.rs`, `incremental.rs`, `inline.rs`,
`regions.rs`, `regions/{physical.rs, tests.rs}`, `floats.rs`, `notes.rs`,
`display.rs`, `snapshot.rs`, `codes.rs`, `sections/mod.rs`,
`sections/tests/bands.rs`, `pages/tests.rs`, `inline_tests.rs`, `lib.rs`.

**Outside the allowed list, each small:**

- `layout/src/inline.rs`: `FieldInstance::Running`, `FieldEnv.{labels, placed,
  trace}`, the `?` failure text for `page-of`. The brief asks for these, in
  inline's file.
- `edit/src/{lib.rs, plan.rs, editor.rs, error.rs}`: the preflight for the section
  commands (`Plan::sections`), and `PlacedHit` is non-exhaustive.
- `doc/src/domain.rs`: `validation_fork` is the preflight fork; `edit/src/paste.rs`:
  the whole-document emptiness check asks for no authored block in any domain.
- `fixtures/{src,tests}/hostile.rs`: the dedicated `sections_regions_hostile`
  fixture; count 109 with main's.
- `reprise/tests/collab.rs`: explicit `Domain::Body` where the
  clippy guard flagged main's new code (`reprise/tests/collab.rs`) and
  `edit/src/paste.rs`'s whole-document emptiness check.
- `format/src/features.rs`, `format/SPEC.md`, `doc/src/sync.rs`: bit 17 next to
  main's bit 18 and sync bits 3-5, in the merge commit; SPEC.md's registry row
  for bit 17 now says it is assigned, with a section describing it.
- `tools/check-sections.sh`: builds into F: (G: was saturated by other builds).
- Docs: `docs/CODEMAP.md` (rows), `docs/contracts.md` (codes table row and the
  two non-exhaustive notes), `format/SPEC.md` (bit 17 is assigned). The design
  note `docs/sections.md` is **not** edited; its deviations are in §4.

No `PROGRESS.md`, no other worktree, no dependency and no CI file was touched.

## 4. Design choices and deviations from `docs/sections.md`

1. **Scheduling units** (§11): see §0.
2. **Reservation of non-orthogonal frames** (§9). A main frame is cut only when
   its block axis is the page's vertical (horizontal text) or its inline axis is
   (vertical text). A rotated, mirrored or shaped spatial main frame keeps its
   authored geometry and the region paints over it, without a diagnostic.
3. **Hit area** (§12). A header answers hits over its frame widened up to the
   body's top edge, and a footer from the body's bottom edge, so a click
   anywhere in the margin edits the region. Empty stories cannot take a caret
   and are skipped.
4. **Signatures** (contract wording). `Navigator::hit_placed(page, point, mode)`
   carries the page and the `HitMode`; the contract text writes `hit_placed(point)`.
   `RegionInstance.slot` is the slot's stable token (a string); `SlotId` is the
   in-process index inside `FieldInstance::Running` and serialises as the token.
5. **Derived fields on contract structs.** `PageDescriptor` gains `reserve` and
   `first_owned` and `SectionsLayout` gains `notes`. All three are
   `#[serde(skip)]`, `pub(crate)`, and only constructible in layout.
6. **Memo key.** The whole plan is out of the key; the page-owned physical planes
   (floats and notes across continuous bands) are still kept whole, when there
   are any, because they are projected into frames that may not exist yet.
   The section's number format and its page-specific template are in the key.
7. **Diagnostics from composition** are collected per page and added to
   `diagnostics` once each when flow ends, so replay cannot duplicate them.
8. **Balancing** (§8) declines to balance a band whose main frames are rotated or
   flipped against the owner's axis, instead of approximating (blocker 2).
9. **Body isolation** reports a story-owned relation with the existing code
   `relation.owner-not-placeable` rather than a new code.

## 5. New public API and diagnostic codes

`reprise-layout`: `SlotId` (`token`, `parse`, `hit_priority`, `reserves_top`,
`reserves_bottom`, `paints_below`, `artifact`), `Artifact`,
`FieldInstance::Running { page, slot, repeat }`, `WorkCounters.{running_compositions,
running_reused}`, `codes::{RUNNING_MISSING, RUNNING_CLIPPED, STORY_UNSUPPORTED}`.
`reprise-edit`: `Navigator::{hit_placed, caret_in, selection_rects_in, shows}`,
`PlacedHit` (non-exhaustive), `RegionInstance`, `HitMode`. `PageKind` in
`reprise-layout` is non-exhaustive.

| Code | Severity | Why |
| --- | --- | --- |
| `layout.running-missing` | Warning | The binding names no live story; the region is empty, which differs from what was asked. |
| `layout.running-clipped` | Error | Content of a region did not fit its frame, or was past the read caps, and was left out. |
| `layout.story-unsupported` | Error | A float, note or relation-placed block in a story is not laid out; content was left out. |
| `section.region-writing-mode` | Warning | A region's vertical writing mode is rendered horizontal, which changes what was asked. |
| `section.page-number-range` | Warning | A counter the section's format cannot show is rendered decimal. |

All five are in the contracts codes table. `FieldInstance`, `Reason` and
`Command` were already non-exhaustive.

## 6. Snapshot changes

No existing snapshot changed. `git diff 0d8d296 HEAD --name-status -- '*.snap'`
shows eight additions and nothing else: six from part 1 (geometry, retained and
bands, each with its content-only twin; reviewed in the B1 report) and the two
new in this part, hand-reviewed against computed values (the frame depths below
were read back out of the snapshot's JSON):

- `hostile__sections_regions_hostile.snap`: five 300x300 pt pages, 36 pt
  margins, a 2 pt header distance, a 13.2 pt line. Page 1 takes the `first`
  header, a deliberately tall text. Its cap is 300-36-57-2 = 205 pt, so 15 whole
  lines (198 pt) fit and the rest is clipped; the body starts at 2 + 198 = 200 pt
  and keeps exactly its quarter, 57 pt. The footer has 7 pt left, too little for a
  line, so it is clipped too. Page 2 takes the empty `even` header (zero depth,
  reserves nothing) and the one-line footer (body 214.8 pt = 300-36-49.2). Page 3
  takes the default header: three lines and two 8 pt gaps, 55.6 pt, so the body is
  193.2 pt. Page 4 starts a section whose header names a deleted story (empty,
  Warning `layout.running-missing`) and whose footer is an explicit None (body
  228 pt). Page 5 binds a story holding a note: Error `layout.story-unsupported`,
  its paragraph is shown (13.2 pt header, body 214.8 pt). The background shows on
  every page. Diagnostics, all checked against the fixture's expectations: two
  `layout.running-clipped` (page 1's header and footer), one `layout.running-missing`,
  one `layout.story-unsupported`, and `section.property` (a negative
  `header.distance`), `section.region-writing-mode` and `section.region-slot`.
- `hostile__content_sections_regions_hostile.snap`: the same pages' drawing,
  with the background group first, the body next and the header and footer last,
  the region groups clipped to their frames.

## 7. New dependencies

None.

## 8. Contract proposals

None that change a frozen contract. Two clarifications for the contract text in
`docs/contracts.md` "Section additions": the `hit_placed` signature carries the
page and the mode (§4.4), and `PageDescriptor`/`SectionsLayout` have derived,
unserialised fields (§4.5).

## 9. Known gaps and follow-ups (hand-off for PROGRESS.md)

- **S7 scheduling units and first-page composition** (§0): the main open item.
- **Root-presence hazard, not mine to fix here.** `Document::check_edit_preflight`
  (main's, reached by `fork_for_edit` and now by `validation_fork`) calls
  `get_value()` to find root trees, the pattern the orchestrator's Astra review 4
  note calls unsafe on a 1,024-deep tree. This workstream added no such call.
  When `ws/review-fixes-2` lands its shared helpers, `validation_fork` and the
  section commands pick them up through `fork_for_edit` with no further change.
- S9 (clipboard), S10 (exports, including artifact subtypes from
  `SlotId::artifact`), S11 (facade and WASM queries `page_sections`,
  `section_pages`, `page_label`, `page_regions`, `region_instances`,
  `page_fields`) and S12 (pageless) are not started, as agreed.
- The `sections_regions_hostile` fixture is dedicated, not in `all()`, until S9
  carries sections through the shared roundtrip (same approved deviation as
  part 1).
- Running text in a vertical section is horizontal; per-band note areas and a
  chain placement are still future work (§24).
- A spatial template's non-orthogonal main frames are not cut for the header and
  footer (§4.2).

### What is left for B3

1. S7 scheduling units, the look-ahead that makes page creation hit the running
   memo only (§0), including the job's first page. Decide whether to build it or
   to narrow `docs/sections.md` §11.
2. S9 clipboard (envelope, paste, `import_copy`; the shared story table), then
   moving the dedicated sections fixtures into `all()`.
3. S10 exports: PDF artifact subtypes from `SlotId::artifact`, per-page
   MediaBox and labels, HTML and plain text.
4. S11 facade and WASM: the commands and queries listed in the contract,
   `State.sections`/`stories` already exist; bindings for `hit_placed`,
   `caret_in` and `selection_rects_in`; then regenerate `types.d.ts`,
   `reprise_wasm.d.ts` and `API.txt` (B2's own changes altered none of them).
5. S12 pageless, gated.
6. Adopt `ws/review-fixes-2`'s shared preflight and root-presence helpers.

### How later features plug in

- **Header and footer editing, a page-setup dialog.** The app holds a
  `RegionInstance { page, slot }` as view state, asks `shows` after each
  relayout, and uses `hit_placed`, `caret_in` and `selection_rects_in`; text
  edits are the ordinary commands on the story's blocks, and bindings change
  through `SetRegion`/`CreateRegion`, all in the isolated preflight.
- **A table of contents.** Entries are `page-of` fields, which now render the
  target page's label, so roman front matter and a decimal body are right and a
  target the layout does not place shows `?`.
- **A further running slot** (a margin note, a line-number gutter) is one more
  entry in `SLOTS` with a box, side, paint order, hit priority and artifact.
- **Frames and chains.** `FieldInstance` is non-exhaustive for a chain
  instance; `PlacedHit` and `PageKind` are non-exhaustive for a chain hit and a
  canvas tile; a chain's frames reuse the one-frame story adapter and the
  read-set memo.

### App seam

Available now through the layout and edit crates: running instances in
`LayoutSnapshot.sections.running`, `Navigator::hit_placed` (with `HitMode`),
`caret_in`, `selection_rects_in` and `shows`, and the existing story, section and
region commands inside `batch`. The facade queries are S11.

## 10. Verification

All of this ran at head `71adaba` with a clean tree, in Git Bash with TMP/TEMP on F:,
the compiler wrapper off, `CARGO_PROFILE_{DEV,TEST}_DEBUG=0`, and builds in
`F:/reprise-target/eng-sections2-b2` (G: was writing at about 2.7 MB/s because
of other agents' builds; this is the only departure from the brief's "build into
your G: target"). The gate is `tools/check-sections.sh` (a copy of
`G:/tmp/gate-integrate.sh` pointed at this worktree, with the test step under
`timeout --kill-after=10s 3600`). Log: `F:/reprise-target/b2-logs/final-gate.log`.

**Result: every step exited 0 and the log ends `=== GATE DONE`, `exit 0`.**
1,473 tests passed, 0 failed, 1 ignored (the existing `export_geometry_previews`,
which writes inspection files) across 146 suites. Steps, in order: types
generation (`types.d.ts` and `API.txt` unchanged by B2's own work), debug WASM
build, web and Node `wasm-bindgen`, `tsc` strict and CommonJS worker build,
`smoke.cjs` and `worker-smoke.cjs`, six native/WASM package parity cases
("sections native/WASM parity: geometry, continuous and vertical bands, notes
and floats passed"), `cargo fmt --check`, workspace/all-targets clippy
`-D warnings`, the prescribed wasm32 check, and `cargo test --workspace
--no-fail-fast`.

Suites that matter to this part (from the same log):

| Suite | Result |
| --- | --- |
| `reprise-layout` unit tests | 163 passed (running regions, fields, labels, bands, regions, S7 incremental) |
| `hostile.rs` (fixtures) | 123 passed; every existing snapshot unchanged (`INSTA_UPDATE=no`) |
| `edit/tests/section_preflight.rs` | 1 passed (new) |
| `edit/tests/{sections, regions}.rs` | 13 and 5 passed |
| `edit/tests/refusal_atomicity.rs` (main's) | 10 passed |
| `clipboard/tests/story_isolation.rs` | 1 passed (new) |
| `fixtures/tests/incremental.rs`, `incremental_fuzz.rs`, `incremental_steps.rs` | 15, 1 and 5 passed |

### The four commands, verbatim tails

```text
=== fmt
=== rc=0
=== clippy
    Checking reprise v0.1.0 (F:\reprise-wt\eng-sections2\crates\reprise)
    Finished `dev` profile [unoptimized] target(s) in 4.02s
=== rc=0
=== wasmcheck
    Checking reprise-wasm v0.1.0 (F:\reprise-wt\eng-sections2\crates\reprise-wasm)
    Finished `dev` profile [unoptimized] target(s) in 2.79s
=== rc=0
=== test
   Doc-tests reprise_wasm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

=== rc=0
=== GATE DONE
exit 0
```

### Regressions proved by mutation

Each new regression was run against the code it guards and failed there: the body
limit test and the clipboard test without the domain filter; both band tests on
the old `bands.rs`; the placed/unplaced and label tests when `resolve` ignored
labels, when `page-of` kept its old text, when running blocks left the field
enumeration, and when the section's format left the memo key; and
`section_preflight` without `Plan::sections` and, for the story copy, without
the `validation_fork` change.

### Incremental timings against main

The brief's reference times on idle main, debug: `incremental.rs` about 235 s,
`incremental_fuzz.rs` about 52 s, `incremental_steps` about 40 s.

Other agents were building on this machine during every run here (CPU 46-100%,
and the G: disk queue near 15 earlier on), so wall times are not comparable with
the idle reference: the same unchanged layout unit tests took 57 s alone and
322 s inside the gate. So main `0d8d296` was exported with `git archive`, built
in its own target, and its three test binaries were run back-to-back with this
head's, same machine, same minutes (`F:/reprise-target/b2-logs/ab.txt`; all six
runs passed):

| Suite | main 0d8d296 wall / CPU | sections 71adaba wall / CPU | Difference |
| --- | --- | --- | --- |
| `incremental` (15 tests) | 439.8 s / 510.9 s | 462.7 s / 563.1 s | +5% wall, +10% CPU |
| `incremental_fuzz` (1) | 39.5 s / 37.6 s | 46.1 s / 44.5 s | +17% wall, +18% CPU |
| `incremental_steps` (5) | 79.6 s / 177.2 s | 87.1 s / 199.2 s | +9% wall, +12% CPU |

Under this load main itself takes 440 s for `incremental`, not 235 s, so the
idle references cannot be used directly. Against main in the same conditions
this head is 5-17% slower in these three suites and nowhere near several times
slower. The likely cost is the read recording on every memoised transition
(frame address, provenance and field values per read), which replaces cloning
the whole plan into the key, for every document with feedback; the regions
outcome check adds to it only for a document with running regions.
Earlier isolated runs of this tree at lower load (before the second merge)
gave 218 s, 25 s and 50 s wall. I did not bisect the 5-17%.
