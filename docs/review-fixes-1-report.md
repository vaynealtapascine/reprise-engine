# Review fixes 1 handoff

Worktree: `F:/reprise-wt/eng-fixes1`; branch: `ws/review-fixes-1`; base: `f13a4ee`; integrated engine main: `4131ec8`.
All Part A and Part B findings are implemented. The resumed orchestrator brief
selected completing the identity/compatibility migration, superseding the earlier
redesign stop. Design was committed before implementation.

## 1. Summary by decision

- Decisions 29/37: refused composite edits and selection replacement execute
  implicit capacity effects on an isolated same-peer replica before publication.
  Basic malformed input and ancestry checks run first. Refusals preserve bytes,
  retained history, revision and undo/redo; late errors retain their command index.
  Reauthoring malformed imported style properties during rename follows that
  boundary too, while permitting a preceding repair in the same batch.
- Decisions 07/29: fresh documents and independent copies carry durable document
  identity. Source snapshots, deltas and legacy updates cannot merge into a copy
  by rewriting only the outer packet ID, including after reopen/transitive copy.
- Decisions 37/38: the existing 33554432-byte cap covers total sync wire bytes;
  WASM checks typed-buffer size before copying. Invalid font-setting keys cannot
  mutate settings or stale an existing job. Explicit session font declarations
  survive workspace refresh.
- Decisions 19/37: missing anchor diagnostics reach mark explanations and filtered
  queries. Shared integer paragraph measures honor logical indents in table cells,
  notes and floats. Unsupported context properties produce a warning.
- Decisions 05/07: the Part B addendum specifies durable style identity, whole
  authored graph duplication and required compatibility signaling. It records
  the selected legacy fallback policy and immutable owners. Rename changes only
  labels/selectors; concurrent property patches keep their original owner. Built-in
  references are tagged, and unaware package/sync readers refuse. Duplication
  transfers raw roots, aliases, tombstones and built-in bases in fresh history.

## 2. Commits

```text
e98c700 fix(identity): retain durable binding without shifting content IDs
f6e0805 docs(identity): preserve content counters during durable binding
86d9830 test(edit): retain redo after list and inline matrix setup
2421b8c style(doc): use the identity map emptiness predicate
66a0e06 fix(edit): preflight list and inline writes without read mutations
e328162 merge(engine): integrate lists and inline before refusal coverage
3f134a8 fix(styles): checkpoint bounded root cache and historical joins
e24d4a5 docs(styles): retain historical context across replica joins
af2660c fix(styles): preserve selectors and allocation across copied history
e82e792 docs(styles): guard owner allocation after fresh-history copying
15e4b1a fix(styles): retain immutable owners and signal built-in semantics
4648aad docs(styles): select stable owners and versioned reference migration
f49eb9d test(styles): retain deferred migration acceptance probes
99ce2d2 fix(styles): duplicate the complete authored graph in fresh history
2e24dc2 docs(styles): isolate authored graph duplication from migration
4cec4a5 fix(sync): scope historical packages without authored migration
1e5ed21 fix(edit): preflight captured malformed style values
a4da353 perf(doc): avoid sibling indexing during edit preflight
dc307c8 docs(styles): specify durable identity migration and unresolved compatibility
bda0d74 fix(engine): preflight refused edits and bind independent copies
```

Every commit has the requested GPT-6.1-Sol coauthor trailer. Nothing was pushed.
The final verification/report commit follows this recorded checkpoint.

## 3. Files touched

Paths below are relative to the worktree. No app files or `PROGRESS.md` changed.
The brief identifies affected subsystems rather than a closed file allowlist.
Supporting edits beyond its explicitly named files are listed in full here.

- `crates/doc/src/{identity.rs,style_identity.rs,style_identity_tests.rs,lib.rs,style/store.rs,style_graph.rs,sync.rs,sync_tests.rs}`:
  durable binding, pure lookup guards, edit staging and packet boundaries.
