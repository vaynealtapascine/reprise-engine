# Sections, Stage B part 3: report

Branch `ws/sections2`, worktree `F:/reprise-wt/eng-sections2`. Part 3 started
from `93f8ef8` (S1–S8b, main `0d8d296` merged). It covers S7c (scheduling
units), S9 (clipboard), S10 (exports) and S11 (facade and WASM). **S12
(pageless) is not started**, as agreed.

This part was done by two agents. The first built S7c (`19d9acb`), started S9
(`e27720e`), merged main `88ede78` (`b34230b`, which carries review-fixes 2 and
the contract commit) and fixed the six findings of the Astra review of part 2.
A thread that grew too large was handed over; the second agent finished the
scheduling tests, S9, S10 and S11 from the committed state. Everything below is
on the branch.

## 0. Status

| Slice | State |
| --- | --- |
| S7c scheduling units | Done (`19d9acb`), hostile tests of the synchronous escape paths added (`bcaf038`). |
| Astra review of part 2, six findings | All fixed (table below). |
| S9 clipboard | Done: envelope, paste, `import_copy`, native resources, the dedicated fixtures in `all()`. |
| S10 exports | Done: PDF (MediaBox, labels, artifact subtypes), HTML (sections, stories, round trip), plain text. |
| S11 facade and WASM | Done: eight commands, six page queries, three placed-navigation calls, worker kinds, types and API pin regenerated. |
| S12 pageless | Not started (gated; the orchestrator decides after review). |
| Main `0c05a19` | Merged (`81a752b`); the S7 keys compose with `PassKey::resolved` (§11). |

Orchestrator decisions of the brief: (1) the scheduling units are built, not
narrowed; (2) the story table holds **16,384** distinct stories, counted across
the whole envelope, and a count at 16,384 is accepted while 16,385 is refused
before any write with the document and both undo stacks unchanged (tests below);
(3) S12 is out of this part.

### The six findings of the part 2 review

| # | Finding | Fixed in | Regression tests (`crates/layout/tests/`) |
| --- | --- | --- | --- |
| 1 | Footnotes overlapped the running footer | `859c42c` | `review_b2_notes.rs`: Astra's probe, both edges, first/even variants, continuation pages |
| 2 | A running table's edit reused a stale composition | `d248557` | `review_b2_tables.rs`: cell text and style, row insert/delete, table geometry, header flag, each in one persistent session |
| 3 | A continuous section's pending restart was not in the flow key | `59a292e` | `review_b2_flow.rs`: the probe, four properties of the section edited in the same way, the column rules |
| 4 | A story image's relation ownership was not in the memo key | `18672c3` | `review_b2_relations.rs`: both directions, each in one persistent session |
| 5 | Tables bypassed the story read budgets; the top-level list was enumerated whole | `be58669` | `review_b2_budgets.rs` (the 1,048,577-byte cell, a table of 4,097 cell paragraphs, at most 4,096 + 128 IDs looked at) and `doc/tests/child_reader.rs` |
| 6 | An unchanged digest was compared on every page | `a2ea452` | `review_b2_digest.rs`: a 2,000-block header on 101 pages, digest comparisons counted per job |

## 1. Summary

**S7c, scheduling units (§11).** A page's regions are made by a look-ahead
state machine (`flow/ahead.rs`) before the unit that makes the page: the story's
digest 64 blocks a unit, then the composition one flow unit at a time, then the
memo entry. `LayoutJob::new` composes nothing; the first page waits for its
regions like any other. The synchronous path runs the same machine to the end.
A memoised transition waits for the regions of the pages it made (more than five
pages: redone instead). `StepWork` gains `pages_made`, `region_units`,
`region_blocks` and `digest_blocks`; `WorkCounters::running_inline` counts the
compositions the flow had to make itself and is zero in every ordinary test.

*New in this agent's part: the hostile tests of the escape paths.* The Astra
review asked for the paths where a stepped job still composes inline to be made
to happen and bounded by real counters. Four tests
(`running/stepped_tests.rs`, `bcaf038`), each with its mutation checked:

| Test | What it makes happen | Real counters asserted | Without the guard |
| --- | --- | --- | --- |
| more than 64 distinct missing regions in region feedback | a 700-line note: 101 pages, each with a numbered header, so feedback needs a header per continuation page | output equals the unbounded layout; `running_inline` is 100 (≤ pages); every step keeps the budget-1 bounds except the allocation unit; at least `MAX_REGION_ROUNDS` look-ahead units | with the 64-round cap removed the job does not finish (killed at 420 s) |
| compositions evicted from the memo | 88 pages, an edit at the top, the 64-entry memo has lost the early headers | `running_inline` 0; compositions ≥ pages − 64 and ≤ 2·pages; all in look-ahead units; budget-1 bounds hold | n/a (natural eviction) |
| a memo that loses what the look-ahead just made | the running memo is emptied after every step (a `cfg(test)` hook), so `Cursor::look` finds the need it just met still missing | the job finishes with the same snapshot; of 22 pages made, every step that composed inline is one that made a page (44 compositions, two regions a page); every other step keeps the budget-1 bounds | with the stall check in `Cursor::look` disabled: "did not finish in 1,315 steps" |
| a memoised transition whose regions keep vanishing | the same, in a warm session after an edit | finishes; the transitions are redone (19 compositions against 1 with a memo that keeps what it is given) | with the stall check in `bands.rs` disabled: does not finish |

Two things these showed, worth knowing:

-   The flow confirms its fields with a second pass. For a document of more than
    64 pages with a per-page header field (a page number), the memo (64
    compositions a story, FIFO) is flooded by the first pass, so the second finds
    nothing: **a header is composed twice a page**, in look-ahead units, not
    inline. The numbers: 88 pages, 176 compositions. A size-aware cap would
    remove it; I did not change the cap.
-   Past `MAX_REGION_ROUNDS` the region allocation composes the remaining
    regions itself, in the allocation unit. That unit was already one unit of its
    own (§11), so this adds to an unbounded step rather than breaking a bounded
    one. In the 101-page case that is up to 100 compositions in one step. It is
    counted (`running_inline`) and the output is the unbounded one.

**S9, clipboard (§14).** `Fragment.sections` is a version-1 JSON envelope,
absent when there is nothing to carry. A copy of a selection carries the
boundaries strictly inside it (a boundary at the selection's start stays behind,
as in deletion) with their raw current-lifetime keys and their bindings written
as story labels, and the first copied section's inherited bindings materialised,
so a paste never picks up the destination's headers. A copy of the whole body
(copy-all, `import_copy`, or a selection that covers it) is `complete`: it also
carries the leading boundary, inactive records, `first`'s own keys and the
geometry-mode record; `import_copy` adds the orphan stories. Lifetimes never
travel: paste mints a fresh one for every record it writes. Paste reads and
validates the envelope in preflight and flattens the stories it copies into the
fragment, so the one machinery that stages blocks, ranges, annotations, lists,
formatting, images and inline objects serves a story as it serves the body.
Within the document a copy came from, bindings refer to the same live stories;
anywhere else each source story becomes one new story however many references it
has. `first`'s keys and the geometry mode are written into an empty destination
only (else Warning `clipboard.section-context`); a paste into a story drops
every boundary and binding with the same warning. Limits are
`Reason::CopyLimit(name)` (code `clipboard.limit`): 16,384 stories, 16,384
boundaries, 64 keys of 2 KiB, and the fragment's own block, range and size limits
across body and stories together; a copy is refused with `FragmentError::Limit`.

*Details the design left open* are in the §14 "As built" list of
`docs/sections.md`; the ones with a consequence: a story is referred to in place
only if the destination is the source document **and** the story is still live;
a story only a dropped `first` bound is not copied; a boundary on a block joined
into the caret paragraph has no place and is counted in Info
`clipboard.section-dropped`; a copy copies all stories in one pass (one at a time
was quadratic at 16,384: the first run of the 16,384-orphan test did not finish
in ten minutes, the fixed one takes seconds); a `page-of` in a story aimed at body
text of the copy follows the copy.

The native fragment bundles the image bytes and fonts the stories use and checks
its envelope when decoded. The dedicated fixtures `sections_geometry_hostile`,
`sections_bands_hostile` and `sections_regions_hostile` moved into
`hostile::all()` (111 → 114; their snapshots already existed and are unchanged),
and the shared clipboard round trip now compares each envelope's shape and the
sections layout, and checks that four fixtures really carry one.

**A defect the move found (`9d233d0`).** The shared incremental equivalence test
(`fixtures/tests/incremental.rs`) failed on `sections_geometry_hostile`, edit 3,
the first time the three fixtures ran in it: the session's final-pass cache kept
the finished snapshot *before* sorting its section diagnostics, so a job that hit
the cache (an edit nothing in the layout reads; here a style redefinition that
resolves to the same size) returned `section.*` diagnostics ahead of the body's
instead of after. The defect dates from S4 and no earlier test could see it,
because the dedicated tests compare a cold session with a full layout. The sort
now comes first (it is idempotent). The regression test
(`incremental_sections.rs`) asserts the cache really was hit and compares the
result and the diagnostic order with a full layout for four sections fixtures;
it fails without the change with the order above.

