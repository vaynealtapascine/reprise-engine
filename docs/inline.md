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

### Visibility follows flow breaks

A U+FFFC character in a host is **visible** in its paragraph's view exactly when
it has a readable record (`v` = 1) with `active = true`. It is then one
3-byte character of the paragraph text: carets, anchors, ranges and selections
address it with ordinary byte offsets. Every other U+FFFC is **hidden** like an
inactive break: it is never part of a paragraph's text and layout never sees it.

Like U+FDD0, U+FFFC is reserved in authored text: `Text::insert` refuses it
(`TextError::Reserved`). Staged blocks (`stage_block`) and remote peers may
still contain one; without a record it is hidden. Plain-text and HTML import
drop literal U+FFFC characters.

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
    (copying split, cross-host join, move-by-copy, clipboard paste) carries the
    atoms in order: staged blocks get freshly keyed records for their own
    U+FFFC characters, and text inserted into a live paragraph stages its
    markers first. A copied object never shares a record with its source.

### Hostile input (37)

All read-time normalisation, never repaired on import:

| State | Result |
| --- | --- |
| U+FFFC without a record, with an unreadable or future record, or inactive | Hidden; an unrecorded or unreadable one is reported `layout.inline-orphan-marker` (Warning). |
| A record whose character is missing, deleted or not U+FFFC | Ignored; reported `layout.inline-orphan-record` (Info: nothing visible changed). |
| A known kind with unreadable parameters, or an unknown kind | Drawn as a zero-advance atom; `layout.inline-kind` (Warning). The record is kept verbatim (34). |
| Two characters claiming one record | Impossible: the key contains the character ID. |

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
-   `middle`: centred on half the surrounding face's x-height above the
    baseline (half the cap height, then 0.25 em, when the face lacks one).
-   `text-top` / `text-bottom`: the box's top (bottom) at the surrounding face's
    ascent (descent).

In vertical frames atoms are **upright**: the inline advance is the box's
physical height and the box is centred on the vertical baseline.

All sizes are clamped to `0..=frame extent` (inline size to the frame's inline
extent, block size to the deepest frame of the thread), so a box can neither
overflow a line nor make one taller than any frame. Zero sizes are legal (a
zero-advance atom still has caret positions before and after it).
`Length::MAX` and negative sizes clamp and report `layout.inline-size`.

