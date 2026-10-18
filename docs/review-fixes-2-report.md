# Review fixes 2

Verification is in progress. This checkpoint records implemented changes and measured evidence; the final gate results will replace this paragraph.

## Summary

Decisions 07 and 29: annotation refusals preserve history and undo; segmented restore hints retain annotation identity through undo. Decisions 33 and 35: copied annotations retain both endpoints, and atom PDF links retain geometry and tags. Decisions 34 and 37: bounded root discovery and deep history codecs avoid native/WASM stack exhaustion. Decision 08: explicit per-reference context preserves historical style meaning through identity joins, while cheap ownership checks avoid decoding properties during authoring. Decision 38: ordered index lookups and work-count guards preserve deterministic output while removing repeated scans.

## Findings

| Finding | Fix | Regression | Negative control |
| --- | --- | --- | --- |
| R1 / N1, blocker | Model target overlay and exhaustive command/step trial selection. | `annotation_target_deletions_are_atomic_in_additions_and_updates`; matching-revision facade refusal retains redo. | Base `plan.rs` makes the history/revision/undo assertion fail (exit 101). |
| R2, blocker | Shared bounded named-root check; vendored root emptiness and iterative history codecs. | Native `deep_metadata`, depths 1,024/4,096; hidden hostile goldens; executable 1 MiB WASM smoke. | Verified by targeted reversal; see negative-control logs below. |
| R3 | Add optional final portable endpoint; remap it with one range/comment identity. | Clipboard selected-later/whole-copy coverage 10/16 bytes; facade import-copy link and comment coverage; disjoint and boundary-touching partial-copy slices. | Verified by targeted reversal; see negative-control logs below. |
| R4 | One restore hint per visible segment; each endpoint restores in its own host. | `undo_annotations_after_deleted_atom` repeats undo/redo and merges a concurrent payload edit; copied-end regression. | Both verified: omitting gap hints moves 1..3 to rebound 2..4; the copied-end regression failed before its fix (0..4 rebound versus 0..3). |
| R5 | Wrap atom Figure/text inside Link/Reference with placed rectangle and contents. | `pdf_links_over_atoms_in_lists`: authored rectangle count must equal emitted/tagged PDF links, image and field. | Verified by targeted reversal; see negative-control logs below. |
| R6 | Inclusive end interval vector once per host, binary endpoint lookup, one cached document order for cross-host spans. | Work-count tests for 32/256 paragraphs and 40/240 hosts; existing boundary/hidden-segment parity test. | Verified by targeted reversal; see negative-control logs below. |
| N2 | Check ranges-root presence before acquiring its creating tree handle. | Missing remove/update compares exported History bytes, revision and undo/redo. | Getter-before-guard control fails history assertion (exit 101). |
| N3 | Pin each side's historical built-ins or original-owner fallback before identity metadata can erase context; preflight migration and exclude it from undo. | Genuine pre-fix history, both delivery orders, native/framed, source return merge, History/Shallow reopen, unrelated fallback and pending text undo/redo. | Verified by targeted reversal; see negative-control logs below. |
| Astra-5 performance | Cheap built-in ownership predicate; carry the already-read reference through block/style/layout input reads. | Authoring performs zero full property-record reads; computed styles read each block reference once; repeated manual phase probe. | Verified: restoring property resolution gives 100 record reads versus 0; duplicate lookup gives 200 reference reads versus 100. |

## Performance evidence

Debug phase timings in seconds at 10,000 paragraphs; minimum of three runs for each column. Construction is excluded from the other phases. `block10` reads 100,000 blocks; `styles3` computes 30,000 styles. The retained ignored test uses the review's phase source. The control is the review's separately compiled `4131ec8` executable; before/after executables were copied out before subsequent builds could replace them.

| Phase | 4131ec8 control | This branch before style optimization | After style optimization |
| --- | ---: | ---: | ---: |
| Construct | 1.4075641 | 2.3231422 | 1.0175484 |
| Block reads | 4.4938257 | 4.3008137 | 2.8411226 |
| Styles | 5.1902428 | 5.2805529 | 3.6036930 |
| Layout | 32.5084644 | 28.4993678 | 21.5989138 |

Logs: `G:/tmp/fixes2-perf-{baseline,before,after}-{1,2,3}.log`. Other agents share CPU; these are local debug measurements, not release or WASM latency claims. Phase and logical-work instrumentation identified unnecessary property resolution during ordinary authoring and duplicate metadata lookup during style/layout input reads. No sampled CPU profile was collected. The original 2.3–2.6× read regression did not reproduce after the root fixes; the remaining construction slowdown did. The deterministic work guards establish the removed work independently of wall-clock noise.

## Preflight command audit