**S10, exports (§15).** *PDF:* each page already had its own `MediaBox` (now
pinned by a portrait, landscape, portrait test); a running region's glyph and
image runs are `/Artifact << /Type /Pagination /Subtype Header | Footer |
Watermark >>` through a new optional `Structure.artifact_kinds` beside the
unchanged run list, with the file still claiming PDF/UA-1 and the body tagged
exactly as without the regions (same roles, text and leaves); `/PageLabels` is
written, one entry a page, only when every page's label is exactly what `/D`,
`/r`, `/R`, `/a` or `/A` writes for its counter (compared with the PDF's own
scheme, in which a letter repeats), else none, with a Warning `export.sections`
that says which page and why. *HTML:* a document with sections is written
`<section data-reprise-section>` by section, with stories written once under
labels (`<header>`/`<footer>` for a default variant, else `<template>`), the
three binding states, `break-before` for page and column breaks, CSS columns for
viewers and `data-reprise-geometry` on `<body>`; the importer reads all of it back
into the import document, so copying it builds the envelope. A document without
sections is exported byte-for-byte as before. *Plain text* stays the body only and
reports what it leaves out. All losses are `export.sections` notes.

**S11, facade and WASM (§17).** Commands `insert-break`, `remove-break`,
`set-section`, `create-story`, `set-region`, `create-region`, `link-region` and
`delete-story`; queries `page_sections`, `section_pages`, `page_label`,
`page_regions`, `region_instances` and `page_fields`; navigation `hit_placed`,
`caret_in` and `selection_rects_in`; the same names in WASM with typed payloads;
nine request kinds in the reference worker; `types.d.ts`, `reprise_wasm.d.ts` and
`API.txt` regenerated by their generators, not edited. `docs/bindings.md` has a
section for it.

## 2. Commits (`git log --oneline 93f8ef8..HEAD`)

```
9d233d0 fix(layout): the final-pass cache keeps section diagnostics in their sorted place
200d3e9 feat(reprise): S11 facade and WASM - section commands, page queries, placed navigation
c617492 feat(clipboard): S10 exports - PDF page labels and artifact subtypes, HTML sections, plain text
db906a9 feat(clipboard): S9 import_copy, native resources and the design note's as-built details
38d5832 feat(clipboard): S9 sections envelope - copy and paste of boundaries, bindings and stories
bcaf038 test(layout): hostile tests of the look-ahead's synchronous escape paths
a2ea452 perf(layout): an unchanged running story's digest is compared once per job, not per page
be58669 fix(layout): a running story's tables are charged to its read budget, and reading it is bounded
18672c3 fix(layout): relation ownership is part of a running story's memo key
d248557 fix(layout): a running story's memo keys its tables' structure and every cell
59a292e fix(layout): a flow transition's key holds everything it reads of its section
8dc48e7 test(format): pin KNOWN_REQUIRED to the assigned bits after the merge of main 88ede78
859c42c fix(layout): footnotes are allocated inside the body left after the running reservation
b34230b merge: engine main (88ede78, review fixes 2 and contract commit) into ws/sections2
e27720e wip(clipboard): S9 start - envelope in the fragment, paste test draft, 16,384-story cap
19d9acb feat(layout): S7c scheduling units - region composition and the first page are stepped
88ede78 feat(contracts): review fixes 2 endpoints and style contexts; reserve frames bits
305fd5a docs: finalize review fixes verification and measured performance
3c01727 docs: checkpoint final-source clipping regression and corpus evidence
670b460 fix(clipboard): clip annotation pieces before subtracting byte offsets
bbaea45 docs: checkpoint complete file inventory and retained-marker follow-up
b5bb750 fix(format): retain new format declarations after marker deletion
b2f7a28 docs: record negative controls and versioned compatibility proposals
17a09aa style(loro): format the isolated tree replay patch
0625eca fix(styles): version explicit context migration and retain old encoded references
d748da3 test(fixtures): record hidden 1024 and 4096 depth goldens
ca9f190 fix(format): require endpoint-aware readers for portable cross-host ranges
5b3dc6f Merge engine main documentation before review fixes verification
3788e2a test(clipboard): keep internal range targets across copied paragraphs
25ec2ca docs: checkpoint review fixes evidence and remaining verification
4e3fda0 test(wasm): run deep metadata packages with a 1 MiB stack in CI
40c9ede fix(doc): restore copied range ends in their own text host
280183f test(styles): cover historical identity roots and undo across joins
d68f7cd fix(loro): preserve pending undo through unchanged tree replay
10b59e2 perf(styles): avoid property resolution during authoring and duplicate reference reads
d2b985a test(perf): retain repeatable review phase measurements
624b304 perf(doc): build cross-host annotation order once and count index work
c9fd96c test(doc): bound deep fixture construction and exercise history round trips
1322076 fix(loro): bound deep tree snapshot traversal and decoding
fcb3a97 test(edit): read the nested export content bytes in the matching-revision refusal test
25bf3d1 fix(styles): pin historical references before joining identity metadata
b87c4f4 test(edit): cover matching revision annotation refusal and retained redo
d3f5fb5 fix(edit): exhaustively select preflight and keep missing annotation reads pure
9d8ffa0 test(doc): exercise metadata on 1024 and 4096 deep trees
9478c4b fix(doc): bound root discovery independently of tree depth
d818990 fix(clipboard): retain cross-paragraph annotation endpoints
b52a88a fix(pdf): retain authored links around inline atoms
3f5e0a7 perf(doc): resolve annotation endpoints with ordered host intervals
573e29e fix(doc): retain annotation restore hints across hidden marker gaps
d8eb32e fix(edit): preflight annotation writes against transaction targets
```

## 3. Files touched

Everything is in the sections workstream's areas except the following, each a
small or additive edit:

-   `crates/display/src/pdf.rs`, `src/pdf/tags.rs`: artifact kinds, page labels
    (additive fields with defaults; the one literal `Structure` that listed every
    field now uses `..Default::default()`); `tests/pdf_tags.rs` follows.
-   `crates/display/tests/common/checker.rs`: the checker reads `MediaBox`,
    `/PageLabels` and artifact subtypes (test helper).
-   `crates/layout/src/display.rs`: `pdf_artifact_kinds`.
-   `crates/layout/src/incremental.rs`, `running/compose.rs`: `MAX_REGION_ROUNDS`
    and `MAX_ENTRIES` are `pub(crate)`; two `cfg(test)` hooks; and the one
    behaviour change outside the slices, the final-pass cache fix (`9d233d0`, §1).
-   `crates/doc/src/table.rs`: `table_block` is `pub(crate)` (story blocks reuse it).
-   `crates/doc/src/fragment.rs`: `Fragment.sections`, `copy_blocks` split out of
    `copy_fragment_scoped`, `copy_fragment_bare`.
-   `crates/clipboard/{native,export,html,inline}.rs`, tests.
-   `crates/fixtures/src/hostile.rs`, `tests/hostile.rs`: three fixtures into
    `all()` and the count (111 → 114), as the CODEMAP recipe says.
-   `crates/clipboard/tests/annotations.rs` and
    `crates/fixtures/tests/review_fixes2_performance.rs`: two tests merged from
    main called `Document::blocks()`, which the sections clippy guard forbids;
    they name the body domain now. (Main's `-D warnings` run did not see them
    because the guard is this workstream's.)
-   `docs/{sections,contracts,bindings,CODEMAP}.md`.

## 4. Design choices

-   **One envelope, stories flattened into the paste.** The alternative, a
    separate paste per story, would define styles and rename clashes once per
    story and could not remap a `page-of` that crosses from a story into the body.
    The cost is that `write` knows which blocks are story roots.
-   **Stories nothing refers to are not copied** into a destination that doesn't
    take `first`, so a paste never leaves orphans behind it; an empty destination
    takes every story of a complete envelope.
-   **`/PageLabels` or nothing.** A page whose label no PDF style writes (a
    counter below 1, `decimal-leading-zero`, a roman numeral past 3,999) makes the
    whole table be left out with a Warning, rather than a prefix-only label that
    would read the same but isn't a numbering. A prefix-only label for those pages
    is possible (a PDF label may be just a prefix) and would be the follow-up if
    exact labels for odd counters matter.
-   **`CopyLimit` once an envelope is present, even for aggregate limits**, because
    the stories' blocks count against the fragment's limits as pasted blocks. A
    fragment without an envelope keeps the errors it had.
-   **`Applied.stories`** is left out when empty, so every existing result is
    byte-identical.
-   **`break_kind`**, not `kind`: `kind` is the command's tag (as `block_kind` is
    for `insert-block`).
-   **Queries on a layout with no sections** answer as one section, `first`, with
    decimal numbers, rather than failing: an app needs the page label of a plain
    document too.

## 5. New public API and diagnostic codes

*Rust.* `reprise_doc::fragment_sections::{Envelope, Boundary, Story, Record,
Binding, EnvelopeMode, ...}`, `Fragment.sections`,
`Document::{copy_fragment_bare, write_pasted_starts, write_geometry_record,
append_story_block}`, `sections::PastedStart`; `reprise_edit::Reason::CopyLimit`,
`Pasted.stories`; `reprise_display::pdf::tags::{ArtifactKind, LabelStyle,
PageLabel}` and `Structure.{artifact_kinds, page_labels}`;
`LayoutSnapshot::pdf_artifact_kinds`. *Facade* (`API.txt`): `page_sections`,
`section_pages`, `page_label`, `page_regions`, `region_instances`, `page_fields`,
`hit_placed`, `caret_in`, `selection_rects_in`; the DTOs in `types.d.ts`.

*Codes.* New: `export.sections` (Info where the format cannot hold something;
Warning where the output differs from the document, for example page labels left
out or unequal columns written equal). Used from the approved table:
`clipboard.section-context` (Warning: the destination can't take it, the output
differs from the copy), `clipboard.section-dropped` (Info: inert records the copy
could not attach), `clipboard.limit` (Error: nothing was pasted).

