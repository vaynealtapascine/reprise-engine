# Sections, columns, manual breaks and running regions

Decisions 01, 05, 06, 07, 11, 20, 24, 25, 26, 27, 28, 33, 34, 35, 37 and 38
apply. This is the design for the sections workstream, revision A.3. It answers
the advisor's first review (`reviews/sections-design-review.md`, R1–R11), the
second (`sections-design-review-2.md`, N1–N8), and the orchestrator's
decisions on both. The first draft is `fdfe2b4` and A.2 is `06d0896`. Ideas
adopted from
the cancelled GPT-6.1-Sol design (`eng-sections` `5b70673`) are marked
**[Sol]**.

The work is in four layers:

1.  **Stories and placements.** A story is authored content. A placement says
    where that content's frames come from. The body is one distinguished
    story, with the default paged placement. Headers, footers and a
    background are running placements: one copy per page.
2.  **Section boundaries.** Start records in a root map, keyed by the block the
    boundary sits before. Manual page and column breaks use the same records.
3.  **Pages.** One page factory and stable logical frame addresses, shared by
    every layout subsystem.
4.  **The seams later work plugs into.** Pageless mode, linked text boxes and
    the canvas.

Nothing derived from layout is written to Loro (05).

## 1. Stories and placements

| Concept | What it is | Stored? |
| --- | --- | --- |
| **Story** | An ordered run of blocks, edited with the ordinary text machinery. It has an identity of its own and doesn't know where it is shown. | Yes. |
| **Body story** | The distinguished logical story: the root's children that aren't story nodes. It has no node of its own, and existing documents are not migrated. | Yes, implicitly, as today. |
| **Placement** | Where a story's frames come from. | Yes, in the kind's own records. |
| **Instance** | One placement's realisation in a layout: a page's header, a page's band of body columns. | No, derived. |

A story has no slot, no section and no page. One story can be bound to several
placements: the same story as the header of three sections, or as both header
and footer. A story bound to no placement is still live, editable and
recoverable (§6).

There are four placement kinds. This workstream builds the first two:

-   **Body (default).** The body story flows through the sections' pages (§7).
    In pageless or canvas documents later, this placement may be **absent**:
    the body story then exists but isn't laid out. An empty body never stops
    other stories being placed.
-   **Running.** A section binds a story to a `(slot, variant)`. The binding is
    stored in the section's start record (§3). Each page the section owns gets
    one instance per bound slot.
-   **Chain (future: frames and canvas).** An authored, ordered list of
    frames, possibly shaped, rotated or vertical, that one story threads
    through, InDesign-style. One story flows through the whole chain. There is
    not a story per box.
-   **Canvas (future: frames and canvas).** Placements on an unbounded 2D
    plane, with an authored reading order across them.

What a later placement kind must supply:

1.  its own authored record root;
2.  a **frame source** for layout (§7.1);
3.  a variant of `FieldInstance` (§10);
4.  a hit-test and caret instance (§12);
5.  a reading-order policy: whether it's an artifact or semantic (§13);
6.  copy and remap rules (§14).

The seams in this design take a new kind without changing the stored form of
stories or sections.

## 2. Traversal domains (R1)

There is one `content` tree, which avoids splitting `NodeId` lookup across
trees. It has three **typed traversal domains**, so the compiler can see which
one each caller uses. The first draft's "filter `children(None)` and
everything works" was wrong, and the review showed why: `document_order()` is
used by image packaging and list numbering, and selection replacement starts
from `blocks()`.

```rust
pub enum Domain { Body, Story(NodeId) }

impl Document {
    pub fn order(&self, domain: Domain) -> Vec<NodeId>;     // that domain, document order
    pub fn top(&self, domain: Domain) -> Vec<NodeId>;       // its top-level blocks
    pub fn authored_order(&self) -> Vec<NodeId>;            // body, then each live story, in tree order
    pub fn stories(&self) -> Vec<NodeId>;                   // live top-level story nodes
    pub fn domain_of(&self, node: NodeId) -> Option<Domain>;
}
```

-   `blocks()`, `children(None)` and `document_order()` keep their signatures,
    and are defined as the **body** domain. They're documented as aliases of
    `top(Domain::Body)` and `order(Domain::Body)`. For documents without
    stories they return exactly what they return today. Their wording in
    `contracts.md` changes in the contract commit (§16).
-   Every caller is switched to an explicit domain function. Stage B removes
    every internal call of the aliases outside the facade. A clippy
    `disallowed_methods` entry for `reprise-doc`'s own crates keeps it that
    way, so a new caller has to choose a domain.

How each consumer is routed:

| Consumer | Domain |
| --- | --- |
| `structure.rs` children and `blocks` | Body |
| `position.rs:84`, locating a cursor | **Authored**, so carets work in stories |
| `invariants.rs:41`, including tombstones | Authored and tombstones |
| `persist.rs`, `hostile.rs` | Authored |
| Image resources, `image_assets.rs:24` | Authored: a header image is packaged |
| List numbering, which today walks `document_order()` | **Per domain**: the body numbers the body, and each story numbers itself (§20) |
| Selection replacement, `replace.rs:69` | The selection's domain |
| Ranges and annotations | The domain of the range's start. A range whose ends are in different domains is refused at creation, and resolves as unresolved if it arrives from a hostile peer. |
| Body reading order, outline, select-all, plain text | Body |
| Copy-all, `import_copy` | Authored, through the fragment envelope (§14) |
| History views (`history.rs:57`) | Per domain, with the same functions at a past version |

**Story nodes.** A story is a `content` node with meta `kind = "story"` and
`v = 1`, whose **parent is the root**.

-   Its children are ordinary blocks: flow hosts with breaks, embedded tables
    and images, and inline atoms. Stories are created by staging (07).
-   A `story` node anywhere other than directly under the root is
    **outside every domain**, and so is its subtree. `is_live`, `block`,
    `locate`, `domain_of` and every enumeration agree on this. `audit` reports
    it as Warning `story.nested`.
-   A story inside a story is the same case.
-   Start records inside a story are orphans (§3).

## 3. Section boundaries: start records (R3)

A boundary is "a section, page or column **starts at this block**". It is
keyed by the block's `NodeId`.

-   For a break paragraph, that `NodeId` is `host/mark`, so the boundary *is*
    the paragraph's U+FDD0 marker.
-   It also covers a host's head paragraph and legacy top-level tables, which
    have no break record. Embedded tables and images do have one (their embed
    record, `flow.rs:689–710`), and the block key covers them too.
-   It isn't copied by split. `stage_break` (`flow.rs:510–518`) copies only
    kind, style and overrides, so a new attribute on the record could also be
    left uncopied. So this isn't an argument against record storage. The
    argument against `overrides` stands: `overrides` *are* copied on split.

Section containers (candidate B) are rejected, because a flow host can't cross
a tree parent without copying text.

**Why block keys rather than record attributes.** Sol's alternative, an
attribute on the `breaks1` record, could cover break paragraphs and embeds.
It can't cover head paragraphs or legacy top-level tables without first
inserting a marker, which retires the head's identity. Sol's proposal
therefore restricts boundaries to paragraphs. Block keys cover every block
kind with one rule and one lookup, and leave `breaks1` records exactly as old
engines know them. That is a choice of **uniformity**; record storage isn't
impossible.

### Keys

The root map is `starts1`. It's a root, like `breaks1` and `inline1`, so
concurrent first writes don't overwrite each other's container. Keys are flat.

-   An **address** is a block `NodeId` in `Display` form, or `first`.
-   Address-level keys: `<addr>/active` (bool) and `<addr>/life` (a lifetime
    ID).
-   Lifetime-level keys: `<addr>/<life>/<prop>`.

Parsing works from the right:

-   The last `/`-segment is the property. `active` and `life` are reserved
    address-level names.
-   For any other property, the segment before it is the lifetime: a Loro ID
    in its `counter@peer` form, or `0` for `first` only. The rest is the
    address. Every lifetime-level key has a lifetime segment, so a key like
    `12@1/5@3/width` can only mean address `12@1`, lifetime `5@3`.
-   Keys that don't parse are retained and never rewritten. They are reported
    once per address, aggregated.

**Lifetimes (N1)** answer the address-reuse and atomic-reset point without
numbers. A boundary's **address** is stable: its block. Each lifetime has an
identity that is **never reused**: the Loro operation ID of a staged write.

-   **Minting.** `InsertBreak`, on an inactive or absent record, first makes
    an excluded (staged, non-undoable) commit whose first operation writes
    `<addr>/<L>/born = true`, where *L* is that operation's own ID
    (`Document::mint_lifetime`, which reads the peer's next counter from the
    oplog version vector and checks it after committing). Loro never reuses
    an operation ID, so *L* is unique in the document's history and across
    peers.
-   **Activation.** The undoable commit then writes `<addr>/life = L`, the
    properties under *L*, and `<addr>/active = true`.
-   **Redo keeps the identity.** Undo reverts `life`, the properties and
    `active`; the staged `born` key stays. Redo writes the same *L* back. A
    fresh insertion after an undo mints a new *L*, so it can't collide with
    the undone one.
-   **No counter.** No integer of ours is incremented, so there's no
    `i64::MAX` case. The lifetime's range is Loro's operation-ID space.
-   **No reset race.** The old lifetime's keys, including unknown ones, stay
    and are unread. A reset deletes nothing. A concurrent edit to the old
    lifetime lands there and stays inert.

**Divergent histories.** `life` and `active` are ordinary last-writer-wins
entries, so every merge order gives one result.

| Histories | Merged result |
| --- | --- |
| Two peers each reset the same address (lifetimes *L₂* and *L₃*) | `life` takes one of them. That reset wins whole, with **no** mixing of the two property sets; the other's keys stay inert. |
| A removes the boundary in lifetime *L₁*, while B has removed and reinserted it as *L₂* | `life = L₂` (only B wrote it). `active` takes A's `false` or B's `true`. If A wins, *L₂* is inactive: A's intent, "no boundary at this block", is kept. If B wins, *L₂* is active. |
| A undoes its own reset (`life` goes back to *L₁*), while B edits *L₂*'s properties | `life = L₁`. B's edits stay in *L₂*, inert, and come back if A redoes. Undo restoring an older lifetime is intended. |
| A reset and a concurrent property edit to the old lifetime | The new lifetime wins; the edit is inert. |

