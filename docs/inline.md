# Inline objects and fields

This note describes things that sit **inside a line** like a glyph: inline images,
page-number and page-count fields, cross-reference fields (`page-of`), and later
footnote reference markers, equations, mentions and date chips (decisions 09, 10,
11, 21, 22, 23, 26, 30, 33). Flow embeds (`flow.md`) put blocks *between*
paragraphs and image blocks are block boxes; neither can sit in a line.

## The building block

An **inline object** (an *atom*) is one character in a flow host's shared text,
U+FFFC OBJECT REPLACEMENT CHARACTER, plus a **record** that gives it identity,
kind and parameters. Layout treats the character as one unbreakable cluster
whose advance, ascent and descent come from the record, not from a font.

U+FFFC was chosen over a reserved noncharacter because its Unicode properties
are already the ones an object needs: line break class CB (UAX #14 offers a break
opportunity before and after it), bidi class ON (it takes the direction of its
neighbours, UAX #9), vertical orientation U (upright, UAX #50), one grapheme.
A noncharacter such as U+FDD1 sits in a block whose default bidi class is AL and
would have to be special-cased in every Unicode algorithm.

### Storage (05, 07, 09)

```text
host text:   ... "see page " U+FFFC " for details" ...
root map inline1 / "<host>/<char>"  ->  { v: 1, active: true,
                                          kind: "field",
                                          p:field: "page-of", p:target: <anchor> }
```

-   The record lives in the root map `inline1`, keyed like break records by the
    host's tree ID and the character's ID (`NodeId::at_break(host, char)`
    formatting). A root map, as for `breaks1`, so concurrent first objects in a
    host cannot overwrite each other's record container.
-   A record is a map whose entries are independent: `v` (1), `active`, `kind`,
    optional `align`, and one `p:<name>` string per parameter. Two peers
    changing different parameters (an image's width and its alt text) both
    win; the same parameter is last-writer-wins, like every other map entry.
-   Atoms live only in **flow hosts** (paragraph and annotation nodes without a
    table role; that includes paragraphs inside table cells). Image alt text and
    table/row/cell containers are not hosts, and an object can't be inserted
    there. This narrows the brief: every text a user types into is a host today.
-   **Reads never create the root** (a package of a document with no objects is
    byte-identical to before), and they look it up by name once and keep the
    handle. They must not call `LoroDoc::get_value`: it computes the value of
    every root to tell whether it is empty, so each call costs the whole content
    tree and recurses to its depth. A record read per object made 50,000 objects
    quadratic and a 1,024-deep tree overflow the stack. The same call is in
    `has_page_setup`, `has_marks` and `has_ranges_root`, which other workstreams
    own; `has_ranges_root` is also on the annotation edit paths. They were left
    alone and are listed in the hand-off notes.

### Visibility follows flow breaks

A U+FFFC character in a host is **visible** in its paragraph's view exactly when
it has a readable record (`v` = 1) with `active = true`. It is then one
3-byte character of the paragraph text: carets, anchors, ranges and selections
address it with ordinary byte offsets. Every other U+FFFC is **hidden** like an
inactive break: it is never part of a paragraph's text and layout never sees it.

Like U+FDD0, U+FFFC is reserved in authored text: `Text::insert` refuses it
(`TextError::Reserved`, `Reason::Inline` from the edit kernel). Staged blocks
(`stage_block`) and remote peers may still contain one; without a record it is
hidden. Plain-text and HTML import drop literal U+FFFC characters.

### Identity through undo, redo and collaboration

Undoing a text insertion deletes characters; redoing inserts new ones with new
IDs, which would orphan a record keyed by the old ID. So, exactly as for flow
breaks:

-   **Insertion is staged.** The marker character and its record (inactive) are
    written in a commit excluded from the undo history, then the undoable commit
    sets `active = true`. Undo and redo flip the flag; the object keeps its ID.
-   **Deletion deactivates.** Deleting a range through a paragraph view keeps
    each visible atom character and sets its record inactive, in the same
    undoable step, instead of deleting the character. `reprise-text` gains one
    defaulted `Span` method, `retire`, through which the view asks `reprise-doc`
    to do this. Undo reactivates the record; the object and its parameters
    come back unchanged.
-   **Concurrency.** Two peers inserting next to an atom: ordinary text merge.
    A peer deleting an atom (`active = false`) while another resizes it
    (`p:width`) converge to a deleted atom whose record holds the new size;
    undoing the delete shows the resized object. A raw peer that physically
    deletes the character deletes the object: its record is then orphaned and
    ignored.
-   **Copies make new identity.** Every path that copies paragraph text
    (copying split, cross-host join, move-by-copy, clipboard paste,
    `import_copy`) carries the atoms in order: staged blocks get freshly keyed
    records for their own U+FFFC characters, and text inserted into a live
    paragraph stages its markers first. A copied object never shares a record
    with its source. A `page-of` whose target is inside the copied text is
    rewritten (`copy:<node> <offset>`) and re-anchored in the destination once
    its text exists; a target outside the copy keeps its persistent position.

### Hostile input (37)

All read-time normalisation, never repaired on import:

| State | Result |
| --- | --- |
| U+FFFC without a record, with an unreadable or future record, or inactive | Hidden; an unrecorded or unreadable one is reported `layout.inline-orphan-marker` (Warning). |
| A record whose character is missing, deleted or not U+FFFC | Ignored; reported `layout.inline-orphan-record` (Info: nothing visible changed). |
| A known kind with unreadable parameters, or an unknown kind or field function | Drawn as a zero-advance atom; `layout.inline-kind` (Warning). The record is kept verbatim (34). |
| An image whose asset is missing, or whose header can't be read | The gray placeholder box; `layout.image-missing`, `layout.image-header` or `layout.image-limit` (Warning). |
| Negative sizes, or a size past the frame | Clamped; `layout.image-size` (Warning). |
| A `page-of` with no target, an unreadable target or a target that is gone | Keeps the text it was composed with; `layout.field-target` (Warning). |
| Two characters claiming one record | Impossible: the key contains the character ID. |
| An object a peer left unreadable inside copied text | The copy is refused whole (`clipboard.invalid`), like an unreadable block. |

Composition measures an object again for every composition (a paragraph is
composed more than once per pass: a trial for widows and orphans, a fragment per
frame), so layout drops repeats of these notes before it publishes them. Each
names the object's three bytes.

## Metrics and composition (19, 22, 23)

Each atom resolves, per paragraph preparation, to an **atom box**:

-   `advance`: its inline size, a `Length`;
-   `ascent` and `descent` above and below the baseline, `Length`s;
-   for display, kind-specific paint (an image rectangle, or field glyphs).

**Alignment** (`align` entry): `baseline` (default), `middle`, `text-top`,
`text-bottom`.

-   `baseline`: the box's bottom sits on the baseline (ascent = height,
    descent = 0), as CSS `vertical-align: baseline` for replaced elements, and
    what users expect of an inline picture.
-   `middle`: centred a quarter of the surrounding font size above the baseline
    (the CSS fallback for a face without an x-height).
-   `text-top` / `text-bottom`: the box's top (bottom) at the surrounding face's
    ascent (descent).

In vertical frames atoms are **upright**: the inline advance is the box's
physical height and the box is centred on the vertical baseline, half its
physical width either side. An image's display item is not turned with the
column; a field is text and is drawn exactly as the text around it (sideways
text takes the frame's turn or reflection with it).

Sizes are clamped to `0..=frame extent`: an image is scaled, keeping its aspect
ratio, to the frame's inline extent and then to the deepest frame of the thread
(`layout.image-size`), so a box can neither overflow a line nor make one taller
than any frame. Zero sizes are legal (a zero-advance atom still has caret
positions before and after it). `Length::MAX` and negative sizes clamp.

**In the line.** After shaping, the glyphs of each U+FFFC cluster are replaced by
one advance-only glyph (`reprise_compose::ATOM_GLYPH`, the tab cell mechanism
of `compose_marks`) whose advance is the atom's. Reshaping at line edges does
the same. Composers therefore see an ordinary unbreakable cluster with break
opportunities around it (UAX #14 class CB), and bidi reordering moves it like
any neutral character. The cell is never painted: display draws the object.

### Per-line metrics

Until now every line of a paragraph had the paragraph's strut height (the
style's line height). `reprise_compose::LineExtents` is the small, documented
API for a line that needs more:

```rust
pub struct LineExtents {
    pub strut_top: Length,                    // the strut above the baseline
    pub strut_bottom: Length,                 // ... and below
    pub objects: Vec<(usize, Length, Length)>, // byte offset, ascent, descent; sorted
}
impl LineExtents {
    pub fn varies(&self) -> bool;                       // does any object exceed the strut?
    pub fn line(&self, bytes: Range<usize>) -> (Length, Length);
    pub fn height(&self, bytes: Range<usize>, strut: Length) -> Length;
}
```

`compose_marks_with(request, tabs, composer, Some(&extents))` composes with it.
A line is as tall as `max(strut_top, tallest ascent) + max(strut_bottom,
deepest descent)`, never less than the strut, and the baseline sits at the first
term. Composition is first-fit and per line: each line is tried at the strut
height, the objects that landed on it are measured, and the line is asked of
geometry again at the taller height when needed (at most
`MAX_LINE_HEIGHT_ATTEMPTS` = 4). If it still needs more, the paragraph stops
there and reports `compose.line-height-limit` (Error: text was left out).

A paragraph whose objects all fit the strut (`varies()` is false) composes
exactly as before with the configured composer, so no existing layout changes.
Only a paragraph with an object taller than its strut is first-fit, whatever
composer the engine selects. Other builders of lines can use the same type: a list
marker is an object-like entry at the line start, a link area can read the
line's `(top, bottom)` from `line()`, and nothing needs to know about images.

**A field asks nothing of its line.** Its text is drawn in the surrounding face,
which the line already has room for, so a page number does not make its line
taller than the line's other words. Its box (`AtomLayout::rect`) still spans the
face's ascent and descent so a handle can be drawn on it, but
`Sized::line_extent` is zero for text. Images ask for their whole box.

## Kinds: the extension point

Stored form: `InlineObject { kind: String, align, params: BTreeMap<String,
String> }` in `reprise-doc` (`inline.rs`). Typed views parse the parameters of
the known kinds; unknown kinds stay readable as raw parameters.

Layout behaviour is a registry of `InlineKind` implementations
(`reprise-layout/src/inline.rs`, the `KINDS` slice), matched on the stored
`kind`. A new kind supplies:

1.  `measure`: its atom box and paint from its parameters, the surrounding run's
    face and size, the frame extents and (for fields) the field text it was
    given. `paginated()` says whether the box depends on pagination, so it is
    measured per composition rather than once per preparation;
2.  its paint: `AtomPaint` is an image item, or glyphs of resolved text, so SVG,
    PNG, PDF and the web canvas need no code for a new kind that uses them;
3.  its text for copy, export and accessibility: `AtomLayout::text()` (alt
    text, a field's resolved text);
4.  its reading-order entry: `role()` is `Figure` (with `Alt`) or `Text`.

Built-in kinds:

-   **`image`** — `p:asset` (64 hex SHA-256, as image blocks), `p:width`,
    `p:height` (`pt:<1/1024 pt>`, `em:<thousandths>` or `pct:<thousandths of the
    frame's inline extent>`), `p:alt`. A missing size comes from the intrinsic
    size (assets) with the aspect ratio kept, else 1 em square. A missing or
    unreadable asset draws the gray placeholder box block images use.
-   **`field`** — `p:field` names a **field function** from a second registry
    (`FIELDS`); its text is computed during layout and drawn in the surrounding
    style. A field function supplies `guess(object, env)`, the text to assume
    before a pass has resolved it, and `resolve(object, context)`, the text a
    finished pass finds:
    -   `page-number`: the 1-based page the atom is on;
    -   `page-count`: the number of pages;
    -   `page-of`: the page of `p:target`, a persistent position (an encoded
        anchor in a named block; `Document::field_target`), the first
        cross-reference;
    -   `text`: `p:text` verbatim — the placeholder field that proves the
        extension point carries a field function with no pagination dependency.
        A `section-heading(level)` or `date(format)` joins by adding one entry
        to `FIELDS`.

## Fields and pagination feedback (26)

A field's text depends on pagination and pagination depends on its width. The
loop reuses the region feedback coordinator of decision 26 (`regions.rs`):

-   The plan gains `fields`: resolved text by **`FieldKey`**, derived after each
    complete flow pass and never stored in the document (05).
-   While composing, a field takes its text from the plan; with no entry yet,
    `page-number` takes the page being composed, `page-count` the pages made so
    far (at least the current page) and `page-of` a `?`: a guess cannot see the
    pages a pass has not finished. So page numbers settle in **one pass**: the
    page being composed is the page the line is on. A `page-of` (a backward
    reference too) or a `page-count` that was guessed wrong needs a second pass,
    which composes with what the first found.
-   After the pass, every field is resolved from the snapshot. If any text
    differs from what was assumed, the pass repeats with the new plan, up to the
    shared limit of 16 passes, with the shared convergence rule: equal plans
    converge; a plan seen before is an oscillation; the limit is exhaustion.
    Oscillation and exhaustion keep the last complete input plan (texts and
    geometry agree with each other) and report `layout.field-unsettled`
    (Warning). Documents without objects don't iterate.
-   We found no document that makes the built-in fields cycle: the text of
    each is a page number, which only gets longer as pages are added, so
    widening a field can push content later but not pull it earlier. The loop's
    control is therefore tested directly on constructed plans (equal plans
    converge, a seen plan is an oscillation, the limit bounds it, and an
    unsettled loop reports `layout.field-unsettled` and no region code), and a
    document that oscillates for another reason (a float that moves its own
    anchor) beside page numbers stays bounded and reports only that cause. A
    field function added later that can shrink with pagination would need no
    change to the loop.

### One object, many placements (headers and footers)

An object is one record, but content can be placed more than once: a shared
header or footer shows the same paragraph, with the same objects, on every page;
a repeated table header row shows its cells again on each continuation frame.
Each placement is composed separately and may resolve differently, so a resolved
value is keyed by the object **and the placement**, not by the object alone:

```rust
pub enum FieldInstance { Single, Page(u32) }     // reprise_layout
struct FieldKey { id: InlineId, instance: FieldInstance }
```

-   `Single` (the default) is content placed once, wherever flow puts it; its
    page is not part of the key, so a value found on one page can be tried again
    on another while the loop moves it.
-   `Page(n)` is the copy for 0-based page `n` of shared content. Its value is
    resolved from the lines of that copy, so the footer on page 4 says 4.
-   `AtomLayout::instance` records which placement an atom is (left out of JSON
    for `Single`). Composition carries it in `FieldEnv::instance`, and the lookup
    for the text to assume is `(id, instance)`. `resolve_fields` reads every
    block of a finished pass, including the derived copies in
    `LayoutSnapshot::repeated_headers`, and keys what it finds the same way.

**This is in use today:** a repeated table header row composes its copy on each
continuation frame as `Page(n)`, so a page number in a header row shows the page
it is on (2, 3, 4, ... on the copies; the authored row keeps `Single`). The
tests are `a_repeated_table_header_resolves_its_fields_for_each_page` and the
unit tests in `inline_tests.rs`.

**For headers and footers (foundations wave 2):** compose each page's header and
footer paragraph through `Prepared::compose` with
`FieldEnv { page, pages, instance: FieldInstance::of_page(page), values: plan.fields }`
(or `Flow::fields().placed_as(FieldInstance::of_page(page))`), place the lines
into that page's frame, and add the blocks to a place `resolve_fields` reads
(`field_blocks`: today `snapshot.blocks` and `repeated_headers`). Nothing else
changes: the loop, convergence, the limit and the diagnostics are shared, and a
`page-number` in a header settles in the pass that composes it. Because a
section can restart numbering, a later instance variant (`Section(n)`) joins the
enum; `FieldKey` and the maps already order by it.

Queries by `(node, offset)` (`atom_rect`, hit testing, selection) address the
authored placement only, as they do for the derived copies of repeated table
header rows today: a copy is not authored text and is not in
`LayoutSnapshot::blocks`. A header's placements are the same kind of derived
copy, so wave 2 adds a `page` argument to the queries that need one (a handle on
the footer of page 4) rather than a second identity for the object.

**Known gap.** Fields inside floats and notes are composed with the page they
are on (so `page-number` is right) but not resolved by the loop: their blocks
are produced while the next plan is built, after the snapshot the loop reads.
A `page-count` or `page-of` in a note body therefore keeps its guess, and a
`page-of` whose target is a note body is reported `layout.field-target` (the
target is not in the pass's blocks). Closing this means resolving `Plan.blocks`
as well and keying the region cache on the field values; the keying already
supports it.

## Editing (30)

-   The atom is one grapheme cell whose width is its advance, so carets stop
    before and after it, never inside, and selection covers it as a unit.
    Backspace and Delete remove it through `DeleteText`, which deactivates it.
-   Hit testing a point on an atom returns its node and the offset before or
    after it (the nearer edge: the left edge is the start of a left-to-right
    object and the end of a right-to-left one).
-   Movement over atoms in bidi text uses the ordinary visual cell order: one
    step crosses an object, as wide as its box.
-   `Navigator::atom_rect(node, offset)` returns the atom's page rectangle for
    selection handles; `None` unless `offset` is the start of a placed object.
-   Commands: `InsertInline { node, at, object }` and `UpdateInline { node, at,
    patch }`. `at` is where the earlier commands of the transaction leave the
    text, so both work in `batch` and respect `expected_revision`.

## Display and export (32, 33, 35)

-   An inline image is an `Item::Image` at the atom box; a field is an ordinary
    `Item::Glyphs` run of its resolved text, so SVG, PNG, PDF and canvas need no
    special code. The atom's own glyph is advance-only and never painted.
-   HTML: `<img data-reprise-asset=HASH alt=… data-reprise-inline=JSON>` for
    images and `<span data-field="page-number" data-reprise-inline=JSON>7</span>`
    for fields; both import. The JSON is the whole stored object, so import
    restores it exactly; without it, an `img` with a Reprise asset and a `span`
    with `data-field` are read from their visible attributes. An `img` without
    a Reprise asset is left out with `clipboard.html-dropped`. Anything that
    does not validate becomes no object (the text around it stays).
-   Tagged PDF: an image is a `Figure` with `Alt`; a field is text in its
    paragraph.
-   Plain text: the alt text, or the field's text (`?` for a field that needs
    pagination when no layout is given).

## Facade

Commands `InsertInline { node, at, object }` and `UpdateInline { node, at,
patch }`, the `inline` list per block in `State` (omitted when empty, so
existing listings are unchanged), `atom_rect({ node, at })` and
`field_target({ node, at })`, and native clipboard support (`copy`, `copy_as`,
`paste`, `import_copy`). Package feature bit **15** (`REQUIRED_INLINE_OBJECTS`;
14 is lists and 16 annotations) is set by any `inline1` record, live or not, so
older engines refuse such packages; documents without atoms are unchanged.

### App seam

-   Insert a picture: `apply`/`batch` with `{ kind: "insert-inline", node, at,
    object: { kind: "image", align: "baseline", params: { asset, width, height,
    alt } } }` after `register_asset`; sizes are `pt:<1/1024 pt>`, `em:<n>` or
    `pct:<n>` strings.
-   Insert a page number: the same with `{ kind: "field", params: { field:
    "page-number" } }`. For a cross-reference call `field_target({ node, at })`
    for the string to put in `params.target` of a `page-of`.
-   Resize: `update-inline { node, at, patch: { params: { width: "pt:…" } } }`
    (a `null` value removes a parameter; `align` is optional).
-   Draw handles: `State.blocks[].inline` lists `{ id, at, object }` per live
    object; `atom_rect({ node, at })` gives its page and rectangle after layout.
-   Resolved text and sizes come from the display list (fields are glyph runs,
    images are `Image` items); `diagnostics_for({ code_prefix: "layout.inline" })`
    lists the problems with `start`/`end` bytes.

## How later features plug in

-   **Headers and footers** place a `page-number`/`page-count` field in their
    paragraphs and compose them as `FieldInstance::Page(n)` (see above); page
    numbers settle in the composing pass.
-   **Footnote reference markers** are a new kind whose `measure` shapes the
    note's number superscripted and whose `role()` links to the note; the number
    can come from a field function that counts markers before the atom.
-   **Table of contents** entries use `page-of` with a target per heading; they
    resolve in the second pass, and `field_target` gives the targets.
-   **Inline images in the app** use `InsertInline` with an `image` object and
    `UpdateInline` for resizing, with `atom_rect` for handles.
-   **Equations and mentions** are new kinds: an equation measures and paints
    its own layout; a mention is a field-like kind resolving a person's name.
-   **Date and smart chips** are field functions (`date(format)`) or kinds with
    a chip background in `paint`.
-   **Lists and links** can read per-line metrics through `LineExtents` rather
    than the strut, so a marker or a link area on a tall line sits on its
    baseline.
