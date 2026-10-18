# Review fixes 2

Verification is in progress. This checkpoint records implemented changes and measured evidence; the final gate results will replace this paragraph.

## Summary

Decisions 07 and 29: annotation refusals preserve history and undo; segmented restore hints retain annotation identity through undo. Decisions 33 and 35: copied annotations retain both endpoints, and atom PDF links retain geometry and tags. Decisions 34 and 37: bounded root discovery and deep history codecs avoid native/WASM stack exhaustion. Decision 38: ordered index lookups and work-count guards preserve deterministic output while removing repeated scans.

## Findings

| Finding | Fix | Regression | Negative control |
| --- | --- | --- | --- |
| R1 / N1, blocker | Model target overlay and exhaustive command/step trial selection. | `annotation_target_deletions_are_atomic_in_additions_and_updates`; matching-revision facade refusal retains redo. | Base `plan.rs` makes the history/revision/undo assertion fail (exit 101). |
| R2, blocker | Shared bounded named-root check; vendored root emptiness and iterative history codecs. | Native `deep_metadata`, depths 1,024/4,096; hidden hostile goldens; executable 1 MiB WASM smoke. | Pending final result. |
| R3 | Add optional final portable endpoint; remap it with one range/comment identity. | Clipboard selected-later/whole-copy coverage 10/16 bytes; facade import-copy link and comment coverage. | Pending final result. |
| R4 | One restore hint per visible segment; each endpoint restores in its own host. | `undo_annotations_after_deleted_atom` repeats undo/redo and merges a concurrent payload edit; copied-end regression. | Copied-end regression failed before its fix (0..4 rebound versus 0..3); hidden-marker control pending. |
| R5 | Wrap atom Figure/text inside Link/Reference with placed rectangle and contents. | `pdf_links_over_atoms_in_lists`: authored rectangle count must equal emitted/tagged PDF links, image and field. | Pending final result. |
| R6 | Inclusive end interval vector once per host, binary endpoint lookup, one cached document order for cross-host spans. | Work-count tests for 32/256 paragraphs and 40/240 hosts; existing boundary/hidden-segment parity test. | Pending final result. |
| N2 | Check ranges-root presence before acquiring its creating tree handle. | Missing remove/update compares exported History bytes, revision and undo/redo. | Getter-before-guard control fails history assertion (exit 101). |
| N3 | Pin each side's historical built-ins or original-owner fallback before identity metadata can erase context; preflight migration and exclude it from undo. | Genuine pre-fix history, both delivery orders, native/framed, source return merge, History/Shallow reopen, unrelated fallback and pending text undo/redo. | Pending final result. |
| Astra-5 performance | Cheap built-in ownership predicate; carry the already-read reference through block/style/layout input reads. | Authoring performs zero full property-record reads; computed styles read each block reference once; repeated manual phase probe. | Pending work-count control. |

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

Additive APIs: `FragmentEndpoint`, `FragmentRange.end`, `Document::stage_fragment_range_end`, and `Document::block_with_style_reference`. No new diagnostic codes. No facade DTO fields or command variants are intentionally added.

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

Required package/sync feature allocation for retained endpoint metadata is awaiting the orchestrator's response. The stored key is `end-node1`; older range readers otherwise silently ignore it. Packet identity and historical built-in declarations can coexist; this is needed to preflight joins of historical identity roots.

## Dependencies and snapshots

No new dependency/version. The existing Loro 1.16.2 internal crate is vendored under its MIT license, compatible with AGPL-3.0-or-later. New development tests use existing workspace dependencies. Four new deep hostile goldens will be recorded and inspected; existing snapshots must remain unchanged.

## Files, commits and merge

Changed file groups are mapped in `docs/CODEMAP.md`. Shared edits include root Cargo manifest/lock (vendor patch), CI (mandatory deep WASM execution), feature inspection, layout inputs, and the code map; these are needed for the requested cross-crate fixes. No `PROGRESS.md` edits, push or other branch merge.

Final commit list and merge evidence will be inserted after merging authorized main `0d8d296`.

## Known limits and hand-off

History snapshots, metadata queries and physical hidden-depth fixtures are covered at 4,096 levels. State has its existing 1,024 live-depth limit. A fully live deep tree still incurs repeated ancestor walks in existing readers/layout; the 4,096 live fixture tests the History codec, while State/save/WASM use hidden chains. Shallow snapshots still recursively build nested tree values and are outside this patch's depth guarantee. These are explicit remaining depth/complexity limits, not claims of an unrestricted deep document model. The vendor patch should be removed only after verifying equivalent upstream behavior.

## Verification

Final gate commands, exit codes and verbatim tails will be inserted here after execution.