`first` is the document's leading section.

-   It has no `active` and no `life`. Its keys are `first/0/<prop>`.
-   It can't be removed and has no block, so liveness never applies to it.
-   Its `break` and `start` keys are ignored.

### Properties (per lifetime)

| Key | Value | Notes |
| --- | --- | --- |
| `break` | `section` \| `page` \| `column` | Required on a block address. Unknown values are retained, and the boundary is ignored with Warning `section.property`. |
| `start` | `new-page` \| `odd-page` \| `even-page` \| `continuous` | Section only. Default `new-page`. |
| `template` | the name of a stored page template | Section's base template. Absent means the document's. A name with no live stored template also uses the document's, with Warning `section.template` (N3). The seam for spatial (*House of Leaves*) sections (§19). |
| `mode` | `paged` \| `pageless` | The geometry-mode override (§18). Absent means the document's mode. |
| `width`, `height`, `top`, `right`, `bottom`, `left` | i64 units | A `PageSetupPatch` over the document default (§4). |
| `orientation` | `portrait` \| `landscape` | Swaps the merged width and height when they disagree. A square page never swaps. Margins stay physical. |
| `header.distance`, `footer.distance` | i64 | From the page edge. Default 36 pt. |
| `columns.count`, `columns.gap`, `columns.widths`, `columns.rule` | see §4 | |
| `direction` | `ltr` \| `rtl` | Column order only. Unicode bidi inside paragraphs is unchanged **[Sol]**. |
| `writing-mode` | `horizontal-tb` \| `vertical-rl` \| `vertical-lr` | |
| `numbering.restart` | i64 decimal string | Absent means continue. |
| `numbering.format` | lists `NumberFormat` token | Absent means the previous section's; the first section defaults to `decimal`. `bullet` and `none` are refused, with Warning **[Sol]**. An unknown token parses to lists' `NumberFormat::Unknown`, which renders decimal; that is reported as Warning `section.property`, not silently accepted. |
| `region.first`, `region.even` | bool | Variant switches (§9). |
| `region.<slot>.<variant>` | story `NodeId` \| `""` | `""` means explicitly none. **Absent means same as previous.** |
| `region.<slot>.writing-mode` | as above | Optional. v1 renders only `horizontal-tb` (§9). |
| `filler.regions` | `blank` \| `running` | Policy for the parity filler that **this** section's start creates (§7.4). Default `blank`. |

Only `break` and `start` mean anything on page and column breaks. Their other
keys are retained and ignored. So are unknown properties and unknown slots.
State reports raw keys it doesn't understand, so nothing is silently dropped.

**Inheritance (Q1, decided).**

-   Geometry, template, mode, columns, direction and writing mode come from
    the built-in template, then the document default, then the section's own
    keys. They are never taken from the previous section.
-   `InsertBreak` copies the containing section's own keys for this **exact
    set**:
    -   `template`, `mode`;
    -   `width`, `height`, `top`, `right`, `bottom`, `left`, `orientation`;
    -   `header.distance`, `footer.distance`;
    -   `columns.*`, `direction`, `writing-mode`.
-   It never copies `start`, `numbering.*`, `region.*`, `filler.*` or unknown
    keys.
-   Numbering format and region bindings are intentional cascades from the
    previous section in body order. `numbering.restart` never cascades.

### When a boundary is in effect

A block address is **effective** when all of these hold:

-   `active` is true;
-   `life` is readable and names a lifetime with a readable `break`;
-   `break` is readable;
-   the block is live;
-   the block is in the body domain's top-level order (`top(Domain::Body)`).

Otherwise it's an orphan. `audit` reports inactive records as Info
`section.inactive`, which is expected history. It reports everything else
(dead block, wrong domain, unreadable) as Warning `section.orphan-start`,
aggregated with a bounded, numerically ordered sample.

**First section and body index 0.** The section sequence is `first`, then
every effective section boundary in body order.

-   If an effective section boundary is on the **first** body block, `first`
    is an empty section. That boundary's section holds the body, and its
    `start` kind is ignored, because there's nothing to break from. No filler
    is added.
-   An empty `first` makes no pages, but **does take part in inheritance**.
    A later section that inherited `first`'s header still does after the body
    prefix is deleted.
-   A page or column break on the first body block has no effect.
-   **Empty body.** One page, owned by `first`, with its running regions. A
    story-only document behaves the same way.

## 4. Interpreting geometry (R4)

**Parsing.** Every value is read with explicit bounds, before any arithmetic.
Values are i64 in Loro. `Length` is `Length(pub i32)` (`geom/src/lib.rs:39`),
and there is no checked constructor from i64 today. The proposed helper is
`reprise_doc::sections::stored_length(v: i64) -> Option<Length>`, which is
`i32::try_from(v).ok().map(Length)` after the range check below. It's new, in
this workstream's own module, and changes nothing in `reprise-geom`.

| Property | Accepted |
| --- | --- |
| `width`, `height` | `1..=MAX_PAGE_LENGTH` (14,745,600 units) |
| margins, distances, `columns.gap`, `columns.rule` | `0..=MAX_PAGE_LENGTH` |
| `columns.count` | `1..=64`. Above 64 it's clamped to 64, with Warning `section.columns-limit`. |
| `columns.widths` | A CSV string of at most 1 KiB and at most 64 entries, each `0..=MAX_PAGE_LENGTH`. Its size is checked before it's split. The empty string means "no explicit widths", the same as absent, with no diagnostic. |
| `numbering.restart` | Any i64 |

A value outside its range is ignored. That property inherits instead, with
Warning `section.property`.

**Merge, then validate.**

1.  Overlay the section's geometry keys on the document default, property by
    property.
2.  Apply orientation.
3.  Validate the merged `PageDimensions` with `page_setup.rs`'s existing check:
    positive width and height, and a positive text area. So opposing margins
    are judged only after the merge.
4.  If the merge is invalid, the section uses the **whole** document default
    geometry, with Warning `section.geometry` **[Sol]**.

The document default has its existing template fallback.

**Logical axes.** Margins are physical.

-   The **inline extent** *A*:
    -   horizontal-tb: `width − left − right`;
    -   vertical-rl and vertical-lr: `height − top − bottom`.
-   Columns partition the inline axis. Bands advance along the block axis:
    down in horizontal-tb, leftward in vertical-rl, rightward in vertical-lr
    **[Sol]**.
-   Column widths are logical inline measures: physical heights in vertical
    sections.

**Columns.** Let *n* be the column count and *g* the gap.

-   **Defaults (N3).** Any column key switches to generated columns (rule 3
    under *Templates*). Then each absent key takes its default:
    -   `columns.count`: the selected template's main-thread frame count,
        which is 1 for the built-in template, clamped to `1..=64` **[Sol]**;
    -   `columns.gap`: 18 pt;
    -   `columns.rule`: 0, meaning no rule;
    -   `columns.widths`: equal widths. An empty CSV is the same.

    So a lone `columns.gap` or `columns.rule` on a one-column template gives
    one generated column with that rule setting and no visible gap.
-   **Arithmetic is checked i64.** *n* ≤ 64 and every length is ≤ 2²⁴, so
    `(n−1)·g` < 2³⁰, `Σw` < 2³⁰ and `W·w_i` < 2⁴⁸. All of these fit i64, and
    every sum and product uses `checked_add` or `checked_mul`. A `None` would
    mean the bounds above were bypassed; it falls back to one column with
    Warning `section.columns-fit`. Results are converted back to `Length`
    only after they are ≤ *W*, which fits i32.
-   `W = A − (n−1)·g`. If `W < n·MIN_COLUMN`, the section falls back to **one**
    column, with Warning `section.columns-fit`. `MIN_COLUMN` is 1 pt.
-   **Equal columns:** `q = W / n`, `r = W mod n`. The first *r* columns in
    thread order get `q + 1`.
-   **Weighted columns:** `w_i` from `columns.widths`.
    -   Base width: `floor(W·w_i / Σw)`.
    -   The remainder, `W − Σ base`, which is less than *n*, goes 1 unit at a
        time to the columns in thread order.
    -   `Σw = 0` falls back to equal widths, with Warning `section.property`.
    -   When `columns.widths` is valid it sets *n*. A disagreeing
        `columns.count` is ignored with Warning `section.property`.
-   **Rounding.** This is **integer apportionment of an exact total**, not
    `Length` division. Every quantity is non-negative and the remainder is
    distributed explicitly, so no rounding takes place and the frozen
    half-away-from-zero rule has nothing to apply to. Widths always sum to *W*
    exactly.
-   **Narrow columns.** A column under `MIN_COLUMN`, from a zero weight, is
    skipped by the thread, with Warning `section.column-narrow`. If all are
    skipped, the section falls back to one column.
-   **Rules.** `columns.rule` is clamped to *g*, with Warning when clamped.
    Its offset in the gap is `(g − rule) / 2`, integer floor. That's a
    placement of non-negative integers, again not `Length` division. Rules
    are derived display paths: never frames and never in reading order.
-   **Order.** Columns are ordered from inline-start: right to left for
    `rtl`, top to bottom in vertical modes.

**Templates.** These rules settle the first draft's contradiction between
keeping template frames "exactly" and applying section geometry.

1.  **No section geometry or column keys.** The template's frames are used
    exactly as today: `page_setup.rs:324` projects the document default onto
    the **first** main frame only.
2.  **Geometry keys, no column keys.** The same first-main-frame projection,
    with the merged dimensions. Other main frames, non-main frames, transforms
    and paths are untouched. `writing-mode`, if set, is applied to every main
    frame.
