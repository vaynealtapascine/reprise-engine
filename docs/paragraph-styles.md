# Paragraph styles: properties, named styles and the facade

Decisions 08, 17, 18, 19, 23, 24, 34 and 37. This note is the design for the
`ws/style` foundation. Everything below extends the existing style model in
`crates/doc/src/style.rs` (named styles with parents, four value stages,
`em`/`lh`/`%` and expressions). There is no parallel mechanism.

## What the building block is

A **paragraph property** is one authored value that

1.  goes through the four stages (specified, inherited, computed, used),
2.  is stored as one string per property, in a named style, in a block's
    direct overrides, or as a flat merge-safe patch,
3.  is resolved per block by `Computed::used` and published in
    `ComputedStyle` (layout) and in the facade's `State`,
4.  has layout behaviour, export behaviour (HTML, PDF, native clipboard) and a
    package feature bit.

A **named style** is a set of such properties with an optional parent. The
engine ships built-in named styles; a document can override them per property.

## Properties added

| Property | Stored key | Kind | `%` is of | Used value |
| --- | --- | --- | --- | --- |
| `indent-start` | `indent-start` | length | the block's inline size | clamped to ≥ 0 |
| `indent-end` | `indent-end` | length | the block's inline size | clamped to ≥ 0 |
| `indent-first` | `indent-first` | length | the block's inline size | may be negative (hanging) |
| `space-before` | `space-before` | length | the block's inline size | clamped to ≥ 0 |
| `space-after` | `space-after` | length | the block's inline size | clamped to ≥ 0 |
| `alignment` | `alignment` | keyword | | gains `justify` |
| `keep-with-next` | `keep-with-next` | `true`/`false` | | |
| `keep-together` | `keep-together` | `true`/`false` | | |
| `widows` | `widows` | integer 1–32 | | minimum lines at the top of a frame |
| `orphans` | `orphans` | integer 1–32 | | minimum lines at the bottom of a frame |
| `background` | `background` | RGBA (JSON array) | | the extension proof (below) |

Lengths use the existing stored forms (`pt:N`, `em:N`, `expr1:<text>`), so they
take `pt`, `em`, `lh`, `%` and every expression. `em` is the element's font
size and `lh` its used line height: neither of those depends on these
properties, so there is no cycle. A bare `%` is of the block's inline size
(`block-width`), as CSS margins are of the containing block's width. Like
`line-height`, the last layer with a usable value wins; a degraded layer is
skipped with the usual `style.basis-*` warning.

All inline properties are **logical**: start and end follow the paragraph's
resolved base direction, so `indent-start` is on the right of a right-to-left
paragraph, and on the top of a vertical line. Layout works in the frame's
logical space (20), so vertical and rotated frames need nothing extra.

## Where values are stored (05, 29, 34)

Nothing changes for documents that never touch these features: the existing
`styles` root (one nested map per style) and the block `overrides` map are
read exactly as before.

New writes are **flat**, following `marks1` and `page-setup1`, so concurrent
first edits never compete to create a container:

-   **`styles1`** holds named-style properties as `<property>/<style name>`
    (property names never contain `/`, so the first `/` splits the key) plus
    one `state/<style name>` key: `defined` (a full definition: no built-in
    base), `deleted`, or `renamed:<new name>`.
-   **`para1`** holds per-block property patches as `<node>/<property>`.
    `alignment` and `tabs` keep their existing `marks1` keys, so every
    property lives in exactly one flat root and overlay order never matters.

Reading a named style is: the nested container if one exists, else the
built-in definition of that name if there is one, then the `styles1`
patches on top. Reading a block's overrides is: the nested container, then
`marks1`, then `para1`.

**Clearing** a property (meaning "inherit") deletes its flat key and, if a
nested container already exists, deletes the key inside it. Deleting a key in
an existing container is an ordinary merge-safe map operation; no container is
ever created to clear something, and no sentinel value is stored.

Concurrent edits to different properties of one style both win (different
keys). Concurrent edits to the same property resolve last-writer-wins, as Loro
does for every map key.

## Named-style lifecycle

-   **Define** writes every supplied property, clears every other one, and sets
    `state` to `defined`. A defined style no longer inherits its built-in base.
