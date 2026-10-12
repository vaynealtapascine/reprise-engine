# Block roles, headings and lists

This note is the design for `ws/lists` (decisions 05, 06, 07, 08, 13, 14, 16, 29,
33, 34, 35, 37). It adds two reusable building blocks:

-   a **semantic role** on every paragraph: body, heading 1–6, title, subtitle,
    block quote, code, and verse (the extension proof);
-   **lists**: shared list definitions, list membership per paragraph, numbering
    derived at query time, marker layout, editing semantics and exports.

A role says what a paragraph *is*. It never fixes how it looks (06): appearance
stays with named styles (08), which engine main now supplies.

## 1. Roles

### Representation: a property stored with the paragraph, not a `BlockKind`

A role is one map entry, `role1`, in the paragraph's own metadata: the tree
node's metadata for a head paragraph or a tree block, and the break record for a
break paragraph (`flow.md`). Its value is a token: `body`, `heading-1` …
`heading-6`, `title`, `subtitle`, `quote`, `code`, `verse`. Absent means body.

A new `BlockKind` was rejected:

-   `kind` decides whether a node is a flow host (`is_host_meta`), and joins
    refuse different kinds. A heading must split from and join with body text
    in one flow without copying characters (07), so it must stay a `paragraph`.
-   The role is an independent fact. Kind is about *placement* (in the flow, out
    of it, an image box); role is about *meaning*. An annotation (a note) can
    contain a code block or a quote.

Decision 06 says "headings are tree nodes": they are. Every paragraph is a node
with a `NodeId`; the role marks that node as a heading.

### Split, join, undo and concurrency