## 6. Snapshot changes

None. No existing snapshot was changed, and no new snapshot was added: the three
fixtures moved into `all()` already had their `hostile__*` and
`hostile__content_*` goldens, and `debug_families_preserve_content_for_every_fixture`
checks the same `content_<name>` names. `INSTA_UPDATE=no` throughout.

## 7. New dependencies

None.

## 8. Contract proposals

None needed. Additions to record in `contracts.md`, all additive and made:
`export.sections` in the codes table; `Structure.page_labels` beside
`artifact_kinds`; `Pasted.stories` (already approved); the facade names above.

## 9. Known gaps and follow-ups

-   **S12 pageless** is not started.
-   **The 64-composition memo** is flooded by documents of more than 64 pages with
    a per-page field in a header: two compositions a page (§1). A cap that grows
    with the page count, or a per-job table, would remove it.
-   **Region feedback past 64 rounds** composes the rest in the allocation unit
    (§1).
-   **Body → story `page-of`**: a field in the body aimed at a story keeps its
    absolute target and shows `?` (§10 says it should).
-   **HTML:** stories bound to no section are not in the HTML; a table in a story
    is flattened on import; unequal column widths are CSS-equal (kept in the stored
    form). **DOCX** remains design-level.
-   **PDF labels** for odd counters (see §4).
-   Running text in a vertical section is horizontal; per-band note areas and a
    chain placement are future work (unchanged from part 2).

## 10. Incremental timings against main

The brief's reference times on idle main, debug: `incremental.rs` about 235 s,
`incremental_fuzz.rs` about 52 s, `incremental_steps` about 40 s. As in part 2,
other agents were building on this machine, so main `0d8d296` (its three test
binaries from `F:/reprise-target/main-0d8d296`) and this head (the same three
suites, rebuilt after the last code change) were run back-to-back, one after the
other, from the fixtures crate directory (`G:/tmp/ab3.ps1`, log
`G:/tmp/b3c-logs/ab3.txt`; all six runs passed):

| Suite | main 0d8d296 wall / CPU (load at start) | this head wall / CPU (load at start) | Difference |
| --- | --- | --- | --- |
| `incremental_steps` (5 tests) | 54.6 s / 128.3 s (4%) | 53.3 s / 122.9 s (45%) | −2% wall, −4% CPU |
| `incremental_fuzz` (1) | 28.4 s / 26.4 s (39%) | 29.8 s / 27.7 s (57%) | +5% wall, +5% CPU |
| `incremental` (15) | 299.0 s / 359.5 s (45%) | 223.7 s / 284.2 s (32%) | −25% wall, −21% CPU |

No suite is more than 20% slower than main, so nothing needed investigating. The
`incremental` difference runs the other way (the head is faster) and is within what
the differing machine load explains; the head runs three more fixtures than main
(114 against 111) and all of S7c to S11. For part 2's +5% to +17% (read recording
on every memoised transition) this part adds nothing measurable. Measured before
the merge of main `0c05a19`, which changes only the region-pass key and a paste
branch.

## 11. Merge of main 0c05a19

Main moved to `0c05a19` (it merged `hardening/fuzz-2`) after the report above
was first written. At the orchestrator's instruction it is merged, on its own
commit (`81a752b`), before the final gate.

*What main brought:* `PassKey::resolved` in the region-pass key (what the fields
of the flowed blocks resolve to, and the `page-of` targets that failed), a paste
fix (a full paste into a document with no blocks keeps list definitions), a
`flow.rs` visibility change, the fuzz foundations harness and its tests.

*Conflicts, both small:*

-   `crates/layout/src/incremental.rs`: main's `allocate(fields)` is this branch's
    `allocate(previous, collect)`. The key keeps `previous.fields` and takes main's
    `key.resolved = resolve_fields(doc, snapshot)` line.
-   `crates/format/SPEC.md`: main's sentence about the required bits an older
    reader doesn't know, without bit 17, which is `REQUIRED_SECTIONS` and known
    here; main's reserved-root table kept.
-   `crates/edit/src/paste.rs` auto-merges: main's `write_semantics` call in the
    empty-fragment path sits before this branch's story activation and envelope
    records. A complete envelope pasted into an empty document still activates
    its stories and writes `first` (the paste tests pass through that path).

*Does the S7 running cache compose with `PassKey::resolved`?* Yes, and the
region-pass key needs it for running fields. `resolve_fields` enumerates the
running instances' blocks as well as the body's (`field_blocks`, added with S8a),
so a running `page-of` is in `resolved`. The running memo's own check
(`still_valid`: label, guessed page, and every field it read against the plan)
and the flow's read-set keys (`PlanRead::Field`) read the plan that pass produces,
so they are right when the pass is. The test is
`running/incremental_tests.rs::a_running_page_of_is_resolved_again_when_its_target_or_the_target_moves`:
a header whose `page-of` names body text, in one persistent session, compared
with a full layout after each of: retarget to the second page, retarget to the
third, the target's page break removed (the target moves back a page), the break
restored, the target deleted (`?`), the field removed. **With the `resolved` line
removed the first retarget stays stale; with it, all pass.**

*Byte identity, rerun after the merge:* no golden file differs from main's
(`git diff --diff-filter=MD main HEAD -- '*.snap' '*.hex' ...` is empty; the only
snapshot additions are the eight sections goldens from parts 1 and 2), and the
full gate below runs every snapshot test with `INSTA_UPDATE` unset to record.
Native equals WASM (the parity step).

## 12. Verification

*After the merge of main (`81a752b`)* the full gate ran again on that commit: every step `rc=0`, 1,610 passed, 0 failed, 12 ignored (`G:/tmp/b3c-logs/gate-full-3.log`). The record of the earlier run follows.

The gate is `G:/tmp/gate-b3.sh` (a copy of `gate-integrate.sh` pointed at this
worktree, with the sections native/WASM parity steps of `tools/check-sections.sh`;
the test step under `timeout --kill-after=10s 3600`). Builds go to
`G:/reprise-target/eng-sections` and `-wasm`; `TMP`/`TEMP` are `G:/tmp`;
`CARGO_PROFILE_*_DEBUG=0`; `INSTA_UPDATE` unset in the gate and `no` in my own runs.
The full log of the final run is `G:/tmp/b3c-logs/gate-full-2.log`, run on the
last commit's code (the report is the only file changed after it). It ran twice:
the first run's workspace tests failed once (the final-pass cache defect, §1,
`incremental.rs` edit 3 of `sections_geometry_hostile`); that is fixed (`9d233d0`)
and this is the second run, every step `rc=0`:

```
=== types / wasmbuild / bindgen-web / bindgen-node / tsc / tsc-worker   rc=0 each
=== smoke        rc=0
WASM smoke: native JSON parity, editor loop, sync, defaults, images, formatting, emphasis/decoration/colour, host fonts, synthetic glyph outlines and ink bounds, authored marks, bytes, and hostile objects passed
paragraph style WASM smoke passed
plumbing WASM smoke passed
page setup WASM smoke passed
roles/lists WASM smoke passed
annotations WASM smoke passed
inline objects WASM smoke passed
sections WASM smoke passed
roles/lists worker smoke passed
Worker smoke: edit effects, created IDs, split selection, emphasis/decoration/colour, authored marks, host font configuration/job invalidation and image results passed
sections native/WASM parity: geometry, continuous and vertical bands, notes and floats, section queries and navigation passed
=== fmt         (cargo fmt --all -- --check)   rc=0, no output
=== clippy      (cargo clippy -q --workspace --all-targets -- -D warnings)   rc=0, no output
=== wasmcheck   (cargo check -q --target wasm32-unknown-unknown -p reprise-layout -p reprise-display
                 -p reprise-fixtures -p reprise-format -p reprise-edit -p reprise-plugin
                 -p reprise-clipboard -p reprise -p reprise-wasm)   rc=0, no output
=== test        (timeout 3600 cargo test --workspace --no-fail-fast)   rc=0
```

The tail of the test step, verbatim:

```

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests reprise_wasm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

=== rc=0
=== GATE DONE
gate exit 0
```

Totals over the whole run: **162 test suites, 1584 passed, 0 failed,
2 ignored.** No warnings in any step. Existing snapshots: none changed
(`INSTA_UPDATE` was never set to record; `git diff --name-status 93f8ef8 HEAD --
'*.snap'` lists only the four `metadata_depth_*` files that came with main's merge
`88ede78`); this part added none.

**New tests in this part**, by crate:

| Where | Tests |
| --- | --- |
| `layout` `running/stepped_tests.rs` | 4 hostile tests of the synchronous escape paths (§1), on top of S7c's 13 |
| `edit` `tests/paste_sections.rs` | 13: boundaries in a selection, fresh lifetimes and one undo, inherited bindings, same- and cross-document, the N7 cases, context and geometry into an empty document only, paste into a story, 14 malformed envelopes refused with nothing written and no section root, the 16,384 / 16,385 story limit, orphans and running-only stories counted, a header `page-of` following the copy, a story's formatting, list, role and inline objects |
| `doc` `sections/tests.rs` | 2: a record holding a container refuses the copy; stories over the block limit are refused |
| `clipboard` | `story_resources` (3), `pdf_sections` (7), `html_sections` (6), the strengthened `roundtrip` (envelope and sections-layout comparison over 114 fixtures), `pdf_labels` unit test |
| `reprise` | `sections_api` (6), `import_copy_sections` (2), `public_api` re-pinned |
| `fixtures` | `incremental_sections` (the cache defect), `hostile` count 114 |
| WASM | `smoke.cjs` and `worker-smoke.cjs` sections blocks; the parity example's new `regions` case (7 native cases, each compared with WASM: display, diagnostics, section state, every section query and a hit, caret and selection in a header) |

