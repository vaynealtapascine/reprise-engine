# Incremental evaluation and jobs

Decisions 05, 13, 16, 19, 26, 27, 28, 37, 38, 39 and 41 apply.
`Engine::layout` remains the uncached reference implementation, with its frozen
signature, snapshot structure and query semantics. Incremental evaluation is
`reprise_layout::incremental::LayoutSession`.

## Session and configuration lifetime

A session borrows one `Engine` immutably. The borrow includes the font store,
shaping adapter implementation and version, composer implementation **and its
settings**, schema registry, pure-function definitions, medium and flow settings.
These inputs are constant for every memo in that session. Changing configuration
requires dropping the session and constructing another one. An adapter name or
composer name alone is deliberately insufficient as a cache identity. Sessions
cannot be transferred between engines; there is no persistent or global cache.

```rust,ignore
let mut session = LayoutSession::new(&engine);
let snapshot = session.layout(&doc)?;
let counters = session.counters();
let graph = session.graph();
let mut job = session.start(&doc, Viewport::Pages(0..1));
let progress = job.step(64)?;
let view = job.partial()?;
view.publish(&doc)?; // gate immediately before presentation
```

`layout` returns `Result<LayoutSnapshot, JobError>` so even the synchronous
convenience path cannot spin forever or silently return stale/unfinished output.
No authored or Loro state contains caches. Existing document accessors suffice;
this workstream adds no doc accessors and touches no editing-kernel files.

## Exact read keys

Keys compare complete typed values, rather than lossy hashes. Configuration is
included by immutable session lifetime, rather than a caller-assembled fingerprint.

| Unit | Variable inputs captured by its key | Retained output |
| --- | --- | --- |
| Preparation | Block identity/kind, complete UTF-8 text, overrides, bounded named-style chain (including missing/cyclic endpoints), complete starting-frame `ResolutionContext` | Used style, itemisation, bidi, breaks, shaping, empty-line fallback, ordered diagnostics; failures are memoised too |
| Itemisation/shaping | Complete `ParagraphInput` (owned by exhaustive destructuring), resolved fallback face/size, immutable engine fonts/adapter | Shaping-only portion of preparation; reused when context or line height changes without changing shaping inputs |
| Paragraph flow | Preparation inputs, resolved template, incoming page/frame cursor and relevant fills, predecessor identity, complete input region plan | Emitted blocks/lines, diagnostics in original order, new pages/frames, outgoing fills and cursor |
| Annotation | Preparation inputs and resolved template (including its margin-frame context) | Provisional unplaced block/extent and ordered diagnostics |
| Region allocation | Complete flowed snapshot except revision, resolved template, ordered authored relations, actual document target resolutions, owner existence and preparation inputs, semantic tree order | Complete next plan, including diagnostics and applied relation IDs |
| Relations and reading order | The same derived inputs, pending annotations, applied region IDs | Complete final snapshot and ordered diagnostics |

The text's exact content is its **semantic text revision**: returning to identical
text may reuse shaping. A CRDT revision alone cannot express that identity. Anchor
and tombstone changes are captured separately, using the document resolver's
actual outcomes when deriving region/final-pass keys. These include deleted and
rebound ranges, structural matches, succession, snapshot contents and history
availability. Consequently deleting/reinserting identical bytes cannot reuse an
incorrect range or snapshot resolution. A full document revision still tags every
published result and gates every job step.

Preparation is bounded to 32 memoised contexts and 32 shaping inputs per node, flow to 16 transitions per
node, annotation to one per node, allocation to 16 complete plans, and the final
pass to one complete memo. Eviction only affects work, never layout. The session
retains node maps until dropped, including deleted identities; hosts should drop
long-lived sessions when their cache retention is no longer useful.

## Why reuse preserves the reference result

1. A memoised preparation reads only captured authored inputs, its context, and
   the immutable engine. The full preparation routine produces the memo, including
   failure diagnostics. Exact equality substitutes the same pure computation.