- `crates/edit/src/{editor.rs,error.rs,paste.rs,plan.rs,replace.rs}` and
  `crates/edit/tests/{refusal_atomicity.rs,style_graph_copy.rs,transactions.rs}`: publication and refusal coverage.
- `crates/font/src/lib.rs`: additive registered-face iterator for provenance.
- `crates/layout/src/{codes.rs,flow.rs,flow/keeps.rs,table_flow.rs,notes.rs,floats.rs,relations/alignment.rs}`:
  shared geometry and context/anchor diagnostics.
- `crates/fixtures/src/hostile.rs`, `crates/fixtures/tests/{hostile.rs,paragraph_styles.rs,regions_geometry.rs,incremental_steps.rs}`,
  and the two new foreign-copy snapshots listed below.
- `crates/reprise/src/{lib.rs,session.rs,session/collab.rs}`;
  `crates/reprise/tests/{collab.rs,hostile_peer.rs,import_copy.rs,packet_bound.rs,plumbing_edit.rs,plumbing_marks.rs,workspace_fonts.rs,sync_lineage_delta.hex}`.
- `crates/reprise-wasm/src/lib.rs`, `crates/reprise-wasm/ts/{smoke.cjs,reprise_wasm.d.ts}`.
- `docs/{CODEMAP.md,contracts.md,collaboration.md,plumbing.md,paragraph-styles.md,review-fixes-1-report.md}`.
- Supporting `doc/src/{changes,edit,flow,formatting,fragment,lifecycle,marks,persist,style,table}.rs`,
  `layout/src/incremental.rs`, `format/src/{container,features}.rs`, and `format/SPEC.md`
  propagate stable references and compatibility flags. Additional supporting
files are `doc/src/{identity,hostile,lists}.rs` and
`reprise/tests/legacy_open_concurrent.rs`, for counter-free lineage, legacy
fixture encoding, read-only list queries and framed historical convergence.
- `docs/style-migration-probes.rs` was removed after promoting its acceptance
  coverage into ordinary Cargo tests.

This fix stream adds no dependencies or DTO shapes and changes neither API
version nor PacketBound. The requested main merge supplies list/inline DTOs.
The required package mask now includes the brief-authorized bit 18. Generated WASM declarations are rebuilt after integrating the list/inline DTOs from main.
The delegated WASM probe kept `.r3-repro/` artifacts in this worktree and added
that exact ignore rule to the shared Git metadata's `.git/info/exclude`; this is
the only reported metadata edit outside the source worktree.

## 4. Design choices and narrowed behavior

The small planner still handles completely modeled effects without copying the
document, including ordinary typing. Paste, replacement and structural formatting
transfers, and renames capturing invalid imported properties, additionally
execute the actual writes in isolation and replay successful
plans locally, preserving undo origin and deterministic ID allocation. Trial
history is never merged into the live editor.

Loro's fork implicitly commits pending raw writes. Composite staging therefore
refuses an uncommitted native store rather than publishing those writes during a
failed edit. Native callers mutating `Editor::document()` directly must commit
first; facade sessions already maintain that boundary. Staging checks physical
ancestry of every root tree, including tombstones, at depth 64 before
Loro's recursive codec; deeper trees produce `edit.limit`. Ordinary model-only
editing retains its previous ancestry bound. The final parent scan avoids sibling
index computation, which was quadratic for a wide formatting tree.

Copy isolation compares durable identity before merging either content or history.
It does not require an ancestor list: any foreign lineage is refused. Old unmarked
documents remain unmarked when read, but facade sessions attach the existing
durable package ID as immutable runtime sync context. This closes the historical
unmarked-copy gap without authored migration. Framed sync carries this ID;
unmarked legacy binary histories are refused because their source is unverifiable.
Raw unscoped core documents retain legacy interoperability. Host-authenticated room routing is still
needed against an attacker rewriting the entire payload; lineage is not a signature.