3.  **Any column key.** Generated rectangular main frames replace **all** main
    frames, inside the body rectangle the merged margins give.
    -   If a replaced frame had a path, a non-identity transform or a mirror,
        the section reports Warning `section.template` **[Sol]**.
    -   Non-main frames (notes, margin, other named flows) are kept as
        authored.
    -   `clear` of the last column key returns to rule 2.

## 5. Operations (R3)

These are edit-kernel commands. They work inside `batch`, respect
`expected_revision`, and validate the whole sequence before any staging. A
failed command writes no root and adds no undo step **[Sol]**.

-   **`InsertBreak { at, kind, patch }`.** `at` is a position or `Before(block)`.
    -   At offset 0 of a paragraph, or `Before(block)`: write the boundary on
        that block.
    -   Elsewhere: split, and write it on the paragraph the split returns.
        That includes `split_block`'s copy-and-lineage path for a split inside
        an inherited prefix after head deletion (`flow.rs:522–548`). Whichever
        `NodeId` the split returns is the address.
    -   On an **effective** boundary: refused with `Reason::BreakExists`.
        To change a page break into a section break, use `RemoveBreak` then
        `InsertBreak` in one batch.
    -   Refused in a story or a table cell.
    -   All three break kinds end the paragraph **[Sol]**: a page break inside
        `abcd` at byte 2 gives two paragraphs.
-   **`RemoveBreak { block }`** writes `active = false`. The paragraphs stay,
    and the content joins the previous section.
-   **`SetSection { section, patch, clear }`** writes keys in the current
    lifetime. `clear` writes nothing new; it removes that lifetime's key.
    Concurrent edits of other keys survive.
-   **Undo.** An insert reverts its map writes and, through the flow, the
    marker's activation. The marker and the lifetime were both staged, so
    redo gives back the same address and lifetime (§3).

**Deleting a boundary (Q4, decided).** A boundary lives and dies with its
block. A deletion removes every boundary strictly inside the range and keeps
the one at its start. That follows from `flow.md`'s join semantics, in which
the first paragraph keeps its identity.

-   Joining or range-deleting across a boundary: the earlier section's
    properties win.
-   Deleting exactly the first paragraph's text and end (`P.0..Q.0`) keeps the
    boundary on `P`.
-   Deleting a one-paragraph section together with the next boundary
    (`P.0..R.0`) keeps **`P`'s** section and drops `R`'s. Word keeps `R`'s.
-   `DeleteBlock` of a start block drops the boundary. There is no re-keying.
-   Undo of any of these restores the block, and with it the record, which was
    never touched. A section whose content is deleted down to its first,
    now-empty paragraph is still a section, and still makes its pages.

### Concurrency

| Concurrent edits | Merged result |
| --- | --- |
| Insert + insert at the same mid-paragraph offset | Two markers, with an empty paragraph between them, so an extra section and page. This is the same as two concurrent Enters. No coalescing. Both orders give the same result. |
| Insert + insert making the same block a boundary | Two lifetimes; `life` takes one, whole. One section, with exactly one peer's properties; the other's are inert. |
| Page break + section break at one block | The same: one lifetime wins, with its `break` kind and its properties. |
| Remove + property edit | Inactive, with the edit kept. Undoing the remove shows it. |
| Remove + reset from divergent states | See *Divergent histories* in §3. |
| Reset + property edit to the old lifetime | The edit lands in the old lifetime and is inert. |
| Concurrent edits to one story | Ordinary text merging. |
| Link-away + typing or binding | See §6. Nothing is deleted. |

## 6. Stories: bindings, orphans and recovery (R2)

**Bindings** are the `region.<slot>.<variant>` keys. A key has three states:

-   absent: Inherit, "same as previous";
-   `""`: None;
-   a story `NodeId`: that story.

Inherit resolves to the same variant of the nearest earlier section, in one
forward scan, so 10,000 sections take linear time with no recursion **[Sol]**.
`first` inheriting resolves to unset. A binding that names a node that isn't a
live, root-level story renders empty, with Warning `layout.running-missing`. It
never falls through to an earlier story.

**Commands:**

-   `CreateStory { blocks? }` creates a staged story with no binding.
-   `SetRegion { section, slot, variant, binding: Inherit | None | Story(id) }`
    binds a new story, an existing one, or an orphan.
-   `CreateRegion { section, slot, variant, from: Empty | CopyOf(story) }` is
    `CreateStory` then `SetRegion` in one step. **Unlink** is `CreateRegion`
    with `CopyOf` the effective story, with fresh IDs and formatting kept
    **[Sol]**.
-   `LinkRegion` is `SetRegion { binding: Inherit }`.
-   `DeleteStory { story }` is an explicit, ordinary soft delete: tombstone
    semantics, and undo restores it. Bindings to it then read as missing.

**What never deletes a story.** `SetRegion`, link, unlink, removing a section
and `RemoveBreak` only ever write binding keys. No command deletes a story
because it looks unreferenced. Reference counts are only locally true: one
peer links away while another binds the same story elsewhere, and a deletion
would leave the second peer pointing at a tombstone.

**Orphans.** A live story that no effective binding names is an orphan. It is
kept, never laid out, and still editable.

-   `State.stories` lists every live story with `bound_by: Vec<{section, slot,
    variant}>`; an orphan's list is empty.
-   `audit` reports orphans as Info `story.orphan`, aggregated with a bounded
    sample.
-   **Recovery doesn't depend on undo.** The app lists orphans, with their
    text, from `State.stories`, and rebinds one with `SetRegion` or deletes it
    with `DeleteStory`.
-   Concurrent "unlink" by two peers leaves one orphan. Its text is preserved
    and can be found from `State`.

## 7. Layout: pages, frames and bands (R6)

### 7.1 Page factory, frame addresses and frame sources

Every subsystem that makes pages or frames today does its own arithmetic from
one template's frame count. That's `flow::new_page` and `advance`,
`floats.rs:40–74`, `notes.rs:96–98` and `225–275`, `table_flow.rs:374`, and
`regions::ensure_page`. One new module, `pages.rs`, replaces all of them.

-   **`PageFactory`** is the only code that pushes to `snapshot.pages` and
    `snapshot.frames`. It creates a page of a given **kind**:
    -   `Content { section }`;
    -   `Filler { owner, requested_by }`;
    -   `NoteContinuation { anchor_section }`.

    It enforces the page limit once for all three. It records a
    `PageDescriptor` for each page:
    -   physical index;
    -   owner;
    -   counter and label;
    -   kind;
    -   bands;
    -   body area after running reservations.
-   **`FrameAddr`** is a stable logical address:
    -   `Main { page, band, column }`;
    -   `Other { page, role, ordinal }`;
    -   `Running { page, slot }`.

    The factory maps it to the snapshot frame index for that pass.
-   **Plan entries are keyed by `FrameAddr`, not by snapshot index.** That
    covers `Plan.reservations`, exclusions, note carries and placements.
    Balancing or a continuous start can change frame counts between passes, so
    each entry is checked against the current pass. A stale index can never
    land on the wrong frame **[Sol]**.
-   **Provenance, not just existence (N4).** An address can exist in two
    consecutive passes and mean different frames: band 1 of page 3 can belong
    to section *A* in one pass and section *B* in the next. So each plan entry
    records its frame's **provenance**:
    -   the owning section's ID and lifetime;
    -   the frame's resolved geometry: rectangle, transform and writing mode;
    -   for running frames, the slot and the story bound to it.

    An entry is used only when the current frame at that address has equal
    provenance. Otherwise it's ignored and the feedback loop re-derives it,
    exactly as for an address the pass doesn't produce. Provenance is part of
    the plan the 16-pass coordinator compares, so an entry that keeps
    changing its frame counts as non-convergence, not as a silent mismatch.
-   **One note area per page, projected into every band (N4).** v1 keeps one
    note area per page, the owner's notes frame. `notes.rs:225–275` already
    projects the occupied notes box into each intersecting main frame of the
    template and reserves from the earliest intersecting corner. The factory
    generalises "each main frame of the template" to **every current body
    frame on that page, across all bands**, by `FrameAddr`. So a tall note
    area that reaches an earlier band reduces that band's depth too, and can
    never overlap its text. A band that the note area has already overlapped
    in this pass is re-flowed in the next pass, through the ordinary feedback
    loop. Single-band pages give exactly today's reservations. Per-band note
    areas are deferred (§24).
-   **The layout loop consumes a `FrameSource`**: a resumable iterator of frames
    with the factory behind it. The body's source asks the factory for bands
    and pages section by section. A running instance's source is the
    **one-frame adapter**: one frame, after which it's exhausted. It never
    creates a page, so a story runner ends at its frame. A future chain source
    yields the chain's frames in authored order. Flow takes a block sequence
    from a `Domain` plus a `FrameSource` (§20).
-   **Note-continuation pages** that notes create after body flow ends are
    owned by **the owner of the page the carried notes come from**. That
    replaces A.2's "the note's anchor section", which couldn't say what
    happens when notes anchored in different sections overflow together. On
    a page with bands from sections *A* (owner) and *B*, notes anchored in
    both carry into **one shared** continuation page owned by *A*. They are
    never split into separate pages by section. A continuation page uses the
    owner's geometry, continues its counter and labels, shows its running
    regions as on any owned page, and counts against the same page limit.
-   **When the page limit is reached** while a parity filler is pending, the
    filler isn't made; the existing Error `layout.page-limit` covers it.

For documents without sections, the factory produces exactly today's pages,
frames and order. That is the S3 gate in §21: every existing snapshot is
byte-identical.

### 7.2 Ownership of a shared page

**The page owner is the section in effect at the page's top.** It owns:

-   the page size, the margins on the block axis, and the template's non-main
    frames;
-   the running regions;
-   the counter and label;
-   first and even variant selection;
-   the filler policy.

A continuous section starting on the page:

1.  closes and balances the previous band (§8);
2.  opens a band below it, with its own inline margins, columns, gap, rule
    and direction.

Exceptions:

