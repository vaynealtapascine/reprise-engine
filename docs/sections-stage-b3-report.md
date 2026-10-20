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

{{COMMITS}}

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
    and `MAX_ENTRIES` are `pub(crate)`; two `cfg(test)` hooks.
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

## 10. Verification

{{VERIFICATION}}