**In the line.** After shaping, the glyphs of each U+FFFC cluster are replaced by
one advance-only glyph (`reprise_compose::ATOM_GLYPH`, the tab cell mechanism
of `compose_marks`) whose advance is the atom's. Reshaping at line edges does
the same. Composers therefore see an ordinary unbreakable cluster with break
opportunities around it (UAX #14 class CB), and bidi reordering moves it like
any neutral character.

**Per-line metrics.** Until now every line of a paragraph had the paragraph's
strut height (1.2 × the largest inline size). A paragraph containing an atom
whose ascent or descent exceeds the strut's composes with first-fit lines of
**per-line height**: each line is tried at the strut height, its atoms measured,
and the line re-queried at the taller height when needed (at most 4 attempts;
the line keeps the height its final content needs). Line height is
`max(strut top, tallest atom ascent) + max(strut bottom, deepest atom descent)`,
and the baseline sits at the first term. Paragraphs without atoms, or whose atoms
fit the strut, compose exactly as before with the configured composer, so no
existing layout changes.

## Kinds: the extension point

Stored form: `InlineObject { kind: String, align, params: BTreeMap<String,
String> }` in `reprise-doc` (`inline.rs`). Typed views parse the parameters of
the known kinds; unknown kinds stay readable as raw parameters.

Layout behaviour is a registry of `InlineKind` implementations
(`reprise-layout/src/inline/`), matched on the stored `kind`. A new kind
supplies:

1.  `measure`: its atom box from its parameters, the surrounding run's face and
    size, the frame extents and (for fields) the field environment;
2.  `paint`: its display items at a placed box (image item, glyph runs, path);
3.  `text`: what copy, plain-text export and accessibility read (alt text, a
    field's resolved text);
4.  `reading`: its reading-order entry (an image is a `Figure` with `Alt`, a
    field is text in its paragraph).

Built-in kinds:

-   **`image`** — `p:asset` (64 hex SHA-256, as image blocks), `p:width`,
    `p:height` (`LengthExpr` text: `pt`, `em`, `%` of the frame's inline
    extent), `p:alt`. A missing size comes from the intrinsic size (assets) with
    the aspect ratio kept, else 1 em square. A missing or unreadable asset draws
    the gray placeholder box block images use and reports
    `layout.inline-image-missing` (Warning).
-   **`field`** — `p:field` names a **field function** from a second registry;
    its text is computed during layout and drawn in the surrounding style. Field
    functions supply `resolve(env) -> text`:
    -   `page-number`: the 1-based page the atom is on;
    -   `page-count`: the number of pages;
    -   `page-of`: the page of `p:target`, a persistent position (an encoded
        anchor in a named block, the first cross-reference);
    -   `text`: `p:text` verbatim — the placeholder field that proves the
        extension point carries a kind with no pagination dependency.

## Fields and pagination feedback (26)

A field's text depends on pagination and pagination depends on its width. The
loop reuses the region feedback coordinator of decision 26 (`regions.rs`):

-   The plan gains `fields`: field atom → resolved text, derived after each
    complete flow pass and never stored in the document (05).
-   While composing, a field takes its text from the plan; with no entry yet,
    `page-number` takes the page being composed, `page-count` the pages made so
    far (at least 1) and `page-of` the target's page if already placed, else `?`.
    So page numbers settle in **one pass**: the page being composed is the page
    the line is on.
-   After the pass, every field is resolved from the snapshot. If any text
    differs from what was assumed, the pass repeats with the new plan, up to the
    shared limit of 16 passes, with the shared convergence rule: equal plans
    converge; a plan seen before is an oscillation; the limit is exhaustion.
    Oscillation and exhaustion keep the last complete input plan (texts and
    geometry agree with each other) and report `layout.field-unsettled`
    (Warning). Documents without fields don't iterate.

## Editing (30)

-   The atom is one grapheme cell whose width is its advance, so carets stop
    before and after it, never inside, and selection covers it as a unit.
    Backspace and Delete remove it through `DeleteText`, which deactivates it.
-   Hit testing a point on an atom returns its node and the offset before or
    after it (nearer edge).
-   Movement over atoms in bidi text uses the ordinary visual cell order.
-   `Navigator::atom_rect(node, offset)` returns the atom's page rectangle for
    selection handles.

## Display and export (32, 33, 35)

-   An inline image is an `Item::Image` at the atom box; a field is an ordinary
    `Item::Glyphs` run of its resolved text, so SVG, PNG, PDF and canvas need no
    special code. The atom's own glyph is advance-only and never painted.
-   HTML: `<img data-reprise-asset=HASH alt=…>` for images and
    `<span data-field="page-number">7</span>` for fields; both import.
-   Tagged PDF: an image is a `Figure` with `Alt`; a field is text in its
    paragraph.
-   Plain text: the alt text, or the field's text.

## Facade

Commands `InsertInline { node, at, object }` and `UpdateInline { node, at,
params }`, the `inline` list per block in `State`, and native clipboard support.
Package feature bit **13** (`REQUIRED_INLINE_OBJECTS`) is set by any `inline1`
record, live or not, so older engines refuse such packages; documents without
atoms are unchanged.

## How later features plug in

-   **Headers and footers** place a `page-number`/`page-count` field in their
    paragraphs; page numbers settle in the composing pass.
-   **Footnote reference markers** are a new kind whose `measure` shapes the
    note's number superscripted and whose `reading` links to the note.
-   **Table of contents** entries use `page-of` with a target per heading.
-   **Inline images in the app** use `InsertInline` with an `image` object and
    `UpdateInline` for resizing, with `atom_rect` for handles.
-   **Equations and mentions** are new kinds: an equation measures and paints
    its own layout; a mention is a field-like kind resolving a person's name.
-   **Date and smart chips** are field functions (`date(format)`) or kinds with
    a chip background in `paint`.