`command_needs_trial` and the step selector both match exhaustively, with no wildcard that could silently accept a future command.

- Always trial: `Paste`; `SetPageSetup`, `SwapPageOrientation`; `SetAlignment`, `SetTabStops`, `AddAnchor`, `ClearAlignment`; `DefineStyle`, `PatchStyle`, `RenameStyle`, `DeleteStyle`, `SetStyleName`, `SetStyleOverride`, `PatchOverrides`; `SetRole`; `CreateList`, `UpdateList`, `DeleteList`, `SetListItem`, `RemoveListItem`, `SetListLevel`, `IndentListItem`, `RestartList`, `ContinueList`; `AddAnnotation`, `UpdateAnnotation`, `RemoveAnnotation`; `InsertInline`, `UpdateInline`. These can allocate/check retained metadata, reference targets, bounds or ownership against the resulting state. Paste remains standalone.
- Text-only `InsertText`, `DeleteText`, `InsertLineBreak`, `InsertTab`: sizes, UTF-8 bounds and live targets are checked by the evolving transaction model; no late semantic allocation. Deletion restore hints retain their bounded existing behavior.
- `FormatText`: explicit action budgets and values are modeled; clearing and structural transfers additionally trial through the exhaustive step selector.
- `InsertBlock`, `DeleteBlock`, `MoveBlock`, `SplitBlock`, `JoinBlocks`, `Enter`, `BackspaceAtStart`: topology/positions/live targets are modeled; staged styles, paragraph properties and implicit formatting transfers additionally force a trial. No unchecked late semantic path is intentionally exempted.
- `AddRelation`, `RemoveRelation`: schemas, targets, ownership and liveness are modeled before staging/publication; no later semantic validation is introduced by publication.

## Root audit and vendor patch

All workspace `get_value()` uses were inspected. Named-root presence checks in lists, roles, formatting, emphasis, page setup, ranges and style storage use the shared bounded path. Remaining map-value calls read shallow records; `identity.rs` and edit depth preflight intentionally enumerate the shallow root catalog, which the vendor patch makes safe. `format::features::table_features` calls the audited predicates and uses an explicit node stack.

A smaller pure Reprise fix is sufficient for map-root checks, but not the complete requirement: unpatched Loro 1.16.2 has no public non-creating root catalog/presence API that includes empty tree roots and never constructs tree values. `get_by_str_path` filters names first but still recursively tests emptiness of a matching tree root; `has_container` always accepts root IDs and `try_get_*` materializes missing roots. Fast snapshot decode/export also recurse independently of root discovery. The four narrowly scoped vendor changes are documented in `vendor/loro-internal/REPRISE-PATCH.md`. The fourth elides only exact unchanged creation round trips during concurrent import; without it, the N3 regression preserves style but loses its pending text undo. No upstream cargo registry files were edited; the source is pinned through `[patch.crates-io]`.

## Public API and contract proposal

Additive APIs: `FragmentEndpoint`, `FragmentRange.end`, `Document::stage_fragment_range_end`, `Document::block_with_style_reference`, `Document::has_cross_paragraph_ranges`, and `Document::has_style_reference_contexts`. No new diagnostic codes. No facade DTO fields or command variants are intentionally added.

The brief explicitly permits the portable endpoint addition. Exact type proposal:

```rust
pub struct FragmentEndpoint { pub node: NodeId, pub at: usize }
pub struct FragmentRange {
    // existing fields unchanged
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<FragmentEndpoint>,
}
```

Proposed contracts wording: `FragmentRange.node/bytes` remains the first-piece compatibility view. An absent `end` encodes byte-identically to existing fragment JSON. A present `end` names the final copied paragraph and its UTF-8 endpoint; all intermediate selected paragraphs belong to the same retained range identity. Validation and paste remap both endpoints and internal links that target the range. Older strict fragment readers refuse the additional field. This affects native clipboard producers/consumers and document-copy callers; it adds no layout-coordinate storage.

The user-approved allocation is package bit 20 / sync bit 6 (`range-endpoints1`) for retained `end-node1`. Explicit style-context migration requires package bit 21 / sync bit 7 (`style-reference-contexts2`), marked by `styles2.reference-contexts2 = "1"`. Bit 19 remains free for sections. New readers preserve the old global interpretation of encoded owner references until migration; migration pins the historical built-ins and then opts owner references into explicit interpretation. Older readers must refuse the new declaration. Unknown required bits already refuse at the existing package/packet boundary; tests also strip each new sync declaration and verify byte-identical history on refusal.

