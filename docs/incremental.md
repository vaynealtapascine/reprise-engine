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

Preparation is bounded to 32 memoised contexts per node, flow to 16 transitions per
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
calls, whole-paragraph shaping calls, paragraph-transition misses/reuses, actual
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

A `LayoutJob` owns its cursor, per-pass snapshot and region history. A unit is one
top-level flow block (a table is one block), finishing a flow pass, a bounded
complete region allocation, a relation pass, or a reading-order pass. A step uses
at most its explicit unit budget. A paragraph can span several frames/pages
inside one unit; a unit is not a fixed character count or a promise about elapsed
time. Future finer-grained continuation could yield within a very large paragraph,
table or relation pass. Startup enumerates the content tree; viewport-first here
bounds expensive layout units, not the cost of reading authored topology.

On first reaching demand, `step` yields even if budget remains. Later steps spend
budget on the rest. Pagination requires a prefix; for a distant viewport that
prefix is a real dependency, so it is evaluated first. The 100,000-paragraph test
covers the first page within 64 units without shaping the remaining paragraphs.

A partial wrapper contains only pages the flow cursor has passed (or all pages
when that pass has finished), preserving frame addresses. A block crossing the
coverage boundary contains only its covered lines, while retaining its source
text. `Coverage` explicitly states the pass, page set, completion and `settled`.
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

No OS APIs, clocks or threads are needed. The job is single-threaded on native
and WASM. Native workers are intentionally deferred.

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
vocabulary. `Computation` names style, shape, paragraph composition, relation,
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