-   **Update** writes the supplied properties and clears the listed ones; the
    rest are retained. Updating a built-in name without defining it overlays
    the built-in values, so "make Heading 1 blue" keeps its size.
-   **Rename** `A` to `B` copies `A`'s effective properties to `B` (defined)
    and leaves `A` as `renamed:B`. Blocks and child styles naming `A` follow
    the forward, so a rename is O(1) and needs no rewrite of block records;
    a peer that applies `A` concurrently also ends up on `B`. Forwards count
    toward the 32-style chain bound; a forward loop is reported with
    `style.rename-cycle` and cut.
-   **Delete** sets `state` to `deleted` and keeps the properties (and
    parent). A block or style still naming it resolves through its parent
    instead, with `style.deleted` (Warning). That makes "one peer deletes a
    style while another applies it" converge without a special case.
-   **Apply** sets a block's style name; `null` sets the document base (the
    empty name, which the facade defines on create and open).

The empty-named base style can be updated (it is the document's "Default")
but not deleted or renamed. Names are at most 256 bytes, without control
characters; a document may hold at most 1,024 live named styles.

### Built-in defaults

`Normal`, `Title`, `Subtitle` and `Heading 1`–`Heading 6` are engine data,
never written to the document. `Normal`'s parent is the base (`""`); the
others' parent is `Normal`. Sizes and spacing are in `em`, so they scale with
whatever base the document has. Headings and Title set `keep-with-next`.
A missing `""` parent reached from a built-in is not reported (the facade
always defines it; bare engine documents simply use the engine defaults).

A built-in applies only to a name that has no nested container, so a
document from before this work that defined its own `Heading 1` lays out
exactly as before. A document that never names a built-in sees no change.
Headings keep their PDF semantics through `heading_level` (style name), the
existing single seam.

## Layout (23, 24, 37)

### Indents

A new `Indented` geometry provider in `reprise-compose` wraps the frame's
measure before float exclusions are subtracted: every interval loses
`indent-start` at its logical start and `indent-end` at its logical end, and
the first line of the paragraph (only when composition starts at byte 0)
loses `indent-start + indent-first`. A negative `indent-first` is bounded by
`indent-start`, so a hanging first line never leaves the frame. If the
indents would leave less than a quarter of the frame's inline size, they are
reduced (end first, then a positive first-line indent, then start) to leave exactly a quarter, and
`layout.indent-clamped` (Warning) is reported. Indents larger than the frame,
`Length::MAX` and negative values therefore all terminate.

### Spacing: collapse, and truncate at a frame start

Adjacent spacing **collapses to the larger value**: the gap between two
paragraphs is `engine.flow.paragraph_spacing + max(space-after, space-before)`.
That is what CSS margins and Google Docs do, and it is what HTML export
produces in a browser (`margin-top`/`margin-bottom` collapse), so export keeps
the layout's gaps. Adding the two (Word's default) double-spaces every heading
followed by a paragraph that also has space before.

`space-before` is **truncated at the start of every frame**, including the
first page, as CSS fragmentation truncates margins at a break; a paragraph
continuing into a new frame has none either. `space-after` of the last
paragraph in a frame is never carried into the next. Negative spacing is
clamped to zero at the used stage (`layout.style-clamped`).

### Justification

`alignment: justify` composes the paragraph with `Optimal` in
`Mode::Justified` (the existing Knuth–Plass mode with integer word-spacing
adjustments), whatever the engine's configured composer is. The last line, and
every line that ends at a hard break, keeps its natural width and is set at
the logical start (right in RTL). A paragraph containing a tab uses the
existing bounded first-fit tab composer: tab fields keep their stops and those
lines are set as `start`. A `marks1` line default (`reprise.alignment`) applies
after composition; on a justified line that already fills its interval it has
no visible effect, so in practice it only moves a last line. Pins still
translate lines.

### Breaking: keeps, widows and orphans

The flow places paragraphs in order through a resumable machine, so these
rules are applied by **bounded lookahead**, never by undoing placed lines:

-   **keep-together**: a paragraph that would split in a frame that holds other
    content moves to the next frame.
-   **orphans** `n`: a paragraph whose first fragment would have fewer than `n`
    lines (and continues) in such a frame moves to the next frame.