Session faces have explicit local provenance, including restored pins/imported
resources. Workspace refresh only replaces workspace-owned faces. A same-scope
redeclaration replaces descriptors; an identical workspace face is deduplicated.

Cells support spacing collapse and shared indents. Notes/floats honor indents but
warn about unsupported spacing/keeps. Auxiliary properties are bounded and
diagnosed instead of silently borrowing body pagination policy.

Legacy unmarked built-in names cannot distinguish old intent from a writer that
omitted its opt-in. The selected policy retains legacy fallback, while existing
marked paragraph-style documents retain their built-in interpretation. Current
authoring tags references and declares bit 18 through retained `styles2` root
presence, without an extra CRDT operation or changes to pinned node IDs/revisions.
Old marked adoption pins `legacy-builtins1`; a renamed/patched built-in pins its
version-1 base. Rename never relocates property keys, so queued legacy property
patches still merge. Labels resolve through a derived per-revision index. Reused
names allocate a fresh peer/counter owner; old block and parent edges retain theirs.

The independent graph-copy correction transfers all three style roots and legacy interpretation without
resolving or rewriting their values. Capture is bounded at 65536 keys/entries,
16 MiB of keys/values and literal depth 64; unsupported CRDT containers refuse
explicitly instead of losing authored data. Graph installation and content paste
are preflighted together against an empty target. Partial clipboard paste keeps
its appearance projection; duplication skips its style materialisation phase.

## 5. Public API and diagnostics

Additive native APIs: `Document::{document_lineage,bind_document_lineage,set_sync_scope,
fork_for_edit,check_edit_preflight}`, `sync::packet_lineage`,
`Features::DOCUMENT_LINEAGE`, and `FontStore::registered_ids`.
Style additions: `Document::{style_identity,style_reference,has_style_identities}`,
`Features::{STYLE_IDENTITIES,LEGACY_BUILTINS}`, and `REQUIRED_STYLE_IDENTITIES` (bit 18).
Graph-copy additions: opaque `AuthoredStyleGraph`,
`Document::{authored_style_graph,restore_authored_style_graph}`, and
`Editor::paste_document_fragment`. These add no DTO; their new stored semantics are declared separately below.
New typed refusals are `DocError::{UncommittedEdit,EditPreflightDepth}` and
`Reason::{UncommittedEdit,PreflightDepth}`. They use existing `edit.invalid-command`
and `edit.limit` Error codes: a refused edit is omitted entirely.

New `layout.paragraph-context` is Warning because requested spacing/keep policy
does not apply in that composition context. Existing `relation.missing-target`
and `layout.indent-clamped` codes are reused, not renamed.

## 6. Snapshots

All existing layout/content snapshots and `sync_delta.hex` remain unchanged.
Two new fixture goldens were recorded and read:

- `hostile__foreign_copy_snapshot.snap`: complete geometry/report for a receiver
  that refuses another durable document's snapshot; original receiver text remains.
- `hostile__content_foreign_copy_snapshot.snap`: content-only display for the same
  refused merge. This branch adds one fixture; after the requested main integration the count is 108.

`sync_lineage_delta.hex` is a separate new wire golden for the scoped feature;
native and WASM tests pin it without rewriting the unmarked legacy golden.

## 7. Dependencies

None added; no license changes.

## 8. Contract and stored-form changes, separately

1. Durable metadata: fresh facade documents/copies author lowercase 32-hex
   an empty `document1:<32-hex-ID>` root (the earlier `document1.id` form remains readable). Package ID agreement is checked on reopen. Reads do not add it.
   The root is opaque preserved Loro metadata, with no rendering semantics, so no
   package rendering bit was allocated.
2. Sync format-2 extension: required sync bit 3, `document-lineage1`, prefixes the
   JSON body with 32 ASCII hex bytes. Existing fixed framing stays unchanged;
   unaware sync readers refuse the required bit. Legacy binary imports compare
   stored lineage before merge too.
