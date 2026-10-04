# Workstream 9a: persistence and file format

Worktree: `D:/!!Self/dev/reprise-wt/format`; branch: `ws/format`.
Implementation commit: `4e01e2e feat(format): persist versioned document packages and pinned assets`.
The documentation-only hand-off commit follows this implementation; the final message lists both.

## 1. Summary by decision

- **34:** New `reprise-format` crate: one versioned, checksummed container, persistent
  document identity, required/optional feature negotiation, raw JSON manifests and
  binary sections, exact preservation of opaque optional data, assets, sequential pure
  migrations, synthetic v0 and pinned v1 fixtures, and compatible newer read-only open.
  Added to workspace, CI and the AGENTS WASM command.
- **21:** Font pins use FaceId family/hash plus an explicit distribution version.
  Full asset hashes and FaceId hashes/family are checked; rejected bundles cannot become
  usable fonts. Assets may be bundled or external; the host receives resolution needs.
- **05:** Derived caches remain opaque optional bytes outside Loro, with full tags and
  validation against document identity/revision and engine inputs. Corrupt/stale caches
  cannot be returned as valid.
- **07, 09, 29:** `doc/src/persist.rs` is the only new code that touches Loro types.
  History and shallow exports are explicit. IDs, anchors, tombstones, authored unreadable
  forms and retained revisions survive history round trips; merges converge.
- **37:** Checked framing and iterative manifest limits reject malformed files with
  typed errors, while safely framed cache damage is recovered. An allocation-free
  Loro/KV/LZ4 preflight bounds inner expansion before Loro metadata/import runs.
- **38:** Ordered sections/manifests, no introduced timestamps, deterministic fixture
  bytes and fresh snapshot exports. Every hostile fixture and the spike reopen with
  identical `engine.layout` JSON on peers 1 and 2.

## 2. Commits

```text
4e01e2e feat(format): persist versioned document packages and pinned assets
```

The final response includes the subsequent report-only commit from
`git log --oneline main..HEAD`. Neither commit was pushed, merged or rebased.

## 3. Files touched

All edits are within the allowed list. `doc/src/lib.rs` contains only the new module
and re-export lines; AGENTS changes only the WASM command. `.cargo/config.toml` and
PROGRESS were left alone. Build output and verification logs stay on G:.

- `.github/workflows/ci.yml`
- `AGENTS.md`
- `Cargo.lock`
- `Cargo.toml`
- `crates/doc/src/lib.rs`
- `crates/doc/src/persist.rs`
- `crates/fixtures/src/hostile.rs`
- `crates/fixtures/tests/hostile.rs`
- `crates/fixtures/tests/snapshots/hostile__content_persistence_tombstones.snap`
- `crates/fixtures/tests/snapshots/hostile__persistence_tombstones.snap`
- `crates/format/Cargo.toml`
- `crates/format/SPEC.md`
- `crates/format/src/assets.rs`
- `crates/format/src/container.rs`
- `crates/format/src/json.rs`
- `crates/format/src/lib.rs`
- `crates/format/src/migration.rs`
- `crates/format/src/package.rs`
- `crates/format/src/tests.rs`
- `crates/format/tests/data/v0.reprise`
- `crates/format/tests/data/v1.reprise`
- `crates/format/tests/roundtrip.rs`
- `docs/CODEMAP.md`
- `docs/contracts.md`
- `crates/format/REPORT.md`

## 4. Design choices and format spec

The normative wire specification is [SPEC.md](SPEC.md). Version 1 uses an 80-byte
checksummed header and ordered records with 52-byte checksummed headers; payloads
are uncompressed. JSON metadata is inspectable and canonical on authoring, binary
payloads avoid base64 expansion, parsing is sequential, and no ZIP timestamp,
compression/directory ambiguity or new dependency is introduced. Unknown optional
section flags, codec and payload bytes survive exactly; untouched JSON metadata is
retained raw, including whitespace and unknown fields.

Document IDs are supplied by the host and carried through save/migration. They do
not derive from a peer or Revision. Existing in-document snapshot references are
scoped by that document ID; cross-document references use SnapshotReference.

Narrowings: one opaque cache section in v1; one font pin per asset (current font
API reads one face, and this prevents repeated copying of a large bundle under
many claimed versions); no outer compression; compatible newer files expose a
read-only DocumentAt rather than the interior-mutable Document. Explicit hard
limits are documented in SPEC. A Loro encoding upgrade must review the preflight.

## 5. New public API and diagnostic codes

- `Document::export(PersistenceMode) -> Vec<u8>`, `try_export -> Result<Vec<u8>,
  DocError>`, `Document::import(bytes, peer) -> Result<Document, DocError>`;
  PersistenceMode::History/Shallow and encoded/expanded/operation limits.