-   **widows** `n`: when a paragraph continues, the rest is composed once, as a
    trial, in the next frame's geometry. If it would finish there with fewer
    than `n` lines, this frame's fragment is recomposed with its depth bounded
    so the missing lines move over, as long as the fragment keeps
    `max(orphans, 1)` lines; otherwise the paragraph moves (if that helps).
-   **keep-with-next**: before a paragraph `H` that fits is committed, the chain
    `H, N1, N2, …` is trial-composed in the rest of the frame: every member
    with `keep-with-next` must fit whole, and the first member without it must
    place `max(orphans, 1)` lines (all of them if it is `keep-together`). If
    not, `H` moves to the next frame.

"Moving helps" only when the frame holds something that is not itself a
keep-with-next chain leading into this paragraph; the frame records that
(`anchored`). An empty frame is anchored, so every paragraph moves at most
once, and in the next frame breaks anyway. A rule that cannot be met even
there is reported with `layout.keep-unsatisfied` (Warning) and the paragraph
breaks normally. The chain walk stops after 32 blocks (reported with the same
code) or as soon as it leaves the frame, so `keep-with-next` on every paragraph
of a long document costs at most one frame's worth of trial compositions per
paragraph. Tables and flowed images end a chain (they are treated as
fitting); annotations placed by relations are skipped. Lookahead reads the following blocks; the flow memo key includes
their inputs for exactly the blocks the chain may read, so incremental layout
re-lays a heading when the paragraph after it changes.

## Unset per property: one representation

"Clear" is always a **property name in a `clear` list**, from one vocabulary
(`StyleProperty`, kebab-case): `set-style`, `update-style` and `format-text`
all take `clear`. `null` cannot mean "clear" in `format-text`, because the app
already sends `families: null`, `size: null` for "unchanged" and would
silently lose formatting. `set-alignment` takes `alignment: null`, which is
defined as exactly `clear: ["alignment"]` at its scope: the paragraph's direct
alignment for `at: null`, the line default for a point.

-   `set-style` without `clear` keeps its existing meaning: replace the
    direct overrides. With `clear` (even empty) it **patches**: supplied
    fields are set per property, listed ones cleared, the rest kept.
-   `format-text` with `clear` adds a formatting action whose listed fields
    mask earlier actions, revealing the paragraph style, like a per-field
    reset. Stored as an optional `clear` member of the `format1` patch, absent
    when empty, so existing envelopes are byte-identical.

## Resolved style in `State` (proposals 2 and 16)

-   `State.styles`: the live named styles (built-ins included), each with its
    parent and authored properties, sorted by name, at most 1,024.
-   `State.blocks[].style`: the one additive per-block field, always present:
    `{ name, direct, resolved }` where `direct` is the block's override DTO and
    `resolved` is an index into `State.resolved`.
-   `State.resolved`: the distinct resolved paragraph styles: family chain,
    size, line height, alignment, tabs, indents, spacing, keeps, weight, slant,
    decoration, colour and background, resolved in the first main frame of the
    effective page template.

Blocks that share a named style and have no direct overrides share one entry,
so output is bounded by the number of distinct styles, not blocks. The session
caches resolutions per (style name, overrides) for the current revision; a
`state()` call at the same revision resolves nothing again.

## Package bit

Required bit 13, `REQUIRED_PARAGRAPH_STYLES`, is declared when `styles1` or
`para1` exists (even emptied: its operations stay in history), when any nested
style or override map holds one of the new keys, when `alignment` is
`justify` anywhere (including `marks1`), or when a formatting envelope has a
`clear` member. An older engine refuses such a package. Bit numbers will
collide with parallel workstreams; 13 is the next free bit after 8495a66.

## How later features plug in

-   **`ws/lists`**: a list level is a named style (`List 1`…) with
    `indent-start` and a negative `indent-first`; the marker hangs in the
    first-line outdent, which `Indented` already reserves. Lists add marker
    properties as new keyword properties (the extension recipe below).
-   **Headings**: built-in `Heading 1`–`6` are named styles; their semantic
    role is read by `heading_level` from the name today. A `role` keyword
    property would make the role independent of the name: one new property.
-   **Style menu and paragraph dialog**: `State.styles`, `State.resolved`,
    `define-style`/`update-style`/`rename-style`/`delete-style`/`apply-style`,
    and `set-style` with `clear` for the dialog's per-property "inherit".
