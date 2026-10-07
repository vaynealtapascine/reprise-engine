# Cross-crate fuzzing

Every crate has hostile tests, and the bindings fuzz their boundary. This is the
fuzzing that runs **across** crates: sequences that go through format, doc, edit,
layout (full and incremental), display, clipboard and export, with invariants
checked after every step (decision 39).

The harness is `crates/fuzz-harness` (`reprise-fuzz-harness`, `publish = false`). It
has no dependencies beyond the workspace crates: the scenario decoder is
hand-written, so no new licence enters the tree.

## What a scenario is

A scenario is a byte string. `Scenario::decode` is **total**: every byte string
decodes to a starting point and at most 48 ops, and once the bytes run out the
decoder answers zero, which is always a legal choice. That one property is what
lets libFuzzer mutate raw bytes, a seeded generator produce them, and the
minimiser delete any slice of them.

*Start:* a synthetic document (blocks, authored ranges with the three policy
presets, follow/note/float relations, a page template, extra styles), forked for
the second peer; or one of the hostile fixtures, saved to a package and opened
once per peer (so the format's open path is exercised first).

*Ops* hold selectors, not identities ("the block at index `n` modulo the live
blocks"), so they stay meaningful after any mutation:

| Op | What it does |
| --- | --- |
| `Edit` | one command, or a batched transaction, on a peer: insert/delete text, split, join, insert/delete/move block, style override, add/remove relation. Selectors may deliberately pick invalid offsets, dead blocks and unknown schemas |
| `Undo`, `Redo` | per-peer undo and redo |
| `Sync` | merge in several orders (A then B, B then A, crossed from pre-merge forks, through exported bytes, one way only) |
| `Copy`, `Paste` | native fragments (whole document or block selections with byte ranges), through the wire encoding or not, pasted at a caret or appended, into the same or another namespace |
| `InsertImage`, `ImportText` | an image fragment; plain or HTML import (including tag soup, scripts, deep nesting) pasted through the kernel |
| `Template`, `DefineStyle` | page templates (including unreadable ones) and named styles (extreme lengths, cycles, expressions, unreadable stored values) changed on the document and committed as one undo step |
| `Engine` | composer (greedy, optimal, justified, authored break), medium (including zero, negative and saturated), page limit, font registration (collection faces, aliases, damaged bytes), image registration (PNG, JPEG, truncated, not an image) |
| `Layout` | `Engine::layout` twice; the peer's long-lived `LayoutSession`; or a budgeted `LayoutJob` with random budgets (zero and unbounded included), viewport, partial views and cancellation |
| `SaveReopen` | package save and reopen, history or shallow, with fonts and images embedded or not, as another peer |
| `Render` | display JSON, debug overlay, SVG, PNG, PDF, ordered PDF |
| `Export` | plain, HTML, native, PDF, with or without layout |
| `Navigate` | hit tests anywhere, every movement, selections, and copying a selection |

An epilogue then undoes everything, redoes everything and syncs, checking state
and convergence at each stage.

### Through the facade

`facade.rs` runs byte-string scenarios through the public `reprise` crate, as a host
would: two sessions that share history (one is the other saved and opened), edits,
undo and redo, update packets, layout jobs stepped with random budgets and viewports,
a job interrupted by an edit, save and open, copy and paste, exports, SVG and PNG. It
checks that every error code is documented, that a refused call changes nothing, that
any budgets give the pages one unbounded job gives, that an interrupted job is stale
and a fresh one is current, that replicas converge and lay out identically, that a
reopened session draws exactly what the original drew, and that output is
byte-identical when produced twice. Run it with `FACADE=1` on the `explore` example,
or `REPRISE_FUZZ_FACADE_SEEDS=<count>` on the test.

## Oracles

Checked after every op (the name is what a failing run prints in brackets):

| Oracle | Meaning |
| --- | --- |
| `no-panic` | nothing panics, on any content |
| `incremental-equals-reference` | `LayoutSession` and every completed `LayoutJob` equal `Engine::layout`, including diagnostic order; partial views publish; a cancelled job stays cancelled and doesn't poison its session |
| `peers-converge` | after a full sync the replicas hold the same authored state, range resolution, revision and layout |
| `undo-restores-state`, `redo-restores-state`, `undo-everything-restores` | undo returns to the authored state before the step, redo to the state after it, and undoing every step of a peer that never merged returns to its start |
| `reopen-identical`, `reopen-lays-out-identically` | saving and reopening gives the same authored state, ranges, revision and (History mode) the same bytes again and the same layout |
| `copy-preserves-text`, `paste-preserves-text`, `selection-copy-preserves-text`, `export-keeps-text` | text survives copy, paste at any caret, selection copy, and plain/HTML export and re-import (non-whitespace characters, since import normalises whitespace) |
| `reading-order-once` and friends | reading order visits every placed line exactly once, on the page its frame is on |
| `*-repeatable` | the same input gives byte-identical layout, display JSON, SVG, PNG, PDF, exports and saved packages; with `run_twice` the whole scenario's transcript is identical across two runs |
| `undocumented-code` | every diagnostic code any crate reports is in the `docs/contracts.md` table (the harness reads the table) |
| `valid-edit-accepted`, `invalid-edit-refused`, `refused-edit-changes-nothing` | a valid command is accepted, an invalid one refused, and a refusal changes nothing (state, revision, undo counts) |
| `insert-text-model`, `delete-text-model`, `split-model`, `join-model` | a single command's result equals a trivial string model |
| `ids-never-reused` | a block created by an edit never reuses an ID the scenario has seen |
| structure oracles | lines cover their text contiguously and start on grapheme boundaries; frames are on real pages; queries find every position; carets from navigation are valid positions |

## Running it

```sh
cargo test -p reprise-fuzz-harness
```

is the stable, bounded target and runs in `cargo test --workspace` and in CI on
three platforms. It runs the regression corpus (twice each, comparing output) and 40
seeded random scenarios, plus a coverage test that fails if the random scenarios
stop reaching any feature. A debug build takes well under a minute. Widen it:

```sh
REPRISE_FUZZ_SEEDS=300 REPRISE_FUZZ_FIRST=100000 cargo test -p reprise-fuzz-harness
```

A failure prints the oracle, the op index and the scenario's bytes in hex.

To explore many seeds, shrink failures and print them:

```sh
cargo run -p reprise-fuzz-harness --release --example explore -- FIRST COUNT [BYTES]
NO_MINIMIZE=1 ...   # print failures unshrunk, faster
```

The shrinker (`minimize`) is delta debugging over the bytes. It only accepts a
smaller input that fails the *same* oracle. A failure that aborts the process
(an out-of-memory allocation) can't be shrunk in-process; bisect the scenario's
ops by hand instead.

### Adding a regression

1. Save the minimised bytes as `crates/fuzz-harness/corpus/regress/<name>.hex`. Lines
   starting with `#` are comments: say which oracle failed and what the bug was.
2. Add an explicit scenario (or a direct test) to `tests/regressions.rs` that reads
   well and fails without the fix.
3. If the bug is in a file another workstream is changing, mark the test
   `#[ignore = "<owner>: <reproduction>"]` instead of fixing it.

## libFuzzer

`fuzz/` is a `cargo-fuzz` package (not a workspace member; the root manifest
excludes it) with two targets over the same harness:

```sh
cargo install cargo-fuzz
cargo run -p reprise-fuzz-harness --example corpus_to_bin -- fuzz/corpus/scenario
cd fuzz
cargo +nightly fuzz run scenario            # every oracle after every op
cargo +nightly fuzz run scenario_twice      # and the whole scenario twice, byte for byte
```

`cargo-fuzz` needs a nightly toolchain and libFuzzer, which this repository's
Windows development machine doesn't have set up; the targets were checked to
*compile* (`cargo check` in `fuzz/`) but not run there. CI builds them on Linux
nightly (`cargo fuzz build`). Run them on Linux for real fuzzing, with
`-- -max_len=600` to keep scenarios in the range the decoder uses.

## Extending it

-   A new oracle goes in `oracle.rs` (about snapshots and output) or in the
    executor (`run.rs`, about state), returning a `Violation` with a stable name.
-   A new op is a variant in `scenario.rs`, a weight in `decode_op`, and a handler in
    `Cx::step`. Keep selectors modulo the live state so every byte string stays valid.
-   Grow a pool in `pool.rs` rather than inventing data inline; a scenario should
    only ever say "entry `n`".
-   The harness must stay pure: no clock, randomness or environment affecting what
    a scenario does (`REPRISE_FUZZ_*` only choose *which* seeds the test runs).

## What it has found

See the final report of the `hardening/fuzz` branch and `PROGRESS.md`; every bug has
a corpus entry under `corpus/regress/` and an explicit regression.

-   `compose`: a line break was offered between a space and a ZWJ or combining
    mark, starting a line inside a grapheme cluster (fixed).
-   `display/png.rs`: a page of a saturated medium asked for a one-terabyte pixmap
    and aborted the process (fixed: 16 Mi pixel cap, `RenderError::BadSize`).
-   `text`: asking Loro for the position of a character deleted before a shallow
    snapshot panics inside Loro and poisons the document; reopening a history-free
    package aborted on layout (fixed: such an anchor resolves as a tombstone at the
    start of the text).

Gaps it surfaced that the kernel's contract allows, left as they are (see the report):

-   The kernel accepts `InsertText` into a table or row container, or into a cell that
    holds blocks. That text is never laid out or exported, so it is invisible content.
    Copying a selection that covers whole cells promotes the table and copies it too.
-   `Editor::can_redo()` can stay true after a merge while `redo()` returns `false`
    (a collaborator's edit made the step impossible to redo).
-   A package embeds only faces with glyphs in the layout, so a style that merely
    names a face reports `font.fallback` when the package is opened without that face.