-   If the continuous section's merged size, writing mode or `template` differs
    from the owner's, it acts as `new-page`, with Warning
    `section.continuous-new-page`. The authored start is kept.
-   If no band with positive depth is left on the page, it continues on a new
    page with no warning. That's ordinary overflow.
-   Its restart and first-page variant are **pending** until it owns a page
    **[Sol]**.

**Section and page queries (recommended).** A section that never owns a page
consumes neither a restart nor a first variant.

-   `section_pages(s)` returns `{ owned, occupied }`. Both are ranges and can
    be empty. `occupied` includes pages where *s* has only a band.
-   `page_sections(p)` returns the owner plus every section with a band on *p*,
    in band order.

### 7.3 Breaks

-   **Page break:** a new page, owned by the current section.
-   **Column break:** the next column of the band. In the last column, or with
    one column, it's a page break.
-   **Forced transitions.** Manual breaks override keep-with-next across the
    break; the existing unsatisfied-keep warning applies **[Sol]**. Balancing
    trials keep them as forced transitions (§8).
-   **No loops.** A band with zero room can't loop: every step advances a
    frame, consumes a unit, or reaches the page limit.

### 7.4 Parity starts and filler pages (Q2, Q3, decided)

`odd-page` and `even-page` test the **physical**, 1-based parity of the next
content page. Restarts change only labels: an odd physical page can show the
label "2". The same rule holds for both start kinds and for the `even`
variant. Exports that can't keep both, such as DOCX, which uses the number's
parity, report the approximation (§15).

**A filler page** is derived:

-   `PageKind::Filler`;
-   owned by the outgoing section: its geometry, and a **counted** label;
-   no body, no notes.

By default it is **visually blank, including the background**. If the section
that requested the parity break (the incoming one) sets
`filler.regions = running`, the filler shows the **owner's** running slots,
background included, and composes them as on any page the owner owns, never as
a `first` variant. The policy belongs to the requesting section because that's
where the author chooses the break.

### 7.5 Numbering

The counter starts at `first`'s restart, or 1.

-   A content page whose owner has a pending restart takes that value.
-   Every other page takes the previous page's counter + 1, saturating at
    `i64::MAX` with Warning `section.page-counter`.
-   Filler pages and note-continuation pages count.

The **label** is `NumberFormat::parse(fmt).render(counter)`. When that returns
`None`, the label is decimal, with Warning `section.page-number-range`.

## 8. Balancing (R5)

When a section *S* is followed by an effective continuous start on the same
page, *S*'s last band is balanced. The document's end and `new-page` endings
stay ragged.

**Inputs, all frozen for this pass:**

-   checkpoint *C*: the cursor at the band start, with fills, keeps state and
    pending notes;
-   the band's *n* columns and their fixed widths;
-   the field environment;
-   the band's exclusions and reservations, by `FrameAddr`;
-   endpoint *E*, the continuous boundary block.

**`fit(h)`** flows from *C* into *n* frames of depth *h* with the ordinary
rules (keeps, widows and orphans, table rows, column breaks as forced
transitions). It is **true only if all of these hold**:

-   the flow reaches *E*;
-   it never advances past column *n* or to a new page;
-   it omits nothing;
-   it uses no overflow fallback (a forced oversized line or row placement);
-   it reports no unsatisfied keep that the reference didn't also report.

**Reference (N2).** The band as first flowed by ordinary flow at full depth
*H* is complete and kept. Ordinary flow **forces** oversized content (for
example table cells, `table_flow.rs:625–646`), so reaching *E* doesn't prove a
fit. The reference is therefore checked with **the same predicate**, `fit(H)`,
evaluated on its own recorded outcome: no extra flow is needed.

-   **`fit(H)` holds.** The reference is the verified fallback, and the search
    below runs.
-   **`fit(H)` fails** (forced oversized content, an omission, or a new
    unsatisfied keep in this band). **Balancing is skipped.** The band keeps
    the ordinary flow's output and **its diagnostics**, unchanged. The band
    keeps its full depth *H*, so no room is left on the page, and the
    continuous section that follows **starts on a new page**, with Warning
    `section.continuous-new-page` (the authored start is kept). Nothing is
    balanced, and nothing is reported as balanced.

**Search.** Bisection over integers in `[1, H]`:

-   `hi = H` with result *R*; `lo = 1`.
-   While `lo < hi` and fewer than 24 trials have run:
    -   `mid = lo + (hi − lo) / 2`;
    -   if `fit(mid)`, then `hi = mid` and keep that trial's complete output;
    -   otherwise `lo = mid + 1`.
-   The committed result is the kept output for the final `hi`.

No `ceil(content / n)` lower bound is assumed; it's wrong for unequal widths
and spacing that depends on the frame.

**What is claimed.**

-   The result is always a **verified fit no deeper than *H***. It's the
    shallowest depth *tested* within the 24-trial bound.
-   It is the **minimum** fitting depth only when **both** hold: `fit` is
    monotone, **and** the search finished within 24 trials (`lo = hi`), which
    is guaranteed when *H* < 2²⁴. Monotonicity holds for the documented
    subset:
    -   one-column-width-class greedy text;
    -   fixed line metrics;
    -   no keeps;
    -   no floats or notes in the band;
    -   no atoms whose metrics depend on the frame.

    There it's the standard bisection result.
-   Outside that subset no minimum is claimed. The output is deterministic and
    feasible, which is what the snapshot promises.
-   There is no diagnostic for a non-monotone predicate. Bisection never tests
    below a failing point after finding a fit above it, so a violation can't be
    detected without extra trials, and the output is still what was asked: a
    balanced band.

**Bounds.**

-   At most 24 trials per band per pass.
-   *H* ≤ `MAX_PAGE_LENGTH` < 2²⁴ for validated section geometry, so 24
    trials reach one unit.
-   `template::resolve_template` doesn't enforce that bound, so a hostile
    template can give a larger *H*. The search then still stops at 24 trials
    with the shallowest fit found. That's deterministic, with Info
    `section.balance-approx`, since the output is still a balanced, valid band.
-   The total is the reference plus 24 trials per band, at most 32 bands per
    page, inside the shared 16-pass loop.

**Isolation.** Each trial runs on a scratch cursor and a scratch snapshot
segment cloned from *C*. Its diagnostics and placements are discarded unless
it's the kept trial. The committed band is exactly the kept output; no
replay is needed.

**Later passes.** Balancing is recomputed from scratch in every feedback pass
from that pass's inputs. Each pass recomputes and re-checks its own reference,
so there's never a retained depth that a later pass has made infeasible. If a
later pass's reference fails `fit(H)`, that pass skips balancing as above.

**Scheduling.** The search is resumable. See §11.

## 9. Running regions (R7)

### Slots: the extension point

`RunningSlots` is an ordered registry in `reprise-layout/src/running.rs`. A
slot supplies:

-   its name;
-   which variants it supports;
-   its box from the page geometry;
-   its reservation side (top, bottom or none);
-   its paint order;
-   its hit priority (§12);
-   its artifact type (§13);
-   its export form.

| Slot | Box | Reserves | Painted | Artifact |
| --- | --- | --- | --- | --- |
| `header` | The page's inline margins, from `header.distance` at the top edge, growing toward the body. | Top | Above the body | `/Pagination /Header` |
| `footer` | The same width, ending `footer.distance` from the bottom edge. | Bottom | Above the body | `/Pagination /Footer` |
| `background` | The whole page. | None | Below the body | `/Pagination /Watermark` |

`background` proves the extension: it has a different box, no reservation and
a different paint order. It is tested with a decorative watermark. Meaningful
one-off text set behind a page belongs to the future chain or canvas
placement, which may be semantic (§13).

Unknown slot keys are retained and ignored, with Warning `section.region-slot`.

### Variant selection

On a page the section owns:

1.  **first:** the section's first owned content page, `region.first` is set,
    and the first binding resolves to something other than unset;
2.  **even:** otherwise, `region.even` is set, the physical index is even, and
    the even binding isn't unset;
3.  **default:** otherwise.

An explicit None selects an empty region and stops any fallback. So a defined
`first` wins over `even`, and an unset `first` doesn't hide `even` **[Sol]**.

### Reservation budget (R7)

Take a horizontal-tb page with merged geometry *H*, *T* and *B*. Merge
validation guarantees `T + B < H`.

-   `D = H − T − B` is the authored body depth, at least 1.
-   `keep = max(1, D / 4)`, in integers. The body always keeps at least this
    much.
-   *h* is the header's need: `header.distance + composed depth`. *f* is the
    footer's.

The **priority is the header first, then the footer**. The order is
deterministic and uses no proportions **[Sol]**:

```text
top    = min(max(T, h), H − B − keep)
bottom = min(max(B, f), H − top − keep)
```

**Invariant:** `H − top − bottom ≥ keep ≥ 1`. Each term is ≥ its authored margin
unless it's capped, and caps only take from running reservations, never from
the authored margins.

When a cap takes effect, the region's frame depth becomes
`top − header.distance` (for the footer, `bottom − footer.distance`). That is
clamped at 0. If the distance itself exceeds the capped extent, the region gets
a zero-depth frame and shows nothing.

-   **Footnotes** allocate inside the body area that remains, with the existing
    note limits. A running reservation is applied first, as page geometry.
-   **Vertical pages.** Header and footer are still physical top and bottom
    bands. In vertical-rl and vertical-lr, those physical edges are the ends of
    the body's **inline** axis, so a reservation shortens the column length.
    The formula is the same physical one. Clipping is done in physical page
    space.
-   **Running text in vertical sections** is `horizontal-tb`. An authored
    `region.<slot>.writing-mode` other than `horizontal-tb` is rendered
    horizontal, with Warning `section.region-writing-mode`, because that
    changes requested output. A vertical section with no such key reports
    nothing, because nothing different was asked.

### Composition and clipping

A region is composed by the ordinary flow over `Domain::Story(id)` with the
one-frame source, whose depth is the capped frame. It **stops at the frame
end**, because the source is exhausted, never by making a page.

