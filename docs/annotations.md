# Typed range annotations

An **annotation** is authored data attached to a span of text that follows the
text as it is edited (decisions 10, 12, 13, 15, 29, 33, 35). Links and comment
anchors are the first two kinds. Suggestions, bookmarks, spelling-ignore spans
and authorship highlights are meant to be small additions to the same layer.

## Building block

An annotation **is a persistent range** in the existing `ranges` tree
(`ranges.rs`) that carries one extra metadata key, `annot1`. That reuses
everything ranges already do:

-   anchors that follow characters across flow splits and joins (`flow.md`);
-   per-range `RangePolicy` (start and end affinity, emptiness) from decision 12;
-   staged creation, so undo and redo flip a `deleted` flag and keep the ID (07);
-   `resolve_range` and `range_extent`, including spans over several paragraphs
    of one flow;
-   fragment copy with clipping and ID remapping (35).

A new mechanism would duplicate all of this. Formatting (`format1`) made the
same choice. Formatting is flattened on copy because it has no identity anyone
refers to; annotations keep identity, so they are copied as ranges.

| Field | Where it lives |
| --- | --- |
| Stable ID | The range's `RangeId` (`AnnotationId` is an alias). Never reused (07). |
| Kind | `annot1.kind`, a registry name (`link`, `comment`). |
| Range | The range's anchors and its policy, chosen by the kind. |
| Payload | `annot1.payload`, typed and validated per kind. |
| Author | The peer in the range's tree ID: the peer that staged it. Always available; a pasted annotation's author is the paster. Host user identity is a host mapping from peers. |

`annot1` is a JSON string `{"version":1,"kind":…,"payload":…}` with
`deny_unknown_fields`, bounded to 16 KiB before parsing. It is written in the
staging commit together with the range, so redo restores payload and range
together. Payload updates are ordinary undoable writes of the whole envelope; a
concurrent update and update converge last-writer-wins on that one map key.

## The kind registry (extension point)

`annotation::Kind` is a closed enum; each variant's `KindSpec` supplies:

-   `name`: the stored kind name;
-   `policy`: the `RangePolicy` used for its anchors (edge behaviour);
-   `on_empty`: `Orphan` (collapses to a queryable point) or `Disappear`;
-   `copy`: `Duplicate` (a clipboard copy gets a new ID and the same payload) or
    `Drop` (the clipboard leaves it behind);
-   payload validation, and the payload's variant of `AnnotationData`.

A new kind adds a `Kind` variant, an `AnnotationData` variant and a `KindSpec`;
queries, the index, geometry, `State` and the facade pick it up through
`AnnotationData::kind()` and the registry. What a kind must add by hand is its
**export**: each format's treatment (an HTML element, a PDF tag, "omitted and
reported") in `reprise-clipboard`, and its **facade payload** variant. The
second kind, the comment anchor, differs from the first in every `KindSpec`
field, which is how the registry was proved (see the table below).