Exact replacement proposed for the final frozen historical-context paragraph in `docs/contracts.md`: `Required sync feature legacy-builtins1 (bit 5) declares the sender's historical built-in interpretation and may coexist with style-identities1 (bit 4). A receiver pins each side's affected retained references during isolated preflight, then publishes those migration operations outside user undo. Existing unrelated unmarked references retain their original-owner fallback. The reference-contexts2 marker opts encoded owner references into explicit interpretation; package bit 21 and sync bit 7 require aware readers. Old identity histories without the marker retain their earlier interpretation.` The earlier wording excluded receivers with identities and prohibited combined declarations; that is exactly the policy that N3's authorized repair needs to replace. The frozen contract file is left for the orchestrator's dedicated contract update; this report makes the behavioral extension explicit.

## Dependencies and snapshots

No new dependency/version. The existing Loro 1.16.2 internal crate is vendored under its MIT license, compatible with AGPL-3.0-or-later. New development tests use existing workspace dependencies. Four added goldens were recorded and inspected: `hostile__metadata_depth_1024.snap` and `hostile__metadata_depth_4096.snap` show only the live sibling paragraph, one page, no relations and no diagnostics; their content-only counterparts show that same glyph display. The hidden physical chains produce different retained revision counters/IDs but do not add visible content. No existing snapshot is modified or removed (`git diff --name-status 4d99028 -- '**/snapshots/*'`).

## Files, commits and merge

Changed file groups are mapped in `docs/CODEMAP.md`. Shared edits include root Cargo manifest/lock (vendor patch), CI (mandatory deep WASM execution), feature inspection, layout inputs, and the code map; these are needed for the requested cross-crate fixes. No direct `PROGRESS.md` edits, push or other branch merge; its main-branch documentation changes arrived through the authorized merge.

Engine main `0d8d296` was merged with `--no-ff`; the merge completed without conflicts. The code map auto-merged, retaining both documentation streams. No production code arrived from main. The final commit list is appended with the gate results.

## Files touched and prior implementation

The vendor directory contains the unchanged pinned upstream crate plus the four behavioral patches documented above and one unused-import cleanup. Outside that directory, the branch changes these files relative to merged main:

```text
.github/workflows/ci.yml
Cargo.lock
Cargo.toml
crates/clipboard/src/html.rs
crates/clipboard/src/pdf_tags.rs
crates/clipboard/tests/annotations.rs
crates/doc/src/annotation.rs
crates/doc/src/annotation_restore.rs
crates/doc/src/annotation_tests.rs
crates/doc/src/emphasis.rs
crates/doc/src/flow.rs
crates/doc/src/formatting.rs
crates/doc/src/fragment.rs
crates/doc/src/legacy_join.rs
crates/doc/src/lib.rs
crates/doc/src/lists.rs
crates/doc/src/marks.rs
crates/doc/src/page_setup.rs
crates/doc/src/page_setup/fragment.rs
crates/doc/src/roles.rs
crates/doc/src/structure.rs
crates/doc/src/style.rs
crates/doc/src/style/store.rs
crates/doc/src/style_graph.rs
crates/doc/src/style_identity.rs
crates/doc/src/style_identity_tests.rs
crates/doc/src/sync.rs
crates/doc/src/sync_tests.rs
crates/doc/tests/data/review5-legacy.bin
crates/edit/src/paste.rs
crates/edit/src/plan.rs
crates/edit/tests/refusal_atomicity.rs
crates/fixtures/src/hostile.rs
crates/fixtures/tests/hostile.rs
crates/fixtures/tests/review_fixes2_performance.rs
crates/fixtures/tests/snapshots/hostile__content_metadata_depth_1024.snap
crates/fixtures/tests/snapshots/hostile__content_metadata_depth_4096.snap
crates/fixtures/tests/snapshots/hostile__metadata_depth_1024.snap
crates/fixtures/tests/snapshots/hostile__metadata_depth_4096.snap
crates/format/SPEC.md
crates/format/src/features.rs
crates/layout/src/incremental.rs
crates/reprise-wasm/ts/smoke.cjs
crates/reprise/tests/annotations.rs
crates/reprise/tests/deep_metadata.rs
docs/CODEMAP.md
docs/annotations.md
docs/collaboration.md
docs/paragraph-styles.md
docs/review-fixes-2-report.md
```

The earlier implementation's annotation overlay/trials, portable endpoint shape, segmented hint design, PDF atom wrapping and interval lookup are retained. This continuation completed and validated the depth codec/fixtures, added shared document-order work accounting, fixed distinct-host restore, completed context-safe legacy joins and undo, and removed the measured style read/authoring overhead. The source and test additions are within the requested cross-crate repair; the root manifest/lock, CI, feature declarations and code map are the necessary shared integration edits.

## Known limits and hand-off

