# AGENTS.md

Notes for anyone (human or AI) working on reprise-engine.

-   [docs/architecture.md](docs/architecture.md) records the decisions. Change a decision
    by editing it there, in a commit that says why.
-   [docs/contracts.md](docs/contracts.md) describes the frozen interfaces that parallel work
    builds against, and how they may change. Don't change one inside a feature branch.
-   [docs/CODEMAP.md](docs/CODEMAP.md) maps each crate and feature to its files. Update it
    in the same commit when you add, move or remove files.
-   [PROGRESS.md](PROGRESS.md) is the hand-off log: what is done, what is in flight and
    the known gaps. Update it when you finish or stop a piece of work.

## Commands

```sh
cargo test --workspace                     # all tests, including snapshot fixtures
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo check --target wasm32-unknown-unknown -p reprise-layout -p reprise-display -p reprise-fixtures -p reprise-format -p reprise-edit -p reprise-plugin -p reprise-clipboard -p reprise -p reprise-wasm
cargo run -p reprise-cli -- spike out      # writes before/after in every output format
```

Run fmt, clippy and test before committing. CI runs them on Linux, Windows and macOS, plus
the WASM check.

## Rules

-   **Layout is integer arithmetic.** Everything composition produces is a `Length` (1/1024 pt).
    Floats are only allowed in rendering backends and must never feed back into layout.
    This is what makes the cross-platform guarantee (decision 38) possible.
-   **Snapshot fixtures are layout output, and layout must be identical everywhere.** If a
    change alters layout, re-record with `INSTA_UPDATE=always cargo test`, read the diff,
    and say in the commit why the output changed. A snapshot that differs only on one
    platform is a determinism bug, not a fixture to update.
-   **Fixtures pin their inputs:** the bundled font in `fixtures/fonts/` and peer ID 1. Don't
    use system fonts or random peer IDs in tests. Use `reprise-fixtures` for engines and
    documents.
-   **The hostile fixtures must keep passing.** `crates/fixtures/tests/hostile.rs` checks
    every invariant that no workstream may break. Add cases freely; removing one is a
    contract change.
-   **Diagnostics are matched by code, never by message.** Codes are public; add new ones to
    the table in `docs/contracts.md`.
-   **Authored vs derived (05):** `reprise-doc` holds only what is saved and undoable. Never
    store layout results in the Loro document.
-   **Byte offsets everywhere.** Loro counts Unicode scalars; only `reprise-text` converts.
-   **Layout never fails as a whole (37).** Leave out what can't be laid out and add a
    `Diagnostic`.
-   **Dependencies must be GPLv3-compatible.** The project is AGPL-3.0-or-later.
    Permissive and Apache-2.0 crates are fine; GPLv2-only is not.
-   **Commit messages** use conventional style (`feat(layout): …`, `fix: …`) with a body saying why.