2. A paragraph is a transition from an incoming flow state. Its key captures
   every state field it can read: template, current cursor/fills and region plan.
   Earlier pages' fills cannot be read after the monotone cursor has left them.
   Reuse appends exactly the emitted pages, frames, lines and diagnostics and
   restores the outgoing state. Induction over document order therefore gives
   identical flow, including fragmentation, skipped frames and the page limit.
3. Allocation and final-pass keys capture complete upstream layout and the
   document resolver's actual outcomes, not a guessed list of mutable CRDT fields.
   Their computations are the reference routines. An equal key therefore gives
   an equal plan or complete final result.
4. Reference and job paths share the feedback decision and freeze routines.
   Induction over the at-most-16 iterations gives the same plans, convergence,
   oscillation/exhaustion fallback and diagnostic order.
5. Reference relations now internally separate relation placement from reading
   order. Their public `run` still executes those phases consecutively, with
   reading diagnostics before leftover-annotation diagnostics. The job executes
   them across yield points in that same order.
6. Only the revision field is retagged when an equal final-pass key is reused.
   The key omits revision because all relevant version-dependent answers were
   captured. Every result is then checked against the live document frontier.

This is a substitution/transition-induction argument, not a machine-checked
formal proof. `fixtures/tests/incremental.rs` checks complete `PartialEq` equality
of snapshots, including diagnostics and their order, after every seeded edit for
all hostile fixtures and the spike. Existing JSON goldens independently pin the
uncached reference output.

## Invalidation and counters

Flow starts at the beginning to validate incoming state, reuses unaffected
transitions, and recomposes from the first changed transition until state matches
again. A one-line edit preserving paragraph depth can immediately converge:
the 1,000-paragraph test shapes and recomposes one paragraph and reuses 999.
A depth change propagates through subsequent cursor states and pagination. Region
plan keys deliberately invalidate flow conservatively when allocations change.
`follow` overlap tracking runs in relation-ID order, so the final relation pass is
one memoised unit; a relation edit invalidates that pass without reshaping or
recomposing unrelated body text. Reading order is likewise one staged pass.

`WorkCounters` exposes charged job units, style-resolution calls, itemisation
calls, whole-paragraph shaping calls, paragraph-transition misses/reuses, the ordered set of pages touched by those misses
(including old and new placement), actual
composer calls (including table/note/float composition), allocation executions,
and relation/reading executions. They count work, not wall-clock time. Key
capture/comparison, input enumeration, output cloning and graph assembly are
metadata work; the counters do not pretend those costs are zero. A style absent
from every relevant chain leaves preparation and flow intact and can reuse the
region/final passes too. The tests assert zero composer and final-pass calls for
unused-style edits, including documents with annotations.

Table transitions currently rerun table allocation/fragmentation; cell preparation
is reused. This is conservative and equivalent, but finer row/column memos are a
follow-up. Region and final-pass keys copy complete inputs and use coarse memos;
compact interned keys would reduce memory and metadata comparison cost.

## Scheduling and partial semantics

`Viewport::Pages` uses zero-based page numbers. `Viewport::Rect` requests its whole
containing page, conservatively covering the requested page-space rectangle even
under transformed frames. Empty/reversed page ranges demand no pages. Outside
viewports finish the document and advertise `outside_document`; extreme page
indices saturate rather than overflow.