-   **Empty story** (no blocks): extent 0, reserves nothing. A story with one
    empty paragraph has its strut height.
-   **Clipping.** Content that doesn't fit after the cap is left out with
    **Error** `layout.running-clipped`, since content was omitted. The footer
    keeps its fitting **source prefix** and bottom-aligns it, so a tall footer
    loses its tail, not its head **[Sol]**.
-   **Oversized first unit.** A first line, image or table row taller than the
    frame is painted clipped to the frame, with Error `layout.running-clipped`.
    The rest is omitted.
-   **Bounded work** (R7: the cap bounds actual work, not just output):
    -   at most `MAX_STORY_BLOCKS` = 4,096 blocks and `MAX_STORY_TEXT` = 1 MiB
        of visible text are **read** per story per layout **[Sol]**; anything
        beyond is omitted with Error `layout.running-clipped`;
    -   preparation (itemise, bidi, shape) of a story paragraph is memoised
        once per layout, keyed as in §11, so a 1 MiB paragraph is shaped once,
        not per page;
    -   line composition per instance stops at the frame depth;
    -   a 100,000-paragraph story reads 4,096 and composes until full.
-   **Unsupported content.** Floats, footnotes and relation-placed blocks
    inside a story are not laid out in v1: **Error**
    `layout.story-unsupported`, because content is left out. Paragraphs, lists,
    tables (one frame, so headers never repeat), images and inline atoms are
    laid out.

### Instances

`RunningInstance { page, slot, variant, story, frame: FrameAddr, blocks,
clipped }` lives in `SectionsLayout` (§16). It is never in
`LayoutSnapshot.blocks`. Its `BlockLayout`s carry the story's `NodeId`s and
byte ranges, like `repeated_headers`. Its frame role is
`Flow("reprise.running.<slot>")`, never `main`.

## 10. Fields (R9)

This extends inline's existing seam, in `eng-inline2/crates/layout/src/inline.rs`.
It doesn't add one beside it. Inline already has `FieldKey { id, instance }`,
`FieldInstance::{Single, Page(u32)}` and a resolver that visits repeated table
headers.

```rust
pub enum FieldInstance {
    Single,
    Page(u32),                                    // existing: repeated body content on a page
    Running { page: u32, slot: SlotId, repeat: u32 },
    // reserved for frames-and-canvas: Chain { chain: ChainId, frame: u32, repeat: u32 }
}
```

-   **`slot`** is part of the key because one story can be bound to two slots
    on one page. *(page, slot)* determines the variant, so the variant isn't
    in the key. `SlotId` is the registry's dense index, a `u16`.
-   **`repeat` is the recursive qualifier.** Content repeated *inside* an
    instance keeps its parent instance and adds an ordinal. In v1 a running
    instance has one frame, so table headers never repeat inside it and
    `repeat` is always 0. Chains will use it for headers repeated across
    frames.
-   **Table cells inherit the instance (N6).** Today the ordinary table-cell
    compositions in `table_flow.rs:224,240,263` pass `FieldInstance::Single`
    explicitly, which would erase a running instance. They must pass the
    **containing** composition's instance instead, through the same
    `placed_as` path that repeated headers already use (`table_flow.rs:651`).
    Then a field in a table inside a story bound to two slots resolves
    separately in each slot, even though no header repeats inside the one
    frame.
-   **Enumeration.** The resolver enumerates running instance blocks for
    **assumed** values (while composing) and for **resolved** values (after
    the pass), as it does today for repeated headers.

Asks of `FieldEnv`:

-   **Labels (N6).** `FieldEnv` gains `labels: &PageLabels`, plus a
    `placed: bool` saying whether `env.page` is the page the line is
    **finally** placed on.
    -   **Placed** compositions (body flow, running instances, table cells
        inside either): `page-number` renders `labels.label(env.page)`
        directly. It doesn't read a plan value first, so a stale value from
        the page an atom moved off can't win **[Sol]**. This is exact in one
        pass, because labels are fixed when a page is made.
    -   **Unplaced** compositions keep previous-pass feedback. Annotations are
        composed with `FieldEnv::UNPLACED` before their page is known
        (`eng-inline2 layout/src/flow.rs:648–719`), so a direct label there
        would always be page 0. For them `page-number` reads the plan value
        for its key, falling back to `UNPLACED`'s page, exactly as inline does
        today. The after-pass resolver supplies the placed page's label, and
        the 16-pass loop settles it.
    -   `page-of` renders the target page's label.
-   **One page-count rule everywhere.** This replaces the first draft's two
    rules. `page-count` reads the plan's value for its own key. With no value
    yet, it uses the pages made so far (at least 1), as inline already does.
    It is the physical page total, in decimal, fillers included.
-   **`page-of` targets:**
    -   a target in the body, placed once: the label of its page;
    -   a target in a repeated table-header row: the authored row's page;
    -   a target in a story, which has many placements or none: `?`, with
        inline's existing unresolved warning **[Sol]**;
    -   a target in an orphan or deleted story: `?`.
    -   **The failure text is `?`, explicitly.** Today the resolver's failure
        path keeps `atom.text()` (`inline.rs:651–653`), which can be a
        previously resolved page. So a target that was resolved and then
        becomes unplaced would keep showing its old page. For `page-of`, the
        failure path must insert `?` instead. That's a change in inline's file
        (§20).
-   **Convergence.** All of these values are in the plan that the shared
    16-pass coordinator compares and freezes: running reservations, labels,
    field values (body and running) and note carries. A freeze never combines
    new field text with old metrics.
-   **Pageless.** Page fields render `?` in pageless sections (§18).

## 11. Incremental layout and scheduling (R8)

**Dependencies, not whole-plan keys.** Today `FlowKey` clones the **whole**
`Plan` (`incremental.rs:328–363`). So a reservation or field change on page 40
invalidates page 1. That changes as follows:

-   A transition records the plan entries it **read**, as
    `Vec<(PlanAddr, Option<PlanValue>)>`. That covers reservations and
    exclusions by `FrameAddr`, and field values by `FieldKey`.
-   A memo hit requires every recorded read to equal the current plan's value.
    This is decision 27's dependency-driven evaluation. It's exact, because a
    transition can only be affected by what it read.
-   The cursor key gains the section context: section ID and lifetime, its
    resolved properties, the page owner's counter, pending restart, the band's
    `FrameAddr`, and the band's geometry after reservation.
-   It never includes the whole section vector or the document revision.

**Running instance cache.** This is a separate memo. Its key:

-   the story's blocks' `Inputs`: text, resolved style chain, formatting runs,
    list markers from the story's own numbering, inline objects and their
    asset hashes;
-   the slot's box and frame context (writing mode, resolution context);
-   the `FieldKey` values it read, and the labels it read.

Fonts and the composer are fixed for the session's lifetime.

**What is reused.** Reuse is conditional on unchanged inputs. It is not
promised unconditionally.

-   **Body edit in section *k*.** Transitions before *k* hit their memos when
    their reads are unchanged. Tests assert hit counts for an unaffected prefix.
-   **Story edit.** Every instance of that story misses, across every section
    that binds or inherits it. If every reservation is unchanged, body
    transitions hit. Otherwise body re-flows from the first page whose
    reservation changed.
-   **Legitimate backward invalidation.** All of these are expected and tested:
    -   a later edit changes the page total, so an earlier header showing
        `page-count` recomposes, and if its extent changes, so does that
        page's body;
    -   shared and inherited stories span sections;
    -   balancing re-places *S*'s last band when *S* changes, which moves the
        continuous section after it on that page.
-   **Section property edit.** Misses from that section's first block. A
    numbering change reaches later labels, so later instances that read them
    miss.

**Scheduling.** These operations are resumable units charged to
`step(budget)`:

-   a section or band transition;
-   one balance trial: its start, then ordinary flow units, then its end;
-   committing a band;
-   one running instance: per block, then commit.

No unit contains a whole story or a whole search. A budget of 1 still makes
progress. Cancellation and staleness gates are unchanged.

**As built (S7c): the look-ahead.** A page is made by one unit, and its
regions are made by units of their own *before* it (`flow/ahead.rs`), the way
S5's balance trials are. Before any unit that can make a page, the cursor asks
the running memo for that page's regions; if one is missing a small machine
makes it, and the unit that makes the page only finds it in the memo.

-   **What can make a page.** A block unit moves past the last frame of its
    page at most once; a page break, a column break at the last column, and a
    section start make one; a section start with a parity filler makes two, so
    the filler is one unit and the content page (whose label is the filler's
    plus one) is the next. `finish` makes the pages the plan asks for one
    unit each. The first page waits for its regions too, so `LayoutJob::new`
    composes nothing, and an empty body still gets its page.
-   **The machine.** For one missing item at a time: the story's digest, read
    64 blocks a unit; then the composition, which is the ordinary flow over the
    story's blocks with the one-frame source, one flow unit at a time (the
    synchronous path runs the same machine to the end, so the two cannot
    differ); then the memo entry. A budget of 1 charges one such unit a step.
-   **Memoised transitions.** A replayed transition is judged by what the
    regions of the pages it made come to, which needs those regions. A block
    that finds one missing does not start; the machine makes it and the block
    starts again. A transition that made more than eight pages is redone
    instead of waited for.
-   **Counts.** `StepWork` has `pages_made`, `region_units`, `region_blocks`
    and `digest_blocks`; `WorkCounters::running_inline` counts regions the
    flow had to make itself because the look-ahead had not, and is zero in
    every test.

**Partial publication.** A page is **publishable** only when all its bands are
committed and its running instances composed. `partial()` cuts before the
first page that isn't. It drops every frame, rule, band, running instance,
`PageDescriptor` and field result for cut pages, as it already drops
`repeated_headers`. The partial view stays provisional until the job
converges, as today.

## 12. Editing and navigation in stories

-   Text, split, join, formatting, inline and annotation commands take story
    `NodeId`s unchanged.
-   A command whose range spans domains (body and story, or two stories) is
    refused with `Reason::CrossDomain`.