-   **Split (Enter) copies the role.** `stage_break` already copies the kind,
    style and overrides into the new break record; it now also copies `role1`
    and the list membership (below). So Enter in the middle of a heading gives
    two heading paragraphs, exactly as the text was one heading. The copying
    split path (non-flow blocks, inherited-prefix splits) copies them too.
    Turning the second half into body text is an editing *policy* ("Enter at the
    end of a heading starts a body paragraph"), which the app may add in the
    same transaction with `SetRole`.
-   **Join keeps the first paragraph's role.** A flow join deactivates the second
    break, so the second record (and its role) stops being read; the first
    paragraph's metadata is untouched. A copying join deletes the second block.
    Undoing the join reactivates the record, so the second paragraph's role comes
    back with it.
-   **Undo** of `SetRole` restores the previous value: it is one map write in the
    transaction's undoable commit.
-   **Concurrency:** one map entry, last writer wins (Lamport, then peer).
    Concurrent role changes to different paragraphs both survive.

### What a role drives

| Role | PDF tag | HTML | Outline | Default style |
| --- | --- | --- | --- | --- |
| body | `P` (or `H1`–`H6` from a heading-like style name, as before) | `p` | no | `Normal` |
| heading *n* | `H`*n* | `h`*n* | level *n* | `Heading `*n* |
| title | `Title` (role-mapped) | `h1 class="reprise-title"` | no | `Title` |
| subtitle | `P` | `p class="reprise-subtitle"` | no | `Subtitle` |
| quote | `BlockQuote` | `blockquote` | no | `Quote` |
| code | `Code` | `pre` | no | `Code` |
| verse | `P` | `p class="reprise-verse"` | no | `Verse` |

The **default style** column is the agreed mapping with `ws/style`, which ships
`Normal`, `Title`, `Subtitle` and `Heading 1`–`6` as built-in names.
`Quote`, `Code` and `Verse` are reserved application style names; main does not
ship defaults for those three. Define them first if their appearance is wanted. The engine
only exposes it (`Role::default_style`); it never applies a style when a role
changes. The app sends `SetRole` and `ApplyStyle` in one revision-checked `batch`, so
undo treats them as one step. Nothing here depends on `ws/style`'s code.

**Reading order** (33) is unchanged in *order*: a role never moves a block. What
changes is what each step carries: the facade's reading steps, the plain-text
export and the PDF structure name each block's role, and a list item's marker
is read before its text (the PDF `Lbl` precedes `LBody`).

**Outline query.** `Document::outline()` lists every live heading in document
order with its level, text and node. The facade's `outline()` adds the page of
the heading's first line when a current layout exists. A table of contents and a
navigation pane read it.

An unknown token (from a newer engine) is kept verbatim, read as body, and
reported with `role.unknown` (Warning).

## 2. Lists

A list is a **discontinuous group** (06): its items can be separated by other
paragraphs and still number as one sequence. It has two parts:

-   a **list definition**: shared, independent, with its own identity;
-   **membership**: a property of each item paragraph that names a definition.

This is the shape a named style already has (a paragraph property naming a
shared definition), rather than a relation. A relation per item was rejected:
relation records are whole-JSON last-writer-wins values, an owned relation does
not follow a split (Enter would have to stage a new relation in the middle of a
transaction), and level and restart are per-item facts that must ride in the
break record with the role. The narrowing is deliberate: the *group* has
identity like a relation target (a definition ID never reused, soft-deleted with
a tombstone), and *membership* travels with the paragraph like its style.

### Definitions

A definition is a node of a new Loro tree, `lists`, so its `ListId` is an
opaque, never-reused tree ID (07), displayed like `5@1`. It is staged outside the
undo history with `deleted = true` and activated by the undoable step, as blocks
and relations are, so undo and redo keep its ID. Its metadata is flat, so
concurrent edits to different properties all survive:

| Key | Meaning |
| --- | --- |
| `v` | `1`. Any other version: the definition is unreadable. |
| `deleted` | Soft deletion. |
| `base` | Optional `ListId` the unset properties are inherited from (like Word's abstract numbering). |
| `<n>.format` | Level *n* (0–8) marker kind: `bullet`, `decimal`, `decimal-leading-zero`, `lower-alpha`, `upper-alpha`, `lower-roman`, `upper-roman`, `none`. |
| `<n>.text` | The marker template: `%1` … `%9` are the counters of levels 1–9, formatted with *their* level's format; `%%` is `%`. A bullet's text is its glyph. |
| `<n>.start` | The counter's first value (an integer). |
| `<n>.indent` | The text's start indent for level *n* (`Length` units). |
| `<n>.hanging` | How far the marker sits before the text start (`Length` units). |
| `<n>.marker` | Optional JSON `TextStyle` patch for the marker. |

Unset values come from `base` (followed at most 16 links; a cycle or missing
base stops there with `list.definition-cycle` or `list.definition-missing`),
then the defaults: decimal, `%n.`, start 1, indent `36pt × (n + 1)`, hanging
`18pt`; a bullet level defaults to `•`, `◦`, `▪` by level.

### Membership

Three flat keys in the paragraph's own metadata, next to `role1`:

-   `list1`: the `ListId`;
-   `level1`: 0–8;
-   `restart1`: optional; numbering restarts at this item with this value.

Split copies `list1` and `level1` but **not** `restart1`: the new paragraph
continues the list it was split from. Join keeps the first paragraph's values.

### Numbering is derived

`Document::list_numbering()` walks `document_order()` once per revision (cached
like flows) and keeps one counter vector per list:

-   An item at level *L* takes `restart1` if set, otherwise the previous value at
    *L* plus one, otherwise the level's `start`. Counters deeper than *L* are
    cleared, so the next deeper item starts again at its own start.
-   Items of other lists and non-list paragraphs in between change nothing, so a
    list **continues across intervening paragraphs** until told to restart.
-   "Restart numbering here" sets `restart1`. "Continue the previous list"
    clears `restart1` (including one set earlier in the transaction) and, when the nearest earlier item belongs to another
    list, moves this item and every later item of its list into that list.

The result per item: list, level, ordinal and the **rendered marker text**. It is
never stored (05).

Counters saturate. `lower-roman`/`upper-roman` render 1–3,999; outside that, and
alpha formats below 1, the counter falls back to decimal with
`list.counter-range` (Warning). Templates are cut at 64 characters with
`list.limit` (Warning).

### Concurrency

-   Two peers indenting different items write different paragraphs' `level1`.
    Both survive.
-   Two peers indenting the same item write the same key; one wins. Both replicas
    agree.
-   Two peers converting the same paragraphs into a list each create a definition
    and write `list1` on every paragraph. Last-writer-wins resolves each
    paragraph identically on both replicas. When both wrote the same paragraphs
    in the same order, one peer wins every paragraph, so they form one list;
    otherwise the items converge into two lists, each numbered consistently.
-   One peer deletes a definition while another adds an item to it: the item
    keeps `list1` naming a deleted definition. It is laid out and exported as a
    plain paragraph with `list.definition-missing` (Warning). Undoing the delete
    brings the list back.

### Layout

A list item's text is inset from the frame's **logical inline start** by the
level's `indent`: from the left in a left-to-right paragraph, from the right in
a right-to-left one (`base_level` odd). Frame space is logical, so vertical
frames need nothing extra: the inset is along the vertical line axis and the
frame transform turns it. The inset wraps whatever geometry provider the frame
has (measure, runaround, region bounds), and applies in table cells and notes
too, because it lives in `Prepared::compose`.

The **marker** is shaped once per preparation, in the style of the item's first
run (the paragraph style plus the character formatting at byte 0), with the
level's `marker` patch on top, and with the paragraph's direction so `1.` reads
correctly in right-to-left text. It sits on the first line's baseline, starting
`indent − hanging` from the inline start (mirrored in right-to-left).

**A marker wider than the hanging indent pushes the first line's text**, as
Word does: the first line starts at `max(indent, marker start + marker width +
¼ em)`. Later lines start at `indent`. The list definition **replaces** paragraph `indent-start` and `indent-first`;
these do not add. Paragraph `indent-end`, spacing and keeps remain in force.
The same rule applies in main flow, cells and notes. A tab-based rule was rejected because
tab stops belong to `marks1` and the marker is not authored text.

The marker is derived geometry on the first line: `LineLayout.marker:
Option<MarkerLayout>` (text, inline position, width, positioned runs), left out
of JSON when absent, so every existing snapshot stays byte-identical. The
display list draws it before the line's runs; PDF addresses count it the same
way. It is not part of the block's text, so carets, selection and hit testing
never land in it.

The list's numbering is part of every incremental key (`Inputs`), so inserting
an item re-lays out each later item whose marker changed, and only those.

### Editing semantics (`reprise-edit`)

| Command | Effect |
| --- | --- |
| `Enter { node, at }` | On an empty item: outdent one level, or leave the list at level 0. Otherwise a split (the new paragraph keeps role, list and level). |
| `BackspaceAtStart { node }` | On an item: remove its marker (it leaves the list; text and role stay). Otherwise: join with the previous sibling paragraph. |
| `IndentListItem { node, by }` | Tab (`+1`) and Shift+Tab (`-1`), clamped to 0–8. |
| `SetRole`, `CreateList`, `UpdateList`, `DeleteList`, `SetListItem`, `RemoveListItem`, `SetListLevel`, `RestartList`, `ContinueList` | The primitives the facade exposes. |

`CreateList` stages a new definition; commands later in the same transaction name
it as `ListRef::New(i)`. `UpdateList` writes only the properties that changed, so
concurrent edits to different levels both win.

Pasting a native fragment keeps roles and lists: fragments carry each block's
role and membership and the definitions they use; paste gives the definitions
fresh IDs and remaps the items.

### Exports

-   **HTML:** headings `h1`–`h6`, `blockquote`, `pre`; consecutive items become
    nested `ol`/`ul` with `li`, `start` when the first item's value is not 1 (or
    restarts), and `list-style-type` for the format.
    `data-reprise-level` preserves a starting deep level or a gap between levels;
    standard HTML still sees a valid list. Import also preserves literal `pre`
    whitespace and empty semantic paragraphs. Import reads the same, plus
    `ol type`, `ol start`, `li value`, and nests levels by `ol`/`ul` depth.
-   **PDF:** `L` (with its `ListNumbering`) > `LI` > (`Lbl`, `LBody`); nested lists
    go inside the parent item's `LBody`. Headings, `Title`, `BlockQuote` and
    `Code` come from the role.
-   **Plain text:** each item is its marker and a space, indented four spaces per
    level.

### Package bit

Required bit **14** (`REQUIRED_BLOCK_SEMANTICS`; bit 13 is paragraph styles) is set when any paragraph metadata or break record holds `role1`,
`list1`, `level1` or `restart1`, or the `lists` tree has a node. Documents that
never used roles or lists keep their exact bytes and masks.

## Diagnostic codes

| Code | Severity | When |
| --- | --- | --- |
| `role.unknown` | Warning | A stored role this engine cannot read; treated as body. |
| `list.definition-missing` | Warning | An item names a missing, deleted or unreadable definition, or a base is missing; the item is a plain paragraph. |
| `list.definition-unreadable` | Warning | A stored definition property cannot be read; inherits or defaults. |
| `layout.list-indent` | Warning | A list inset would leave less than one em of room; reduced. |
| `list.definition-cycle` | Warning | A `base` chain loops or exceeds 16 links; inheritance stops there. |
| `list.level-clamped` | Warning | A stored level outside 0–8; clamped. |
| `list.format-unknown` | Warning | A stored format this engine cannot read; rendered as decimal. |
| `list.counter-range` | Warning | A counter a format cannot show (roman outside 1–3,999, alpha below 1); rendered as decimal. |
| `list.limit` | Warning | A marker template longer than 64 characters; cut. |

## Extension points

### A new role (proved by `verse`)

1.  Add the variant to `reprise_doc::Role` with its token in `Role::token` and
    `Role::parse`. Every `match` on `Role` is exhaustive, so the compiler names
    each consumer that must decide:
2.  `Role::default_style` (the style the app pairs with it);
3.  `pdf_role` in `clipboard/src/pdf_tags.rs` (its PDF tag);
4.  `element`/`role_of` in `clipboard/src/semantic_html.rs` and the import
    table (its HTML element or class);
5.  `Role::outline_level` (whether it is in the outline).

The token is a stored string, so older engines that meet a new role keep it and
treat it as body (`role.unknown`); the package bit already refuses them.

### A new numbering format (proved by `decimal-leading-zero`)

1.  Add the variant to `NumberFormat`, with its token.
2.  Render it in `NumberFormat::render` (return `None` outside its range, which
    falls back to decimal with `list.counter-range`).
3.  Map it to a CSS `list-style-type` and HTML `ol type` (`semantic_html.rs`)
    and to a PDF `ListNumbering` (`pdf_tags.rs`).

### Number formats outside lists

`reprise_doc::NumberFormat` is the engine's one way of writing a counter as
text, and it is public API on purpose. `render(n)` gives the text, or `None`
where the format has no form for `n` (roman outside 1 to 3,999, alphabetic below
1). `render_or_decimal(n)` is the same with the shared fallback: the number in
decimal, plus a flag so the caller can report it (lists report
`list.counter-range`). Both are defined for every `i64`, never panic, and are
pure. `token()` and `parse()` are the stored spelling.

Section page numbers (wave 2: restarts in decimal, roman or alphabetic form) use
exactly this: store a `NumberFormat` token with the section, count pages from the
restart value, and show `format.render_or_decimal(page_number)`. Nothing about a
page number needs a list. A new format added for lists (see above) is available to
page numbers at once.

## How later features plug in

-   **Table of contents and navigation pane:** read `outline()` (level, text,
    node, page). A TOC is a derived view; a stored TOC would be a block whose
    entries are `reprise.reference` relations to heading nodes, refreshed from
    the outline.
-   **Cross-references to numbered headings:** heading numbering is a list whose
    items have heading roles (`Heading 1` items at level 0, `Heading 2` at level
    1, a template like `%1.%2`). A cross-reference relation targets the heading
    node and reads its marker text from `list_numbering()` at query time.
-   **Captions and figure numbering:** a `caption` role plus a definition per
    sequence ("Figure %1"). Captions are items of that list; numbering continues
    across the document by the same counters. A reference to a figure reads the
    caption's marker.
-   **The app's list and heading toolbar:** reads `State.blocks[*].role` and
    `.list`, and `State.lists`; sends `SetRole` (plus the style from
    `default_style`), `CreateList`/`SetListItem`/`RemoveListItem`, and binds Tab,
    Shift+Tab, Enter and Backspace to `IndentListItem`, `Enter` and
    `BackspaceAtStart`.
-   **Checklists:** a `checkbox` format whose marker text is `☐` or `☑`. The
    checked state is authored per item, so it would be one more flat key next to
    `list1` (`checked1`, a bool), copied on split as `false`. Rendering reads it
    when choosing the glyph; PDF tags the marker as a form-free `Lbl` with
    alternate text "checked"/"not checked"; HTML exports `<input type=checkbox
    checked disabled>`.

## App seam after main integration

The facade has `set-role`, `create-list`, `update-list`, `delete-list`,
`set-list-item`, `remove-list-item`, `set-list-level`, `indent-list-item`,
`restart-list`, `continue-list`, `enter`, and `backspace-at-start` commands.
They work through `apply` or `batch`; `batch.expected_revision` rejects stale
state before any authored change, and role/style pairing shares one undo step:

```json
{"version":1,"data":{"commands":[
  {"kind":"set-role","node":"4@1","role":"heading-2"},
  {"kind":"apply-style","node":"4@1","style":"Heading 2"}
],"expected_revision":null,"selection":null}}
```

For list creation, send `create-list` with `definition: {base: null, levels:
[{level: 0, format: "decimal", start: "1", indent: 36864, hanging: 18432}]}`,
then `set-list-item` with `list: {kind: "new", index: 0}` and `level: 0`.
Existing identities use `{kind: "existing", id: "..."}`. Starts, ordinals and
restarts are canonical signed i64 decimal strings, so JavaScript loses no bits.
Definitions have nine optional authored levels, inherit from `base`, and an
update replaces the authored definition while writing only changed properties.
Duplicate levels, unknown formats, negative indents and invalid marker patches
are refused atomically. `restart-list.value: null` clears a restart;
`continue-list` also adopts the nearest preceding list and remaps later items
of the original list, using current transaction membership in original document
order. Tab is `indent-list-item.by: 1`, Shift+Tab is `-1`.

`State.blocks[].role` is the semantic token (omitted means `body`);
`.list` is `{id, level, value, restart, marker}` when a live definition resolves.
`State.lists` carries `{id, definition, error}` entries in identity order; an
empty registry is omitted. `Applied.lists` returns created identities. The
listing is bounded at 1,024 definitions: past that `State` lists the first
1,024 and adds a `bindings.limit` warning, as it does for named styles and
annotations, so a peer cannot make `state()` fail by creating definitions. The
items of a cut definition still carry their markers. A native fragment (a
copy) refuses more than 1,024 definitions, because a copy cannot be partial.
A full-document `import_copy` preserves live unreferenced definitions as well
as used definitions and bases, giving all definitions and nodes fresh IDs;
tombstones/history stay with the source.

`outline()` returns `{node, level, text, page}` in document order. `page` is
zero-based and null before/currently without layout. Reading steps carry the
role and the derived marker on the first line, while `start`/`end` continue to
slice only authored UTF-8 bytes. List diagnostics are node-scoped layout
warnings and appear through `diagnostics_for` after layout. The WASM session
exposes the same query, and the worker accepts `{kind: "outline"}` and returns
`{kind: "outline", headings: Payload<OutlineEntry[]>}`.

### Takeover decisions

The document storage, numbering and marker abstraction are retained. The draft
editing/export implementation was checkpointed, then main 383cfca was union
merged; package semantics moved from draft bit 13 to fixed bit 14. Enter now
validates the offset even on an empty item, and Continue observes earlier batch
commands. Marker shaping honors first-run language, features, color, size and
decorations with the definition patch on top. Marker run ranges refer to marker
bytes; consumers of `LineLayout.marker` can share the item's baseline and frame
transform without modifying authored text or caret byte offsets.