**Mutation checks** (logs in `G:/tmp/b3c-logs/`): disabling the stall check in
`Cursor::look` (`mut-look.log`) or in `bands.rs` (`mut-bands.log`) fails the two
loop tests ("did not finish in 1,087 / 1,315 steps"); removing the 64-round cap
hangs the 101-page test (`mut-rounds.log`, killed at 420 s); disabling the
story-font lookup in the native bundle fails `the_fonts_of_a_header_story_are_named_by_the_fragment`
("Source Sans 3 … is not bundled"); disabling the importer's section writing
fails three HTML tests (`html_sections`); the cache defect's test fails without
the fix (`inc-sections-red.log`).


## B3 fixes

The Astra B3 review (`F:/reprise-wt/reviews/sections-b3-review.md`, pinned at
`8bc86f4`) asked for changes: five blockers and two should-fix findings, with ten
failing probes. This section is the repair. Astra's five probe files were ported
unchanged (rustfmt only) as `crates/<crate>/tests/review_b3.rs`, and the ten failures
reproduced before anything was fixed (doc 1, edit 4, clipboard 2, layout 2, facade 1).
They are the regression gates; no assertion was weakened and none turned out to be
wrong. The fixes were made in the brief's order, one commit each (findings 3 and 4
share a validator and are two commits, see below), the affected crates' tests green at
each:

| Finding | Commit |
| --- | --- |
| 3. Story ownership in envelopes | `43c84dd` |
| 4. Aggregate limits of the whole envelope | `e253b51` |
| 5. Orphan stories in HTML | `b8606fd` |
| 6. Geometry record | `553b0a0` |
| 7. Navigation fallback | `917487f` |
| 1. Running composition in a budget-one step | `be6240e` |
| 2. Flow-host expansion | `2e2f2d7` |
| follow-up: body `page-of` aimed at a story, documented | `fbc5a4c` |