-   **Headers and footers**: their paragraphs name their own styles (built-in
    `Header`/`Footer` can be added to the built-in table); properties resolve
    in the header frame's context because resolution is per starting frame.
-   **Table cell paragraphs**: cells hold ordinary paragraph blocks, so they
    resolve the same properties; indents shrink the cell's measure through the
    same `Indented` provider once the table composer wraps its cell measure.

## Extension point: adding a paragraph property

`background` (paragraph shading) was added last, end to end, by exactly
these steps; follow them for the next property (a list marker, a border, a
heading role). Each step names the enum or table the property joins.

1.  **Store it** (`crates/doc/src/style.rs`). A length joins `Property` (and
    `Property::ALL`/`PARAGRAPH`): it then gets every stored form, the four
    stages, `em`/`lh`/`%` and expressions for free, with its percentage basis
    in `percent_basis`/`own_scope` and its sign rule in `clamps_negative`. A
    keyword becomes an `Option` field on `Style` (`background: Option<[u8;
    4]>`) with its stored key in `STORED_KEYS` (and `PARAGRAPH_KEYS` if it is
    new), a writer in `Style::stored`, a reader in `Style::apply_stored`
    (unreadable text goes to `unparsed_keywords`, kept verbatim), and an
    `unset` arm. `Style::write`/`read` pick it up from the same tables, and
    flat `styles1`/`para1` storage, patches and clears need nothing more.
2.  **Resolve it.** Inherit it in `Specified::compute` (the last readable
    layer wins, recorded in `keyword_layers` for `explain`), carry it on
    `Computed`, and publish the used value on `ComputedStyle` with
    `skip_serializing_if` at its default, so existing snapshots don't move.
3.  **Lay it out.** Geometry that changes line room belongs in the room a
    frame offers a paragraph (`flow::keeps::Flow::room`, `compose::Indented`);
    breaking behaviour belongs in the paragraph machine's keep step
    (`flow/paragraph.rs`); paint belongs in `layout/src/display.rs`. Shading
    is one filled path per line over its available interval, drawn first;
    because it adds display items, `background_items` keeps PDF reading
    addresses in step. Anything a paragraph's placement reads outside its own
    inputs must join `FlowKey` (as keep-with-next lookahead does).
4.  **Expose it** (`crates/reprise`). Add the field to the `Style` DTO (an
    optional, nullable field), to `ResolvedStyle`, to `StyleProperty` for
    `clear`, and map it in `convert::style`, `style_out`, `resolved_style`
    and `style_keys`; regenerate `types.d.ts`
    (`cargo run -p reprise --example generate_types`).
5.  **Export it.** HTML export writes it in `paragraph_css`
    (`clipboard/src/export.rs`, only when set) and HTML import reads it back
    in `html.rs`'s `css`. Native copy and paste carry `Style` as is. PDF gets
    it from the display list.
6.  **Declare it.** A new stored key joins `PARAGRAPH_KEYS`, so
    `Document::has_paragraph_styles` declares required bit 13 and older
    engines refuse the package. A property whose old-reader behaviour must
    differ from these (for example one an older reader may safely ignore)
    takes its own bit in `format/src/features.rs`.
7.  **Test it** in all three writing directions (`fixtures/tests/
    paragraph_styles.rs`), through the facade, and in a hostile fixture.

A note on clearing: clearing a property of a style whose base is a built-in
reveals the built-in's value (that is what "inherit" means for that layer);
set the property explicitly to turn a built-in's keep off.

## Review addendum: durable identity and compatibility (proposed, 2026-10-12)

### Selected implementation policy (2026-10-14)

The resumed brief requires completing the migration. Keep existing nested and
flat property keys as immutable document-scoped owners. `styles2` stores flat
`alias/<name>` selectors and `label/<owner>` display labels. Renames modify only
these selectors/labels; concurrent legacy-key property patches still reach the
same owner. Two peers' first patches retain the existing deterministic name
owner. Reusing a renamed/deleted name allocates a fresh owner from the local peer
and next operation counter; old block/parent references keep their former owner.
Identity tokens are opaque, versioned references distinct from display names.
Competing labels use Loro's deterministic per-key rule; label collisions receive
deterministic owner-hash suffixes, so each owner remains selectable.