3. Total transport ceiling: `MAX_PACKET_BYTES`/`PacketBound` stay 33554432; the
   ceiling includes vectors, fixed frame and optional prefix on all facade sync
   imports/exports. No extra 1 MiB frame allowance or 64 MiB legacy allowance.
4. Edit staging preconditions: committed native state and physical tree depth 64
   for isolated preflight, with typed refusal and no publication, as detailed above.
5. Font precedence: explicit session declarations/imported/restored faces take
   precedence over the workspace refresh at the same FaceId.
6. Stable style owners: existing nested/flat keys remain owners; `styles2` adds
   `alias/<name>`, `label/<owner>`, `base/<owner>`, `clock/<peer>` and `legacy-builtins1`. New block
   and parent references use reserved versioned control prefixes. Name reuse
   allocates fresh owners; labels use deterministic disambiguation.
7. Required package bit 18 and required sync bit 4 (`style-identities1`) declare
   those semantics, including an empty root retained after undo. Unknown readers
   refuse. Tagged-reference/root sync operations without the flag refuse before
   merging. Reads do not add the root/bit. Unmarked legacy names retain fallback.
8. Whole-document style duplication preserves the authored graph and creates
   fresh history. This corrects copy behavior without changing existing storage.
9. Historical facade package context scopes frames without authored migration;
   unverifiable unmarked legacy binary imports are refused. Historical packages
   must use current framed sync. Raw unscoped native documents remain compatible.

10. Fresh-copy owner allocation retains scalar per-peer allocation floors in
    `styles2.clock/<peer>` without importing source operations or version vectors.
11. Required sync bit 5 (`legacy-builtins1`) preserves historical paragraph-v1
    built-in context, including empty legacy roots. Source export remains pure.
    Receiver adoption adds one preflighted staging operation outside user undo,
    changes its revision, marks styles dirty, and makes package bit 18 required.
12. Integrated list and inline writes enter isolated preflight. Flow validation
    no longer materializes `breaks1` while reading an inline paragraph; existing
    root handles are cached to avoid repeated map scans.

These brief-authorized changes are documented in `contracts.md`, the style
addendum, collaboration protocol and format SPEC. No additional contract proposal.

## 9. Known gaps and later consumers

Handoff for PROGRESS: all ten findings are implemented and regression-covered.
No deferred acceptance tests remain. App findings remain outside this workstream.
The requested main merge integrates list bit 14 and inline bit 15; it is not a
new contract proposed by this fix stream.

Lists and inline objects now model or trial fallible writes. Sections should join
`Plan::needs_trial` before publication. The CODEMAP documents this extension point.
Lists can later reference durable style IDs for levels; the integrated bit-14
work already supplies heading roles. Table cells/headers/footers share the same
inherited style graph.
Partial clipboard/HTML remain explicit appearance projections, while whole-document
copy now exports/imports the complete existing authored graph. The transfer includes identity metadata and its compatibility interpretation.

App seam: existing batch/paste/replace methods and `expected_revision` stay in use.
Existing State revision/undo fields expose atomic outcomes. Existing mark-facts
and `diagnostics_for` queries now include missing-target explanations. Existing
`set_font_settings`, `declare_font` and layout-job calls preserve their DTOs.
There is no new app command required by Part A. The app findings remain out of scope.

## 10. Verification

Before-fix failures were observed in the original task activity, before the
corresponding production changes:

