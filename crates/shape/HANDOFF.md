# Shaping workstream handoff

## Unicode and itemisation

`itemize` resolves UAX #9 paragraph levels through implicit resolution, using
`unicode-bidi` 0.3.18 with ICU4X 2.3.0 properties. Both bidi classes and paired
brackets (including canonical U+2329/U+3008 equivalence) come from ICU4X;
unicode-bidi's hardcoded-data feature is disabled. Explicit levels are bounded
by UAX #9's depth 125; implicit resolution can produce level 126. Overflow
embeddings and isolates are handled by the algorithm's specified counters,
not by dropping text or imposing an arbitrary text-length limit.

`None` direction uses P2/P3, ignoring strong characters inside isolates and
defaulting to LTR for all-neutral/empty text. Explicit direction sets the base
level; strong Latin in an RTL paragraph still resolves to an even level.

`Itemized.base_level` already existed. The added `Itemized.levels` stores
resolved levels before L1 for **every paragraph UTF-8 byte**, including text
outside styles or missing a face. Retain it until each composed line is
positioned. The field also preserves information for later caret/navigation
work (decision 30); this workstream does not alter snapshots or caret APIs.

Items split at style/face/size boundaries, level changes, and script changes.
ICU Script values are mapped to ISO 15924 tags. Common and Inherited use the
preceding script, then the following script for leading characters. Matched
closing brackets use their opening's script; punctuation following that close
continues in the closing bracket's script. All-Common/Inherited text uses
`Zyyy`. This is contextual script itemisation, not language inference.

Script bracket matching keeps at most **63 open pairs**. On overflow it emits
one `shape.script-depth` Warning, clears the stack, and uses contextual script
resolution for the rest of the paragraph. Pairing is disabled rather than
matching an ignored opening's closing bracket to the wrong earlier opening.
This is a deliberate resource bound; text remains shapeable. The passes are
linear in paragraph characters, with bracket searches bounded by 63.

Pinned reproducibility inputs: `icu_properties` **2.3.0**,
`icu_properties_data` **2.3.0**, Unicode **17.0.0**, and `unicode-bidi`
**0.3.18**, all pinned in Cargo.lock. ICU data metadata reports ICU 78.1rc
and CLDR 48.2.1. `reprise_shape::UNICODE_VERSION` exposes the Unicode version.
The 38 conformance rows are copied from the official
[Unicode 17 BidiCharacterTest](https://www.unicode.org/Public/17.0.0/ucd/BidiCharacterTest.txt).

## Layout integration (not made on this branch)

Replace the `visual_order` call in `layout/src/flow.rs::line_layout` **after
composition/reshaping and before positioning the runs**. Pass the logical
fragment runs, full paragraph levels, composed byte range, and base level:

```rust
let visual_runs = reprise_shape::reorder_line(
    text,
    &fragment.runs,
    &itemized.levels,
    fragment.text.clone(),
    itemized.base_level,
);
```

Plumb the two itemisation fields into `line_layout` from `compose_block`.
On success iterate the returned runs directly, assigning x positions from
their widths. **Do not call `visual_order` again**: the result already has L2
visual order. The helper applies L1 to trailing whitespace, B/S separators,
whitespace preceding those separators, isolates and retained X9 controls;
it splits runs wherever the adjusted level changes. It reverses glyph order
when L1 changes parity. It never reruns paragraph resolution or changes glyph
IDs, metrics, advances, clusters, face identities or total width.

Levels for unshaped gaps participate in L2, so missing faces cannot accidentally
join two even-level runs separated by an odd-level span. Empty lines succeed.
The helper rejects malformed ranges, levels and clusters with a `Note` carrying
`shape.bad-line` (Warning); layout should attach it to the block subject and
retain a conservative fallback ordering. Input runs must retain pre-L1 levels;
do not feed a previous helper result back as logical input.

## Shaping-data cache

`HarfRust` remains a unit struct and its trait contract is unchanged. Each
`Face` has one `OnceLock<Box<dyn Any + Send + Sync>>` slot. HarfRust fills it
with `ShaperData` from that exact face; another adapter type owning the slot
causes HarfRust to build the identical data locally. The font crate has no
harfrust dependency, and no global cache retains faces.

**Retention bound:** one adapter-data object per live Face, never one per
request/string/size/direction/feature combination. HarfRust's lookup caches
are bounded by that face's finite font tables (including lazy lookup entries),
and its cmap cache has 256 fixed entries. There is no fixed global byte budget:
retained data scales with the number and table sizes of loaded faces, like
the FontStore itself, and is released when each face's last Arc is dropped.
Repeated shaping cannot grow the number of retained objects. No shaping plans
or paragraph output are retained. Initialization synchronization is supported
on wasm32 and requires no worker threads for correctness.

Tests compare cached and fresh data on Latin ligatures/kerning, Greek/Cyrillic,
RTL letters/numbers/brackets, combining marks, emoji, controls, empty text,
partial ranges/context, both directions, features, scripts, language and
MIN/MAX/zero/negative sizes. Native concurrent initialization and a slot owned
by another adapter produce identical glyph vectors.

## Snapshot changes and existing font behavior

Only the existing `hostile__rtl_mixed.snap` changes. First-line runs in visual
order are now byte ranges **0..14 (L0), 37..40 (L2), 33..37 (L1), 30..33 (L2),
14..30 (L1), 40..42 (L0), 42..47 (L0)**. The second line splits into
**47..61 (L1), 61..62 (L0), 62..96 (L0)**. The extra L0 boundary is a real
Hebrew/Latin script boundary. Line byte ranges, widths, frame geometry,
relations and diagnostics are unchanged; run x positions/widths are the
corresponding split advances. Remaining lines and the annotation are unchanged.

Three new snapshots record the appended fixtures: `bidi_stray_controls`
(stray PDF/PDI, unterminated RLI, 140 nested RLEs), `bidi_override_ligature`
(actual odd-level Latin office/numbers plus a note), and
`scripts_common_inherited` (Latin/Greek/Cyrillic, punctuation, nested pairs,
combining accents). All existing invariant/backend checks pass.

The supplied brief's claim that unavailable Hebrew/Arabic glyphs produce
`font.missing` does not match the current frozen implementation: itemisation
selects the first **available family**, without cmap coverage checking.
Unsupported characters reach HarfRust and become `.notdef`. That behavior,
the adapter contract, and existing fixture expectations are preserved. A
future glyph-aware fallback policy needs grapheme-safe face selection and
separate snapshot/diagnostic review; it is not silently added here.

## Dependencies and contracts

New packages: `icu_properties` 2.3.0 and `icu_properties_data` 2.3.0
(`Unicode-3.0`); `unicode-bidi` 0.3.18 (`MIT OR Apache-2.0`). The copied
Unicode test data is `Unicode-3.0`. No other new transitive packages.
All are GPLv3-compatible. No frozen signatures/semantics were changed and
there are no contract proposals. New APIs are `Itemized.levels`,
`reorder_line`, `UNICODE_VERSION`, `Face::adapter_data`, and the public
`reprise_shape::codes` module. Both new diagnostic codes are Warnings:
script-pairing fallback changes requested contextual script association;
bad-line input prevents the requested reordering while retaining source data.