The text of §1 above ("Past `MAX_REGION_ROUNDS` the region allocation composes the
remaining regions itself") and the second gap of §9 are superseded by finding 1 below;
the read-budget part of the B2 findings by finding 2.

### 3. Story ownership in envelopes (blocker)

*What was wrong.* The envelope's validation flattened the body and every story into
one block list, and the flat validator accepts any earlier block as a parent, so a
story block could name a body paragraph, or another story's root, as its parent.
Paste attached the block to that parent before it looked at the story root mapping:
the content landed in the body and the created story was empty (probes: story sizes
`[2, 0]`). `NativeFragment::validate` shared the path.

*What changed.* `Envelope::validate_in(fragment)` (`doc/fragment_sections.rs`) checks
the records, then each component, the body and every story, as a tree of its own: a
parent is an earlier block of the same component, IDs are unique across all of them,
a range and the end of a spanning range sit in the component that carries them, and a
relation hangs on a body block (copies drop the ones a story owns). Then it runs the
flat validator for the aggregate limits. The copy (`attach_sections`), the native
validation (`NativeFragment::validate`) and the paste (`split`, which runs before it
stages or plans anything) all call it.

*Tests.* The review's cross-domain, cross-story and cyclic probes; `edit/tests/
b3_ownership.rs` (a story range on a body block, one ending in the body, a body range
ending in a story or on a story block, a relation owned by a story block, an ID shared
with the body: each refused in the kernel with nothing written and by `validate_in`,
and the same envelope with everything in its own component accepted);
`clipboard/tests/b3_fixes.rs` (the cross-domain and cross-story parents refused when
the native bytes are decoded). The new tests fail without the fix (checked by
stashing the source).

### 4. Aggregate limits count the whole envelope (blocker)

*What was wrong.* The aggregate fragment limits were checked only after paste had
kept the stories it would copy, so an over-limit story that a nonempty destination
skipped, a story destination discarded, or a same-document paste would reuse in place
was accepted (undo count 1, a block added). And the fragment's own body validation
ran first and mapped its limit to `Reason::Fragment(Limit("items"))`, not
`CopyLimit` / `clipboard.limit`, when an envelope was present.

*What changed.* `validate_in` (above) runs at the top of `paste_sections::split`,
before the story-destination discard and before any story gets a plan, so the body and
all the stories count against the fragment limits whatever the paste does with them.
`prepare_with_styles` maps its body validation's limit to a copy limit when an envelope
is present. The redundant second validation of the kept subset is gone. Every paste
path (`Editor::paste`, replace, paste commands, `import_copy`) goes through
`prepare_with_styles`. A fragment without an envelope keeps its old error.

*Tests.* The review's two probes; `edit/tests/b3_limits.rs`: an over-limit unused story,
and an over-limit body with an envelope, each pasted into a nonempty body, at a caret
and into a story; a story the paste would reuse in place (same document, live source).
Every case has undo and redo both populated and checks the history export bytes,
revision and both counts are unchanged. All three new tests fail without the change.

*Deviation.* Findings 3 and 4 share `validate_in`: the first commit has it and the
paste-side call, the second the error mapping and the limit tests.

### 5. Orphan stories in HTML (blocker)

*What was wrong.* The export planned a home only for stories a binding names; an
unbound live story got an Info loss note and no payload, and the import copied its
document with the clipboard scope, which omits unreferenced stories again.

*What changed.* The plan lists the orphans after the bound stories, in the document's
story order under the next labels (`s{n}`), and the export writes each once as an inert
`<template data-reprise-story>` after the last section, through the writer that wrote
the bound ones (`export.rs` `html_story`). The "bound to no section" loss note is
gone: nothing is left out. The import copies its document with
`Document::copy_imported_fragment` (new, `doc/fragment.rs`): the clipboard copy of
everything plus the stories no binding names, which stay unbound. A paste into an empty
document takes them; one into a document with content does not, as for a native paste
(§14, unchanged). `docs/sections.md` §15 is updated.

*Tests.* The review's two probes; `clipboard/tests/b3_orphans.rs` (orphans written once
with escaping, stable labels and order, an empty orphan, a document of nothing but
orphans, a second export equal to the first, and the nonempty destination);
`html_sections::losses_are_reported_not_silent`, which asserted the old behaviour (the
note, the missing text), now asserts the template and the round trip.

### 6. Geometry record (should-fix)

*What was wrong.* A complete copy kept the raw geometry record only if every value was
a string, boolean or integer; one container-valued key (the reader shows it as null)
dropped `v`, `mode` and the other keys with no report.

*What changed.* The record is carried exactly, every key as it is, or the copy is
refused with `FragmentError::Invalid`, as an unreadable section record is: for a
container, a null, a number that is not an integer, or a root that is not a record.
I chose refusal over carrying the readable keys and reporting the rest because a copy
that quietly differs from its source is what the review objects to, and the section
records already refuse. The same silent drop existed in the HTML import
(`data-reprise-geometry` kept the scalar keys and said nothing of the others); it now
reports `clipboard.html-dropped`.

*Tests.* The review's probe; `doc/tests/b3_geometry.rs` (plain keys of each scalar kind
travel byte for byte and are written back; a container, a float, a list and a root that
is no record each refuse both complete copies while a partial copy still works);
`b3_orphans::an_html_geometry_key_that_is_not_plain_is_reported_not_dropped_silently`.

### 7. Navigation fallback (should-fix)

*What was wrong.* `caret_in` and `selection_rects_in` checked the saved instance's page
against the new layout before the kernel's nearest-placement fallback could run, so
removing trailing pages gave `Invalid("no page 2")` with the header still on page 0.

*What changed.* `DocumentSession::instance` checks the page's representation and the
slot only; the kernel's fallback answers. `hit_placed` still checks its page against
the layout (`page_index`). A story no page shows any more still gives no geometry.
`docs/bindings.md` says so.

*Tests.* The review's probe (caret); `sections_api::a_saved_instance_on_a_page_since_
removed_still_navigates` (the same for the selection, a page index of `u32::MAX`, and
the slot, the position and `hit_placed` on the removed page still refused) and
`a_saved_instance_of_a_story_no_page_shows_any_more_has_no_geometry`.

### 1. Region feedback composes nothing inline (blocker)

*What was wrong.* Region feedback collected one missing composition a round and ran
the allocation again; after 64 rounds it stopped collecting and the allocation
composed the rest itself. The 100-page case (700-line note, numbered header) ran 99
whole running stories in one budget-one step (199 composer calls, 350 KB scanned). The
cap existed because removing it did not finish: the allocation needs a composition per
page at once, the running memo keeps 64 a story, and what the look-ahead made was
evicted before the allocation ran again.

*What changed.* The allocation collects every missing composition and digest in a pass
(deduplicated, in the order it asked). The job queues them (`region_needs`) and makes
each in look-ahead units of its own, at the same charge as any region, and holds what
it makes in a side store of the running memo with no cap (`Memo.held`, `Shared::hold`).
When a pass finds nothing missing, the held compositions go to the memo, oldest first,
so it still keeps the most recent 64 a story. Each round makes at least one thing not
made before, so the rounds end with no cap (`MAX_REGION_ROUNDS` is gone). One last
resort remains, counted in `running_inline`: a need that comes back after it was made
(the memo was cleared under the job) is composed by the allocation itself, once. The
queue, the made set and that flag travel with a suspended job. `docs/sections.md` §11
says it.

*Tests.* The review's probe (`running_inline` 0 for the whole job; it was 99 in one
step; the probe now runs in 1.5 s, not 15). In `running/stepped_tests.rs`:
`region_feedback_over_more_pages_than_the_memo_keeps_composes_nothing_inline` (100 pages
at budgets 1 and 7: a composition is finished only by a look-ahead unit and at most one
a step, more than 64 are held at once, none held at the end, output equal to the plain
layout); cancellation and staleness in the middle of feedback with compositions held,
each leaving the next job clean and nothing held; and the last resort
(`a_need_that_comes_back_...`). The old 101-page test, which accepted the escape, is
replaced. Mutations: holding in the memo instead fails the first; removing the
come-back check never finishes the last (a 100,000-step guard).

*What it does not do.* The allocation is still a unit that runs from the start in each
round; what is gone is any composition inside it. The rounds are the dependency levels
of the plan (pages made while the headers were absent, then with them, ...), typically
three or four, not one per page. A document in which each round's regions change the
page count by one could take many; that is bounded work in each unit, and the output is
unchanged. A resumable mid-pass allocation would be a larger change to `notes` and
`floats` placement; it was not needed to remove the synchronous escape. `Cursor::look`'s
own last resort (the memo evicting what the look-ahead has just made for a page;
counted, zero in every ordinary test) is unchanged: it is a page's two or three
regions, not the allocation's whole set.

### 2. Flow-host expansion (blocker)

*What was wrong.* The digest's reader gathered every physical child up front, and a
physical child can be a flow host that stands for any number of paragraphs: reading it
decoded the host's whole text and listed all its paragraphs, and the digest took the
queue of them uncharged. A one-character story split 130 times at offset zero is one
child and 131 paragraphs, and the first budget-one step read all 131 (limit 64).

*What changed.* `ChildReader` is now a cursor: the next physical child (found by index
through the loro tree handler, `LoroDoc::inner()`, so nothing is gathered and no
dependency was added), and a stack for the host being expanded that keeps its place
between calls and yields paragraphs and embedded nodes in exactly the order `expand`
and `top` list them. `read_children` takes `ReadLimits { units, blocks, host_bytes }`
and returns what it did. A child looked at is a unit and the first block it stands for
comes with it (so an ordinary paragraph is one unit, as before); each further paragraph
or embedded node is a unit; decoding a host costs a unit for each 4 KiB of its text.
A decode dearer than the units left in a call is deferred to a call of its own and
overspends; `DigestBuild` turns that overspend into a debt the following steps pay off
before they read on. So a step reads at most 64 units, and what is charged is what the
work comes to. The digest takes each block as it is read; the queue it kept behind a
table walk is gone, and a table's nodes are counted as they are read. `StepWork` gains
`digest_decoded`.

*Decision to review.* A flow host longer than a story could ever read whole (1 MiB of
text and 3 bytes of break marker for each of 4,096 blocks: 1,060,864 bytes) is not
decoded: the story is cut in front of it, as it is in front of a table that does not
fit. Before, such a host was decoded in full and the story cut inside it, keeping the
first paragraphs. The limit is a constant, so the digest does not depend on how the
work is divided into steps (stepped equals full). Without it a host's decode, which is
linear in its text and not splittable (every `Text` view of a paragraph reads the whole
host), would be unbounded; with it, one unit costs at most about 1 MiB of decoding,
charged and paid back. Splitting the decode itself would mean making `read_flow` and
the paragraph views incremental: a change to the CRDT layer's flow reading, beyond §11.
This is the residual limit of the fix.

*Tests.* The review's probe (`digest_blocks` 64, was 131). `layout/tests/b3_hosts.rs`: a
200-paragraph host read 64 a step with its one ID looked at; a host over the limit and
a cell block over it are not decoded; a 1,000,000-byte paragraph is a step of its own
and is paid back in idle steps. `doc/tests/child_reader.rs` (moved to the new API, with
the old assertions on IDs and prefixes): a host expanded a bounded piece a call; a block
limit stopping mid-host; the decode limit and deferral; an embedded host expanded in
place and read once; the reader against `top` over twelve seeds of random splits, joins,
deletions and embedded blocks, at three chunk sizes. Mutations: leaving a host's
paragraphs uncharged fails the probe; dropping the length limit fails the decode test;
dropping the debt fails the payback test.

### Changes to existing tests and snapshots

-   `running/stepped_tests.rs`: the 101-page escape test replaced (finding 1); the
    comment block above the hostile tests rewritten.
-   `clipboard/tests/html_sections.rs::losses_are_reported_not_silent` (finding 5).
-   `doc/tests/child_reader.rs`: moved to the new reader API (finding 2).
-   No snapshot changed or was added.

### New public API and dependencies

`reprise_doc::{ReadLimits, ReadStep, DECODE_UNIT_BYTES}` and a new `read_children`
signature (B2 API, used only by the digest); `Document::copy_imported_fragment`;
`Envelope::validate_in`; `StepWork::digest_decoded`. The facade, its types and
`API.txt` are unchanged. No new dependencies.

### Left tracked, as the review allows

-   The 64-entry composition memo (`running/compose.rs` `MAX_ENTRIES`): a per-page field
    in a document of more than 64 pages is composed twice a page, in look-ahead units
    (not inline). With finding 1 fixed this is redundant but charged work, which can be
    deferred.
-   Body `page-of` aimed at a story shows `?`; documented in `docs/sections.md` §10.
-   S12 (pageless) stays its own gated slice.

### Gate

Full gate (`G:/tmp/gate-sections-fix.sh`, copy of `gate-integrate.sh` on this worktree, log `G:/tmp/gate-sections-fix.log`, finished 2026-10-24 15:59): 12 x `=== rc=0`, 1,659 passed, 0 failed, no `d.ts` or `API.txt` drift; `gate exit 0`. Filled in by the orchestrator from the log.

## B3 fixes, round 2

Astra's re-review of the B3 fixes (`F:/reprise-wt/reviews/sections-b3-fixes-review.md`,
pinned at `5957d68`) asked for changes: two blockers, one should-fix and two suggestions.
The original ownership, limit, orphan, geometry and navigation fixes passed it. Its new
probes (`G:/reprise-review/sections-b3-fixes-{layout,resume,clipboard}-probes.rs`) were
ported to `crates/layout/tests/review_b3_fixes.rs` and
`crates/clipboard/tests/review_b3_fixes.rs` (rustfmt only; the clipboard port drops the
probe's write of an HTML file to `G:/reprise-review`). Their four failing assertions
(public resume, keep work, keep cache, strict decode) are Astra's evidence at `5957d68`;
this round was done by two agents (the first stopped on a rate limit after findings 1 and
2), and I did not re-run the probes against `5957d68`. No assertion was weakened; the one
that was replaced (finding 3) is explained there.

| Item | Commit |
| --- | --- |
| 1. A suspended job owns what it made (blocker) | `43c832e` |
| 2. Keep-with-next look-ahead (blocker) | `a4a2c84` |
| Suggestions: `top_reader` comment, `running_inline` wording | `8a66f78` |
| Clipboard probes ported | `55ed5f1` |
| 3. The decode of a flow host is one bounded atomic unit (should-fix) | `1be5182` |
| This report and the gate | the commit after them |

I read the source of the first two commits before building on them and found nothing to
change in them. The uncommitted state I took over for finding 3 had two defects, below.

### 1. A suspended job owns what it made (blocker)

*What was wrong.* The digests a job has read and the compositions it holds for a region
allocation live in the session's running memo. `LayoutSession::start` dropped them, but a
continuation suspended earlier still passed `resume` (it checks the document and the
revision only) and its allocation then composed what was missing itself, inline. The
reproduction uses public calls only: step to 40 look-ahead units of feedback, suspend,
start and drop another job, resume. At the next steps `running_inline` was 115, in a
budget-one step (232 composer calls, 410 KB scanned).

*What changed, and why this way.* I chose the **generation gate** over making the
continuation own its holds. The memo has an epoch (`Rc<()>`), a new one each time the
digests and holds are dropped (a job starting, `Shared::clear`). A job carries the epoch
it was made under, a continuation takes it along, and `LayoutSession::resume` returns
`JobError::Stale` for a continuation whose epoch is not the memo's, leaving the newer
job's memo alone. Owning the holds would have meant a second copy of every digest and
held composition per job, or an older job overwriting the newer one's digests in the
shared memo; the gate is one comparison, and the caller's remedy (start a new job) is
the one it already has for a changed document.

Losing memo state can no longer authorise inline composition. A need that comes back after
the job made it, and a story need whose digest has vanished, mark the memo lost
(`Shared::lose`); the job ends with `Stale` at its next unit (`LayoutJob::abandon`) and
the memo is dropped. The synchronous fallback of the region allocation (`region_inline`)
is gone. What remains of `running_inline` is `Cursor::look`'s last resort (a page
composing its own few regions when the memo's 64-entry cap evicted what the look-ahead
just made), which no sequence of public calls reaches, counted and tested with a memo
that evicts everything.

The facade is not affected: its job holds a token that `start_layout` bumps, so an older
job was already refused there, and the cache it passes to `LayoutSession::from_cache`
carries the epoch with it.

*Tests.* In `layout/tests/review_b3_fixes.rs`, Astra's probe (it accepts a refusal at
`resume`, as the first design in the review allows) and an exact version beside it,
`a_continuation_of_a_replaced_job_is_refused_and_the_replacement_is_unharmed`: a job
started, a layout run, and a continuation resumed with nothing started in between, which
goes on and equals the plain layout. In `running/stepped_tests.rs`: a need that comes back
ends the job and composes nothing (it replaces the test that accepted the inline
fallback); a job whose holds another job took cannot resume; a continuation resumes only
on the session whose memo it was made under, and carries across `into_cache` /
`from_cache`.

### 2. Keep-with-next look-ahead (blocker)

*What was wrong.* The digest cuts a story before a host over `MAX_HOST_BYTES` or past a
read limit, but a paragraph that keeps with the next enumerated the whole story with
`doc.top` and prepared the refused block synchronously: 69 adapter requests and
1,100,004 shaped bytes in one budget-one step, in a header whose second paragraph the
digest had refused. What it read was no part of the memo's key either: setting only the
refused paragraph's `keep_together` left a cached layout without the
`layout.keep-unsatisfied` warning a full layout gives.

*What changed.* A running story's cursor is given its digest (`Cursor::with_scope`), and
`Flow::following` returns the digest's nodes, in its order, and no others. Nothing outside
the digest is enumerated, prepared or composed for a chain; what the chain reads of the
blocks inside it (kind, text, style, formatting, inline objects) is what the digest's
items hold, so the story's memo is keyed by all of it. The refused tail is not read into
the key, as the brief requires. The check is a machine (`ChainRun`, `Flow::chain_step`)
instead of a call: a member's preparation stage (`begin_preparation`, `advance_staged`,
the paragraph's own stages made reusable) and its composition are units of their own,
charged like the paragraph's, and the paragraph keeps its composed lines between them
(`Held`) so a unit holds one composer call. The trial composition is counted in
`composer_calls` now. Body flows use the same machine, which runs to the end in a full
layout; they have no digest, so their chain reads the document's top-level blocks as before.

*A visible consequence.* For a story whose chain runs into a block the digest left out,
neither layout notes `layout.keep-unsatisfied` on the paragraph any more: the block is not
looked at, and the story is reported clipped. The full layout used to warn, which is
exactly what made the cached one differ. No snapshot depended on it.

*Tests.* Astra's two keep probes pass as ported (work, and cached equals full). Added: a
chain of long members is checked a shaping request a unit; of short members a composition
a unit (at budgets 1, 2 and 5, equal to the plain layout); a chain that runs into a block
the digest cut gives no warning whether or not that block is edited, and the cached layout
equals the full one after each of three edits; editing the style of a block that is in the
digest and that only the first block's chain reads re-lays out the paragraph that keeps with
it, and the cached layout equals the full one, for both values of `keep_together`.

### 3. The decode of a flow host is one bounded atomic unit (should-fix)

*What was wrong.* A host's text is decoded in one go. The first round let the decoding
step overspend and had the digest repay the difference in the steps after it, so a cold
1,000,000-byte header made a budget-one step decode 244 units of text, and the "paid
back" steps afterwards were idle. The review wanted the decode to be resumable or the
bound to be agreed.

*The ruling, as built.* The orchestrator ruled that the decode stays atomic, with an
explicit bound. One host decode is a single look-ahead unit of its own:

-   at most `MAX_HOST_BYTES`, 1,060,864 bytes, which is 258 units of reading plus the
    first block: **5 look-ahead units of 64** at most (a 1,000,000-byte host is 4);
-   that unit decodes the host, takes the host's first block, and does nothing else;
-   the step that runs it reports its true cost (`Step.used`, `StepWork.units`,
    `region_units`). No debt is carried into later calls; `DigestBuild::debt` is gone. A
    job step that starts it with a unit of budget left charges up to 4 over its budget (a
    budget of 1 still makes progress and charges up to 5). This is the only unit kind
    allowed to;
-   cancellation and staleness are observed before and after the step, never inside the
    decode;
-   a host with more text is not decoded: the story is cut in front of it and reported
    clipped. That cutoff depends on how the text is stored (how many paragraphs share a
    physical host), not on the content, and §11 says it is not a semantic boundary, for
    frames to inherit knowingly.

The maximum atomic work is written in §11 ("The one atomic unit"), in the contract's
scheduling text ("Maximum atomic work", under Section additions), in
`docs/bindings.md` and `docs/incremental.md` where they said a step charges at most its
budget, and on `Step::used` and `StepWork::digest_decoded`.

*A hole in "nothing else", found while verifying.* The reader decided whether a host was
too dear for the call by the units that call had spent. The digest's step makes several
calls (one block each), so a host met after the step had spent 63 of its 64 units, such
as a block embedded in the flow of the paragraph in front of it, was decoded in that step:
63 blocks and a 258-unit decode together, **6 units**. The new probe
`a_decode_is_alone_in_its_step_whatever_the_step_had_done_before_it` reproduced exactly
that before the fix. `ReadLimits` gains a `fresh` flag (`unbounded()` sets it): only a
call that opens a piece of work decodes a host dearer than its units, and the digest sets
it on the first read of a step only (top level and table walk alike). Any other call
stops in front of the host (`ReadStep::deferred`), possibly having done nothing, and the
next step, which opens with it, decodes it alone.

*The probe that was replaced.* Astra's
`a_host_decode_must_fit_the_current_step_instead_of_borrowing_future_steps` asserts that a
budget-one step never decodes more than 64 reading units of text. The ruling makes that
false by design, so it cannot stay: its fixture (a 1,000,000-byte header paragraph) is
kept as `a_host_decode_is_one_bounded_atomic_unit_that_charges_its_cost_in_its_step`
in `review_b3_fixes.rs`, whose assertions are the ruling's: the decode is at most
`MAX_HOST_BYTES`, costs 2 to 5 units, is charged in full in its own step
(`used == units == cost`), nothing else ran in it (one block taken, no other child looked
at, no composition, shaping or page), every other step charges exactly its one unit, and
the host is decoded once.

*Tests.* `layout/tests/b3_hosts.rs`: the 1,000,000-byte paragraph (replaces the payback
test), the largest paragraph a story reads whole (1 MiB, four units), a table-cell block
decode, a host met after sixty-three units of other reads, and a host no story could read
is still not decoded. `doc/tests/child_reader.rs`: a call that is not fresh never decodes
a dear host, however often it is made, and a fresh call does. In `running/stepped_tests.rs`
the bounded-step helper allows the one exception (up to the five units, and only for a
step that decoded) and asserts, for a budget of 1, that nothing else ran in the unit that
decoded. The draft I took over had not updated it: `a_one_mebibyte_paragraph_steps_to_the_
same_snapshot` still required every step to charge at most its budget and failed.

### Suggestions

-   The `Document::top_reader` comment no longer says the child IDs are copied up front.
-   The scheduling-counts paragraph of §11 no longer says `running_inline` is zero in every
    test: it is zero for every sequence of public calls the tests make, and the synthetic
    tests that force the last resort assert that it is not.

### Deviations and decisions

-   Finding 3 changes the scheduling contract in one place: `Step.used` can exceed the
    budget by up to 4. It is stated in §11, the contract, bindings and incremental docs.
    Nothing in the facade, its TypeScript types or `API.txt` asserted the opposite.
-   `ReadLimits.fresh` is a new field of a `reprise-doc` struct (B2 API; the digest is its
    only consumer, and `doc/tests/child_reader.rs` its other callers). The facade, its
    types and `API.txt` are unchanged. No new dependencies.
-   Starting a job, or running a layout, on a session now makes an earlier suspended
    continuation of that session stale (finding 1). A host that wants to keep a job keeps
    it the only one on its session; §11 says so.
-   No snapshot changed or was added. Existing tests changed: the inline-fallback test of
    `stepped_tests.rs` (finding 1), the payback test of `b3_hosts.rs` and the
    bounded-step helper (finding 3).
-   Environment, not repository: linking the layout crate's `review_b3` test failed here
    with `LNK1181: cannot open input file 'kernel32.lib'` until `LIB` was set explicitly
    (MSVC `lib\x64` and the SDK's `ucrt\x64` and `um\x64`); the other test binaries linked
    without it. The gate ran with the same `LIB`.

### Left tracked

As in round 1: the 64-entry composition memo (redundant but charged work), body `page-of`
aimed at a story shows `?`, and S12 stays its own gated slice. For frames F0: a reader has
no document or revision binding of its own and needs a same-revision lifetime; the host
cutoff is representation-dependent; and the digest scope above is the only scope a
look-ahead of a running story may read.

### Gate

Full gate (`G:/tmp/gate-sections-fix.sh`, log `G:/tmp/gate-sections-fix2.log`, run on `1be5182` under `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 timeout 7000`, finished 2026-10-25 07:24): 12 x `=== rc=0` (types, wasmbuild, bindgen-web, bindgen-node, tsc, tsc-worker, smoke, worker-smoke, fmt, clippy, wasmcheck, test), 1,679 passed (round 1: 1,659; the 20 added are this round's tests), 0 failed, 12 ignored (as before), no `d.ts` or `API.txt` drift (`git status` clean after the gate); `gate exit 0`.

## B3 fixes, round 3

Astra's re-review of round 2 (`F:/reprise-wt/reviews/sections-b3-fixes-2-review.md`, pinned
at `c01b344`) asked for changes: one blocker, one should-fix and one suggestion. The
decode, continuation-ownership, engine/font, admitted look-ahead, widows/orphans/table and
non-layout fixes of round 2 passed it. Its seven extra probes
(`G:/reprise-review/sections-b3-fixes2-extra-probes.rs`) were ported to
`crates/layout/tests/review_round2_extra.rs` (SHA-256 checked against the manifest before
formatting; rustfmt only after). It includes `review_b3_fixes.rs` for its fixtures, so that
binary also re-runs that file's tests (27 tests in all; about 90 s in debug).

**Probes first, on the unchanged source (`c01b344`).** 5 passed, 2 failed, as in the review:
`round2_lookahead_relation_owner_is_part_of_digest_validity` (cached keeps
`layout.keep-unsatisfied`, full does not; `running_reused=2`, `running_compositions=0`) and
`round2_old_revision_refusal_must_not_destroy_newer_suspended_job` (`current continuation must
survive: Stale`). Log: `G:/tmp/sec3-probes-before.log`. After the fixes all seven pass. No
assertion was weakened or changed.

| Item | Commit |
| --- | --- |
| 1. Relation ownership in the keep chain (blocker) and 2. ownership before destruction in `resume` (should-fix), the probes and their regressions | `741669d` |
| 3. `read_children` comment (suggestion) | `b085bdd` |
| This report and the gate | the commit after them |

### 1. Relation ownership in the keep look-ahead (blocker)

*What was wrong.* `Flow::chain_step` skipped a following block that was in `region_owners`
(owners of NOTE and FLOAT relations), whatever the block's kind. The running digest keyed
relation ownership for images only (`Item.unsupported`), and its `Inputs` hold no relations,
so a paragraph's relations were read by the chain and keyed by nothing. Adding the schema-valid
NOTE of Astra's probe to the second header paragraph changed the chain in a full layout and not
in a cached one.

*It was wider than the probe.* The body flow has the same mismatch. The body's per-paragraph
memo key (`FlowKey.lookahead`) held each look-ahead block's inputs and whether its table role
was plain, and no ownership. With pages of ten lines, a filler of eight, `head` (keeps with
the next) and a 5,000-byte paragraph that keeps with the next, a NOTE owned by that paragraph
left the cached layout with `layout.keep-unsatisfied` and the full layout without it
(`a_body_paragraph_a_relation_names_owner_is_still_a_paragraph_to_the_keep_chain`). A fix that
only touched the digest would have closed the probe and left this.

*The choice.* Astra offered two: key the classification for every node the chain can affect,
or exclude invalid owners from the decision consistently. I did both, because they are one
function. The flow itself lays out every block of kind paragraph, and every table, whatever
relation names it owner (`Flow::begin_block`: only an annotation or an image can be left to
the regions; the region pass reports any other owner invalid and places nothing). The chain
should look at what the flow lays out, so a paragraph or a table is never skipped for being
an owner; that is the "exclude invalid owners" half, and it makes the cached and full layouts
agree by making the read disappear for those kinds. What remains is real: an image a float or
note owns is skipped, any other image ends the chain, and that is keyed.

The classification is one function, `Link::of` (`flow/keeps.rs`): `Check` (a paragraph:
composed in the frame), `Pass` (an annotation, or an image a float or note owns) and `End` (a
table, any other image, anything else). `chain_step` takes its verdict from `Flow::link`
and from nothing else about structure. The `Link` of every block the chain can reach is
held in the key:

-   the running digest's `Item` has a `link`, read once per block as the digest is made
    (`Classes` is still read only when a story holds an image). In a running story
    `Flow::link` reads it from the digest, by the block's place in the digest's order, so the
    chain reads what the memo is keyed by, not the document;
-   the body's `FlowKey.lookahead` holds `flow::Link` in place of the plain-role boolean
    (a strictly finer key: it is `End` for a non-plain role as before).

This is not the image-only dependency again: every node the chain can reach carries its link,
and the kinds for which ownership is not read cost nothing extra. It also closes a corner the
old image key missed: an image that is both followed (`FOLLOW`) and owned (`FLOAT`/`NOTE`) is
`unsupported` either way, so removing only the owner left the digest unchanged while the chain
went from `Pass` to `End`. The link is exact where `unsupported` was not.

*Behaviour change, stated plainly.* A paragraph, or a table container, that owns a NOTE or
FLOAT relation is now treated by the keep chain as the block it is laid out as. Before, a
full layout skipped it as if region-placed while the flow laid it out anyway. Only an invalid
owner is affected, and the region pass already reports it (`relation.owner-not-placeable` in
the body test). No existing snapshot or test changed.

*The audit Astra asked for: everything the admitted look-ahead reads, against its key.*

| The chain reads | Keyed by (running story) | Keyed by (body) |
| --- | --- | --- |
| which blocks follow, and their order | digest `nodes`, `omitted` | the `ahead` list in `FlowKey` (stops at section starts; 33 blocks) |
| section starts (`following`) | not applicable: a story has no section table | in the `ahead` list |
| relation ownership of a block | **`Item.link`** (was image-only) | **`FlowKey.lookahead` link** (was absent) |
| table role of a block | `Item.link`, and `Item.table` for a table | `FlowKey.lookahead` link |
| block kind | `Inputs.kind` | `Inputs.kind` |
| text, style chain (missing or renamed names included), overrides, text formats, inline objects, image record, list semantics, of the member prepared and composed | `Inputs` of the item | `Inputs` in the look-ahead entry |
| the initiating paragraph's keep flag | its own `Inputs` (it is in the digest) | its own `Inputs` |
| resolution context of the trial | derived from the initiating paragraph; trial preparations are cached by exact `(node, context)` | same |
| frame width, page size | `Key.width`, `Key.page` | `FlowKey.template`, `position` |
| depth, reservations, exclusions | `Key.depth`; a story's scratch frame has none else | logged plan reads (`Reservation`, `Exclusions`) in the memo entry, checked by `reads_hold` |
| field values a trial composition reads | `FieldEnv::text` records each read in the composition's `Reads` (label, values, guess), checked by `still_valid` | recorded in `field_log` and noted as `PlanRead::Field` |
| `paragraph_spacing`, functions, fonts, shaper | engine configuration, immutable for a session (the documented precondition of `LayoutCache` and `LayoutContinuation`) | same |

Correction in round 4: this audit missed a composition dependency. `prepare_ahead` used
`region_owners` to choose speculative preparations (now removed), while `table.rs` used it
to skip preparing annotations in table cells. The unretained plain-annotation scratch probe
did not clear that table path: preparation can emit diagnostics, and an undefined style
exposes cached/full disagreement on NOTE/FLOAT add and remove. The retained review probe
and the round-4 matrix below replace that unsupported conclusion.

*Tests.* In `review_b3_fixes.rs` (each compares every layout in a session with a full
layout, diagnostics included, and covers adding and removing the relation): a paragraph
owner of a NOTE and of a FLOAT, in a running header and in the body, with the keep verdict
unchanged by the relation; an image owned by a NOTE or a FLOAT, whose verdict flips when it
is owned and back when it is not; an image both followed and owned, then owned no longer,
then not followed; a table container as owner. Before the fix, on `c01b344`'s source, the
paragraph (header and body), the followed-and-owned image and the table tests failed. The
plain owned-image test passed there: the digest already keyed it through `unsupported`, and it
stays as the guard for the path that was kept. Astra's probe is the regression she named.

### 2. Ownership before destruction in `resume` (should-fix)

*What was wrong.* `LayoutSession::resume` rejected a mismatching document or revision first,
and cleared the evaluation (the memo, the caches and the counters) before asking whether the
continuation owned the memo epoch, so an obsolete continuation destroyed the newer job's memo.

*What changed.* `resume` now asks `holds(epoch)` first. On a document or revision mismatch,
only the continuation that owns the memo clears it, as before: an edit can race a job and
leave anything in the memo, so the owner drops it with the rest. A continuation that does not
own it is refused with `JobError::Stale` and touches nothing. The same-document, same-revision
case is unchanged. A continuation made on another session is not an owner either, so it
cannot clear this session's memo any more (it could, when it also named another document).

*Tests*, beside the same-revision replacement test: an old-revision continuation refused
while a newer job holds the memo, which then finishes and equals the full layout (Astra's
test, with the session's counters asserted unchanged by the refusal); a continuation
resumed with another document after a newer job replaced it, and a continuation from another
session resumed with another document; and the owner's behaviour kept, for a wrong document
and for an edited one (`counters()` is zero afterwards, and the next layout equals the full
one). The first two failed on `c01b344`'s source; the owner test passed there and still does.

### 3. `read_children` comment (suggestion)

The method comment no longer says every call does at least one thing: a fresh call does, a
call that is not fresh may stop in front of a host dearer than its allowance and say
`deferred`, indefinitely, and the caller must handle that and open a fresh call to get past
it. Comment only.

### Docs

`docs/sections.md` (keep look-ahead, and "A job owns what it made") states the `Link` and the
refusal rule.

### Deviations and decisions

-   The keep chain's treatment of a paragraph or table that owns a region relation changed
    (finding 1), for invalid owners only; no snapshot or test depended on it.
-   `FlowKey`'s look-ahead entry holds `flow::Link` instead of a boolean, and `Item` has a
    field. Both are crate-private. No public API, facade type, `API.txt` or dependency changed.
-   The probe file `include!`s `review_b3_fixes.rs`, as Astra's does, so `review_round2_extra`
    re-runs those 20 tests. I left it as ported rather than move the fixtures.
-   The finding-2 probe and my test of it are the same sequence under two names: the probe
    keeps its own name and helper, and the test beside the replacement test asserts the
    refusal leaves the counters untouched as well.

### Left tracked

As in rounds 1 and 2: the 64-entry composition memo, body `page-of` aimed at a story, and S12
as its own gated slice. For frames F0, a look-ahead of a running story reads the digest's
blocks and their `Link`, and only those.

### Gate

Full gate (`G:/tmp/gate-sections-fix.sh`, log `G:/tmp/gate-sections-fix3.log`, run on `741669d` under `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 timeout 7000`, with `LIB` set as in round 2, finished 2026-10-25 23:53): 12 x `=== rc=0` (types, wasmbuild, bindgen-web, bindgen-node, tsc, tsc-worker, smoke, worker-smoke, fmt, clippy, wasmcheck, test), 1,714 passed (round 2: 1,679; the 35 added are this round's: 8 in `review_b3_fixes.rs`, and the 27 of `review_round2_extra.rs`, 20 of which are that file's re-run of `review_b3_fixes.rs`), 0 failed, 12 ignored (as before), no `d.ts` or `API.txt` drift (`git status` clean after the gate); `gate exit 0`. Before the gate, clippy and `cargo test -p reprise-layout -p reprise-doc` were run on the same code and caught one `clippy::useless_vec` in a new test, fixed in `741669d`.

## B3 fixes, round 4

Reviewed the round-4 brief and the round-3 request-changes review against
`a614597fd468a43bf5ff9deef89fadb758c86abe` on `ws/sections2`. That commit contains the
ownership repair; this completion adds coverage and corrects the audit and documentation.
The coverage completion is committed as `37373d5156a834a3c97ed41c03d2cda55af35feb`.

### Ownership is keyed as a class

The chosen approach preserves composition's existing treatment of owners and keys the
classification it reads. It does not special-case the failing annotation. `ownership.rs`
alone scans relations for composition, producing `Free`, `Follower`, or `Owner` with
NOTE/FLOAT ownership taking precedence over FOLLOW. `Cursor` and `Flow` hold `Ownership`,
not a set of region owners. The body reads its classes once when a fresh flow begins;
its prepared-content cache remains keyed by exact content/context, and calling or skipping
preparation decides which diagnostics are replayed for this layout.

Every running-story cursor is constructed in `running/compose.rs` with `with_scope(digest)`.
Its ownership accessor answers only from that digest. The digest records classifications
for top-level annotations/images and annotations in table cells; `same_as` compares those
answers as well as nodes, items, and truncation. A table's nested classifications are
published with its parts only when the whole table is admitted. No live relation scan can
change composition behind an unchanged key. Paragraphs, tables, and cell images are
composed independently of their ownership, so their relation classification is not needed
in the digest. Rejected nested table descendants are never composed by ordinary table
flow; a valid body-owned note/float can still be prepared separately by the region pass.

The composition reads are now `Flow::begin_block` (images and annotations),
`Flow::link` (body image keep classification; running keeps use the digest's `Link`),
and `Flow::table_rows` (cell annotations), all through `Ownership`. The speculative
`prepare_ahead` ownership filter and `regions::owners` are gone. This closes the dependency
class at its source, including diagnostic-producing preparation.

The enforcement is a **test**, not a Rust privacy guarantee against calling public
`Document` methods: `relation_reads.rs` scans production layout sources for relation reads,
rejects unlisted reader files and the old owner-set names, and confines `Classes::read`
and `Ownership::of` to their construction sites. A direct `.relations()` read newly added
to flow, table, running, or inline composition fails this source guard. New legitimate
reader files require an explicit explanation in its allowlist.

### Regression coverage and completion audit

- The two review probes in `review_round3_matrix.rs` retain their assertions. The saved
  source's SHA-256 is `541ADB2A998A1F3DEA18C0444583B3865DFF6B69BA50E5952860EA1841B7AE9C`,
  matching `sections-b3-fixes3-manifest.json`; formatting a copy with Rust 2024 rustfmt
  produces a byte-identical checked-in file. The review records four failures among
  eight diagnostic transitions before the fix, while its top-level matrix passed.
- The previous implementation's retained `G:/tmp/sec4-matrix-before.log` records all
  three original ownership-matrix tests failing, including 15 snapshot disagreements;
  `G:/tmp/sec4-matrix.log` records all three passing after the fix. These are inherited
  baseline evidence, not a fresh rerun of the old source during this completion.
- The extended matrix covers paragraph/annotation/image x body/header x top level,
  first table cell, later row/second cell, and descendants of rejected nested tables
  x NOTE/FLOAT/FOLLOW x both initial relation states: 144 scenarios, each warmed and
  toggled three times. Every comparison includes complete snapshots and diagnostics.
  Annotation style-diagnostic changes must actually occur in both first and later cells.
  Nested containers must report `layout.table-invalid`. Their descendants stay out of
  ordinary composition; valid body-owned region preparation remains allowed. Changing
  ownership of rejected story descendants must reuse the header composition.
- Overlapping FOLLOW/NOTE, FOLLOW/FLOAT, and NOTE/FLOAT ownership is tested for images
  and annotations in all eight placements, including removing one while another remains
  (48 scenarios). The precision regression checks that an unrelated body's relation
  reuses the header while a cell annotation's changed ownership recomposes it.
- A further 12 scenarios put a paragraph after 70 breaks inside a shared table-cell
  flow host, in body and header, through both starting states and all three schemas.
  Add/remove comparisons include diagnostics, and assert the late paragraph is prepared.
- The full gate also retains valid body-owned note/float placement, stepped suspension,
  keep-chain, and continuation-ownership regressions. No production behavior beyond
  `a614597` needed changing; the follow-up corrects coverage and describes precisely
  which nested kinds read ownership.

### Relation-read inventory

`rg` over the production layout crate found no remaining `region_owners` references.
Every relation lookup and relation-derived ownership use was checked:

- `ownership.rs`: `Classes::read` is the only composition classification scan; both
  construction paths above and the complete-snapshot matrices cover it.
- `incremental.rs`: `PassKey::new` stores authored/resolved relations, owner liveness
  and owner inputs for region/final-pass memo validity. `partial` reads relation emptiness
  only for the settled flag. `record_graph` reads relation dependencies, region schemas,
  owner lines and reading-order dependencies after evaluation. None classify running
  composition; the key and dependency graph are covered by workspace memo/job tests.
- `regions.rs`: `active` detects a needed pass; `allocate` selects, validates, sorts and
  places region relations, rejects story owners, and records applied placement owners.
  `is_region` is the NOTE/FLOAT schema predicate used by this pass and `Classes::read`.
  Region results/diagnostics are memoised under `PassKey`, independently of running
  composition. Existing body region tests and the ownership matrix cover these paths.
- `relations/mod.rs`: final relation dispatch, owner-deletion checks and schema ownership;
  `relations/resolve.rs`: target resolution, including its FOLLOW compatibility check;
  `relations/alignment.rs`: alignment application and ordered relation construction.
  These operate in the final pass covered by `PassKey`, not running-story composition.
- `sections/mod.rs`: the continuous-section FLOAT-presence check selects `FloatSpaces`.
  Its result reaches composition through resolved frames, included in position/context
  keys. Section/band/float workspace tests cover it.
- `floats.rs`, `notes.rs`, and `relations/follow.rs` consume relations supplied by their
  respective passes rather than independently querying composition ownership.
  `display.rs` consumes resolved snapshot relation owners for rendering. Neither
  provides a hidden live-document classification to a running composition.
- `running/compose.rs` reads classes only to build its key. `flow.rs`, `flow/keeps.rs`,
  and `table.rs` consume the accessor as described above; none query relations directly.

`docs/CODEMAP.md` now maps the new ownership module and its regression files.
No dependencies, existing snapshots, public interfaces or architecture decisions changed.

### Gate

Full gate: `G:/tmp/gate-sections-fix.sh`, log `G:/tmp/gate-sections-fix4.log`, run with
`CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 timeout 7000 bash ...` and
`TMP=TEMP=G:/tmp`. Builds used `F:/reprise-target/eng-sections2-b2` and its WASM sibling,
the G: Cargo home, `INSTA_UPDATE=no`, four build jobs, and the MSVC/Windows SDK `LIB`
configuration retained from the earlier review environment. It finished on
2026-10-26 at 16:12:51 (Asia/Manila) against the implementation tree now committed as
`37373d5`; no source or test changes followed the run.

All **12 x `=== rc=0`**: types, wasmbuild, bindgen-web, bindgen-node, tsc, tsc-worker,
smoke, worker-smoke, fmt, clippy, wasmcheck, test. **1,742 passed, 0 failed, 12 ignored**;
`=== GATE DONE`, `gate exit 0`. The increase from round 3's 1,714 is the 22 tests in
`review_round3_matrix` (including 20 fixture tests), two relation-read source guards,
and four ownership-matrix tests. No `.d.ts` or `API.txt` drift, dependency changes, or
existing snapshot changes: checked with `git diff --name-only a614597^` restricted to
those files, which returned no paths. The pre-existing round-4 log was preserved as
`G:/tmp/gate-sections-fix4-inherited.log` before this fresh run replaced the requested log.

Before the full gate, all **28 focused tests** passed in `relation_ownership_matrix`,
`relation_reads`, and `review_round3_matrix` (`G:/tmp/sec4-sol-matrix.log`). The expanded
nested test initially asserted that no body descendant could be prepared; inspection
showed that valid body NOTE/FLOAT placement intentionally prepares it in the region
pass. The corrected new assertion distinguishes ordinary table composition from that
separate pass. Neither retained review probe nor any existing assertion was weakened.

The remaining working-tree changes after the gate are documentation/report updates.
No processes were killed, and no merge or push was performed.
