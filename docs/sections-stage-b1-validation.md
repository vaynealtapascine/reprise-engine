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
