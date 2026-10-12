# Sections, columns, manual breaks and running regions

Decisions 01, 05, 06, 07, 11, 20, 24, 26, 27, 28, 33, 35, 37 and 38 apply.
This note is the design for the sections workstream. It is written before
the code, against `flow.md`, `paragraph-styles.md`, `incremental.md` and
`annotations.md` on engine `main` (f13a4ee). It also relies on two wave-1
designs that haven't merged yet: lists (`NumberFormat`) and inline (fields).
What this work needs from each of them is listed under *What this needs from
wave 1*.

A document divides into **sections**. Each section can have its own page
geometry, columns, page numbering and running regions (headers, footers and a
background). **Manual page and column breaks** use the same boundary
mechanism. Running regions are **stories**: authored content stored outside the
body and composed again for each page.

## Building blocks

| Thing | Authored (stored) | Derived (layout) |
| --- | --- | --- |
| Section boundary, page break, column break | a **start record** in root map `starts1`, keyed by the block the boundary sits before | the section table, and the pages and bands |
| First section's properties | `starts1` under the reserved ID `first` | |
| Document default page setup | `page-setup1`, unchanged | |
| Header, footer and background content | a **story**: a top-level content-tree node of kind `story` whose children are ordinary blocks | one **running instance** per page and slot |
| Which story a section shows | `starts1` keys `region.<slot>.<variant>` | the resolved slot table, with "same as previous" applied |
| Page numbers | `numbering.*` keys | a label per page, and field values per placement |

Nothing derived is stored (05). Running instances, blank filler pages, column
frames, balanced heights, page labels and field texts are all thrown away and
recomputed.

## Why boundaries are records keyed by block identity

The brief offers three representations: a break kind in the flow host text
(candidate A), section container nodes in the tree (B), and boundary records on
leading blocks (C). This design chooses **C, keyed by the block's `NodeId`**. For
a break paragraph, that identity *is* the U+FDD0 marker of candidate A, so for
most paragraphs C and A are the same boundary. Keying by `NodeId` also covers
the three block kinds that have no marker:

-   **break paragraph:** `NodeId` is `host/mark`, so the boundary is the marker
    itself (candidate A);
-   **head paragraph of a host:** `NodeId` is the host's tree ID; there is no
    marker before it;
-   **embedded table or image:** the embedded node's tree ID. An embed's marker
    could carry the boundary, but a non-embedded top-level table (older
    documents) has no marker.

So a boundary is "a section (or page, or column) **starts at this block**".
That's the same whatever kind of block it starts at.

Why not put the fields on the `breaks1` record itself:

-   **Split.** Split copies a record's kind, style and overrides into the new
    paragraph (`flow.md`). A boundary must not be copied, or pressing Enter at
    the end of a paragraph after a page break would add a second page break.
    The same applies to making page-break-before a paragraph style property:
    `overrides` are copied on split, and a style is inherited by every paragraph
    that uses it. A boundary is structure at one place, not style.
-   **Head paragraphs.** These have no record. Adding one means inserting a
    marker before the head, which retires the head's identity (`head = false`).
-   **Old readers.** Records stay exactly as old engines know them. The package
    bit is what refuses (*Package*), but a document opened by a buggy reader
    still has well-formed paragraph records.

Candidate B (container nodes) is rejected. A flow host can't cross a tree
parent, so moving the boundary would copy text and lose character identity,
which is what `flow.md` exists to prevent. It would also change every
`children(None)` walk in the engine.

### The start record

`starts1` is a root map. Root, as with `breaks1` and `inline1`, so that
concurrent first writes don't overwrite each other's container. It uses the
flat-key pattern of `para1` and `styles1`: one map entry per property, key
`"<id>/<prop>"`, split at the **last** `/`. Property names never contain `/`;
they use `.`, and `NodeId`s may contain `/`. Concurrent edits to different
properties therefore both survive.

`<id>` is a block's `NodeId` in its `Display` form, or `first`.

