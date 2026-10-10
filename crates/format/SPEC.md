# Reprise document container, version 1

This is the on-disk contract for `reprise-format`, implementing decisions 05,
07, 09, 21, 29, 34, 37 and 38. The suggested extension is `.reprise`.
The library takes and returns bytes; paths, network access and font resolution
belong to the host. All integers below are unsigned little endian unless stated.

## Encoding choice

One uncompressed binary container carries canonical JSON metadata and opaque
binary sections. A ZIP package would add directory lookups, timestamp and
compression choices, ambiguous duplicate paths, and an additional dependency.
This container can be written and inspected sequentially, keeps binary assets
compact without JSON/base64 expansion, uses only portable Rust, and gives each
section independent integrity and compatibility rules. Small manifest payloads
are UTF-8 JSON, so debug tools can print them directly. Compression can be added
only as a future negotiated format feature; version 1 never decompresses a
container section. Loro's internal snapshot compression is independent and
bounded inside `reprise-doc` before import.

No new third-party dependency is introduced. Dependencies used by this crate:
serde (MIT OR Apache-2.0), serde_json (MIT OR Apache-2.0), sha2 (MIT OR Apache-2.0),
thiserror (MIT OR Apache-2.0), plus the AGPL-3.0-or-later workspace crates doc,
diag, font and shape (fixtures for tests). All support WASM and are GPLv3-compatible.

## Header (80 bytes)

| Offset | Bytes | Meaning |
| --- | --- | --- |
| 0 | 8 | Magic: ASCII `REPRISE` followed by NUL |
| 8 | 4 | Format version; current = 1 |
| 12 | 16 | Opaque persistent document ID, assigned explicitly by the host |
| 28 | 8 | Required feature bits |
| 36 | 8 | Optional feature bits |
| 44 | 4 | Section count |
| 48 | 32 | SHA-256 of bytes 0..48 (exclusive) |

The document ID is neither a peer ID nor a `Revision`, and must be retained on
every save, fork and migration of the same document. A new/copy document needs
a new host-assigned ID. No uniqueness is inferred from a revision or content.
The `SnapshotReference` envelope carries both document ID and `SnapshotRef`;
references already inside the Loro document are scoped by the containing
document ID. Cross-document references must use the envelope. This does not
change or migrate the frozen authored `SnapshotRef` type.

Version 1 defines two table feature bits (`features.rs`), both bit 8, declared on
every save from the live content and recomputed (a removed span clears its bit):
optional `TABLE_HEADERS` (a row is a header row; older readers lay out without
repeating it, so it degrades) and required `TABLE_SPANS` (a cell spans rows or
columns, or has table metadata this version cannot read; a reader without span
support would drop those cells, so it refuses). Spanless, headerless tables set
no bit and keep their bytes. Any other nonzero required bit is
`RequiredFeatures` (carrying only the unknown bits); unknown optional bits are retained exactly. A version newer
than 1 with no unknown required bits/sections and readable core sections opens
read-only. Its document is exposed through `DocumentAt`; editable access and
all save paths return `ReadOnly`. Compatibility assumes that core section
semantics and framing stay stable and future incompatible changes set a
required bit. Changing framing requires a new magic or required feature.

## Sections

Each record has a 52-byte header followed by exactly its declared payload:

| Offset in record | Bytes | Meaning |
| --- | --- | --- |
| 0 | 4 | Section ID |
| 4 | 4 | Flags: bit 0 = required to understand; remaining bits reserved |
| 8 | 4 | Codec: 0 = raw |
| 12 | 8 | Payload length |
| 20 | 32 | SHA-256 of record bytes 0..20 concatenated with payload |
| 52 | length | Payload |

Records are saved in ascending section-ID order, with no duplicate IDs, padding,
timestamps or trailing bytes. Headers and payloads are checksummed together so
ID/length/codec corruption cannot silently change interpretation. Checksums
detect corruption, not adversarial authenticity; this is not a signature scheme.

| ID | Payload | Presence |
| --- | --- | --- |
| 1 | Current Loro snapshot, opaque to format | Required |
| 2 | JSON object: document/package settings | Required, default `{}` |
| 3 | JSON array: pinned fonts | Required, default `[]` |
| 4 | JSON array: asset table | Required, default `[]` |
| 6 | Opaque unknown extension data | Optional |
| 65536..131071 | Raw bundled asset bytes, referred to by the asset table | Optional |
| 2147483647 | Tagged opaque derived cache | Optional |
| Other IDs | Opaque future/extension sections | Optional unless flag bit 0 set |