A `LayoutJob` owns its cursor, the block in progress, per-pass snapshot and
region history. A step uses at most its explicit unit budget; the units are
listed under [Work units and the budget](#work-units-and-the-budget). A unit is
a bounded amount of counted work, not a promise about elapsed time. Startup
enumerates the content tree; viewport-first here bounds expensive layout units,
not the cost of reading authored topology.

On first reaching demand, `step` yields even if budget remains. Later steps spend
budget on the rest. Pagination requires a prefix; for a distant viewport that
prefix is a real dependency, so it is evaluated first. The 100,000-paragraph test
covers the first page within 64 units without shaping the remaining paragraphs.

A partial wrapper contains only pages the flow cursor has passed (or all pages
when that pass has finished), preserving frame addresses. A block crossing the
coverage boundary contains only its covered lines, while retaining its source
text. That includes a paragraph that is still being composed: its lines on
passed pages appear, so a paragraph longer than the viewport does not hide the
first page until it ends. `Coverage` explicitly states the pass, page set, completion and `settled`.
Body geometry in a relation-free document is settled. During region feedback or
before relation placement, fresh body geometry is **provisional**: later passes
may move it or add notes/annotations. Coverage describes the producing pass; it
never claims all later-stage content has appeared. Partial views omit the final
relation results and diagnostics until complete. Authoritative presentation must
require `settled`, while an editor can display an explicitly provisional view.
`complete()` returns a `LayoutSnapshot` only after every stage has finished.

Cancellation is terminal. A shared terminal token also rejects publication of
already-yielded wrappers after cancellation. Every step, `partial`, `complete`
and `publish` commits pending authored operations and compares the live frontier.
A concurrent peer merge or an uncommitted local edit makes the job stale. Stale
jobs reject output and clear the session's memos, because an edit could have
raced a key capture and a unit's reads. Partial wrappers borrow the original
document, use immutable revision/coverage getters, and check its identity as
well as revision; an unrelated document with the same fixture revision cannot
pass the publication gate. Cancellation/staleness are scheduling errors, not
layout diagnostics: they do not describe authored content being omitted.

No OS APIs or clocks are needed, and threads are never needed for correctness.
The job's coordinator is single-threaded on native and WASM; native hosts may
lend it [workers](#native-workers) for pure preparation tasks.

## Work units and the budget

`step(budget)` charges one unit for each entry below and stops when the budget
is spent, the job completes, or demand is first reached. Charging depends only
on the document, the memo state and the sequence of budgets, never on the
number of worker threads or how they are scheduled. `C` is
`SHAPE_CHUNK_BYTES` (16 KiB), `L` is `PREFETCH_BLOCKS` (8) and `R` is
`TABLE_ROW_GROUP` (16).

| Unit | Work it may do |
| --- | --- |
| Block start | Flow-memo check; on a hit, replay the whole stored transition. A block whose text is at most `C` bytes is prepared in this unit (style, itemisation, shaping, breaks) and its first composer call runs here too. Annotations: preparation plus their single composer call. Malformed/table-invalid blocks: their diagnostic. |
| Paragraph fragment | One composer call (one frame or skipped frame) and the conversion of the lines it returns |
| Long preparation, start | Style resolution and itemisation of a paragraph longer than `C` |
| Shaping chunk | One adapter call over at most `C` bytes (or one grapheme cluster longer than `C`) |
| Long preparation, end | UAX #14 break analysis and assembly of the shaped runs |
| Table start | Column validation and listing the rows |
| Table row group | Preparing the cells of at most `R` rows, ending the group early after the row in which `C` bytes of cell text have been prepared |
| Table columns | The column solver and fitting images to the solved widths |
| Table row | Placing one row, through every frame it fragments across |
| Flow end | Blank pages that the region plan requires |
| Region allocation start / end | Plan memo check (a hit completes it); finishing notes and the page count |
| Region placement | Resolving and placing one float or note relation |
| Feedback decision | Comparing the next plan with the history; resetting the flow |
| Relation pass start | Final-pass memo check (a hit completes the job) |
| Relation | Resolving and applying one relation, in relation-ID order |
| Reading order | The reading-order report and leftover-annotation diagnostics |

Speculative preparation ([below](#native-workers)) may additionally prepare up
to `L - 1` following short paragraphs inside a block-start unit: those
preparations belong to block-start units the job expects to run next, and a
later block start that finds them memoised does no shaping. A step therefore
does at most `budget + L - 1` units' worth of work. `LayoutJob::step_work`
reports what the last step actually did, as counters (adapter calls and bytes,
composer calls and lines, table rows, relations, linear scan bytes), and the
`step_work_is_bounded` tests assert these bounds for a 100,000-word paragraph
and a 5,000-row table.

Some work inside one unit is linear in a long paragraph and cannot be split
without changing a frozen contract, so it is listed rather than hidden:

-   Itemisation (bidi resolution is defined over the whole paragraph) and
    UAX #14 break analysis are each one call over the whole text. They are two
    separate units, each a single linear scan.
-   The `Composer` contract takes the whole shaped paragraph. The built-in
    composers' preprocessing is linear in the paragraph on every call, so a
    paragraph fragment unit costs one linear scan plus its own lines. A
    windowed or resumable composer entry point would remove this; see the
    hand-off notes.
-   A paragraph needs all of its chunks shaped before its first line can be
    composed, because composers read the whole `ShapedText`. A very long first
    paragraph delays the first page by its shaping units, which workers run in
    parallel.
-   A line taller than every frame is composed with unbounded geometry, so
    that one composer call places all remaining lines of the paragraph. An
    annotation is composed at a `Measure` with no region end: one call. A table
    row fragmenting across many frames, and a note continuing across pages, are
    one unit each. All of these are already bounded by the page limit.

Memo granularity stays at the paragraph. A memo for a fragment would need a key
for the composer's state in the middle of a paragraph, and the `Composer`
contract exposes no such state: only `start` and the geometry. The flow memo key
is still captured at block start and the transition recorded at block end, so a
block in progress is never memoised and a hit replays a whole block.

## Chunked shaping (a reference rule)

`Shaper::shape` makes one adapter call per item. The `ShapingAdapter` contract
says nothing about locality, so splitting one call into two cannot be proven
equal to the whole call for an arbitrary adapter. Bounded shaping work therefore
had to become part of the reference rule, applied by `Engine::layout` and the
job alike, rather than an optimisation the job alone performs.

Every item longer than `C` bytes is shaped in sub-ranges. Seams are a pure
function of the paragraph text and the item range. From the start of the item
(itself an item boundary), the next seam is the last grapheme boundary within
`C` bytes that directly follows a U+0020 SPACE. If there is none, it is the last
grapheme boundary within `C` bytes. If not even that exists, because one
grapheme cluster is longer than `C`, the seam is the end of that cluster.
Every request keeps the whole paragraph as its shaping context, exactly like
`Shaper::shape`, so contextual joining sees across a seam. Only lookups that
would match glyphs on both sides of a seam (kerning or a ligature with the
space) are lost there. A seam is always a grapheme boundary, and almost always a
UAX #14 break opportunity, where a line could end anyway. The sub-runs of one
item are merged back into one `ShapedRun` in logical order, reversed for
right-to-left items, so the run structure is unchanged. `Prepared::items` keeps
the original items, so composers reshape lines exactly as before.

An item of at most `C` bytes gets the very same single request as before, so
no existing golden changes. Text over 16 KiB in one item is roughly a
five-page paragraph in one font and direction.

## Native workers

`reprise_layout::workers::Workers` runs a batch of independent tasks and
returns after all of them have finished. `Serial` runs them in order and is
what `Engine::layout` and WASM use. `Threads(n)` (native only, behind
`cfg(not(target_arch = "wasm32"))`) runs them on `n` threads with scoped
`std::thread`, the calling thread included. Threads take the next task from a
shared queue, so completion order is not fixed. The `reprise` facade enables it
through its default `native-workers` feature, and
`DocumentSession::set_layout_threads` chooses the count.

Two kinds of task run on workers. Neither touches the document, the session's
memos or any `RefCell`:

-   **Shaping chunks** of a long paragraph: one adapter request each.
-   **Short-paragraph preparation**: itemising, shaping and break analysis of
    one paragraph of at most `C` bytes, from an owned `ParagraphInput`.

Style resolution reads the document, so it stays on the coordinator. When a
block start must prepare a short paragraph that has never been prepared with
its current inputs, the coordinator also looks at up to `L - 1` following
blocks. Each of them is a plain paragraph of at most `C` bytes, and has no
preparation memo with equal inputs in any context. For these it resolves style
with the *predicted* context (that of the frame the cursor is in now), in block
order. It then runs all their preparations as one batch, and inserts the results
in block order. Which blocks join the batch depends only on the document, the
cursor and the memo state.

Scoped threads were chosen over `rayon` because they need no dependency or
global pool. The batch's lifetime is visibly the step's, and work splitting
stays in this crate's hands. The cost is a thread spawn per batch, which is
small against shaping a batch.

## Why finer steps preserve the reference result

1. **One routine, more suspension points.** `Engine::layout` and the job run the
   same resumable machines: the paragraph machine, staged preparation, the
   table machine, region allocation and the relation pass. The reference drives
   each one to completion in a loop. A machine's owned state holds every value
   of the former monolithic routine that is live across a unit boundary: for a
   paragraph that is the resolution context, preparation notes, `Prepared`,
   lines so far, the byte to continue from, the stall/forced/overflowed flags
   and the memo mark; for a table, its rows, column extremes and block count.
   Everything else the routine reads is the job's own snapshot, cursor, plan and
   template, which only the machine mutates, and the immutable engine.
2. **Nothing else writes in between.** Between units the job runs no other
   machine (one block is in progress at a time). `partial()` clones; it builds
   the in-progress block from a copy of its lines. The authored document is
   compared against the job's revision at the start and end of every step, and
   at every `partial`, `complete` and `publish`. An edit between units makes the
   job stale before any further unit can read it, and clears the memos.
   Consequently the sequence of state transitions is the same with or without
   yields, and induction over units gives the same snapshot, diagnostics and
   order.
3. **Memo boundaries are unchanged.** A flow key is captured when a block
   starts and its transition is recorded when it ends. In between, only that
   block appends to the snapshot, so the recorded delta is exactly what a
   monolithic block would have produced. Region-plan and final-pass keys are
   captured before their first placement unit, from the same inputs as before.
4. **Chunked shaping is the same computation on both paths.** The seams, and so
   the adapter requests, are a pure function of the text and the items. Both
   paths issue the same requests, the adapter is a pure function of its request
   (22, 38), and the merge orders sub-runs by seam, not by completion. Items of
   at most `C` bytes are shaped by the identical single request as before.
5. **Speculation cannot change output.** A preparation memo stores, for a key
   (complete inputs, context), the value of the same staged routine on those
   inputs. A predicted context only decides *which* keys get entries early. An
   entry for a key that is never asked for is unused, and an asked-for key
   with no entry is computed as before. Correctness never depends on the
   prediction being right. Inputs are read on the coordinator at batch time,
   inside a step, where the revision gate covers them.
6. **Termination.** Every unit advances a lexicographic measure: block index,
   then machine phase, then frame, chunk, row or relation index. The existing
   page limit, stall bound, table limits and relation limits bound each
   component, so a job has finitely many units. A step with a positive budget
   runs at least one unit.

## Why parallel preparation preserves the reference result

1. **Only pure tasks cross threads.** A task borrows an immutable font store,
   the adapter (`Send + Sync` and pure by contract) and text, and owns
   everything else. `Document` and the session's `RefCell` memos cannot be
   captured, because they are not `Sync`, and the compiler enforces it. A face's
   adapter-data slot is a `OnceLock`: whichever thread fills it first, it holds
   data derived only from the face, and caches may not change output.
2. **Results are placed by index.** Task `i` writes only slot `i`. The executor
   returns after every task has finished (scoped threads join before `run`
   returns). The coordinator then reads the slots in index order. The result
   vector is a function of the task list alone, whatever the thread count,
   interleaving or completion order.
3. **All bookkeeping is serial.** Memo insertion, counters, the dependency graph
   and diagnostic order happen on the coordinator, in block or chunk order,
   after the batch.
4. **The batches themselves are thread-independent.** Batch membership and unit
   charging read only the document, cursor, budget and memo state. With any
   thread count the job therefore yields at the same points, with the same
   partial views and counters, not merely the same final snapshot.
5. **Revision gating is unchanged.** No worker outlives the step that started
   it, so no result can arrive after a later edit. A stored value is a pure
   function of its captured key. A stale step still clears every memo.
6. **No threads are needed for correctness.** `Serial` is a valid `Workers` and
   is the WASM and reference executor. A panic in a task would propagate from
   the scope exactly as it would serially; tasks are adapter calls, which never
   panic by contract.

This is again a substitution and induction argument, not a machine-checked
proof. The tests check exact `PartialEq` equality with `Engine::layout`, under
random budgets including 1, and for `Serial`, `Threads(1)`, `Threads(2)`,
`Threads(n)` and a seeded executor that runs tasks in a shuffled order. They
also check that partial views and counters after every step are identical
across executors.

### Attempts to break it

-   *Edit between two fragments of one paragraph.* The next step fails `current()`
    before running a unit, and the memo is cleared, so neither a fragment of the
    old text nor a delta mixing two revisions survives.
-   *Seam inside a grapheme, or not on a character boundary.* Seams are taken
    only from the paragraph's grapheme boundaries. An oversized cluster is
    shaped whole. Item ranges that are not on character boundaries are shaped
    exactly as `Shaper::shape` would shape them, because short items keep the
    original request.
-   *Right-to-left items.* Glyphs come back in visual order, so a right-to-left
    item's sub-runs are concatenated last seam first. The tests include RTL
    text longer than `C`.
-   *Missing face.* `Shaper::shape` drops an item whose face is missing; chunked
    shaping drops the whole item too, before issuing any request.
-   *Wrong prediction.* Two columns with differently named frames give a
    different context in the next column: the speculative entry is never asked
    for and the flow prepares normally. The thousand-paragraph test still makes
    exactly one style resolution after an edit, because blocks with equal
    inputs in any memoised context are not speculated on.
-   *Budget 1, `usize::MAX`, zero threads, more threads than tasks.* Each step of
    budget 1 runs exactly one unit; `Threads(0)` behaves as one thread; idle
    threads find the queue empty and exit.
-   *Suspend and resume through the facade.* The machine in progress is owned
    state inside `LayoutContinuation`, so it survives being moved between steps
    and sessions with the same revision gate as before.
-   *Viewport inside a long paragraph.* Coverage advances with the cursor's page
    while the paragraph is still being composed, and the partial view includes
    its lines on passed pages.

## Staged cycles and bounds

| Cycle class | Bound and reference fallback |
| --- | --- |
| Flow -> floats/notes -> exclusions/reservations -> flow | 16 complete iterations; repeat-plan detection freezes the actual input plan with `layout.region-cycle` (Warning), exhaustion with `layout.region-limit` (Warning) |
| Note-on-note anchors | 32 rounds and 32 levels; `layout.note-depth` (Error) omits dependent owners |
| Layout-query relation moves its queried content | Region behaviours enter the bounded feedback class above. `follow` is one forward staged pass, not an iterative equation; a self/missing source follows the existing resolver/fallback diagnostics |
| Follow overlap tracking | One finite traversal in relation-ID order; later placements read earlier placements, so the entire relation pass is the memo boundary |
| Reading constraints | Finite iterative topological traversal, one removed node per iteration; at most 4096 edges, same conflict/cycle/missing/limit diagnostics |
| Pagination | `max_pages.max(1)`; `layout.page-limit` and `layout.text-unplaced` (Error) |
| Solver and table/region fanout | Existing 256 solver iterations/variables, 4096 region relations/rows, 256 columns, 65536 table blocks; existing diagnosed omission/fallback rules |
| Computed-graph inspection | Iterative traversal; visited sets admit each finite unit/dependency once, including cycles |

No new diagnostic code or frozen contract change is needed.

## Reverse inspection

`Dependency` unifies expression dependencies and relation dependencies. Shared
node/range/tree inputs map to common variants, with explicit layout-of-node,
layout-of-range and history inputs. Expression dependencies retain their typed
vocabulary. `Computation` names style, shape, paragraph and block/frame composition, relation,
region, reading, page and line units. Graphs include entering-flow predecessor
edges, range-to-layout edges and relation-owned line/page edges. Dependencies are
conservative: an earlier paragraph can influence later pages even when its
current edit converged immediately.

`direct`, `dependencies`, `dependents` and `why_recomputed` return ordered data.
Reverse lookup builds reverse edges and traverses once, rather than repeatedly
expanding every unit's transitive dependencies. Recompute reasons name text,
style, predecessor, template, region or final-pass inputs that changed against a
memo; initial work is identified by its inputs. Final-pass reasons are coarser
than individual relation target caches because that pass is one memo boundary.
Graphs reset each job so deleted blocks/relations/pages do not remain current.
There is no debug-overlay modification in this workstream.

## Salsa evaluation (decision 41)

The hand-written graph is chosen. Salsa's [manifest](https://github.com/salsa-rs/salsa/blob/master/Cargo.toml)
uses `Apache-2.0 OR MIT`, compatible with this project. Its query model assumes
pure deterministic computations, which fits preparation well. Its default
features include optional parallel infrastructure; a WASM integration would need
a selected feature set and an explicit target check. No claim that Salsa is
incompatible with WASM is made; this branch does not add or compile Salsa.

The deciding mismatch is [cycle recovery](https://salsa-rs.github.io/salsa/cycles.html):
fixed-point recovery expects monotone values in a bounded partial order. Float and
note allocations can oscillate and must freeze the *input* plan that composed the
published body. Changing that to monotone widening would change reference output.
An explicit coordinator is therefore still necessary. The existing small number
of staged operations makes exact-value memos and visible work-budget continuation
more direct than adding a query runtime. The chosen implementation's WASM support
is verified by the project's required target check. There are no new dependencies.

## Verified hand-off (2026-10-04)

On `ws/incremental`, `cargo fmt --all`, strict workspace/all-target Clippy,
`cargo test --workspace`, and the required layout/display/fixtures WASM check all
pass. The final workspace run has **431 passing tests, zero failures and one
existing ignored manual geometry-export test**. Fourteen incremental integration
tests and three incremental unit tests are included. The 55 hostile fixtures and
the spike each receive 32 seeded edits; completed budgeted jobs also match every
hostile fixture's full reference snapshot. The 1,000-paragraph edit test records
one paragraph transition miss, one touched page and 999 reused transitions.

Only two snapshots are added: `hostile__incremental_page_seam.snap` records three
pages/nine frames/two body blocks plus an annotation, valid follow/reading
relations and the requested reading-partial Info diagnostic; its paired
`hostile__content_incremental_page_seam.snap` records 37 glyph items across those
pages. No existing snapshot changes. No dependencies, diagnostic codes, frozen
contract changes, editing-kernel changes or out-of-scope files are introduced.

Follow-ups for the orchestrator: finer yield points within paragraphs/tables and
relation/reading passes; interned or compact keys; retention/eviction for deleted
node maps in very long sessions; narrower table/region/relation memos. Style
context memos are distinct internally while public graph style/shape units
aggregate a block's contexts; block/frame and line/page units are explicit.
Provisional region/relation viewport views must remain visibly provisional until
`settled`; they are not a cached full result. Workers and a debug overlay are
optional later additions. Split/join equivalence edits use existing authored
text/node/succession primitives, without depending on the parallel editing kernel.

Command tails from the final verified implementation:

`cargo fmt --all`: no output, exit code 0.

`cargo clippy --workspace --all-targets -- -D warnings`:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.53s
```

`cargo test --workspace`:

```text
   Doc-tests reprise_text

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo check --target wasm32-unknown-unknown -p reprise-layout -p reprise-display -p reprise-fixtures`:

```text
    Checking reprise-layout v0.0.0 (F:\reprise-wt\incremental\crates\layout)
    Checking reprise-fixtures v0.0.0 (F:\reprise-wt\incremental\crates\fixtures)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.75s
```