-   Select-all inside a story selects that story.
-   Section and break commands are refused in stories.
-   **Hit test.** `Navigator::hit_placed(point) -> Option<PlacedHit>`, which is
    additive. It returns `Body(hit)` or `Running { position, instance:
    RegionInstance { page, slot } }`.
    -   Header and footer win over the body in their rectangles.
    -   `background` is hit only when the app passes `HitMode::Background`.
    -   `Navigator::hit` is unchanged and body-only.
-   **Caret and selection.** `caret_in(instance, position)` and
    `selection_rects_in(instance, range)` are additive.
-   **The instance is view state held by the app**, never document state.
    After a relayout, if that page no longer shows the story in that slot:
    1.  use the nearest page that does, by physical distance, lower on ties;
    2.  if none does, return no geometry. The selection stays valid and is
        visible through `State.stories`.

## 13. Reading order and artifacts

Artifact policy belongs to the **placement kind and slot**, not to the story.

-   **In v1, running placements are defined as decorative or redundant**,
    and are always artifacts. That's a **scope decision** about what running
    slots are for, not a claim that repetition proves text redundant: a
    `first` variant, for one, can hold text that appears nowhere else. Text
    that must be read belongs in the body, or in a future chain or canvas
    placement, which can be semantic without changing the story. This is a
    design ruling, not PDF/UA certification.
-   The same story bound later to a chain or canvas placement can be semantic.
    That placement declares its reading order.
-   Body reading order and tagging are unchanged.
-   Column rules and filler pages carry no tags.

## 14. Clipboard and import_copy (R11, N7)

**Fragment envelope.** `Fragment.sections: Option<String>` is a versioned JSON
envelope (`v: 1`), omitted when absent **[Sol]**. Older strict readers refuse
it, as with annotations' optional field. It holds:

-   `mode`: `partial` or `complete`.
-   `boundaries`: `[{ block, active, break, start?, props, bindings }]`.
    -   `block` is the fragment-local block ID.
    -   `active` is the source record's `active` flag. A `partial` envelope
        only ever holds `true`; a `complete` one can hold `false` (N7).
    -   `props` are the raw keys of the source's **current lifetime only**,
        unknown ones included.
    -   `bindings` are its `region.*` keys, with story references written as
        fragment-local story labels.
-   `context`: `first`'s raw keys, including **its own `region.*`
    bindings**, also as story labels (N7). Present for select-all of the body
    and for `complete`.
-   `geometry`: the raw `geometry1` record (§18). `complete` only.
-   `stories`: `[{ label, blocks: [FragmentBlock], ranges }]`. Each source
    story is copied once.
-   `dropped`: a count of records that couldn't be mapped (below), so the
    paste can report them.

**Bounds** are counted against the fragment's existing aggregate size and
resource limits, not a second budget:

-   at most 16,384 boundaries;
-   at most 64 property keys per boundary or context, each value at most 2 KiB;
-   at most 256 stories.

A malformed envelope is refused **before any mutation**.

**Which boundaries are copied.**

-   **`partial`** (a selection): only **effective** boundaries strictly inside
    the selection, meaning the block is in the fragment and is **not** its
    first block. A boundary at the selection's start isn't copied, which
    matches deletion, so cut then paste leaves the source's leading boundary
    in place.
-   **`complete`** (`import_copy`, and select-all copied into an empty
    document) has **a leading-boundary exception** (N7). A boundary on the
    **first body block** is copied too. Without it, a document whose body
    prefix was deleted, so that its first block carries an effective section
    boundary, would lose that section; copying `first`'s context alone can't
    rebuild it. `complete` also carries inactive records whose block is in
    the fragment, with `active: false`.

**Lifetimes are never copied (N7).** A lifetime is a Loro operation ID in the
source document, and it means nothing in the destination.

-   Paste mints a **fresh** lifetime for every boundary it writes
    (`mint_lifetime`, §3). It writes the record's `props` under that lifetime,
    and `active` exactly as the envelope says.
-   Only the current lifetime travels. Older lifetimes' inert keys are source
    history, and aren't copied. Nothing visible depends on them.
-   So a pasted inactive record is inactive with a fresh lifetime, and an undo
    of the paste removes it like any pasted content.

**Dead and unmappable addresses (N7).**

-   A record whose block is **dead**, or outside the body domain, has no block
    in the fragment to attach to. It is inert in the source, so dropping it
    changes no layout. `complete` drops it, counts it in `dropped`, and the
    paste reports Info `clipboard.section-dropped`, with the count.
-   A record whose **story references** name stories that aren't live, or
    aren't at the root, keeps the binding as missing: its raw value is written
    unchanged, and still renders empty with `layout.running-missing`. It's
    kept, not dropped.
-   A record holding a container, or a future envelope version the reader
    can't remap, refuses the whole copy with a typed error before any
    mutation, rather than losing it.

**Paste.**

-   Each boundary is written on the pasted copy of its block: a new
    `NodeId`, a fresh lifetime, and the envelope's `active`. In `partial`, the
    text before the first copied boundary joins the destination's section.
-   **Inherited bindings.** In `partial`, at the **first** copied boundary,
    every Inherit whose predecessor isn't in the fragment is **materialised**
    as its effective value at copy time: a copied story, or None. So a paste
    never picks up unrelated destination headers **[Sol]**. Later boundaries
    keep Inherit. `complete` keeps every binding as authored, because the
    whole chain, `first` included, is in the fragment.
-   **Within one document**, story references stay as references to the same
    stories; there's no copy.
-   **Across documents**, stories are copied with fresh IDs, and sharing is
    kept, so one source story becomes one new story. That includes a story
    bound **only by `first`**.
-   `context`, including `first`'s bindings, is written to `first` only when
    the destination body is empty. Otherwise it's dropped, with Warning
    `clipboard.section-context`. `geometry` follows the same rule.
-   **Pasting into a story** drops every boundary and binding, with the same
    warning.
-   **Remapping.** One transfer map covers:
    -   host and marker IDs, through the new split markers;
    -   boundary addresses;
    -   story labels to new story IDs, in boundaries' **and** `context`'s
        bindings;
    -   ranges and annotations in stories;
    -   relations;
    -   inline `page-of` targets inside the fragment.

    Targets outside the fragment keep the existing cross-document rules.

**`import_copy`** goes through `copy_document_fragment` (`session.rs:178`) in
`complete` mode. It carries:

-   **every** live story, orphans included, which stay orphans;
-   inactive boundaries on live body blocks, as inactive;
-   `first` with its bindings, and `geometry1`;
-   raw unknown keys.

Copying an orphan never makes it bound.

**Tests for N7:**

-   a deleted body prefix, so the first block carries an effective boundary;
-   inactive records;
-   a story bound only by `first`;
-   a record on a dead block, reported as dropped;
-   a binding to a missing story, kept as missing.

## 15. Exports

**PDF**

-   Each page uses its own `MediaBox`.
-   Running instances are `/Artifact` marked content with the slot's type and
    subtype. The renderer's artifact metadata gains an optional run-to-subtype
    map (§16).
-   `/PageLabels` is written when every format maps to `/D`, `/r`, `/R`, `/a`
    or `/A`. Otherwise it's left out, with an export note.
-   The PDF/UA checks inspect the actual marked-content artifacts and confirm
    body tagging is unchanged.

**HTML**

-   Each section becomes `<section data-reprise-section="{v:1, props,
    bindings}">`, with CSS `columns` and `column-gap` for viewers.
-   Each story is emitted **once per document**, with a document-local label:
    -   as visible `<header>` or `<footer>` content in the first section that
        binds it as a default variant;
    -   otherwise as `<template data-reprise-story=label>`.
-   Bindings use the labels and keep the three states: absent (Inherit),
    `""` (None), and the label.
-   Background, first and even variants are bindings like any other, so
    they round-trip.
-   Page and column breaks become `break-before: page | column`.
-   **Geometry mode (N5).** A retained `geometry1` record is written raw as
    `data-reprise-geometry="{v, mode}"` on the export's root element, and a
    section's `mode` key travels in its `props`. Import reads both back. If an
    import target can't hold the record, the import reports it as a loss
    rather than dropping it silently.
-   **Fields.** If the export has a paginated layout, page fields carry the
    first owned page's text plus their semantic attribute. Without one, they
    carry `?`.
-   Losses (pagination, filler pages, per-page values) are reported.

**Plain text:** body only. Stories and pagination are reported as losses.

**DOCX** is design-level only; this workstream doesn't build it.

-   Each section's properties move to DOCX's trailing `sectPr`: on the
    paragraph before its start, and the final section's on the body.
-   `region.first` maps to `titlePg`. `region.even` maps to
    `evenAndOddHeaders`, which is a **document** setting, so mixed values are
    approximated and reported.
-   Inherit maps to an omitted `headerReference`, which DOCX also inherits.
-   None maps to an empty header part.
-   Physical parity versus Word's number parity is reported where they
    disagree.

## 16. Contract and compatibility changes (R10)

These all go in the orchestrator's single contract commit, **before Stage B
slice S1** (N8, §21). S1 already changes the frozen traversal wording and S2
adds error and state contracts, so the commit has to precede both. They are additive; existing signatures and omitted-empty
serialisation are kept.

1.  `LayoutSnapshot.sections: Option<SectionsLayout>`, with
    `#[serde(default, skip_serializing_if = "Option::is_none")]`. It's one
    field rather than the first draft's two **[Sol]**, and is `None` for
    documents with no sections, breaks or stories. It holds
    `pages: Vec<PageDescriptor>`, `running: Vec<RunningInstance>`, and the
    rules. `PageLayout`, `snapshot.template` and existing query signatures are
    unchanged.
2.  `FieldInstance::Running { page, slot, repeat }`, and `SlotId`. These are
    in inline's type and need its sign-off.
3.  `Navigator::{hit_placed, caret_in, selection_rects_in}`, plus
    `PlacedHit`, `RegionInstance` and `HitMode`. `Navigator::hit`, `Caret` and
    `Selection` are unchanged and body-only.