History snapshots, metadata queries and physical hidden-depth fixtures are covered at 4,096 levels. State has its existing 1,024 live-depth limit. A fully live deep tree still incurs repeated ancestor walks in existing readers/layout; the 4,096 live fixture tests the History codec, while State/save/WASM use hidden chains. Shallow snapshots still recursively build nested tree values and are outside this patch's depth guarantee. These are explicit remaining depth/complexity limits, not claims of an unrestricted deep document model. The vendor patch should be removed only after verifying equivalent upstream behavior. The parallel sections stream must add its command variants to the exhaustive trial selector and use the shared bounded root check; package bit 19 is left available for it.

## Verification

Targeted restored-source runs: all 315 document unit tests passed in 36.62 s; all 25 format unit tests passed in 0.34 s; all 17 clipboard annotation tests passed in 3.58 s. Both deep hostile layouts passed in 1.69 s; the all-fixture content-display check passed in 9.89 s and recorded only the two missing content goldens.

Negative-control logs on G:, each original restored afterward:

| Control | Result |
| --- | --- |
| `fixes2-negative-R1.log` | Base `plan.rs`: history/revision/undo refusal assertion fails. |
| `fixes2-negative-N2.log` | Getter before existence guard: history/refusal assertion fails. |
| `fixes2-negative-copy-clipping.log` | Eager clipping subtraction panics on a disjoint partial copy; fixed by rejecting non-overlap before checked subtraction. |
| `fixes2-negative-R3.log` | Final endpoint omitted: copied link coverage `[10, 6]`, expected `[10, 16]`. |
| `fixes2-negative-R4.log` | Contiguity early return restored: span is rebound `2..4`, expected `1..3`. |
| `fixes2-negative-R5.log` | Atom annotation shortcut restored: zero links, expected one. |
| `fixes2-negative-R6.log` | Linear endpoint lookup: 2,592 steps for 32 paragraphs / 40 annotations, budget 592. |
| `fixes2-negative-N3.log` | Pin collection disabled: 10 pt, expected 20 pt. |
| `fixes2-n3-encoded-before.log` | Old encoded-reference context regression before versioning: 10 pt, expected 20 pt. |
| `fixes2-n3-phase.log` | Before the fourth vendor patch, the first concurrent join consumes pending text undo. |
| `fixes2-r4-cross-before.log` | Copied endpoint restored in wrong host: rebound `0..4`, expected `0..3`. |
| `fixes2-negative-perf-author.log` | Full-property built-in predicate: 100 reads, expected zero. |
| `fixes2-negative-perf-reference.log` | Duplicate lookup: 200 reads, expected 100. |
| `fixes2-negative-R2-valid.log` | Unpatched tree emptiness: `STATUS_STACK_OVERFLOW` at the 1,024 live metadata case, process status `0xc00000fd`. |

Assertion controls exit 101. The valid stack-abort control propagated exit 127 through the Windows timeout wrapper. An earlier R2 control had a line-ending compile error; it is discarded, not counted as evidence. The corrected control compiled successfully and produced the stack-abort evidence above.

The first complete post-marker gate passed every generation, WASM, TypeScript,
smoke, formatting and Clippy step. Its workspace run found three failing
cross-crate fuzz oracles with the same new R3 defect: eager `then_some`
subtracted an annotation's earlier endpoint from a later partial-copy start even
when the overlap predicate was false. All other targets passed. Logs are
retained as `G:/tmp/fixes2-gate-preclip-*.log`, and this failed run is not counted
as a successful final gate. Commit `670b460` rejects non-overlap before checked
subtraction; its focused regression reproduced the original runtime overflow
(exit 101). The focused regression passes, and all five cross-crate tests pass (`G:/tmp/fixes2-corpus-fixed.log`, 57.80 s). The full gate is being rerun after this correction with the compiler-cache wrapper disabled; output remains in the assigned G: target directories.


A final compatibility audit found that deleting a version-marker scalar must not
clear its required declaration while the corresponding operations remain in
history. Commit `b5bb750` uses Loro's retained last-editor metadata for both new
features and preserves the style declaration in fresh graph copies. The new
`deleted_version_markers_keep_their_declarations_and_round_trip` regression passes
for History/Shallow reopen, framed sync and fresh graph copies. Reverting the
retained-metadata check makes it fail (exit 101,
`G:/tmp/fixes2-negative-retained-marker.log`). The full gate is being rerun against
the committed production source after the clipping correction, `670b460`.


## Validated commit boundary

The final gate targets production HEAD `670b460`, after the authorized merge of
main `0d8d296`. Only this report is edited while that source is checked. The
following is `git log --oneline main..670b460`; documentation-only checkpoint/finalization commits may
follow this list.

```text
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