| Key | Value | Meaning |
| --- | --- | --- |
| `active` | bool | The boundary is in effect. Removal writes `false` and keeps the other keys (see *Operations*). |
| `break` | `section` \| `page` \| `column` | What starts here. |
| `start` | `new-page` \| `odd-page` \| `even-page` \| `continuous` | Section only. Default `new-page`. |
| `width`, `height`, `top`, `right`, `bottom`, `left` | i64 layout units | A `PageSetupPatch` over the document default. Same bounds and checks as `page_setup.rs`. |
| `orientation` | `portrait` \| `landscape` | Swaps the resolved width and height when they disagree with it. So "landscape" works without restating the size, and it reflows if the default size changes. Margins aren't rotated. |
| `header.distance`, `footer.distance` | i64 | From the page edge to the region. Default 36 pt. Clamped to `0..=height/2`. |
| `columns.count` | i64 | 1 to 64 (*Limits*). Absent: the template's own main frames, exactly as today. |
| `columns.gap` | i64 | Default 18 pt. |
| `columns.widths` | comma-separated i64 | Optional weights, one per column. When present they set the count (see *Columns*). |
| `columns.rule` | i64 | Width of a rule between columns. 0 or absent means no rule. |
| `direction` | `ltr` \| `rtl` | Column order. Default `ltr`. |
| `writing-mode` | `horizontal-tb` \| `vertical-rl` \| `vertical-lr` | For the section's frames. Default: the template's. |
| `numbering.restart` | i64 decimal string | Restart at this value. Absent means continue. A string, so JavaScript loses no bits (as lists does). |
| `numbering.format` | a lists `NumberFormat` token | Absent means the previous section's format; the first section defaults to `decimal`. |
| `region.first`, `region.even` | bool | Use the `first` / `even` variants (Word's "different first page" and "different odd and even"). Default false. |
| `region.<slot>.<variant>` | a story's `NodeId`, or `""` | `""` means explicitly none. **Absent means same as previous**, the default. |

Only `active`, `break` and `start` mean anything on a page or column break.
Every other key is ignored there.

**Inheritance.**

-   Geometry, columns, direction and writing mode: built-in template, then the
    document default (`page-setup1`, unchanged), then the section's own keys.
    They are **not** taken from the previous section, as the brief asks.
-   The insert command copies the containing section's own geometry, columns,
    direction and writing-mode keys into the new record. That gives Word's
    "the new section looks like this one" behaviour without layout-time
    cascading. See *Open questions*.
-   Numbering format and region links come from the **previous section in body
    order**. Restart never inherits.

**When a start is in effect.** All of these must hold:

-   `active` is true;
-   `break` is readable;
-   the block is live;
-   the block is in **body order**: `Document::blocks()`, top level, after host
    expansion. Starts on blocks inside table cells, inside stories, or on
    relation-placed blocks are ignored;
-   the block isn't the first block of the body. A start there would make the
    `first` section empty. It still counts as a section with that block's
    properties, but its `start` kind is ignored, because there's no earlier
    page to break from.

Anything else is an **orphan start**: ignored, and listed by `audit` with Info
`section.orphan-start`. Unreadable values are ignored property by property. That
property falls back to its inherited value, and layout reports Warning
`section.property`.

**Section identity** is the start block's `NodeId`, or `first`. `NodeId`s are
never reused (07), so neither are section IDs. The facade calls this
`SectionId` and writes it as a string.

### Stories

A **story** is a content-tree node whose parent is the root and whose `kind` is
`story`. Its meta also holds `slot`, which the app uses as a hint (`header`,
`footer`, `background`) but which doesn't affect layout. Its tree children are
ordinary blocks: flow hosts with breaks, embedded tables and images, and, once
inline lands, inline atoms. So everything that works on paragraphs works in a
story unchanged: views, split and join, formatting, `para1`, ranges,
annotations, undo, and anchors that follow characters.

-   `Document::children(None)`, and therefore `blocks()` and every body
    traversal, **skips** story nodes. They're outside the body and its reading
    order, as the brief requires.
-   `Document::stories() -> Vec<NodeId>` lists the live stories in tree order.
    `children(Some(story))` gives a story's blocks, and `story_of(block)` gives
    the story a block is in, if any.
-   A story node anywhere other than directly under the root is ignored; its
    subtree isn't shown. So is a story inside a story. Stories have no sections:
    starts inside them are orphans.

Story nodes are created by staging (07). Undo and redo flip the deletion flag,
so a story keeps its ID.

This choice is safe only if code that walks the content tree's root directly,
instead of going through `children(None)`, is updated. Stage B decides each of
these explicitly and tests the decision:

| Place | What stories need there |
| --- | --- |
| `position.rs:84` | Include stories, so a caret can be placed in a story. |
| `invariants.rs:41` | Include stories in the checks. |
| `hostile.rs` | Include stories when generating hostile documents. |
| `persist.rs:58` | Include stories when saving and loading. |
| `lifecycle.rs:43` | Decide liveness the same way as for any other node. |
| clipboard | Leave stories out of copied body fragments. |
| exports | Leave stories out of the body; emit them as artifacts (*Exports*). |

### Slots and variants: the extension point

A **slot** is a running region kind. The registry is a table in
`reprise-layout` (`running.rs`), and each entry supplies:

1.  `name`: the stored key segment (`header`, `footer`, `background`);
2.  `variants`: which of `default`, `first` and `even` it uses;
3.  `place(page geometry, section) -> RegionBox`: its rectangle, and whether
    its extent grows from an edge;
4.  `reserve`: how its composed extent changes the body area: push from the
    top, push from the bottom, or none;
5.  `paint_order`: below the body or above it;
6.  `artifact`: its tagged-PDF artifact type and subtype.

The three slots:

| Slot | Box | Reserve | Painted | PDF artifact |
| --- | --- | --- | --- | --- |
| `header` | Starts `header.distance` from the top edge, across the body's left and right margins, and grows down. | Pushes the body down. | Above the body. | `/Pagination /Header` |
| `footer` | Ends `footer.distance` from the bottom edge, across the same width, and grows up. | Pushes the body up. | Above the body. | `/Pagination /Footer` |
| `background` | The whole page. | None. | Below the body. | `/Pagination /Watermark` |

`background` proves the extension. It has different placement, reservation and
paint order from the other two, and it's what the *House of Leaves* north star
uses for text set behind a page: a watermark, a ghost passage, a mirrored layer.
A side-margin running head would be a fourth slot with `place` in the outer
margin and no reservation.

**Variant selection** on a page owned by section *s*:

1.  If this is *s*'s first owned page and `region.first` is set, use `first`.
2.  Otherwise, if `region.even` is set and the page's **physical** index
    (1-based) is even, use `even`. See *Open questions*.
3.  Otherwise use `default`.

The chosen variant resolves through "same as previous": the nearest section at
or before *s* that has the key. A variant that's enabled but resolves to
nothing gives an empty region.

**Same as previous can't form a cycle.** Inheritance is positional: always
from the section before in body order. No key can name a section, so there is
nothing to cycle through.

The hostile cases are all treated as `""` (none), with Warning
`layout.region-missing`. A key that is:

-   unparsable;
-   names a node that isn't a live story;
-   names a story nested somewhere other than the root.

## Operations

All of these are edit-kernel commands (see *Facade*). They work inside `batch`
and respect `expected_revision`.

**Insert a break** at a position or before a block. If the position is a
paragraph's offset 0, or names a block, the start is written on that block.
Otherwise the paragraph is split there (staged marker, `flow.md`) and the start
is written on the new paragraph. Writing a start means writing `active = true`,
`break`, and, for a section, `start` and the copied keys. All of these go in
the undoable commit.

If an inactive record already exists for that block, the command first
deletes its keys, so the new section doesn't bring back old properties.

**Undo** of an insertion reverts the map writes and, through the flow, the
marker's activation. The paragraph's `NodeId`, and so the section ID, comes
back on redo, because the marker was staged.

**Remove a break** writes `active = false`. The paragraphs stay; the section's
content joins the previous section.

**Deleting a break: which properties win.** One rule covers both cases:

> A boundary lives and dies with its block. A deletion removes every boundary
> strictly inside the deleted range, and keeps the boundary at its start.

That follows from `flow.md`'s join semantics, in which the first paragraph
keeps its identity:

-   Deleting from section 1 into section 2 deactivates section 2's first
    paragraph. Section 2's start goes with it, and its remaining content takes
    **section 1's properties**. The inactive record keeps section 2's
    properties, so undo brings them back.
-   Deleting exactly the text of section 2's first paragraph and its end, from
    `P.0` to `Q.0`, joins `Q` into `P`. `P` keeps its identity, so section 2
    survives.
-   Deleting a whole one-paragraph section 2 together with section 3's
    boundary, from `P.0` to `R.0`, keeps **section 2's** properties and drops
    section 3's. Word keeps section 3's here, because it stores a section's
    properties at its end. Documented and tested.
-   `DeleteBlock` on a start's block removes the start (*Open questions*).

**Join across a section break** (Backspace at section 2's first paragraph) is
the same: the join deactivates `P`, and section 1's properties win. The facade
offers both "remove break" and "join". The app decides what Backspace does
(*App seam*).

**Split inside a section's first paragraph** keeps the boundary on the first
part. Enter at its offset 0 leaves an empty paragraph at the section's top,
which is what word processors do.

**Copy and paste.** A fragment carries the boundaries **strictly inside** it,
the same rule as deletion. Each one is written on the pasted copy of its block
(new `NodeId`, so a new section ID) with the source record's own keys.

-   Region links are kept when pasting within the same document. Across
    documents they're dropped (the section shows "same as previous") and the
    paste reports what it lost, as decision 35 requires.
-   The `first` record is never carried.
-   Pasting into a story drops every boundary.

`import_copy` copies `starts1` and every story. Start keys and story references
are remapped through the node-ID map, so the copy has new identities and the
same structure.

### Concurrency

| Concurrent edits | Result |
| --- | --- |
| Two peers insert a section break at the same mid-paragraph offset | Two markers next to each other, so an empty paragraph between them. The first new section holds only that empty paragraph and makes one page. This is the honest CRDT outcome, the same as two concurrent Enters. It converges, and the app shows both breaks so one can be removed. |
| Two peers make the same existing paragraph start a section | Both write the same keys. One section results, and each property is last-writer-wins. |
| A page break and a section break at the same block | `break` is last-writer-wins. If page wins, the section keys are kept but inert. |
| One peer removes a section break while another edits its properties | Inactive, with the edits kept. Undoing the removal brings the section back with them. |
| Concurrent edits to a header story | Ordinary text merging in one story. |
| Both peers unlink the same header | Each creates a story; the reference key is last-writer-wins. The losing story is an orphan: kept, listed by `audit` and `State.stories`, never shown. Its text isn't lost, but nothing displays it (*Risks*). |
| One peer links a region back to the previous section while another types in it | Linking soft-deletes the story when nothing else references it. The typing lands in a deleted story; undoing the link brings it back. |

## Layout

### The section table

The section table is computed once per document revision from `blocks()` and
`starts1`. Each section has an ID, the index of its first body block, its kind,
its resolved properties, and its slot table. Page and column breaks are a
`BTreeMap<NodeId, BreakKind>` beside it. Sections with no blocks (an empty
`first`) produce no pages.

The resolved template for a section is the page template patched by
`PageDimensions` and columns. `patch_page_template` today patches only the
first main frame. It generalises to: replace every main-flow frame with the
section's columns, cut from the text area the margins leave. Notes and margin
frames are kept. With `columns.count` absent, the template's own main frames
are kept exactly as they are, so documents without sections lay out
byte-identically.

### Pages, bands and ownership

-   A **band** is a run of columns across one stretch of a page's block axis.
    A page has one band, or several when continuous sections start on it.
-   **The page owner** is the section in effect at the page's top. It owns:
    -   the page size;
    -   the top and bottom margins;
    -   the running regions;
    -   the page number, and therefore numbering restart and format;
    -   the page's writing mode.
    That's Word's rule, and it answers "whose geometry, header and page number
    on a shared page".

**A continuous section** starting on page *P*:

1.  It first **balances** the previous section's last band (*Balancing*).
2.  It opens a new band on *P* directly below that one, at the band's used
    block extent.
3.  The new band uses the continuous section's own left and right margins,
    columns, gap, rule and direction. The page's block-axis limits and its
    running regions stay the owner's.

Exceptions:

-   If the continuous section's resolved width, height or writing mode differs
    from the page owner's, a band can't honour it. The start acts as
    `new-page`, with Warning `section.continuous-new-page`, because the output
    differs from what was asked. Word does the same, silently.
-   Its `numbering.restart` takes effect on the first page it owns. If it never
    owns a page, the restart has no effect; that's documented, with no
    diagnostic, the same as Word.

**`odd-page` and `even-page`** test the next page's **physical** parity, its
1-based index. When the parity is wrong, layout inserts one **blank filler
page**. It is derived: it has no authored content and is never stored.

-   It is **owned by the preceding section**: that section's geometry and
    running regions, and its numbering continues on it. That's what Word shows.
-   It's marked `blank` in the per-page record, so the app and exports can tell
    it apart.

**A page break** starts a new page in the same section: same geometry, next
number. On a page shared through a continuous section, the new page is owned
by whichever section is current.

**A column break** moves to the next column of the band's thread. In the last
column it moves to a new page.

Two breaks are redundant and do nothing:

-   a page break on the first block of a `new-page` section;
-   any break on the first body block.

### Columns

The band's inline extent *W* is the page width minus that band's left and
right margins (the physical height in vertical modes).

-   **Equal columns:** `(W − gap·(n−1)) / n`, in integers. The remainder goes
    1 unit at a time to the first columns, so the widths always sum to *W*
    exactly.
-   **Explicit widths are weights.** `column_i = (W − gaps) · w_i / Σw`, with
    the same remainder rule. They always fit and they reflow with page size.
    That is the *relative* in "easy to reflow, and relative". A weight sum of 0
    falls back to equal columns, with Warning `section.property`.
-   Columns are ordered from the band's **inline-start**: left to right for
    `ltr`, right to left for `rtl`. In vertical sections columns divide the
    physical vertical axis and progress downward in both `vertical-rl` and
    `vertical-lr` (decision 20, `vertical.md`). Bands then advance along the
    horizontal block axis.
-   Each column is a `Flow("main")` frame, so the existing threading carries
    text from column to column, then to the next page.
-   A rule is a display item centred in each gap, as long as the band's used
    extent. It is derived and never a frame.

### Balancing

When a continuous section follows section *S* on the same page, *S*'s last band
is balanced: its columns get the smallest common depth *h* into which the
band's content still fits. The search:

-   It's a bisection over integer `Length` *h* in `[lo, H]`:
    -   *H* is the band's full available depth. *H* always fits, because it is
        where the content already landed.
    -   *lo* is `ceil(content depth / n)`.
-   Each trial re-flows the band's content into *n* frames of depth *h*, with
    the ordinary rules (keeps, widows, orphans, table rows).
-   It keeps the best fitting *h* found so far and narrows toward it.
-   **Bound: at most 24 trials** (`MAX_BALANCE_TRIALS`). *H* can be at most
    `MAX_PAGE_LENGTH` = 14,400 pt = 14,745,600 units < 2²⁴, so 24 halvings
    reach a single unit.
-   The predicate needn't be monotone (keeps can make it non-monotone). The
    bisection still ends, and it returns a depth that is known to fit.
-   All the arithmetic is integer, so it's deterministic (38).

A section's last band is balanced only before a continuous start. The end of
the document isn't balanced, as in Word.

### Running regions on a page

Composition order for page *n* owned by section *s*:

1.  For each slot, pick the variant and story. Compose the story into a
    one-frame flow of the slot's width, at depth up to its cap. The field
    environment is this page instance (*Fields*).
2.  The extents give the body's area:
    -   body top = `max(top margin, header bottom)`;
    -   body bottom = `min(height − bottom margin, footer top)`.

**A header taller than the top margin pushes the body down,** as in Word. CSS
page-margin boxes clip instead, but clipping silently hides authored text,
which is the output most at odds with what the author asked for. To stop a
region eating the page, each of header and footer is capped: it may take the
body down to **half its margin-defined depth** at most. The other half stays
body. Content past the cap is clipped, with Warning `layout.region-clipped`.

The cap also bounds the work. A 100,000-paragraph header composes only up to
the cap: preparation is lazy, and composition stops at the frame end.

**Derived instances.** Each composed region is a `RunningInstance { page,
slot, variant, story, frame, blocks }`, in `LayoutSnapshot.running`. It is the
same pattern as `repeated_headers`:

-   its frame is in `snapshot.frames` with role
    `FrameRole::Flow("reprise.running.<slot>")` (a named flow, never `main`);
-   its `BlockLayout`s carry the story's `NodeId`s and byte ranges.

It's never in `blocks`, never in reading order, never stored, and tagged as an
artifact.

v1 composes running regions horizontal-tb even in vertical sections. Vertical
running heads are a follow-up.

What stories hold: floats, footnotes and relation-placed blocks inside a story
aren't laid out in v1, with Info `layout.story-unsupported`. Tables, images and
paragraphs are.

### Page numbers and labels

Each page *n* has a counter:

-   *s*'s restart value, if it has one, on *s*'s first owned page;
-   otherwise the previous page's counter + 1 (saturating i64). The first page
    starts at 1.

Blank filler pages count.

The page's **label** is `NumberFormat::parse(format).render(counter)`, from the
lists API. When `render` returns `None` (roman outside 1–3,999, alpha below 1),
the label is decimal, with Warning `section.page-number-range`. That is the same
fallback as lists' `list.counter-range`, under a code of its own.

Labels are known when a page is made, so a `page-number` field still settles in
one pass.

### Fields

Inline's model keys resolved field values by atom: `Plan.fields:
BTreeMap<InlineId, String>`, read through `FieldEnv { page, pages, values }`.
A story's atom is shown on every page that shows the story, so **values must be
keyed per placement**, as GPT-6.1-Sol pointed out:

```rust
pub enum Placement { Body, Running { page: usize, slot: SlotId } }
pub struct FieldKey { pub atom: InlineId, pub at: Placement }
// Plan.fields: BTreeMap<FieldKey, String>
```

-   Body atoms use `Placement::Body`, so nothing changes for them.
-   Each running instance composes with `at = Running { page, slot }`. A page
    shows each slot at most once, so `(page, slot)` is unique.
-   After a pass, inline's resolver produces one entry per placement.
-   Convergence still compares whole plans, with the shared limit of 16 passes
    and `layout.field-unsettled`.
-   Repeated table header rows have the same latent problem in inline today: a
    `page-number` in a repeated header row shows one value on every page.
    `Placement::Repeat { frame }` would fix it. That's flagged to inline, not
    owned here.

Field meanings with sections:

| Field | Value |
| --- | --- |
| `page-number` | The **label** of its page (`FieldEnv::label(page)`), not the physical index. |
| `page-of` | The label of the target's page. |
| `page-count` | The number of physical pages, in decimal, filler pages included. Word's `NUMPAGES`. |

A per-section page count is a later field function.

In the first pass, `page-count` in a header assumes the pages made so far; the
second pass corrects it. A header whose height depends on the count can
oscillate. That's bounded by the 16-pass limit and reported, not hung (26, 37).

### Incremental layout

Today a paragraph transition's key is its template, incoming cursor, fills and
plan (`incremental.md`). With sections:

-   **The cursor** gains the section index, the band, and the page's **body
    geometry**: the frame rectangles after running-region reservation.
-   **Running instances** are their own memo unit. Key: the story's content
    revision, the slot's box, and the instance's field inputs (label, page
    count, `page-of` targets). They're composed in a per-page stage that body
    transitions don't replay.

That gives the behaviour the brief asks for:

-   **Edit inside section *k*.** Every transition before section *k* hits its
    memo, so earlier sections are reused. The test asserts the session's hit
    counters, not just equal output.
-   **Header edit.** The instances miss. If every extent is unchanged, the body
    geometry is unchanged, so every body transition hits. If an extent changes,
    body transitions miss from that section's first affected page.
-   **Section property edit.** That section's resolved template changes, so
    transitions miss from its first block on. A numbering change alters later
    labels: running instances recompose, and body transitions downstream miss
    only through field-plan changes.
-   **Balancing** is memoized per band, keyed by the band's content transitions
    and its geometry.

Viewport-first `partial()` cuts `running` and the per-page records with the
pages, as it cuts `repeated_headers`. A band waiting to be balanced is never
published unbalanced; the cut happens before that page.

Incremental output equals full output. That's the existing equality test,
extended to these fixtures.

### Snapshot additions

`LayoutSnapshot` gains two fields. Both are left out of JSON when empty, the
way `repeated_headers` is, so every existing snapshot is byte-identical:

-   `running: Vec<RunningInstance>`;
-   `sections: Vec<PageSection>`, one per page: `{ section, label, blank,
    bands: Vec<Band { frames, rules }> }`. It's empty when the document has no
    sections, breaks or stories.

`PageLayout` is unchanged: it already carries a size per page.
`snapshot.template` still names the base template. Adding fields to the frozen
`LayoutSnapshot` is a **contract proposal** (*Open questions*).

## Facade

**Commands.** These are new `Command` variants, appended:

-   `InsertBreak { at: BreakAt, kind: BreakKind, patch: Option<SectionPatch> }`.
    `BreakAt` is `Position(Position)` or `Before(NodeId)`. `BreakKind` is
    `Page`, `Column`, or `Section(StartKind)`. Body only: it's refused in a
    story.
-   `RemoveBreak { block }`.
-   `SetSection { section: SectionId, patch: SectionPatch, clear: Vec<SectionProp> }`.
    The patch works like `PageSetupPatch`; `clear` removes own keys so the
    property inherits again.
-   `CreateRegion { section, slot, variant, from: Empty | Copy(story) }`.
    Creates a story, staged, and sets the reference. "Unlink from previous" is
    `CreateRegion` with `Copy` of the inherited story.
-   `LinkRegion { section, slot, variant }`. Deletes the key, so the region is
    "same as previous" again. It soft-deletes the story when nothing else
    references it.
-   `SetRegionNone { section, slot, variant }`. Writes `""`.

Text, formatting, split, join, inline and annotation commands take story
`NodeId`s unchanged. A command whose range crosses between the body and a
story, or between two stories, is refused with `EditError::CrossStory`.

`set_page_setup` keeps its meaning: it sets the document default every section
inherits from. `State.page_setup` is still that default.

**State.** Two new fields:

-   `sections: Vec<SectionState>`. Each entry has:
    -   `id`;
    -   `first_block`, and `end`, the next section's first block or none;
    -   `break_kind`;
    -   `own`, the section's own patch;
    -   `resolved` (geometry, orientation, columns, numbering, direction,
        writing mode);
    -   `regions`: a list of `{ slot, variant, story, inherited_from }`.
-   `stories: Vec<StoryState { id, slot, blocks, referenced_by }>`. Orphans have
    an empty `referenced_by`.

Each `BlockState` also gains `starts: Option<"section" | "page" | "column">`.

**Queries:**

-   `page_section(page) -> SectionId`;
-   `section_pages(section) -> Range<usize>`;
-   `page_label(page) -> String`;
-   `page_regions(page) -> Vec<RegionInstance { slot, variant, story, rect }>`,
    which also answers "the header and footer rectangles on a page";
-   `region_instances(story) -> Vec<(page, slot)>`;
-   `page_fields(page) -> Vec<{ atom, at, text }>`: resolved field text per
    page instance.

**The caret in a story.** Hit-testing a running region returns the story
paragraph's `Position` plus `instance: Option<RegionInstance { page, slot }>`.
Caret and selection-rectangle queries take that instance as an optional hint.

The instance is **view state, held by the app**, never document state. A story
appears on many pages, and only the app knows which one the user clicked.

After a relayout, if that page no longer shows the story in that slot, the
query falls back to the nearest page that does, then to the first. The app's
caret therefore never points at a page that doesn't exist.

## Exports

**PDF**

-   Per-page `MediaBox` from `pages[i]`.
-   Running instances are marked content `/Artifact` with the slot's type and
    subtype. Body tagging is unchanged.
-   Labels become the document's `/PageLabels` number tree when every format
    maps to `/D`, `/r`, `/R`, `/a` or `/A`.
-   The PDF/UA checks are extended: no story text in the structure tree, and
    every artifact has a type.

**HTML**

-   Each section becomes a `<section>` with `data-reprise-section` holding its
    own keys as JSON, plus CSS `columns` and `column-gap` for viewers.
-   Each section's **default** header and footer stories are emitted once, as
    `<header>` and `<footer>` inside it. HTML has no per-page running regions,
    so `first` and `even` variants are emitted as `<header
    data-reprise-variant="first" hidden>`, and the export reports this as a
    loss.
-   Page and column breaks are `break-before: page | column` on the block.
-   Import reads all of this back.

**Plain text:** body only.

## Package

Bit **17**, `REQUIRED_SECTIONS`. It's set when `starts1` has any key, live or
not, or any `story` node exists. Older engines refuse such packages. Documents
without them write nothing new, so they stay readable by older engines and
their snapshots don't change. The test is: an old engine refuses a document with
one section break, and a document with none is byte-identical.

## Limits and diagnostics

| Limit | Value | Effect past it |
| --- | --- | --- |
| Columns per section | 64 (`MAX_COLUMNS`) | Extra columns dropped, with Warning `section.columns-limit`. A 1,000-column section lays out 64. |
| Narrowest column | 1 pt | Narrower columns, including zero-width ones, are skipped by the thread, with Warning `section.column-narrow`. If all are narrow: one column. |
| Bands per page | 32 (`MAX_BANDS_PER_PAGE`) | A further continuous start acts as `new-page`, with Warning `section.band-limit`. This bounds balancing work per page to 32 × 24 trials. |
| Balance trials | 24 | Always enough to reach one unit (*Balancing*). |
| Sections | Not capped | Each `new-page` section makes a page, so the existing page limit (`layout.page-limit`, Error) bounds 10,000 sections. Resolving the table is linear in starts. |
| Story size | Not capped | Composition stops at the region cap (*Running regions*). |
| Field passes | 16 (shared) | `layout.field-unsettled` (inline's code). |

New codes, for the `contracts.md` codes table:

| Code | Severity | When |
| --- | --- | --- |
| `section.property` | Warning | A start-record value is unreadable or out of range; the inherited value is used. |
| `section.orphan-start` | Info | `audit` only: a start record that isn't in effect. |
| `section.columns-limit` | Warning | More than 64 columns. |
| `section.column-narrow` | Warning | A column under 1 pt was skipped. |
| `section.continuous-new-page` | Warning | A continuous start was made a new page (size or writing mode differs). |
| `section.band-limit` | Warning | Too many continuous sections on one page. |
| `section.page-number-range` | Warning | The format can't show the counter; decimal was used. |
| `layout.region-clipped` | Warning | A running region was cut at its cap. |
| `layout.region-missing` | Warning | A region reference doesn't name a live top-level story. |
| `layout.story-unsupported` | Info | A float, note or relation inside a story was not laid out. |

## What this needs from wave 1

**Inline (`docs/inline.md`)**

-   **`FieldKey { atom, at: Placement }`** as the `Plan.fields` key, with
    `Placement::{Body, Running { page, slot }}`, plus `Repeat { frame }` if
    inline takes the table fix. The resolver yields one entry per placement.
-   **`FieldEnv`** gains:
    -   `at: Placement`;
    -   `labels: Option<&PageLabels>`, so that `page-number` and `page-of`
        render `labels.label(page)`, and decimal `page + 1` when it's `None`;
    -   `pages` taken from the plan's page count for running instances. The
        page count isn't known while the flow is still making pages.
-   The flow in `flow.rs` must be callable on **a block list and a one-frame
    template** (`run_story`), not only on `doc.blocks()` with the document
    template. That's a refactor shared with the cursor changes below; whichever
    lands second rebases.

**Lists (`docs/roles-and-lists.md`)**

-   `reprise_doc::lists::NumberFormat::{parse, token, render}`, used as
    documented.
-   List counters run over a block **sequence**. Body counters must not count
    story paragraphs, and a story numbers its own lists. Because stories aren't
    in `blocks()` this comes free, if lists computes counters over a caller's
    sequence rather than walking the tree root itself.

Until both merge, Stage B builds everything else first:

1.  start records and stories;
2.  the section table;
3.  geometry and columns;
4.  page and column breaks;
5.  bands and balancing;
6.  running-region layout with `text` content;
7.  the commands, `State` and queries;
8.  HTML, PDF sizes and artifacts;
9.  the package bit.

Then numbering labels and fields go on top.

## How later features plug in

-   **Header and footer editing in the app.** Hit-test, then `instance` and a
    story `Position`; edit with the ordinary text commands. A section and
    page-setup dialog reads `SectionState.own` and `resolved`, and writes
    `SetSection` and `clear`. Linking a region is `LinkRegion`; unlinking is
    `CreateRegion` with `Copy`.
-   **A table of contents with page numbers.** Each entry is a `page-of` field
    targeting its heading, and renders the target page's **label**, so roman
    front matter and a decimal body are right without extra work.
    `section_pages` lets a TOC restrict itself to sections.
-   **Footnotes per section.** Note numbering restarts at section starts: a
    `notes.restart` key in the start record, read by the notes allocator from
    the section table. **Endnotes at a section's end** are a slot-like
    derived band placed before the next section's first band. The band
    mechanism already models "content at a section's end".
-   **Print and PDF with mixed page sizes.** This already works through
    per-page `MediaBox` and `/PageLabels`. Print imposition reads
    `PageSection.blank` to skip or keep filler pages.
-   **Spreads and facing pages.** The `even` variant and physical parity are
    the start. Facing pages add mirrored margins: an `inside` and `outside`
    pair resolved by parity, in the same record. A spread view pairs pages by
    physical index.
-   ***House of Leaves*-style spatial pages.** A section whose `template` key
    names a spatial page template (decision 01, explicit composition). Its
    pages come from that template's frames and their explicit reading order,
    not from columns, inside an otherwise flowing document. The section table
    is already where a page's template is chosen. The `background` slot carries
    text set behind those pages.

## App seam

-   **Commands:** `insert_break`, `remove_break`, `set_section`,
    `create_region`, `link_region` and `set_region_none`, plus the existing
    text commands with story positions. All work inside `batch` with
    `expected_revision`.
-   **State:** `sections`, `stories`, and `BlockState.starts`.
-   **Queries:** `page_section`, `section_pages`, `page_label`,
    `page_regions`, `region_instances` and `page_fields`, plus hit-test and
    caret queries carrying `instance`.
-   **App policy, not engine policy:**
    -   Backspace at a section's first paragraph calls `remove_break` first, and
        `join` on a second Backspace, as Google Docs does.
    -   Double-clicking a header area enters story editing on that instance.

## Open questions for review

1.  **Copy on insert versus cascade.** Properties inherit from the document
    default, not the previous section, as the brief says. The insert command
    copies the containing section's own geometry and column keys so a new
    section looks like the current one. The alternative is to cascade
    geometry from the previous section at layout time. That's simpler for
    users, but then changing one section silently changes all later ones.
2.  **Physical parity.** Odd and even starts, and the `even` variant, use
    physical parity. Word uses the page *number*'s parity. Physical parity is
    right for spreads and recto starts, but differs from Word after a restart
    at an even number.
3.  **Running regions on blank filler pages.** Following Word, filler pages
    show the preceding section's regions. Publishers often want them blank. A
    later `blank-pages: plain` key could switch that.
4.  **`DeleteBlock` on a start block** drops the boundary. Alternatively the
    kernel could re-key the start onto the next block of the section in the
    same step. That's friendlier, but a concurrent property edit to the old
    key would be lost.
5.  **Contract addition.** Two `skip_serializing_if` fields on the frozen
    `LayoutSnapshot`, `running` and `sections`. The precedent is
    `repeated_headers`. The other option is a parallel `SectionsLayout`
    returned beside the snapshot, which would double every query path. This
    needs the orchestrator's decision before Stage B reaches layout.
6.  **Where stories are stored.** Stories are in the `content` tree, skipped
    by `children(None)`, so every text facility works on them. A separate Loro
    tree would isolate them better, but then every `NodeId` lookup would need
    to know which tree it's in.
7.  **Inline's API.** `FieldKey`, `Placement` and the `FieldEnv` labels are
    asks of an in-progress workstream. If inline merges first without them,
    this work makes the change additively in `inline.rs` and calls it out in
    the report.

## Risks

-   **The flow cursor refactor.** Bands put frames for a page at
    non-template indices. The cursor's `thread` has to become absolute frame
    indices, and `page_base + thread[pos]` goes. This touches the hottest code
    in layout and the transition key. The guard is that documents without
    sections must take the old path byte for byte; every existing snapshot is
    the test.
-   **Running geometry feeding back into body geometry.** Header height
    depends on fields, fields depend on page count, and page count depends on
    body geometry. That's a real cycle. It's bounded by decision 26's
    coordinator, but a header showing `page-count` that wraps at a digit
    boundary can oscillate and freeze with a Warning.
-   **Balancing inside incremental and viewport-first jobs.** A band is
    re-flowed up to 24 times, and partial publication must stop before an
    unbalanced band. Memoizing per band keeps edits elsewhere cheap; a
    pathological page with 32 bands costs 768 trial flows.
-   **Notes and floats with bands.** `Plan.reservations` is keyed by frame.
    Footnotes on a page with two bands reserve from the owner's last band
    only. Tested, but the interaction with continuous sections is the least
    explored part.
-   **Direct root walks.** Any one missed (the *Stories* table) leaks stories
    into the body. The invariants and hostile suites get a story in every
    generated document to catch this.
-   **Concurrent unlink.** This can orphan a peer's freshly typed header text.
    Nothing is destroyed, but nothing shows it until someone recovers it.
-   **Wave-1 timing.** Fields and numbering arrive late. Building order 1–9
    above avoids blocking on them, but the field keying change in inline is
    still required to finish.