| Finding | Regression and observed failure |
| --- | --- |
| 1 | `refusal_atomicity`: late join and replacement changed frozen history/undo state; the later malformed-style rename probe also reproduced deletion before refusal |
| 2 | `rewritten_source_envelope_cannot_contaminate_reopened_or_transitive_copy`: rewritten envelope merged source history |
| 3 | `legacy_and_framed_import_refuse_wire_limit_before_parsing`: got `bindings.store`, expected `bindings.limit` |
| 4 | WASM smoke extra font-setting key: missing expected exception; prebuilt-package probe also showed the active job became stale |
| 5 | `workspace_refresh_preserves_session_font_descriptors`: unexpected `font.nearest` after workspace refresh |
| 6 | `deleted_anchor_target_explains_missing_mark_and_query`: explanation was None instead of `relation.missing-target` |
| 7 | `table_cell_uses_resolved_paragraph_indents`: first-line indent was 0pt instead of 15pt |
| 8 | Standalone pre-migration probe lost the concurrent blue property after rename |
| 10 | Standalone probes showed no required built-in signal and a legacy unresolved heading rendering at 20pt instead of 10pt; prebuilt Node package also saved builtin use with required flags 0 |
| 9 | Empty and populated parent-alias copies resolved to 10pt instead of 40pt, or remained at 40pt after a 50pt parent update; referenced tombstone was revived |

The later refusal matrix exercises malformed/stale batches across insertion,
deletion, split, join, move and style definition, plus malformed paste/replacement
and text/HTML replacement. Native tests cover late capacity, pending raw writes,
read-root purity and bounded deep staging. Packet tests cover exact total cap,
cap+1 and both maximum-length version vectors. Historical unmarked-copy probes
also failed before the runtime package-scope correction; afterward both refusals
preserve bytes and state, and same-package framed sync works without a stored
marker. Style-copy tests cover empty/populated aliases, later parent edits,
referenced tombstones and built-in forwarding. Native graph tests cover raw
unreadable values, pure capture, fresh peer history, bounds and malformed
containers; editor tests check atomic refusal of bad fragments and occupied
targets. Identity/built-in acceptance cases are now in normal Cargo tests, including both
delivery orders, first patches, deletion, competing renames, label collisions,
undo/redo, name reuse, pinned base clearing, graph copying, old marked/unmarked
reads, metadata limits and downgraded packets. `style-migration-probes.log` and
`builtin-bit-before-wasm.log` retain the before-fix failures. Eighteen identity regressions passed before the last join changes; the document unit suite passed 258 tests before
the last cache/boundary additions (the final full gate passed all 306 document unit tests).
`rename-refusal-before.log` records the additional rename failure; its companion
tests verify refusal preserves bytes/history/undo and a preceding repair succeeds
as one undo step. This changes publication validation, not Part B style identity.

An earlier crowded native refusal suite passed in 333.45s on this machine, including
fixture preparation and persistence comparisons. Exhaustive staging of a large
formatting history is still expensive; that timing is not an isolated benchmark
of preflight. Ordinary model-only typing avoids staging. Further capacity-model
optimization must retain the byte/history/undo refusal guarantees.

Final bounded gate output and performance comparisons are recorded below. Every cargo test and gate invocation is wrapped in GNU timeout;
logs are under `G:/reprise-target/eng-fixes1/`.

Resumed integration evidence: `list-inline-refusal-before.log` records a new
staging command publishing pending raw edits; `semantic-matrix-isolated.log`
records read-only validation adding an empty break root (history bytes changed,
revision/undo unchanged). Both are now corrected; the native list/inline refusal
matrix passes in 0.42s. `legacy-join-before.log` records historical Heading 1
changing from 20pt to 10pt after joining. `legacy-join-after.log` passes snapshot,
native merge, empty/populated legacy roots, save/reopen and undo isolation in
0.04s. Expanded seeded command properties pass all 25 tests in 67.84s;
`semantic-facade.log` validates malformed/stale list and inline batches; the
full run also passes with redo explicitly seeded before the matrix.

## Exact paths relative to integrated main

