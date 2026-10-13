#!/bin/bash
# Copy of gate-integrate's prescribed checks, scoped to this worktree/cache.
set -euo pipefail
cd F:/reprise-wt/eng-sections2
export TMP=G:/tmp TEMP=G:/tmp
export CARGO_BUILD_JOBS=4 RUSTC_WRAPPER=''
T=G:/reprise-target/eng-sections
step() { local name="$1"; shift; echo "=== $name"; "$@"; echo '=== rc=0'; }
step types cargo run -q -p reprise --example generate_types
step wasmbuild env RUSTFLAGS='-C link-arg=-zstack-size=1048576' CARGO_TARGET_DIR=G:/reprise-target/eng-sections-wasm cargo build -q --target wasm32-unknown-unknown -p reprise-wasm
W=G:/reprise-target/eng-sections-wasm/wasm32-unknown-unknown/debug/reprise_wasm.wasm
step bindgen-web wasm-bindgen "$W" --target web --typescript --out-dir "$T/pkg-web"
cp "$T/pkg-web/reprise_wasm.d.ts" crates/reprise-wasm/ts/reprise_wasm.d.ts
step bindgen-node wasm-bindgen "$W" --target nodejs --typescript --out-dir "$T/pkg-node"
step tsc tsc -p crates/reprise-wasm/ts/tsconfig.json
step tsc-worker tsc -p crates/reprise-wasm/ts/tsconfig.json --noEmit false --module commonjs --moduleResolution node --outDir "$T/worker-cjs"
step smoke node crates/reprise-wasm/ts/smoke.cjs "$T/pkg-node"
step worker-smoke node crates/reprise-wasm/ts/worker-smoke.cjs "$T/pkg-node" "$T/worker-cjs/worker.js"
step fmt cargo fmt --all -- --check
step clippy cargo clippy --workspace --all-targets -- -D warnings
step wasmcheck cargo check --target wasm32-unknown-unknown -p reprise-layout -p reprise-display -p reprise-fixtures -p reprise-format -p reprise-edit -p reprise-plugin -p reprise-clipboard -p reprise -p reprise-wasm
step test timeout --kill-after=10s 3600 cargo test --workspace --no-fail-fast
echo '=== GATE DONE'
