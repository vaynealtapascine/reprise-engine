# Sections Stage B1 validation

## S3: page factory

The S3 source in checkpoint `4778c39` passed types, the debug WASM build,
web and Node bindgen, strict TypeScript, the worker build, native/WASM smoke
parity, worker smoke, fmt, clippy with `-D warnings`, and the wasm32 check.
Every native workspace unit/integration test and every existing snapshot passed.
The five new layout tests cover factory limits, frame-address remapping,
provenance rejection, one-frame story exhaustion, and stale placements.

The resumed full gate log is
`G:/reprise-target/eng-sections/s3-resume-2-gate.log`. Its only failed target was
`reprise --doc`, whose stalled rustdoc process was explicitly interrupted.
All other workspace doc-test targets passed. The exact interrupted target was
retried with `cargo test -p reprise --doc` and exited zero; its log is
`G:/reprise-target/eng-sections/s3-doc-retry.log`. No snapshots were re-recorded.
The S4 geometry drafts in the checkpoint were not registered or compiled.

## S4: geometry integration checkpoint

Targeted validation: all 85 layout library tests pass, the new named hostile
fixture passes, and workspace/all-targets clippy with `-D warnings` passes.
The full gate is pending in `G:/reprise-target/eng-sections/s4-bounded-full-gate.log`.
The initial gate was stopped and restarted with the orchestrator-required timeouts.

Only two new snapshots were recorded: `hostile__sections_geometry_hostile.snap`
and `hostile__content_sections_geometry_hostile.snap`. Hand review confirms
612x792 pt portrait pages, an outgoing blank filler, and a 792x612 pt final
landscape page. Counters are exact strings 1, 2, -2, -1. The 64-column clamp
partitions 468 pt into 7,488-unit widths. After two 18 pt gaps, weights 1,0,2
partition 432 pt into 144,0,288 pt; the zero column is skipped. RTL origins are
396,378,72 pt. The 1 pt rules start at 386.5 and 368.5 pt, and cover the 648 pt
body depth. The filler content list is empty. The word "cap" exceeds a clamped
column's measure, explaining the existing compose.overflow warning. Every
section diagnostic and the two physical rule paths were reviewed. No existing
snapshot was re-recorded or changed.

S4 gate caught an integration-scope conflict: registering the effective geometry
fixture in `all()` makes the existing all-fixture native clipboard roundtrip
require section transport, assigned to S9. The clipboard test is unchanged.
The effective fixture and both reviewed goldens remain in its dedicated
layout/backend test. `all()` instead registers a separate retained/inactive
section fixture with hostile column bounds and unknown lifetime keys. This
registration deviation is temporary until S9; all previously registered fixtures
remain unchanged. Its two additional goldens show ordinary one-page body layout.
The orchestrator approved the separate fixtures on 2026-10-14. The retained fixture passes its bounded dedicated test; hand review confirms a 420x300 pt page, ordinary 220x228 pt main frame at (36,36), two complete paragraph lines, no section metadata, no diagnostics and unchanged content-only rendering.

## S5 draft checkpoint

Unregistered draft modules preserve work while the corrected S4 gate runs.
They are not compiled by S4. The proposed physical note/float source frames
retain full source provenance and project occupied boxes into each current main
frame, including scratch trial depths; no main-entry provenance check is relaxed.
The body prefix remains in the ordinary snapshot during search; scratch headers
clone geometry and identity, not completed text. Integration and tests are pending.

## S4 completed validation

The corrected bounded full gate (`s4-bounded-full-gate-2.log`, on G:) passed
all static checks, native/WASM parity, smoke/worker smoke and all native
workspace unit/integration tests and snapshots. Layout: 85 tests. Fixture
incremental timings were 187.50 s, 25.12 s fuzz, and 44.98 s steps, comparable
to the orchestrator's main baselines. The outer gate exited 124 after rustdoc
stalled. A bounded workspace doc-test retry with the compiler wrapper disabled
and one build job completed all targets successfully (`s4-doc-isolation.log`).
The combined native suite and doc retry validate S4; the outer gate itself did
not exit zero. S5 must end with a complete zero-exit full gate. No existing
snapshot was changed.

## S5 integrated checkpoint

All 100 layout library tests pass, including 15 new S5 acceptance tests, in
3.72 s. Workspace/all-targets clippy with `-D warnings` passes. Continuous
full layout uses the same charged cursor units as incremental jobs; table row
groups yield after each fragment or move. Search keeps ordinary output as its
verified fallback, retains successful trial output, caps trials at 24 and
bands at 32, and checks recorded `fit(H)` before searching. Note areas and
float boxes retain full physical-source provenance and project into current
band depths; no provenance comparison is weakened. Full workspace and WASM
validation remain pending. Gate tooling disables the compiler wrapper locally
after the S3/S4 rustdoc stalls, without changing machine configuration.

S5 acceptance now includes unequal widths with successful widows/orphans and
an unbroken keep chain, plus full physical-source provenance rejection. All
102 layout tests pass in 3.84 s. The unchanged incremental_steps binary passes
all five tests in 41.90 s. Workspace/all-targets clippy remains clean.

Two new dedicated band goldens were recorded and reviewed: a 200x200 pt page
owned by first (counter 1), 1/2/1 main frames, band offsets 0/12/36 pt and depths
12/24/144 pt, only the first two balanced. The weighted RTL band has widths
40/80 pt at x=120/30 pt and y=22 pt; four 12 pt lines fill both columns. Its
1 pt black rule starts at x=114.5 pt and y=22 pt and ends at y=46 pt. The last
band is x=8 pt, y=46 pt, width=183 pt. Incoming 60 pt block margins are ignored
on the owned page and its -7 restart remains pending. There are no diagnostics.
The content golden contains six glyph lines and exactly that rule path. The
inherited non-main margin frame is unchanged. This fixture stays dedicated
under the approved S9 transport coverage split. No old goldens changed.

The sections_parity example generates four authored packages and native facade
outputs: bounded geometry/parity/counters, ordinary continuous bands, vertical
continuous bands, and notes plus floats. The gate compares actual WASM display
JSON bytes, diagnostics and State.sections with the native outputs, using
budgets zero and one. The native generator passes; actual WASM check is pending.

Before the first S5 workspace gate reached native tests, review found that
continuous band append could expand authored main depths when no column keys
were present. That gate was intentionally interrupted. Generated columns now
use the owner's remaining depth; spatial frames retain their authored depths,
clipped to the band's remaining room, and overflow uses their actual deepest
frame. A regression covers ordinary and oversized text in that path, with unit
budget equality. Recorded-reference fit checks now scan only suffix blocks
since C, rather than repeatedly visiting completed prefix text. All 103 layout
tests pass in 3.63 s and workspace/all-targets clippy passes. Native/WASM parity
now includes two additional spatial-template cases. The restarted full gate
will be recorded separately as s5-full-gate-2.log.