Existing strings in block/parent fields are legacy owner references. New identity
references and built-in-v1 references use reserved control-prefixed strings in
the same field, not extra metadata operations. Public block/style queries decode
labels; resolution and incremental keys use the underlying references. Current
writers tag built-in use, including inheritance. Untagged, unmarked legacy
references use legacy fallback; explicit legacy definitions remain authoritative.
Existing paragraph-style opt-ins retain their current built-in interpretation.

Any new identity or built-in authoring creates `styles2`, even when empty. Root
presence persists in binary history/shallow snapshots without adding operations,
so node IDs/revisions and existing layout goldens stay unchanged. Required package
bit **18** declares the new semantics, including retained root presence after
undo. Required sync bit **4** (`style-identities1`) carries the same declaration
across JSON sync, including an empty root; decoding a successful declared packet
retains that root. Reads never create it or set the package bit. Unaware package
and sync readers refuse. Authored-graph duplication includes this root and reuses
its local tokens in the copy's fresh document namespace, preserving all edges
without importing source operation history. Partial clipboard remains an
appearance projection and does not transfer identity ownership.

The earlier redesign discussion below records the rejected eager property-copy
migration. The selected policy avoids its concurrent-adoption and snapshot-ID
problems; the previously deferred probes become normal regression tests.

This is a design addendum for the identity/compatibility migration. Review-fixes
Part A is independent of it. Durable identity and built-in adoption need a wider
migration design than a local rename patch, as described below. The independent
authored-graph copy correction can proceed without selecting that migration.
Required package bit **18** remains reserved for this work; bits 14–17 belong
to lists, inline, annotations and sections. No read may allocate identity,
materialise a built-in definition, or set a package feature bit.

### The compatibility ambiguity that must be resolved

Two documents can have the same authored form: a paragraph names `Heading 1`,
there is no definition of that name, and the required paragraph-style bit is
absent. One was authored before built-ins existed and rendered with the base
style; the other was authored by the current paragraph-style writer and was
intended to render as a built-in heading. Package format version 1 has no
mandatory writer-version or built-in-semantics marker that distinguishes them.
The test `format::container::paragraph_style_tests::
old_reader_refuses_paragraph_styles_even_after_undo_but_not_untouched_documents`
explicitly demonstrates that the latter still has no required style signal.

Consequently, neither always enabling nor always disabling built-ins on an
unmarked read can preserve both histories' intended appearance. Materialising
definitions on open violates read purity. Materialising them on every current
authoring path adds CRDT operations and changes revision/node identities in
`paragraph_styles__heading_kept_with_next.snap` and
`hostile__paragraph_style_storm.snap`, even if the geometry is the same. Those
existing goldens may not be changed under this brief. No such snapshot change
has been made here.

The integrator must select an explicit migration policy for ambiguous v1 files:
use legacy name resolution, use an externally established writer version, or
require a user/host-authored adoption operation that preserves current built-in
appearance. An unresolved name alone is insufficient evidence of an opt-in.
New authored references must carry a durable, versioned opt-in and require bit
18 (and a corresponding sync feature), so an older reader refuses them.

### Identity owns properties; names select and describe it

Introduce an opaque `StyleId`, scoped to the document, separate from its display
name. A live record's properties, parent reference, base kind and deletion flag
belong to that ID. Renaming changes a label/alias entry, never copies properties.
Deletion retains the record and parent edge; later property edits cannot revive
it implicitly. Reusing a name for a different style must not reuse its ID.

Use flat per-property storage (for example a version-2 style root with
`<property>/<id>` keys), retaining the existing independent-property CRDT merge
rule. Keep **base kind** (`defined`, legacy definition, or a pinned built-in
version) separate from **lifecycle** (live/deleted). Current `state/<name>`
overloads these facts, so copying an effective `StyleRecord` cannot recover the
full authored distinction reliably. Parent edges and block references bind to
IDs; aliases are retained selectors pointing to IDs, not new property owners.

Legacy records need a deterministic compatibility identity view that two peers
opening the same file compute identically without writing. The first authored
adoption must preserve concurrent first-property patches, which current tests
require to merge even when the style did not previously have a definition.
Simply allocating an independent ID on each peer's first touch loses one patch.
Likewise, moving legacy properties into a new record by copying them can race
with an already-open legacy peer's update. Specify adoption and queued-v1-sync
handling before replacing the existing keys.