Unknown optional sections preserve flags, codec and every payload byte. A
required unknown section returns `RequiredSection`. No unknown codec executes;
it is preserved only in opaque optional sections. Known sections require codec
0 and recognized section flags. Core metadata is retained verbatim after open,
including whitespace and unknown fields. Explicit setters replace only their
section, using compact canonical JSON: recursively sorted object keys, sorted
font pins (face, version, asset), and assets sorted by ID. Section checksums are
recomputed deterministically. Array order inside arbitrary settings remains
authored order. The pinned v1 fixture checks the entire file, including Loro.

## Loro persistence

Only `reprise-doc::persist` sees Loro types. `PersistenceMode::History` exports
a snapshot with retained history; `Shallow` explicitly retains current state
and frontier and discards earlier addressability. Already compacted history
cannot be recreated by History mode. `Document::import(bytes, peer)` chooses
the editing peer explicitly and rejects unsupported encodings and incomplete
update streams. A caller must never edit two replicas concurrently under one
peer ID. Tests pin peers 1 and 2.

`Document::try_export` returns encoding failures as `DocError`. The requested
`export(mode) -> Vec<u8>` convenience returns an empty, invalid blob if encoding
fails; package creation and edited saves always use the fallible API. Import
restores fractional-index configuration. No layout state enters Loro.

Before Loro reads metadata, an allocation-free preflight walks its 1.16 fast
snapshot envelope, KV-table block descriptors and LZ4 sequences. It rejects
unsupported encodings and bounds total expanded bytes, including compressed
blocks whose frame omits an expanded length. Loro still validates checksums,
CRDT encoding and authored state. This guard is version-coupled: changes to
Loro's snapshot/KV encoding require updating and testing the guard, not treating
opaque newer Loro encodings as readable.

## Fonts and assets

A font pin is `{face:{family,hash},version,asset,...}`. FaceId supplies family
and its lowercase 32-character hash (first 16 bytes of SHA-256). Version is an
explicit nonempty host-supplied distribution version because current FaceId
has no version field. Unknown font-pin fields are preserved.
Each asset has at most one pin, so a hostile table cannot repeatedly parse and
copy one large font bundle under thousands of different version declarations.

An asset is `{id,kind,hash,source,...}` with unique nonempty ID, kind `font`,
`image` or `other`, and full lowercase SHA-256 (64 hex characters). Source is
`{kind:"bundled",section:N}` or `{kind:"external",location:{kind:"path"|"url",
value:"..."}}`. Bundled section IDs must lie in the asset range and may be used
by only one table entry. Unknown asset fields are preserved. Missing bundles
are allowed and reported, rather than losing the document.

`AssetAvailability` reports all `needed` resources, verified `bundled` bytes,
`missing` resources (including external and rejected bundles), validated `fonts`,
and diagnostic notes. The library never opens a path or fetches a URL. On open
it checks bundle SHA-256, each font's FaceId hash, readable font structure and
actual family name. Rejected pinned font bytes are excluded from both usable
fonts and verified bundles. Versions are pinned as declarations; current font
API cannot independently extract/validate the distribution version.

## Cache envelope and validity

Payload: u32 JSON-tag byte length, that many JSON bytes, then opaque cache bytes.
Tags are `{document_id,revision,engine_version,adapter,configuration_hash}`.
Revision is reprise-doc's sorted frontier; adapter is the complete `AdapterInfo`
(name, version, platform_independent); configuration hash is full SHA-256 of
a canonical configuration provided by the host (including composer, dictionaries,
fonts, medium and layout settings as applicable).

`usable_cache(expected)` returns bytes only when every tag equals the requested
context and its document/revision matches the package's captured snapshot.
Mismatches return no bytes and `format.cache-ignored` (Info). Open drops
malformed caches or caches for another document/revision. Edited save drops
revision-stale caches. Hash/framing or JSON damage to an otherwise framed cache
is recoverable: discard it with `format.cache-dropped` (Info). A truncated final
cache payload is also recoverable. Ambiguous section framing, oversized declared
lengths and non-final truncation are rejected. Cache omission changes no layout
output, so these diagnostics are Info. Caches are not stored in Loro. Layout
snapshot serialization and application remain layout/host work.

## Migrations

