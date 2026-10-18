# Cross-crate fuzzing

Every crate has hostile tests, and the bindings fuzz their boundary. This is the
fuzzing that runs **across** crates: sequences that go through format, doc, edit,
layout (full and incremental), display, clipboard and export, with invariants
checked after every step (decision 39). The wave-1 features (styles, lists, inline
objects, annotations, `batch`, `import_copy`) have their own target, described in
[The wave-1 foundations](#the-wave-1-foundations).

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

## The wave-1 foundations

Wave 1 added paragraph styles and named styles with durable identities, `batch`
with `expected_revision`, `import_copy`, roles and lists, inline objects and
fields, annotations, and preflight refusal. The core harness and `facade.rs`
know none of them. This target (`hardening/fuzz-2`) does, in two parts:

-   **the command fuzzer** (`src/foundations/`) drives the whole vocabulary
    through the public `reprise` facade;
-   **the hostile peer** (`crates/doc/src/hostile/foundations.rs`,
    `tests/foundations_hostile.rs`) writes raw, malformed and adversarial values
    into the stored forms those features added, and delivers them through a sync
    packet.

### The command fuzzer

A scenario is a byte string, as above; decoding is total. It starts with one
document of one to three paragraphs, a few interaction batches already applied,
and two or three replicas that share its history (saved, then opened under other
peers). `import_copy` adds independent documents (other lineage, at most five
documents alive). Fonts are the bundled Source Serif face; two images are
registered as assets in every session.

| Op | What it does |
| --- | --- |
| apply | one command (the vocabulary below) through `apply` |
| batch | one to four commands through `batch`, with `expected_revision` absent, the current revision (*fresh*), or one that is not the current revision (*stale*: a revision from before the last edit, or a counter bumped by one, or a foreign peer). A stale one must be refused as `bindings.stale` and a fresh one never is. A selection travels with some batches and must come back as real positions |
| recipe | an interaction batch whose later commands depend on the earlier ones (below) |
| undo, redo | on any replica; the replica's own trail predicts the state |
| sync | all replicas of one lineage exchange snapshots, rotated or reversed, until their vectors agree |
| copy, paste | a selection from one block to another (mixed range), offsets at boundaries, pasted at a caret or appended into the same replica, another replica or another document; or the last clipboard, which may be stale |
| import-text | HTML (headings, lists, links, tag soup, a field span, a literal U+FFFC) or plain text, at a caret or appended |
| reference | save, reopen under another peer, compare state and pages with the live session, check the bits |
| import-copy | `import_copy` of a saved replica as a new document; checks content, annotations, independence; then an edit and a reference check on the copy |
| layout | a layout job in random budgets, viewport and page limit against one unbounded job |
| queries | outline, page links, annotation queries and geometry, `field_target`, filtered diagnostics |
| export | plain, HTML, native and PDF twice each, optionally SVG and PNG |

After each op the replica is observed: `State` is checked for coherence, a full
layout is run and its pages and the document are folded into the repeatability
transcript, and every diagnostic code is checked against `contracts.md`.

#### Command coverage

Every command of the wave-1 vocabulary is generated in two ways: by percent
weight (below) and, in a quarter of the draws, evenly from the fifteen commands
the weights make rare. Selectors are modulo the live state, so every byte string
stays valid; about one value in ten is deliberately wrong.

| Command | Generated by | Interactions it is aimed at |
| --- | --- | --- |
| `insert-text` (10%) | block (any), boundary offset, text pool incl. a literal U+FFFC, ZWJ, RTL, line separator | text typed beside an atom or at a link edge; the reserved U+FFFC is refused |
| `delete-text` (5%) | range: whole text, around an inline object, around an annotation end, or random | deleting an atom inside a link; a delete across a retired marker |
| `split-block` (5%) | at a range edge of an annotation half of the time | a link across a split; a split in a list item |
| `join-blocks` (3%) | adjacent paragraphs (a non-adjacent join drops link coverage, see below) | a join of list items or paragraphs holding atoms |
| `insert-block`, `delete-block`, `move-block` | index and block selectors | deleting a block a link points at |
| `set-style` (3%), `format-text` (4%) | paragraph fields with expressions (valid, huge, garbage), `clear` lists; character styles with `reset` and `clear` | a format across an atom and a link |
| `define-style` (4%), `update-style` (3%), `rename-style`, `delete-style` | names incl. built-ins, a 2,000-byte name, parents, clears | rename then apply; delete a style in use |
| `apply-style` (5%) | any block, mostly list items and role-bearing blocks, a name or `null` | a style applied to a list item |
| `set-role` (4%) | all twelve roles | a heading role then its default style |
| `create-list`, `update-list`, `delete-list` | definitions: mostly sound; a sixth carry a duplicate or out-of-range level, an unknown format, negative or extreme indents, bad starts; `base` links | a list updated or deleted under its items |
| `set-list-item` (4%), `remove-list-item`, `set-list-level`, `indent-list-item`, `restart-list`, `continue-list` | levels 0 to 255, restart values incl. overflow; `ListRef::New` inside a batch | numbering commands on a plain paragraph get a list and an enrolment first |
| `enter`, `backspace-at-start` | list items, offsets | Enter and Backspace in a list item that holds an atom |
| `insert-inline` (10%) | kinds `image`, `field` (`page-number`, `page-count`, `page-of`, `text`), an unknown kind and an empty one; sizes valid and extreme; targets from `field_target`; aimed at annotated paragraphs | an atom inside a link; a field in a list item |
| `update-inline` | at an object's offset; parameters set, removed (`null`), alignment | a resize beside a delete |
| `add-annotation` (6%) | links (URL, schemes, node, heading, range) and comments, ranges over a paragraph, an atom or an annotation edge | overlapping links and comments |
| `update-annotation`, `remove-annotation` | real IDs mostly | update after a split |
| `insert-line-break`, `insert-tab`, `set-alignment` | low weight | a tab inside a link |

Recipes (each a batch, validated command by command): an atom inside a link; a
field in a list item; a link across a split followed by an atom; a style applied
to a list item with its role; a list built and split with Enter, indented and
backed out; a format across an atom and a link followed by a partial delete; a
style defined, applied to a list item, updated and renamed; a heading role with
its default style; a comment and a link over the same text with a field between;
a list's life (create, enrol, restart, continue, level, indent, delete, remove);
annotation churn (two annotations, text deleted between their ends, one updated
or removed).

#### Oracles

| Oracle | Meaning |
| --- | --- |
| `no-panic`, `undocumented-code` | nothing panics, no session is poisoned; every code in an error, `Applied`, `State`, `diagnostics()`, an export or a loss is in `contracts.md` |
| `state-coherent`, `inline-matches-text` | blocks have unique IDs, real parents, resolved styles, ordered formatting runs on boundaries; list items name lists; annotation spans are in range, on boundaries and non-empty when active; every U+FFFC of a block's text is exactly one entry of `Block.inline` |
| `refused-leaves-document-identical` | a refused command, batch, paste or import leaves every operation byte-identical (an operations-only export), the revision, the authored state and the undo flags |
| `stale-revision-refused`, `batch-selection-valid` | stale ⇒ refused as `bindings.stale`; fresh or absent ⇒ never `bindings.stale`; a mapped selection is a real position |
| `undo-restores-state`, `redo-restores-state`, `undo-redo-noop` | an undo or redo that reports nothing to do changes nothing; undo returns the state before the step, redo the state after it, right after a step and from the replica's own trail |
| `incremental-equals-reference`, `reopen-identical`, `package-bits-stable` | the live session (laid out incrementally across every edit) draws exactly what a session reopened from its package draws, and reads the same state; saving the reopened package declares the same bits |
| `package-bit-missing`, `package-bit-spurious` | bits 14, 15, 16 are set when roles/lists, objects or annotations are live, and only when the scenario could have written them; 18 only when a style op ran; saving a reopened package keeps the bits (`package_bits_follow_the_stored_forms` pins the exact rules) |
| `sync-accepts-replica`, `peers-converge`, `peers-converge-layout`, `merge-order-independent` | a replica accepts a snapshot of another replica of its lineage (only `bindings.limit` may refuse); replicas that exchange all updates reach one vector, one state and one layout; the same saves merged in two orders reach the same state |
| `import-copy-keeps-content`, `import-copy-keeps-annotations`, `import-copy-independent`, `import-copy-leaves-source` | a copy has the same content with new identities, the same annotations that sit in one paragraph and no more than the source's, refuses its source's updates, and leaves the source untouched |
| `paste-keeps-atoms`, `paste-drops-comments`, `paste-keeps-annotations` | a paste adds exactly the objects of the copied range, no comment (they are not copied), and removes nothing |
| `reading-order-ranges`, `outline-valid`, `page-links-are-links`, `annotation-matches-brief`, `diagnostics-for-filters` | the queries agree with `State` |
| `foundations-repeatable`, `export-repeatable` | the whole scenario run twice gives an identical transcript (states, pages, saved packages, exports); exporting twice gives the same bytes |

A refusal is compared by *operations*, not by snapshot bytes: a read, `state()`
included, declares empty root containers in a snapshot (they are not
operations). `package_bits_follow_the_stored_forms` and
`a_refusal_writes_no_operation` pin both facts.

#### Known faults

Faults whose fix is not this branch's are counted, not hidden, so a run goes on
to find new bugs. A `known:` count in the run's report says how often one
masked a difference; a generator exclusion says how often a shape was kept out.

| Id | Shape | Handling |
| --- | --- | --- |
| Astra 4 R1 | a refused batch publishes the commands before it when an annotation's internal link target was deleted, moved or joined away earlier in the transaction | `exclude_known_shapes` turns such a link into a comment (`excluded:astra-4-R1`); `#[ignore = "known: astra-4 R1"]` regressions |
| Astra 4 R3 | a cross-paragraph annotation is cut to its first paragraph on copy and `import_copy` | `import_copy` is compared exactly only for annotations inside one paragraph |
| Astra 4 R4 | an annotation over text next to a retired inline object comes back rebound after undo | counted as `known:astra-4-R4` when only annotations differ and an object was retired in the lineage |
| fuzz-2 `annotation-redo` | an annotation created in the same step as the text it covers is gone after undo then redo | `known:fuzz2-annotation-redo`; ignored regression |
| fuzz-2 `format-undo` | formatted text that was deleted comes back unformatted after undo | `known:fuzz2-format-undo`; ignored regression |
| fuzz-2 `link-resurrected` | a link removed along with its text comes back when a later delete of the same re-typed text is undone | `known:fuzz2-link-resurrected` (a strict superset of annotations after undo); ignored regression |
| fuzz-2 `tombstoned-endpoint` | an annotation endpoint that an earlier delete left on a tombstone is not restored when a later delete covering that point is undone: the comment comes back orphaned at the paragraph's end (also through the move-by-copy of the flow paragraph that holds it), or a link's end and a comment's start at the same offset both land at the end of the restored text | `known:fuzz2-tombstoned-endpoint` (the same annotations with changed spans or status, in a lineage that deleted text while an annotation existed); three ignored regressions |

### The hostile peer

`reprise_doc::hostile::scribble_foundations` (feature `hostile-peer`) writes
seeded raw operations into a fork of the victim; the packet that carries them
is the only way they reach it, as for a remote peer. It is a separate entry
point from `scribble`, so the older seeded run is unchanged. New tree nodes go
under the root, never deep: a 1,024-deep tree overflows the stack in
`list_numbering` and `has_block_semantics` (Astra 4 R2).

| Stored form | Attack |
| --- | --- |
| `inline1` records (`v`, `active`, `kind`, `align`, `p:*`) | wrong types and versions, an inactive or future record, an unknown or empty kind, oversize parameters, extreme sizes; a record for a character that does not exist, for a real host, or under a garbage key; deleting a record or an entry; a bare U+FFFC with no record, or three in a row |
| `annot1` on `ranges` nodes, `annot-restore1` | malformed, oversize (past 16 KiB) and 5,000-deep envelopes; version 0 and 2; an unknown kind; `javascript:` and empty URLs; links to nothing, to a paragraph that is gone, to the annotation itself; restore hints that lie (a count of 2³²−1, a foreign peer); a range with only an envelope |
| `lists` tree (`v`, `deleted`, `base`, `<n>.format/.text/.start/.indent/.hanging/.marker`) | a base that is itself, a paragraph, nothing; version 2; unknown formats; templates of every counter and `%0`; overflowing starts; extreme indents; markers that are garbage JSON; levels past the deepest; physical delete and nesting |
| `role1`, `list1`, `level1`, `restart1` on content nodes and break records | roles from the future, `heading-0`, `heading-7`, `heading-99`; a list that is gone, is a paragraph or is the empty string; levels 9, 255, −1, 2⁶³−1; restart past `i64` |
| `styles1`, `para1`, `styles2` | `state/<name>` as `defined`, `deleted`, `renamed:` to itself, to another, in a cycle; huge, negative and garbage lengths; a patch for a block that does not exist; `clock/`, `label/`, `alias/`, `base/` and `legacy-builtins1` of the wrong type; control-prefixed owner and built-in references (`\u0001style1:…`, `\u0002builtin1:…`, `\u0003…`) that name nothing |
| `document1`, `lineage-index1`, `document1:<id>` | named attacks only for `document1` itself (the attacker cannot frame a packet once its own lineage is unreadable) |
| any root | a root of another type under the name: a text root (and a map root beside a tree) named `inline1`, `breaks1`, `marks1`, `styles2`, `ranges`, `lists` and every other root the victim holds |

`tests/foundations_hostile.rs` imports each packet into a victim that holds
every wave-1 feature (a heading, a two-level list with a field and an image,
links, a comment, a `page-of` cross reference, custom and renamed styles, an
alignment, a flow break, a page setup) and checks: a refused packet changes
nothing; an accepted one lays out, renders in SVG and PNG, exports in four
formats, keeps `State` coherent, reports only documented codes, still takes an
edit with undo and redo, saves, reopens to the same state and pages, and
declares the package bits for whatever is live. The named attacks pin one stored
form each; a failure there reads without a seed.

### Running the foundations

```sh
cargo test -p reprise-fuzz-harness --test foundations --test foundations_hostile \
    --test foundations_regressions        # the bounded target, under a minute in debug
REPRISE_FOUNDATIONS_SEEDS=300 REPRISE_FOUNDATIONS_FIRST=100000 \
    cargo test -p reprise-fuzz-harness --test foundations
REPRISE_FOUNDATIONS_HOSTILE_SEEDS=200 \
    cargo test -p reprise-fuzz-harness --test foundations_hostile
```

To explore, shrink and replay:

```sh
FOUNDATIONS=1 cargo run -p reprise-fuzz-harness --example explore -- FIRST COUNT [BYTES]
FOUNDATIONS=1 TWICE=1 ...            # also run each scenario twice
FOUNDATIONS=1 NO_MINIMIZE=1 ...      # print failures unshrunk
HEX=<bytes> FOUNDATIONS=1 FOUNDATIONS_TRACE=1 cargo run -p reprise-fuzz-harness \
    --example explore                # replay one scenario with a running account
```

The soak (at least thirty minutes, with a timeout) is the explorer over many
seeds on a few processes, each with a disjoint range:

```sh
for first in 100000 200000 300000 400000; do
  FOUNDATIONS=1 TWICE=1 NO_MINIMIZE=1 timeout 2400 \
    target/debug/examples/explore.exe $first 100000 300 > soak-$first.log &
done
```

A debug build is the point: overflow panics are on. `libFuzzer` takes the same
scenario (`cargo +nightly fuzz run foundations`, `foundations_twice` in `fuzz/`).

### Adding a foundations regression

An explicit test in `tests/foundations_regressions.rs` that reads as a short
story through the facade; for a scenario worth keeping as bytes, a `.hex` file in
`corpus/foundations/` (`#` lines say which oracle failed). An open fault gets
`#[ignore = "fuzz-2: <id>, see report"]`; one that belongs to another stream,
`#[ignore = "known: astra-4 R<n>"]`.

## libFuzzer

`fuzz/` is a `cargo-fuzz` package (not a workspace member; the root manifest
excludes it) with two targets over the core harness, and two more (`foundations`,
`foundations_twice`, below) over the wave-1 foundations scenario:

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

### The wave-1 foundations (`hardening/fuzz-2`)

Fixed:

-   `shadow-root` (`reprise-doc`): a Loro root is identified by its name *and* its
    type, so a peer can write a text root named `inline1` (or `breaks1`, `styles2`)
    next to the map the engine uses. A lookup by name alone resolved to whichever
    root a replica had registered last, which depends on how the replica came to
    hold the document, not on the operations: a replica that opened a snapshot hid
    every inline object while the live one still saw them, and `State` text and
    `Block.inline` disagreed. `Document::map_root` now looks the map up by type
    (`flow.rs`, `inline.rs`, `style_identity.rs`); `a_root_of_another_type_under_the_*`
    in `inline_tests.rs` and `style_identity_tests.rs` fail without it, and the
    hostile test's shadow attacks fail without it too.

Open, each with a minimal `#[ignore]`d regression in
`tests/foundations_regressions.rs` and a suggested fix in the final report:

-   `annotation-redo`: a link made in the same undo step as the text it covers
    is gone after undo then redo (so a paste's links too).
-   `format-undo`: deleted bold text comes back unformatted after undo.
-   `link-resurrected`: a link removed with its text comes back when a later
    delete of the re-typed text is undone.
-   `tombstoned-endpoint`: an annotation endpoint left on a tombstone by an
    earlier delete is not restored when a later delete over it is undone.

Pinned as designed (not faults), each with a test or an oracle that knows it:

-   A refusal writes no operation, but a read declares empty roots in a snapshot,
    so "unchanged" is compared by operations.
-   `import_copy` drops a link whose target block is gone, and says so
    (`clipboard.annotation-dropped`).
-   Opening a package authors the base style again if a peer deleted it, so a live
    session and its reopened copy differ for that one hostile document.
-   Bit 14 is set while some paragraph *holds* a role or list key: undoing the only
    role clears it (bits 15 and 16 stay for retained records).
-   `JoinBlocks` named in the wrong order appends the earlier paragraph's text to the
    later one and keeps only the part of a crossing link that stays put; the
    generator joins adjacent paragraphs in order.
-   A peer whose own `document1` is unreadable cannot frame a packet, and a list
    root under a document root's name is refused by sync (`sync.invalid`).