- Container, Header, DocumentId, FeatureFlags, Section, section IDs, Limits,
  CURRENT_VERSION, `Container::decode`, FormatError and `FormatError::note`.
- Package construction/open/save, settings/fonts/assets/extension/unknown-section
  setters and accessors; OpenedFile editable/view/into_document/save and reports.
- FontPin, Asset/AssetKind/AssetSource/ExternalLocation, AssetNeed,
  AssetAvailability (needed/bundled/missing/usable fonts/notes), content_hash.
- Migration function type, MigrationRegistry::builtin/register/migrate,
  MigrationStep. A migration must advance exactly one version while retaining
  identity, flags, unknown sections and extension data.
- SnapshotReference, CacheTags/CacheContext, DerivedCache::validate,
  Package::set_cache/usable_cache. The host supplies canonical configuration hash.

| Code | Severity | Reason |
| --- | --- | --- |
| format.invalid | Error | Opening or saving omitted the document because data is invalid |
| format.limit | Error | Opening or saving exceeded a stated format bound |
| format.asset-hash | Error | An invalid bundled resource was excluded |
| format.font-hash | Error | A mismatched font bundle/pin was excluded |
| format.font-unreadable | Error | An unreadable/differently named pinned font was excluded |
| format.cache-dropped | Info | Disposable cache discarded; authoritative content remains exact |
| format.cache-ignored | Info | Cache inputs do not match; recomputation is required |
| format.asset-missing | Info | The declared resource requires the host's expected resolution step |
| format.migrated | Info | Container upgraded without changing authored state |
| format.read-only | Info | Compatible newer authored state is preserved in a read-only view |

`export` is the requested Vec-returning convenience: an encoding/limit failure
returns empty invalid bytes. Package creation and edited saves always use
`try_export`, so failures are reported and cannot become a saved container.

## 6. Snapshot changes

No existing snapshot changed. Two new goldens were recorded with INSTA_UPDATE=always
and read:

1. `hostile__persistence_tombstones.snap`: one live Unicode paragraph after deleting
   the first character of corridor, one historical range reference retaining the
   original corridor at its revision, and no live deleted block. Pins layout,
   IDs and historical resolution without changing other fixture output.
2. `hostile__content_persistence_tombstones.snap`: content-only display for that
   paragraph, including the combining character and emoji source/glyph mapping.

The synthetic `tests/data/v0.reprise` is 649 bytes and migrates to the 757-byte
`v1.reprise` golden; both are checked in and compared byte for byte. Fixture
regeneration is an explicit test-only FORMAT_WRITE_FIXTURES action.

## 7. Dependencies and licenses

No new registry package or third-party dependency was introduced into the workspace.
The new crate reuses:

| Dependency | License |
| --- | --- |
| serde 1.0.229 | MIT OR Apache-2.0 |
| serde_json 1.0.151 | MIT OR Apache-2.0 |
| sha2 0.11.0 | MIT OR Apache-2.0 |
| thiserror 2.0.21 | MIT OR Apache-2.0 |
| reprise-doc, reprise-diag, reprise-font, reprise-shape | AGPL-3.0-or-later |
| reprise-fixtures (dev) | AGPL-3.0-or-later |

Licenses were checked in the installed crate manifests. All are GPLv3-compatible;
WASM compilation verified the full new dependency closure.

## 8. Contract proposals

None. Existing frozen contracts were not changed. The added File format contract
text is reproduced below verbatim, and all new codes were added to the codes table.

## File format: `reprise-format`

- `Package` is one versioned container with a checksummed magic/version header,
  persistent host-assigned `DocumentId`, required and optional feature masks,
  and ordered checksummed sections. [The v1 spec](../crates/format/SPEC.md) pins
  the wire encoding, hard bounds, compatibility and diagnostic severity rules.
- `Document::export(PersistenceMode) -> Vec<u8>`, `try_export(...) ->
  Result<Vec<u8>, DocError>` and `import(bytes, peer) -> Result<Document, DocError>`
  live in `doc/src/persist.rs`. Only doc sees Loro; format carries its snapshot
  opaquely. History is retained unless Shallow is explicitly requested. The
  infallible convenience returns an empty invalid blob on encoding failure;
  package APIs always use try_export. Import validates self-contained snapshots,
  bounds embedded LZ4 expansion before Loro runs, and never chooses a random peer.