An unknown kind name (a newer engine's) is kept verbatim, reported as
`annotation.unreadable` and never applied or copied. Its stored range still
keeps the package refused by older engines through the feature bit below.

| Kind | Start | End | Empty | Clipboard copy | Payload |
| --- | --- | --- | --- | --- | --- |
| `link` | typing at start stays outside | typing at end stays outside | disappears | duplicate | `Url`, or internal `Node`, `Heading`, `Range` (a persistent position) |
| `comment` | typing at start stays outside | typing at end joins | orphaned point | drop | `{ "thread": "<id>" }` |

So links use `RangePolicy::FIXED`. Comments use start `After`, end `After`,
empty `Keep`: a reply typed at the end of the commented sentence stays covered.
Both kinds refuse empty spans when authored, so an empty resolved comment is
always an orphan, never an intended point.

## Behaviour

-   **Overlap:** any number of annotations of any kinds may overlap or nest.
    Queries report them in `(start, ID)` order.
-   **Split** (flow break): no text moves, anchors stay; the annotation spans
    both paragraphs and resolves as two paragraph-local pieces.
-   **Join** (adjacent flow paragraphs): the break goes; pieces merge.
-   **Copying structure edits** (cross-host join, move by copy, split by copy):
    annotations wholly inside the moved text are re-anchored onto the copy in
    the same undoable step, so they keep their IDs. An annotation that crosses
    the boundary of a copying split keeps its part before the split.
-   **Copy and paste (`CopyScope::Clipboard`):** the fragment carries each copied
    annotation range with its data. `Duplicate` kinds are pasted with new IDs;
    `Drop` kinds (a comment thread has one anchor) are left out of the copy with
    `clipboard.annotation-dropped` Info. A link only partly inside the copy is
    clipped to the part that is. Internal link targets are remapped through the
    paste's `IdMap`; a target outside the copy is kept only within the same
    namespace, otherwise the link is dropped with `clipboard.annotation-dropped`
    Warning (the text stays, the link goes).
-   **Duplicating a document (`CopyScope::Document`, `import_copy`):** every
    readable annotation comes along, comments and orphaned points included, with
    new IDs. The thread contents a later workstream stores are the host's to
    carry with the same IDs mapping.
-   **Delete all of its text:** a comment resolves as `Orphaned { node, at }`;
    a link is gone from queries. Both records stay in the CRDT.
-   **Undo:** Loro undo recreates deleted characters under new IDs, so plain
    cursors would stay collapsed. Before text is deleted through
    `Document::delete_text` (the kernel's text deletion, flow paragraph
    deletion and copying splits), each annotation with an endpoint in the
    deleted span gets a **restore hint** in an excluded staging commit: the
    live character before the span, the span's length and SHA-256 digest,
    each endpoint's offset in it and its stored anchor, and the local op
    counter. After an undo or redo, `restore_annotation_anchors` rebinds a
    tombstoned endpoint only when its stored anchor equals the hint's, the
    digest matches at the base, and the characters there were created by this
    peer after the hint. The rebind and the hint update are one excluded
    commit, so repeated undo/redo cycles keep working. At most 8 hints are kept
    per annotation, newest first.
-   **Concurrency:** edits inside the span merge as text does. One peer deleting
    the text while another edits the payload: both survive; a comment is then
    an orphan with the new thread payload, a link is gone with its new URL
    retained. Concurrent overlapping additions are independent ranges.

## Bounds and diagnostics

-   At most `MAX_ANNOTATIONS` (65,536) live annotation records are indexed, in
    ID order; more is reported as `annotation.limit` Warning and local
    authoring refuses past it.
-   Query results are capped at `MAX_QUERY_RESULTS` (4,096) and say when they
    were truncated. Geometry returns at most 4,096 rectangles per span and a page
    lists at most 4,096 links.
-   URLs are at most `MAX_URL_BYTES` (8,192); thread IDs at most 256 bytes of
    `[A-Za-z0-9._:-]`.
-   Unreadable, future or invalid envelopes (a hostile peer) are kept verbatim,
    never applied, and reported: `annotation.unreadable` Warning. They are in
    `DocumentSession::diagnostics()` and so in `diagnostics_for`.

Authoring refusals carry stable codes: `annotation.link-scheme` (a scheme
outside `http`, `https`, `mailto`, `tel`, such as `javascript:` or `data:`),
`annotation.link-invalid` (empty, relative, whitespace or control characters,
no host for http(s)), `annotation.link-length`, `annotation.payload-invalid`,
`annotation.empty`, `annotation.missing` and `annotation.limit`. The same checks
run on HTML import.

## Index and queries

Annotations are read once per document revision into an index (as formatting
does): one pass over the range tree, one resolution per annotation, and
paragraph-local pieces grouped by paragraph. Queries never rescan the tree.
`Document::annotation_view()` hands out the shared index for readers that make
many lookups (layout asks for the links of each block on a page).

-   `annotation(id)`;
-   `annotations_at(node, at)`: pieces with `start <= at < end`, and orphans at `at`;
-   `annotations_in(node, bytes)`: pieces overlapping `bytes`;
-   `annotations_of_kind(kind)`;
-   geometry, in `reprise-layout` (`span.rs`):
    -   `LayoutSnapshot::annotation_geometry(doc, id)` and
        `annotation_geometries(doc, ids)` (one pass shared by many, for a
        sidebar), `span_geometry(pieces)` for any byte ranges;
    -   each `SpanRect` gives the page and frame, the line, the rectangle in
        frame space, its page bounds and its four page-space corners (so
        rotated, mirrored, vertical and spiral frames hit-test exactly);
    -   `top` is the first page the annotation is on and the smallest
        page-space y there, for aligning a sidebar entry;
    -   an orphan has one zero-width rectangle at its point;
    -   the document must be the revision the snapshot was made from, else the
        result is `stale` and empty (as the reading-order query requires);
    -   the rectangles equal `Navigator::selection_rects` for the same bytes, a
        property a test pins.

The per-line facts geometry reads are the small set a line fragment already has:
`text`, `rect`, `frame`, and its visual-order `runs` with their `range`, `level`,
`x`, `width`, `combined` and glyph clusters and advances. Features that put
things on lines (list markers, inline objects) should keep those meaning the
same; geometry will then cover them.

## Display, export and import

-   **Link areas:** `LayoutSnapshot::page_links(doc, page)` gives each link on a
    page with its target and its rectangles there (a derived copy of a repeated
    table header lists its links too, marked `repeated`). The facade's
    `DisplayPage` carries them as `links`, beside the frozen display list, which
    is unchanged (so backends and existing goldens are untouched): the app
    hit-tests them for hover cards.
-   **The link style is formatting, not forced.** Underline and colour are not
    drawn by the layer. Cmd+K is one transaction: `AddAnnotation` and a
    `FormatText` with the app's link style (underline and a link colour), so
    exports draw what the editor shows and the author can restyle or unstyle a
    link. Removing the link does not remove its style; the app's "unlink" sends
    `RemoveAnnotation` and a `FormatText` with `clear` for the same properties.
-   **HTML export:** a link is `<a href>` around its text, inside each formatted
    stretch's `<span>` boundaries so nesting is always valid. Internal links are
    `#r<node>` and the target paragraph gets that `id` (only targets do). A
    link across paragraphs becomes one anchor per paragraph; where links overlap
    the first keeps the shared text (HTML has no nested anchors); comment
    anchors are omitted. Each is reported in the `export.annotations` loss.
-   **HTML import:** `<a href>` goes through the same validation as authoring.
    A refused address (`javascript:`, `data:`, entity-obfuscated forms, relative
    or oversized URLs) keeps its text and reports the refusal's own code
    (`annotation.link-scheme`, `-invalid`, `-length`) as a Warning. `<a href="#id">`
    becomes an internal link when the pasted HTML has that `id`. Anchors close
    where the parser repairs tags, a new anchor closes the open one, and an
    anchor around several paragraphs makes a link in each. At most 4,096 links are
    kept (`clipboard.html-approximated` says the rest became text).
-   **PDF:** each linked run part is a `Link` structure element holding its
    content and a link annotation with `/Contents` alt text (PDF/UA), a URI
    action for external links or a destination for internal ones. Note
    references keep precedence over links on shared text and the first of two
    overlapping links keeps what they share (`pdf.link-unresolved` Warning says
    so, also when an internal target is not on a page).
-   **Plain text:** link text only; the loss is reported (`export.annotations`).
-   **Native:** annotations travel in the fragment. `FragmentRange.annotation` is
    an optional field absent from fragments without annotations, so their bytes
    are unchanged; a strict older reader refuses a fragment that has one (see the
    contract proposal in the report).

## Plumbing

-   `import_copy` rebuilds a document from `copy_document_fragment`, so links,
    comments and orphans survive with new identities.
-   The three commands are ordinary kernel commands: they work inside `batch`,
    respect `expected_revision`, and a refused one names its index and writes
    nothing.
-   Links add no text, so the reading-order byte ranges are unaffected.
-   An annotation addition inside a transaction skips the capacity re-count
    (`add_annotation_prechecked`): the planner has already counted.

## Package

Any range with `annot1`, or a restore hint, sets the required package bit
**16** (`REQUIRED_ANNOTATIONS`; 13 is paragraph styles, 14 and 15 are lists and
inline objects), including tombstoned and unreadable records, so an older
engine refuses the file rather than show bare ranges. Documents without
annotations are unchanged.

## Comment threads: in the document CRDT

Thread contents (messages, authors, resolved state) belong in the document's
CRDT, under a dedicated root keyed by thread ID, not on a server:

-   anchors and threads then sync, merge, work offline and save together, so a
    thread can never point at an anchor a replica hasn't seen (or the reverse);
-   undo of "delete this comment" restores anchor and thread in one step;
-   the engine already has bounded, validated sync (format 2 JSON deltas), so
    a server can still enforce comment-only permissions by checking that a
    peer's update only touches the thread root and comment annotations;
-   exports and packages carry review state without another service.

The server keeps what isn't document state: notifications, mentions, mapping
peers to accounts, and retention. Deleting a message tombstones it, as with
any CRDT content; purging needs the explicit compaction from decision 07.

## Suggestions (design only)

Suggestion mode would add a `suggestion` kind. One suggestion is a **group**: a
suggestion ID shared by one or two annotations (a replacement is a deletion and
an insertion). Payload: `{ group, op: "insert" | "delete", at }`, where `at` is
a host-supplied timestamp string, bounded like a thread ID; the author is the
annotation's author peer, as for every kind.

-   **Insertion:** typed text gets an annotation over it, edge policy
    `After`/`After` so continued typing joins it, `Disappear` when empty. Accept
    removes the annotation; reject deletes the text and the annotation.
-   **Deletion:** the text is not deleted but covered by a `delete` annotation
    (`FIXED`, `Disappear`). Accept deletes the text and the annotation; reject
    removes the annotation.
-   **Typing in suggestion mode** is a kernel-level rewrite, planned like the
    other commands: an `InsertText` becomes the insert plus its annotation; a
    `DeleteText` over the author's own pending insertion is a real deletion, over
    anything else it adds `delete` annotations (a deletion inside another
    author's insertion nests inside it). A transaction flag `suggesting(author)`
    selects it.
-   **Copy** pastes the accepted view by default: suggestion annotations are
    `Drop` for the clipboard and the deleted text is left out.

The engine would additionally need:

1.  **Hidden-but-present text in layout.** A per-span display mode resolved like
    formatting runs: *marked* (the deletion struck through, the insertion
    underlined, in the author's colour) or *final* (a deletion collapsed to zero
    width). Flow paragraphs already carry hidden segments between visible ones
    (`HostFlow` segments, used for breaks); a collapsed deletion is the same
    mechanism for a span instead of a break, with the composer still producing
    caret stops and source mappings so offsets stay authored. Reading order,
    search and spell-checking read the view the mode chooses.
2.  **Accept and reject as one transaction.** A kernel command
    `ResolveSuggestions { groups, action }` that deletes text and annotations (or
    only the annotations) for every group atomically, one undo step, working in
    `batch` with `expected_revision`. Two peers accepting the same group is a
    no-op for the second (the annotations are already gone).
3.  Overlap rules between suggestions (a deletion inside an insertion, two
    authors' overlapping deletions), export (`<ins>`/`<del>` in HTML, the
    final view by default in PDF with the marked view optional), and the
    permission check a server makes (a suggester may add suggestions and
    nothing else).

## How later features plug in

-   **Links in the app (hover cards, Cmd+K):** `DisplayPage.links` (or
    `page_links`) for hit-testing; `annotation(id)` for the card;
    `AddAnnotation` + `FormatText` in one transaction for Cmd+K;
    `UpdateAnnotation` to edit; `RemoveAnnotation` (+ clearing the style) to
    unlink. An internal link to a heading: `LinkDestination::Heading`.
-   **Comment threads and the sidebar:** the app creates a thread ID, then
    `AddAnnotation { comment { thread } }`; the sidebar lists
    `annotations_of_kind("comment")` (or `State.annotations`), orders by
    `annotation_geometry(id).top`, and shows orphans in a separate section.
    Thread storage is a later root (above), keyed by the same thread ID.
-   **Suggestion mode:** a new kind with the needs listed above.
-   **Bookmarks and internal links:** a bookmark is a small new kind (`POINT`
    policy, orphan on empty, payload a name). Internal links already target a
    node, a heading or a range; a bookmark is a range target, and its HTML
    export is the `id` mechanism the internal links already use.
-   **Search highlights:** **local-only overlays, not authored annotations.**
    They are per user, transient and recomputed per query, so storing them would
    pollute history and sync and would make every keystroke a document change.
    The app computes matches over block text and asks
    `LayoutSnapshot::span_geometry` for rectangles (it takes no document, so it
    works for any byte ranges); nothing touches the CRDT.

## App seam

Commands (in `apply` and `batch`): `add-annotation { node, start, end, annotation }`,
`update-annotation { id, annotation }`, `remove-annotation { id }`;
`annotation` is `{ kind: "link", target: { type: "url" | "node" | "heading" |
"range", … } }` or `{ kind: "comment", thread }`. Results: `Applied.annotations`
(the IDs created). `State.annotations` (compact, at most 2,048, with a
`bindings.limit` diagnostic past that). Queries: `annotation(id)`,
`annotations_at`, `annotations_in`, `annotations_of_kind`,
`annotation_geometry(id)`, `page_links(page)`; `DisplayPage.links`. Worker
kinds: `annotation`, `annotations-at`, `annotations-in`, `annotations-of-kind`,
`annotation-geometry`, `page-links`.

## Changes from the first draft of this note

-   The package bit is 16, not 13 (13 is paragraph styles on main).
-   Copy lineage distinguishes `CopyScope::Clipboard` from `CopyScope::Document`
    so duplicating a document keeps comment anchors.
-   Link areas are `page_links(doc, page)`; the draft's `link_areas` name is
    gone. Geometry gained a stale check, a batched form, and repeated header
    copies.
-   Suggestions now say what groups are, how typing in suggestion mode is
    planned and which layout mechanism hidden text reuses.