```text
crates/doc/src/changes.rs
crates/doc/src/edit.rs
crates/doc/src/flow.rs
crates/doc/src/formatting.rs
crates/doc/src/fragment.rs
crates/doc/src/hostile.rs
crates/doc/src/identity.rs
crates/doc/src/lib.rs
crates/doc/src/lifecycle.rs
crates/doc/src/lists.rs
crates/doc/src/marks.rs
crates/doc/src/persist.rs
crates/doc/src/style.rs
crates/doc/src/style/store.rs
crates/doc/src/style_graph.rs
crates/doc/src/style_identity.rs
crates/doc/src/style_identity_tests.rs
crates/doc/src/sync.rs
crates/doc/src/sync_tests.rs
crates/doc/src/table.rs
crates/edit/src/editor.rs
crates/edit/src/error.rs
crates/edit/src/paste.rs
crates/edit/src/plan.rs
crates/edit/src/replace.rs
crates/edit/tests/refusal_atomicity.rs
crates/edit/tests/style_graph_copy.rs
crates/edit/tests/transactions.rs
crates/fixtures/src/hostile.rs
crates/fixtures/tests/hostile.rs
crates/fixtures/tests/incremental_steps.rs
crates/fixtures/tests/paragraph_styles.rs
crates/fixtures/tests/regions_geometry.rs
crates/fixtures/tests/snapshots/hostile__content_foreign_copy_snapshot.snap
crates/fixtures/tests/snapshots/hostile__foreign_copy_snapshot.snap
crates/font/src/lib.rs
crates/format/SPEC.md
crates/format/src/container.rs
crates/format/src/features.rs
crates/layout/src/codes.rs
crates/layout/src/floats.rs
crates/layout/src/flow.rs
crates/layout/src/flow/keeps.rs
crates/layout/src/incremental.rs
crates/layout/src/notes.rs
crates/layout/src/relations/alignment.rs
crates/layout/src/table_flow.rs
crates/reprise-wasm/src/lib.rs
crates/reprise-wasm/ts/reprise_wasm.d.ts
crates/reprise-wasm/ts/smoke.cjs
crates/reprise/src/lib.rs
crates/reprise/src/session.rs
crates/reprise/src/session/collab.rs
crates/reprise/tests/collab.rs
crates/reprise/tests/hostile_peer.rs
crates/reprise/tests/import_copy.rs
crates/reprise/tests/legacy_open_concurrent.rs
crates/reprise/tests/packet_bound.rs
crates/reprise/tests/plumbing_edit.rs
crates/reprise/tests/plumbing_marks.rs
crates/reprise/tests/sync_lineage_delta.hex
crates/reprise/tests/workspace_fonts.rs
docs/CODEMAP.md
docs/collaboration.md
docs/contracts.md
docs/paragraph-styles.md
docs/plumbing.md
docs/review-fixes-1-report.md
```

The merged outline regressions failed with each node counter shifted by one;
`roles_lists__outline_before_layout` changed 4@1 to 5@1 and 31@1 to 32@1,
and `outline_after_layout` shifted 4/938/1872 to 5/939/1873. Production binding
now retains an empty named root without an author operation, so both original
snapshots pass without updates. The foreign-copy fixture intentionally retains
the earlier field encoding through a hostile-store helper, preserving its own
pinned IDs and checking compatibility. Only the branch-added
`sync_lineage_delta.hex` changes: its new binding no longer emits an ID-field
operation or advances content counters. The original `sync_delta.hex` stays fixed.
Copy refusal also exposed list registry reads materialising an empty root;
those reads are now pure. Historical-open convergence uses framed transport,
as the already documented unmarked legacy policy requires.

## Final gate results

The warm full run passed 1,343 tests, zero failures; the one existing ignored
test, `export_geometry_previews`, writes optional inspection artifacts. No new
ignored or deferred tests were added. All 119 hostile tests passed, including
the 108-fixture count; all 10 paragraph-style fixture tests passed.