- Unknown optional features, unknown sections (including flags/codec/payload),
  extension bytes and untouched manifest fields round-trip verbatim. Unknown
  required features/sections refuse with typed `FormatError`s. Corrupt authored
  data or ambiguous framing refuses; safely framed corrupt caches are discarded
  with Info `format.cache-dropped`. All parsing is bounded and length-checked.
- `FontPin` records FaceId family/hash plus an explicit host-supplied version.
  `Asset` records bundled bytes or external path/URL and a full content hash.
  `AssetAvailability` returns needed, verified bundled, missing, usable fonts
  and notes. Fonts with mismatched bytes or identities are reported and excluded.
  External resources are never fetched; the host resolves them.
- `MigrationRegistry` applies pure N -> N+1 container migrations on open and
  reports each step. Saves write the current version. Compatible newer files
  expose `DocumentAt` read-only; editable access and saving return `ReadOnly`.
  Container migrations never duplicate authored schema migrations inside Loro.
- `CacheTags` binds opaque derived bytes to document ID, revision, engine
  version, complete AdapterInfo and canonical engine-configuration hash. Bytes
  are exposed only through `usable_cache` with a matching context. Stale caches
  are ignored (Info `format.cache-ignored`); caches never enter Loro. Producing
  and interpreting layout caches is layout/host work.
- A saved snapshot reference must have a document identity: `SnapshotReference`
  pairs `DocumentId` with `SnapshotRef`. Existing in-document references are
  scoped by their containing document. A Revision alone identifies no document.


## 9. Hand-off notes for PROGRESS

- Persistence and packages are ready to integrate with the editing kernel/host.
  Hosts must assign/retain document IDs and choose unique active editing peer IDs.
- External resolution is intentionally host-owned; format never fetches URLs or
  reads paths. On successful resolution, hosts should validate the declared hash.
- Current FaceId has no version member. FontPin carries a declared distribution
  version, while bytes/hash/family are validated. Independently extracting that
  distribution version needs future font API work.
- Cache serialization, engine configuration canonicalization and interpreting
  cache bytes remain layout/host work. No LayoutSnapshot round-trip was introduced.
- Compatible newer files have DocumentAt inspection. Rendering such a view would
  benefit from a layout entry point accepting a read-only snapshot; Engine currently
  accepts Document, so this workstream does not expose it through the read-only API.
- Loro 1.16 fast-snapshot/KV/LZ4 framing is guarded inside doc. Upgrade its encoding
  deliberately and re-run hostile/golden/expansion tests; unsupported blobs fail closed.
- History mode cannot recreate history that was already compacted. Shallow mode
  deliberately makes older snapshot references unavailable; authored schema
  migrations remain their own versioned forms inside the opaque CRDT history.
- The configured hard maxima deliberately reject oversized valid documents as
  well as attacks, on both save and open. Hosts can lower format limits.

## 10. Verification

All commands ran in the format worktree and exited 0 with no compiler/test warnings.
Workspace results: **331 passed, 0 failed** (sum of each test suite's results).
The format suite has 11 adversarial/golden unit tests plus 6 integration tests;
doc persistence adds 4 tests, including declared/undeclared inner LZ4 bombs.

`cargo fmt --all` produced no output (exit 0).

`cargo clippy --workspace --all-targets -- -D warnings` tail, verbatim:

```text
    Checking reprise-doc v0.0.0 (D:\!!Self\dev\reprise-wt\format\crates\doc)
    Checking reprise-layout v0.0.0 (D:\!!Self\dev\reprise-wt\format\crates\layout)
    Checking reprise-format v0.0.0 (D:\!!Self\dev\reprise-wt\format\crates\format)
    Checking reprise-fixtures v0.0.0 (D:\!!Self\dev\reprise-wt\format\crates\fixtures)
    Checking reprise-cli v0.0.0 (D:\!!Self\dev\reprise-wt\format\crates\cli)
    Checking reprise-display v0.0.0 (D:\!!Self\dev\reprise-wt\format\crates\display)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.36s
```

`cargo test --workspace` tail, verbatim:

```text
   Doc-tests reprise_shape

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests reprise_text

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

```

`cargo check --target wasm32-unknown-unknown -p reprise-layout -p reprise-display -p reprise-fixtures -p reprise-format` tail, verbatim:

```text
    Blocking waiting for file lock on build directory
    Checking reprise-doc v0.0.0 (D:\!!Self\dev\reprise-wt\format\crates\doc)
    Checking reprise-layout v0.0.0 (D:\!!Self\dev\reprise-wt\format\crates\layout)
    Checking reprise-format v0.0.0 (D:\!!Self\dev\reprise-wt\format\crates\format)
    Checking reprise-fixtures v0.0.0 (D:\!!Self\dev\reprise-wt\format\crates\fixtures)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.22s
```
