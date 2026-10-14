# Review fixes 1 handoff

Worktree: `F:/reprise-wt/eng-fixes1`; branch: `ws/review-fixes-1`; base: `f13a4ee`.
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

- `bda0d74 fix(engine): preflight refused edits and bind independent copies`
- `dc307c8 docs(styles): specify durable identity migration and unresolved compatibility`
- `a4da353 perf(doc): avoid sibling indexing during edit preflight`
- `1e5ed21 fix(edit): preflight captured malformed style values`
- `4cec4a5 fix(sync): scope historical packages without authored migration`
- `2e24dc2 docs(styles): isolate authored graph duplication from migration`
- `99ce2d2 fix(styles): duplicate the complete authored graph in fresh history`
- `f49eb9d test(styles): retain deferred migration acceptance probes`
- `4648aad docs(styles): select stable owners and versioned reference migration`
- `15e4b1a fix(styles): retain immutable owners and signal built-in semantics`
- The verification handoff commit records the final gate output; obtain the full
  list with `git log --oneline f13a4ee..HEAD`.

Every commit has the requested GPT-6.1-Sol coauthor trailer. Nothing was pushed.

## 3. Files touched

Paths below are relative to the worktree. No app files or `PROGRESS.md` changed.
The brief identifies affected subsystems rather than a closed file allowlist.
Supporting edits beyond its explicitly named files are listed in full here.

- `crates/doc/src/{identity.rs,style_identity.rs,style_identity_tests.rs,lib.rs,style/store.rs,style_graph.rs,sync.rs,sync_tests.rs}`:
  durable binding, pure lookup guards, edit staging and packet boundaries.
- `crates/edit/src/{editor.rs,error.rs,paste.rs,plan.rs,replace.rs}` and
  `crates/edit/tests/{refusal_atomicity.rs,style_graph_copy.rs}`: publication and refusal coverage.
- `crates/font/src/lib.rs`: additive registered-face iterator for provenance.
- `crates/layout/src/{codes.rs,flow.rs,flow/keeps.rs,table_flow.rs,notes.rs,floats.rs,relations/alignment.rs}`:
  shared geometry and context/anchor diagnostics.
- `crates/fixtures/src/hostile.rs`, `crates/fixtures/tests/{hostile.rs,paragraph_styles.rs,regions_geometry.rs}`,
  and the two new foreign-copy snapshots listed below.
- `crates/reprise/src/{lib.rs,session.rs,session/collab.rs}`;
  `crates/reprise/tests/{collab.rs,hostile_peer.rs,import_copy.rs,packet_bound.rs,plumbing_edit.rs,plumbing_marks.rs,workspace_fonts.rs,sync_lineage_delta.hex}`.
- `crates/reprise-wasm/src/lib.rs`, `crates/reprise-wasm/ts/{smoke.cjs,reprise_wasm.d.ts}`.
- `docs/{CODEMAP.md,contracts.md,collaboration.md,plumbing.md,paragraph-styles.md,review-fixes-1-report.md}`.
- Supporting `doc/src/{changes,edit,flow,formatting,fragment,lifecycle,marks,persist,style,table}.rs`,
  `layout/src/incremental.rs`, `format/src/{container,features}.rs`, and `format/SPEC.md`
  propagate stable references and compatibility flags.
- `docs/style-migration-probes.rs` was removed after promoting its acceptance
  coverage into ordinary Cargo tests.

No dependencies, DTO shapes, API version or PacketBound literal changed.
The required package mask now includes the brief-authorized bit 18. Generated WASM declarations change only the cap documentation.
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
`Features::STYLE_IDENTITIES`, and `REQUIRED_STYLE_IDENTITIES` (bit 18).
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
  refused merge. Fixture count increases from 102 to 103.

`sync_lineage_delta.hex` is a separate new wire golden for the scoped feature;
native and WASM tests pin it without rewriting the unmarked legacy golden.

## 7. Dependencies

None added; no license changes.

## 8. Contract and stored-form changes, separately

1. Durable metadata: fresh facade documents/copies author lowercase 32-hex
   `document1.id`. Package ID agreement is checked on reopen. Reads do not add it.
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
   `alias/<name>`, `label/<owner>`, `base/<owner>` and `legacy-builtins1`. New block
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

These brief-authorized changes are documented in `contracts.md`, the style
addendum, collaboration protocol and format SPEC. No additional contract proposal.

## 9. Known gaps and later consumers

Handoff for PROGRESS: all ten findings are implemented and regression-covered.
No deferred acceptance tests remain. App findings remain outside this workstream.

Lists, inline objects and sections should model all fallible writes or join
`Plan::needs_trial` before publication. The CODEMAP documents this extension point.
Lists can later reference durable style IDs for levels; headings need an authored
semantic role; table cells/headers/footers share the same inherited style graph.
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
`builtin-bit-before-wasm.log` retain the before-fix failures. Thirteen initial
identity regressions pass in 1.61s; the document unit suite passed 258 tests before
the last cache/boundary additions (the full gate below validates the final code).
`rename-refusal-before.log` records the additional rename failure; its companion
tests verify refusal preserves bytes/history/undo and a preceding repair succeeds
as one undo step. This changes publication validation, not Part B style identity.

The six crowded native refusal tests passed in 333.45s on this machine, including
fixture preparation and persistence comparisons. Exhaustive staging of a large
formatting history is still expensive; that timing is not an isolated benchmark
of preflight. Ordinary model-only typing avoids staging. Further capacity-model
optimization must retain the byte/history/undo refusal guarantees.

Final bounded gate output and performance comparisons are appended below once
all checks finish. Every cargo test and gate invocation is wrapped in GNU timeout;
logs are under `G:/reprise-target/eng-fixes1/`.