Every cargo test and gate run was bounded. The compile-heavy attempt spent
41m21s rebuilding and hit its 3600-second limit during incremental tests, with
no assertion failures. Its log is `compile-heavy-timeout-tests.log`. The complete
warm rerun used the same `timeout 3600 cargo test --workspace --no-fail-fast`,
needed 8.82s of preparation and passed.

| Debug binary | This branch | Supplied main reference | Ratio |
| --- | ---: | ---: | ---: |
| `incremental_steps` | 55.38s | about 40s | 1.38x |
| `incremental.rs` | 297.32s | about 235s | 1.27x |
| `incremental_fuzz.rs` | 25.77s | about 52s | 0.50x |

These are observed run times, not an isolated attribution of every difference.
The earlier runs before root-cache work measured 68.06s/389.34s for the first
two binaries. Root absence/root handles now avoid repeated metadata lookup
work; ordinary typing still avoids full staging. The final refusal suite passes
all 10 tests in 255.63s (its crowded histories exercise expensive actual staging),
and the expanded transaction properties pass 25 tests in 54.16s.

The required wasm32 crate check, `generate_types`, debug WASM build with a 1 MiB
stack, wasm-bindgen **0.2.129** for web and Node, strict TypeScript, worker build,
`smoke.cjs` and `worker-smoke.cjs` all passed. The generated WASM declaration
diff only corrects the cap comment from packet body to complete wire bytes.

GNU timeout initially selected npm's POSIX `tsc` shim, whose Unix helper tools
were absent from that launch environment. The check failed before TypeScript
compilation. The remaining gate resumed with Node invoking the already installed
TypeScript compiler by its absolute JS path; strict checking and worker emission
then passed. This required no dependency installation or source fix. The failed
launcher log is retained as `tsc-launcher-before.log`.

Final status and all logs are in `G:/reprise-target/eng-fixes1/`. The status log
preserves the first launcher failure and records its successful resumed checks.
The original snapshots in all crates and original `sync_delta.hex` remain
unchanged against integrated main `4131ec8`.

`cargo fmt --all` — exit 0.

The log is empty (formatter produced no output).

`cargo clippy --workspace --all-targets -- -D warnings` — exit 0.

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 16.55s
```

`timeout 3600 cargo test --workspace --no-fail-fast` — exit 0.

```text
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests reprise_wasm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

```

`cargo check --target wasm32-unknown-unknown -p reprise-layout -p reprise-display -p reprise-fixtures -p reprise-format -p reprise-edit -p reprise-plugin -p reprise-clipboard -p reprise -p reprise-wasm` — exit 0.

```text
    Checking reprise-layout v0.0.0 (F:\reprise-wt\eng-fixes1\crates\layout)
    Checking reprise-format v0.0.0 (F:\reprise-wt\eng-fixes1\crates\format)
    Checking reprise-edit v0.0.0 (F:\reprise-wt\eng-fixes1\crates\edit)
    Checking reprise-fixtures v0.0.0 (F:\reprise-wt\eng-fixes1\crates\fixtures)
    Checking reprise-clipboard v0.0.0 (F:\reprise-wt\eng-fixes1\crates\clipboard)
    Checking reprise v0.1.0 (F:\reprise-wt\eng-fixes1\crates\reprise)
    Checking reprise-wasm v0.1.0 (F:\reprise-wt\eng-fixes1\crates\reprise-wasm)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 02s
```

final-smoke.log:

```text
WASM smoke: native JSON parity, editor loop, sync, defaults, images, formatting, emphasis/decoration/colour, host fonts, synthetic glyph outlines and ink bounds, authored marks, bytes, and hostile objects passed
paragraph style WASM smoke passed
plumbing WASM smoke passed
page setup WASM smoke passed
roles/lists WASM smoke passed
annotations WASM smoke passed
inline objects WASM smoke passed
```

final-worker-smoke.log:

```text
roles/lists worker smoke passed
Worker smoke: edit effects, created IDs, split selection, emphasis/decoration/colour, authored marks, host font configuration/job invalidation and image results passed
```