4.  `Fragment.sections: Option<String>`, as in §14.
5.  Display artifact metadata: an optional `artifact_kinds` run-to-subtype
    map beside the unchanged artifact run list.
6.  `Document` domain functions (§2), plus the contract wording that
    `blocks()`, `children(None)` and `document_order()` enumerate the **body**
    domain.
7.  Facade:
    -   the commands in §5 and §6;
    -   `State.sections`, `State.stories`, and `BlockState.starts`;
    -   the queries in §17.
8.  `reprise_edit::Reason::{BreakExists, CrossDomain}` (the kernel's error type is `Reason`).
9.  The diagnostic codes in §17.
10. **Package bit 17, `REQUIRED_SECTIONS`.** It is set when the retained
    authored state contains any of these:
    -   a `starts1` root, **even if emptied**;
    -   any key in it, active, inactive or unreadable;
    -   any `story` node, **live or tombstoned**;
    -   a `geometry1` root (§18), **even if emptied or malformed** (N5).

    It is detected without creating roots. Reads, empty patches and refused
    commands never create it. Sync feature negotiation refuses too, not just
    package save. An old reader's refusal is tested with a real package, not
    by checking a mask **[Sol]**.

Regression evidence for the contract commit: every existing layout snapshot,
package and fragment is byte-identical, including frame order and diagnostic
order.

## 17. Facade summary, limits and diagnostics

**State**

-   `sections`: each entry has:
    -   `id`, which is the address string or `first`, and `life`;
    -   `first_block` and `end`;
    -   `break`;
    -   `own` (raw and parsed);
    -   `resolved` geometry with provenance, plus columns, direction, writing
        mode, numbering, filler policy and mode;
    -   `bindings`, each as `{ slot, variant, authored: Inherit | None |
        Story, resolved: Option<story>, from_section }`;
    -   `unknown_keys`.

    Page indices come only from queries.
-   `stories`: `{ id, bound_by, blocks }`.
-   `BlockState.starts`.

**Queries**

-   `page_sections(p)`;
-   `section_pages(s) -> { owned, occupied }`;
-   `page_label(p)`;
-   `page_regions(p) -> [{ slot, variant, story, rect, clipped }]`;
-   `region_instances(story) -> [(page, slot)]`;
-   `page_fields(p) -> [{ atom, instance, text }]`.

| Limit | Value | Past it |
| --- | --- | --- |
| Columns | 64 | Clamped, Warning |
| Narrowest column | 1 pt | Skipped, Warning |
| Bands per page | 32 | The 33rd continuous start acts as `new-page`, Warning `section.band-limit` |
| Balance trials | 24 per band per pass | Shallowest fit found, Info `section.balance-approx` past 2²⁴ |
| Story read | 4,096 blocks, 1 MiB | Rest omitted, Error |
| Sections | Not capped | `new-page` sections are bounded by the page limit, and continuous ones by the band limit. Resolution is one linear scan. |
| Envelope | §14 | Refused before mutation |
| Field passes | 16, shared | `layout.field-unsettled` |

**New codes** (severity per 37: Info means as asked, Warning means differs,
Error means left out)

| Code | Severity |
| --- | --- |
| `section.property` | Warning |
| `section.geometry` | Warning |
| `section.template` | Warning |
| `section.columns-limit` | Warning |
| `section.columns-fit` | Warning |
| `section.column-narrow` | Warning |
| `section.continuous-new-page` | Warning |
| `section.band-limit` | Warning |
| `section.balance-approx` | Info |
| `section.page-counter` | Warning |
| `section.page-number-range` | Warning |
| `section.region-slot` | Warning |
| `section.region-writing-mode` | Warning |
| `section.inactive` | Info (audit) |
| `section.orphan-start` | Warning (audit) |
| `story.orphan` | Info (audit) |
| `story.nested` | Warning (audit) |
| `layout.running-missing` | Warning |
| `layout.running-clipped` | **Error** |
| `layout.story-unsupported` | **Error** |
| `clipboard.section-context` | Warning |
| `clipboard.section-dropped` | Info (N7: inert records with no live block, not copied) |
| `section.mode-unsupported` | Warning (N5: a pageless section laid out paged until S12) |

Every `section.*` diagnostic is sorted by section order, then key.

## 18. Pageless: the seam only

**Stored form.** A versioned **geometry-mode record**: root map `geometry1`,
with `v = 1` and `mode = paged | pageless`. It isn't a seventh `page-setup1`
key: that reader refuses unknown keys, and its contract fixes six. A section
overrides it with its `mode` key. It's absent by default, which means
`paged`, so nothing changes for existing documents.

**Viewport.** The engine's existing `Medium { width, height }` input
(`page.rs:44`) supplies the viewport. It is an engine input and part of the
session identity, and is **never stored**. The viewport's **inline** extent
is taken along the section's inline axis (N5): `Medium.width` for
horizontal-tb, and `Medium.height` for vertical-rl and vertical-lr. It never
comes from `Medium.width` alone.

**Compatibility and transfer (N5).**

-   `geometry1`, including an emptied or malformed one, sets bit 17 (§16).
-   Complete copy carries it raw, in the envelope's `geometry` field (§14).
    Partial paste writes it only into an empty destination, like `first`.
-   HTML preserves it as `data-reprise-geometry` (§15).
-   An unknown `mode` value or `v` is retained, laid out `paged`, and reported
    as Warning `section.mode-unsupported`.

**What Stage B settles** (in S1 and S4):

-   the record and the section key;
-   reading them;
-   resolving each section's mode;
-   reporting the mode in `State`.

A pageless section, in this seam-only state, lays out **paged** with Warning
`section.mode-unsupported`. The output differs from what was asked, but no
content is lost.

**What the full slice (S12) adds,** only if the frame-source work lands
cleanly:

-   **Frames.** A pageless section's body source yields frames of the
    viewport's inline extent (above) minus the section's inline margins, each `PAGELESS_CHUNK` deep
    (fixed, such as 2,048 pt). They are joined without breaks, so the depth is
    unbounded in principle but always produced in bounded, checked chunks, and
    `Length::MAX` never stands in for infinity.
-   **Mixed modes.** A pageless section starts a new sheet; the next paged
    section starts a new page.
-   **Inactive features.** Manual page and column breaks and running bindings
    are kept but inactive in pageless sections, with Info. Page fields
    render `?`.
-   **Export.** PDF and print require an explicit paginated configuration,
    which lays pageless sections out paged with their merged geometry.

## 19. Generalisation to text boxes, canvas and the canvas map

These belong to a later **frames and canvas** workstream. This design reserves
the seams it plugs into. It does not design them in detail.

| Requirement | Fit | Seam reserved here |
| --- | --- | --- |
| Linked text boxes | **Needs its own primitive** (a chain placement) | Story identity independent of placement (§1). A `FrameSource` that yields a chain's frames, shaped and transformed, with the existing geometry providers (§7.1). `FieldInstance::Chain` (§10). The placement-owned artifact policy (§13). Chain topology is owned there: merge-safe order, cycles, multiple successors, wrap and exclusions, copy and remap. |
| Infinite canvas | **Needs its own primitive** (canvas placement and document composition) | The body is a logical story whose default placement may be absent (§1). Empty-body rules (§3). `authored_order` (§2). Canvas owns unbounded coordinates with checked limits, cross-placement reading order, bounded spatial queries and print tiling. |
| Canvas map | **Fits with this split now** | Placement **bounds** (authored, or resolved without shaping) are kept separate from story **extents** (from layout). `SectionsLayout` already separates frame rectangles from composed extents. Canvas adds a frame index and `bounds(placement)` and `extent(story) -> Known \| Estimated \| Pending`, tied to revision and layout inputs. The minimap UI is separate. |
| Spatial sections | Fits | A section's `template` key (§3) selects an authored spatial template. Its frames and reading-order relations are kept when no column key is set (§4, rule 1 or 2). |

## 20. What this needs from wave 1

**Inline** (`eng-inline2`; not merged yet)

-   the `FieldInstance::Running` variant and its enumeration (§10);
-   table-cell compositions passing the containing instance, not `Single`
    (`table_flow.rs:224,240,263`, §10);
-   `FieldEnv.labels` and `FieldEnv.placed`: direct labels for placed
    compositions, previous-pass feedback for `UNPLACED` annotations (§10);
-   `page-of`'s failure path inserting `?` instead of `atom.text()`
    (`inline.rs:651–653`);
-   flow callable on `(Domain, FrameSource)`, which is the refactor in S3.

**Lists** (merged at `95b74ef`; checked against the merged code)

-   `NumberFormat::{parse, token, render}` (`lists.rs:96,112,136`) as
    documented. `render_or_decimal` (`lists.rs:157`) already returns the
    decimal fallback plus a flag, which drives `section.page-number-range`.
-   **Per-domain counters (N8).** `number_lists` (`lists.rs:867–917`) walks
    `document_order()` with one `counters: BTreeMap<ListId, …>` for the whole
    document. Once `document_order()` means the body (§2), story list items
    would silently stop being numbered. The change:
    -   walk `order(Domain::Body)`, then each `order(Domain::Story(s))` in
        `stories()` order;
    -   **reset `counters` at the start of each domain**, so a story numbers
        its own lists from their start values. The body's lists never count
        story paragraphs, and story lists never count body paragraphs;
    -   keep the result as one aggregate, `ListNumbering.items:
        BTreeMap<NodeId, ListItem>`, which now covers every domain.
-   **The cache stays one aggregate per revision, now covering every
    domain.** `list_numbering()` (`lists.rs:750–773`) caches one
    `ListNumbering` per state frontier. The aggregate now holds every
    domain's items, each computed with its own counters, so the one cache is
    correct for all of them. A story edit changes the frontier and
    recomputes, as any edit does today.
-   **`list_item(node)` is correct for stories** (`lists.rs:750`): it looks up
    the aggregate by `NodeId`, and story items are in it.
-   **The layout consumer needs no change.** `list_marker::semantics`
    (`list_marker.rs:39–49`) reads `numbering.items` and `numbering.notes` by
    node. Story paragraphs get their own markers through the same lookup.
-   Tests:
    -   a list in a story numbers from 1 while the body's list continues;
    -   a story edit doesn't renumber the body;
    -   `list_item` for a story paragraph.

These are changes in lists' file. They are additive. I make them in S2 and
call them out in the report.

## 21. Build order: Stage B slices (N8)

Each slice is one reviewable unit with its own tests, and ends green on the
full check. The gates follow the advisor's slicing. A slice marked
**partial** can start, but its acceptance tests that depend on inline or
lists aren't claimed until the matching adapter slice lands.

**Before inline merges** (lists is already merged)

| Slice | Builds | Tests |
| --- | --- | --- |
| **C: Contract commit** (orchestrator reviews) | §16, as type, field and doc changes in `docs/contracts.md`, with no behaviour. **Before S1.** | Regression evidence: existing snapshots, packages and fragments are byte-identical. |
| **S1: Storage** | Domains (§2) and the clippy guard; `starts1` with lifetimes (`mint_lifetime`); stories; `geometry1`; bit 17; `audit`. | Traversal by domain; nested and deleted stories; boundary on the first block; deleted body prefix; empty body; bit 17 for inactive, unreadable, emptied and tombstoned state, and for `geometry1` alone; reads create no roots; an old reader refuses. |
| **S2: Commands** (non-list part first) | Breaks, `SetSection`, stories and bindings, `CrossDomain`; `State` (sections, stories). Then the per-domain lists change (§20). | Offset 0, end, and inherited-head copied splits; joins across hosts; embedded tables; multibyte and emoji offsets. **Two-peer matrix:** insert+insert, insert/remove, remove+edit, reset+edit, page vs section, **divergent remove/reset histories and undo of a reset (N1)**, all merge orders, save and reopen, undo and redo, stable IDs. Link-away vs typing and binding; simultaneous unlink; orphan recovery; body/story cross-range refusal. Story lists (§20). |
| **S3: Page factory** | `PageFactory`, `FrameAddr` with provenance, `FrameSource`; plan entries by address; floats, notes (projected across bands), tables and `ensure_page` moved onto them. **No output change.** | Every existing snapshot is byte-identical; native and WASM are equal. **The byte-identity gate is rerun after inline is integrated**, because inline changes the same flow, notes, tables and plan code. |
| **S4: Geometry** | Section table, merge and validation, column defaults, templates, page and column breaks, parity fillers and filler policy, ownership, decimal counters, mode resolution. | Portrait→landscape→portrait; three unequal columns with a rule; lone `columns.gap`; empty CSV; unknown template; odd-page filler; RTL; vertical with asymmetric margins; negative, extreme and huge-CSV properties; combined invalid margins; 1,000 columns; all-narrow columns; custom transformed templates; filler at the page limit; sorted diagnostics. |
| **S5: Bands and balancing** | Continuous bands; resumable balancing; the `fit(H)` check on the reference. | 1→2→1 columns on one page; unequal widths with keeps, widows, tables, oversized rows, floats and notes; **a reference that fails `fit(H)` (a forced oversized cell): no balancing, diagnostics kept, next section on a new page (N2)**; non-monotone trials; an infeasible later pass; *H* > 2²⁴; 33 bands; **notes from two anchor sections sharing a continuation page, and a note area projected into an earlier band (N4)**; budgets 0 and 1. |
| **S6a: Running regions, plain** (partial) | Slots, variants, reservation, the story runner on the one-frame source, instances, navigation and hit-testing, without fields. | First and even headers; three-section inheritance and explicit None; tall header; header and footer both tall, plus notes across bands; 100,000-paragraph and 1 MiB single-paragraph stories; one story in several slots and pages; caret fallback; story lists in a header. |
| **S7a: Incremental machinery** (partial) | Read-set keys with provenance; the running cache; scheduling units. | Full equals incremental after body, header, style, image and geometry edits; reuse of an unaffected prefix is asserted; plan entries with changed provenance are ignored; varied chunking, cancellation and staleness; no unbalanced partial page. |

**After inline merges**

| Slice | Builds | Tests |
| --- | --- | --- |
| **S8a: Field adapters** | `FieldInstance::Running`; instance propagation through table cells; `FieldEnv.labels` and `placed`; running blocks in assumed and resolved enumeration; `page-of` failure text; shared convergence. **Rerun the S3 byte-identity gate.** | Moved annotations with `page-number` (feedback, not page 0); a field in a table inside a story bound to two slots; a formerly resolved target that becomes unplaced shows `?`; `page-count` backward invalidation; digit-boundary oscillation and the 16-pass freeze. |
| **S6b, S7b: Acceptance** | The field parts of S6 and S7. | S6's and S7's field tests, which are accepted only now. |

**After lists, the numbering parts** (lists has merged; these parts consume its
numbering)

| Slice | Builds | Tests |
| --- | --- | --- |
| **S8b: Labels** | Non-decimal labels through `NumberFormat`; restarts; `page-of` labels. | Roman then decimal restart; an unknown format token; a counter roman can't show. |

**Then**

| Slice | Builds | Tests |
| --- | --- | --- |
| **S9: Clipboard** | The envelope, paste and `import_copy`. It can be designed earlier, but complete remapping needs both integrations. | Internal and endpoint boundaries; the N7 tests (§14); same- and cross-document paste; paste into a story; remapped field targets; a malformed envelope refused before mutation; story images in packages and `import_copy`. |
| **S10: Exports** | PDF, HTML, plain text. | MediaBox per page, labels, artifact subtypes, PDF/UA with body tags unchanged; HTML round-trip of sections, links, None, variants and `geometry1`, with honest losses; plain text is body only. |
| **S11: Facade and WASM** | Bindings, types, worker. | Native and WASM equality; the full check. |
| **S12: Pageless** (optional, gated) | §18's full slice, only if S3 and S5 landed cleanly. | Viewport resize with no authored writes; vertical pageless uses the viewport's height; explicit paginated export. |

## 22. How later features plug in

-   **Header and footer editing; a section and page-setup dialog.** The app
    uses `hit_placed` and `caret_in` for the instance, and the ordinary text
    commands for editing. `State.sections` gives `own`, `resolved` and
    bindings. `SetSection` and `clear` change properties, and `SetRegion` and
    `CreateRegion` change bindings.
-   **A table of contents with page numbers.** Entries are `page-of` fields
    that render the target's label, so roman front matter and a decimal body
    are right. `section_pages` scopes a TOC to sections.
-   **Footnotes per section.** A `notes.restart` start key, read by the notes
    allocator from the section table.
-   **Endnotes at a section's end.** A derived band placed before the next
    section's first band.
-   **Mixed page sizes in print and PDF.** Per-page `MediaBox`, `PageDescriptor`
    and `PageLabels`. Imposition reads the filler kind.
-   **Spreads and facing pages.** Physical parity, filler descriptors and the
    `even` variant are the start. Mirrored `inside` and `outside` margins
    become new geometry keys resolved by parity.
-   ***House of Leaves* spatial pages.** A section's `template` names a spatial
    template. Chain and canvas placements (§19) hold text set across or behind
    pages, which may be semantic.

## 23. App seam

-   **Commands:**
    -   `insert_break`, `remove_break`, `set_section`;
    -   `create_story`, `set_region`, `create_region`, `link_region`,
        `delete_story`;
    -   text commands on story positions.

    All are inside `batch` with `expected_revision`.
-   **State:** `sections`, `stories`, `BlockState.starts`.
-   **Queries:** as in §17, plus `hit_placed`, `caret_in` and
    `selection_rects_in`.
-   **App policy, not engine policy:**
    -   Backspace at a section start calls `remove_break` first, then joins.
    -   Double-clicking a header edits that instance.
    -   Typing into an inherited header edits the shared story. The app offers
        unlink first; the engine never copies on typing **[Sol]**.

## 24. Decisions and remaining questions

**Decided.**

-   **Q1:** copy on insert, with the exact property set in §3.
-   **Q2:** physical parity everywhere.
-   **Q3:** fillers are blank, with a policy owned by the requesting section.
-   **Q4:** a boundary dies with its block.
-   **Q5:** the contract commit (§16).
-   **Q6:** one tree with typed domains.
-   **Q7:** extend `FieldInstance`, and flow over a `FrameSource`.

**Still open:**

1.  **Direct page numbers for placed atoms.** §10 now makes labels direct
    only where the final page is known, and keeps feedback for `UNPLACED`
    annotations (N6). That still changes inline's "plan value first" order
    for placed body atoms, so it needs inline's agreement at S8a.
2.  **Band-level note areas.** A page with bands has one page-level note area
    in v1. Per-band areas belong to the section-endnotes follow-up.

## 25. Risks

-   **S3's refactor** moves every subsystem's frame arithmetic onto one
    factory and switches plan keys to addresses. Byte-identical snapshots are
    the gate. If it slips, every later slice slips.
-   **Read-set memo keys** are a new invalidation mechanism. A missed read is
    a stale-output bug. The full-versus-incremental equality tests are the
    guard, run on every fixture.
-   **Lifetime minting** (`mint_lifetime`, §3) reads the peer's next
    operation counter before an excluded commit. If Loro's commit ever
    reorders or merges operations, the check after committing fails the
    command, rather than writing a wrong lifetime. S1 tests that.
-   **Header, field and page-count feedback** is a real cycle. It's bounded by
    the 16-pass freeze, but a header with `page-count` at a digit boundary can
    freeze with a Warning.
-   **Balancing cost:** up to 25 band flows × 32 bands × 16 passes on one
    pathological page. It's resumable and bounded, but slow in that case.
-   **Domain routing:** any consumer left on a body alias leaks or drops
    stories. The clippy guard, plus a story in every hostile and invariants
    document, catches this.
-   **Wave-1 asks** (inline's `FieldInstance` and `page-number` order, lists'
    per-domain counters) land in other workstreams' files.