`MigrationRegistry` stores pure `fn(Container) -> Result<Container, FormatError>`
steps indexed by source version. Open runs N -> N+1 until current, reporting each
`MigrationStep` and `format.migrated` (Info). Every step must advance exactly one
version and preserve document ID, feature flags and unknown sections; the
registry verifies those invariants. Missing or invalid steps are typed failures.
The checked-in synthetic v0 fixture lacks font/asset manifests; builtin 0 -> 1
adds empty arrays and a settings default without touching the Loro blob.
Every successful writable save emits version 1. Newer files cannot be downgraded.

Authored schema versions (`expr1`, relation JSON, page-template envelopes) live
inside Loro and keep their own readers/migrations. Container migrations do not
interpret or rewrite them, and never replace CRDT history with a materialized
JSON rendering of document state.

## Bounds and errors

Hard maxima (callers may lower them using `Limits`, never raise them): file
128 MiB, section 64 MiB, sections 4096, each JSON manifest 1 MiB, JSON nesting
32, structural manifest entries 4096. Manifests are scanned iteratively before
serde allocates or recurses; string escapes do not change depth counts. Lengths
and offsets are checked before slicing/allocation. Loro additionally caps encoded
bytes at 64 MiB, total expanded KV bytes at 128 MiB, blocks/table at 65536, and
declared operation counters/change count at 4 million. Limit failures report
`format.limit` (Error) through `FormatError::note`; other fatal errors map to
`format.invalid` (Error). Fatal failures never return a partially imported
editable document.

Recoverable resource rejection uses `format.font-hash`, `format.asset-hash` or
`format.font-unreadable` (Error: a resource was omitted). `format.asset-missing`
is Info: resolving an external or absent resource is deliberately the host's
job, with no substitution claimed. `format.read-only` is Info: readable newer
state is preserved. Unknown extension payloads may contain arbitrary non-UTF-8
bytes; malformed UTF-8 in a known JSON section is a typed metadata error.

Deterministic adversarial tests cover every truncation and every single-bit flip
of a small file, length/count lies, duplicates, trailing bytes, unknown required
features/sections, non-UTF-8 and unbalanced/huge/deep manifests, forbidden outer
compression and inner LZ4 expansion limits, corrupt caches, migrations and font
hash mismatches. Every hostile fixture and the spike round-trip with identical
layout on peers 1 and 2; history merges converge and explicit shallow retention
is tested separately.

## Additive font supply metadata

`FontPin.extra["font-declaration1"]` is the versioned frontend declaration
(family, integer weight/style/stretch descriptors, and collection face index).
When present, opening checks the font-derived version as well as the identity.
Legacy pins without this field retain their original interpretation.
Standalone fonts and collection face zero keep SHA-256(file)[0..16]; other
collection faces hash file bytes followed by ASCII `reprise-face-index` and the
u32 little-endian index. The asset hash always covers the original whole file.

`Package::new_with_layout`, `embed_layout_fonts` and
`OpenedFile::save_with_layout` embed all faces with glyphs in the supplied
current layout, including glyph zero. Empty runs and unused style declarations
are excluded; existing pins/assets are retained. The caller supplies a layout
from this document and its engine configuration; the revision must match.
Old metadata-only creation/saving APIs remain for opaque container workflows.
Opening validates bundles; `open_with_fonts` opens and restores in one call,
or `restore_fonts` moves verified faces into the supplied
store. `missing_fonts` lists unavailable pins and `format.font-missing` warns
that frontend resolution is required. Layout substitutes through explicit chains.
Generic overrides are engine configuration; peers must use the same configuration
(the package does not infer overrides from which defaults happened to be used).


### Emphasis capability (required bit 10)

REQUIRED_TEXT_EMPHASIS declares weight/slant, independent underline/strike and
RGBA colour in paragraph styles or format1 patches. Bit 9 continues to declare
anchored formatting. Readers predating emphasis know bit 9 but ignore diagnosed
unreadable patches; bit 10 makes them refuse the package. Detection includes
retained/tombstoned authored state and named styles; unreadable envelopes safely
over-declare. Legacy records set no new bit.

### Required marks feature (bit 11)

`REQUIRED_MARKS` declares retained hard-break/TAB characters, paragraph alignment,
versioned tabs1 stops and reprise.alignment relations. Detection includes
soft-deleted content, style/break records and tombstoned relations; unknown
alignment/tab stored forms conservatively require the bit. Older readers must
refuse it. Marks guide geometry is derived and is never persisted.