Concurrent label changes use the CRDT's deterministic conflict rule. A label
collision must preserve both identities and their data, with an unambiguous
selector/query result or a deterministic disambiguated display name. The current
name-only commands cannot silently select a different identity after a collision
or name reuse. Add an optional identity selector to the facade commands and an
identity field to style query results if needed; append fields and regenerate
both declaration files. Legacy name selectors remain supported through aliases.

### Copy the authored graph, not clipboard appearance

Add a bounded authored-graph export/import boundary in `reprise-doc`. It carries
all records, parent edges, aliases, tombstones, base kinds and unreadable retained
values, including unused records. A duplicate allocates fresh identities and
remaps every internal edge/reference consistently. It retains the source's
built-in semantic version and authored patches rather than treating every
non-live built-in as a deletion.

`Workspace::import_copy` must install that graph independently of the clipboard
projection. Fragment style materialisation remains appropriate for partial
clipboard appearance, but must not overwrite the duplicate's remapped graph or
detach a renamed parent. This requires coordinating block/override/parent
references in `doc::{lib,style,fragment}`, editor paste/style commands, copy
plumbing, package detection, sync negotiation and the facade query cache. A
patch only to `rename_named_style` or `style_names` does not cover those seams.

### Acceptance tests for the dedicated migration work

- Rename versus a different-property patch, rename versus deletion, and competing
  renames converge in both delivery orders while retaining each authored effect.
- Two peers' first patches to the same legacy/undefined style both survive
  adoption; queued v1 updates have a documented, tested compatibility outcome.
- Empty and populated duplicates retain unused aliases and tombstones; changing
  a renamed parent afterwards changes the same inheriting styles in both copies.
- Applying an untouched built-in declares bit 18 and an old-known-mask reader
  refuses it; legacy undefined names follow the selected migration policy.
- Reads preserve stored bytes/revision and never set bit 18. Explicit definitions
  and all existing geometry/content goldens remain unchanged.
- Undo/redo retains identity through rename, deletion, name reuse and copy. Cyclic,
  malformed and over-limit graphs remain bounded and diagnosed deterministically.

Lists can reference style IDs for their levels while keeping labels editable.
Headings should eventually derive their semantic role from an authored role or
pinned built-in identity rather than a mutable display name. Headers/footers and
table cells share the same parent graph and context resolution. The style menu
uses stable identity for selection and display names for presentation; partial
clipboard and HTML remain explicit appearance projections.

### Implementation checkpoint: copying the existing authored graph

The current `styles` and `styles1` roots already encode the complete name-based
graph. The opaque native `AuthoredStyleGraph` captures both roots, including
unused records, raw property values, forwarding records and tombstones. It is
bounded before allocation and imported only into a fresh document with no style
roots. Container-valued properties outside the documented nested style-map form
are refused explicitly rather than silently flattened or discarded. This is an
in-memory transfer type, not a new persistence or clipboard format.

Whole-document duplication installs that graph into new history before
pasting content. A dedicated editor entry point for a whole-document fragment
preserves the installed graph: it validates and stages the same
content, range and relation operations as normal paste, but skips the appearance
projection's style clash/materialisation phase. The entry point requires an
empty content/style target, and preflights graph installation and paste together.
Partial clipboard paste keeps its existing projection. Capture is bounded at
65536 keys/entries, 16 MiB of keys/values and literal value depth 64.

This corrects inheritance after copying independently of durable identity.
No new package bit, stored representation, DTO or snapshot is needed for that
correction. The durable-identity and built-in migration remains subject to the
compatibility ambiguity and unchanged-snapshot constraints above.

`docs/style-migration-probes.rs` retains three standalone acceptance tests for
the deferred migration, outside the passing workspace gate. Against the current
native library, they fail with the renamed color absent, built-in use lacking a
required paragraph semantic signal, and an unmarked undefined heading resolving
to 20pt instead of its legacy 10pt fallback. Compile with `rustc --edition=2024
--test docs/style-migration-probes.rs --extern reprise_doc=<current rlib>
-L dependency=<target>/debug/deps -o <target>/style-migration-probes.exe`.
Run the resulting test executable; all three failures are expected until the
coordinated migration is implemented.
