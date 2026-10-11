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
-   `copy`: `Duplicate` (a copy gets a new ID and the same payload) or `Drop`;
-   payload validation, and the payload's variant of `AnnotationData`.

A new kind adds a `Kind` variant, an `AnnotationData` variant and a `KindSpec`.
An unknown kind name (a newer engine's) is kept verbatim, reported as
`annotation.unreadable` and never applied or copied. Its stored range still
keeps the package refused by older engines through the feature bit below.

| Kind | Start | End | Empty | Copy | Payload |
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
-   **Copy and paste:** the fragment carries each copied annotation range with
    its data. `Duplicate` kinds are pasted with new IDs; `Drop` kinds (a comment
    thread has one anchor) are left out of the copy with
    `clipboard.annotation-dropped` Info. Internal link targets are remapped
    through the paste's `IdMap`; a target outside the copy is kept only within
    the same namespace, otherwise the link is dropped with
    `clipboard.annotation-dropped` Warning (the text stays, the link goes).
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
    were truncated. Geometry returns at most 4,096 rectangles.
-   URLs are at most `MAX_URL_BYTES` (8,192); thread IDs at most 256 bytes of
    `[A-Za-z0-9._:-]`.
-   Unreadable, future or invalid envelopes (a hostile peer) are kept verbatim,
    never applied, and reported: `annotation.unreadable` Warning.

Authoring refusals carry stable codes: `annotation.link-scheme` (a scheme
outside `http`, `https`, `mailto`, `tel`, such as `javascript:` or `data:`),
`annotation.link-invalid` (empty, relative, whitespace or control characters,
no host for http(s)), `annotation.link-length`, `annotation.payload-invalid`,
`annotation.empty` and `annotation.limit`. The same checks run on HTML import.

## Index and queries

Annotations are read once per document revision into an index (as formatting
does): one pass over the range tree, one resolution per annotation, and
paragraph-local pieces grouped by paragraph. Queries never rescan the tree:

-   `annotation(id)`;
-   `annotations_at(node, at)`: pieces with `start <= at < end`, and orphans at `at`;
-   `annotations_in(node, bytes)`: pieces overlapping `bytes`;
-   `annotations_of_kind(kind)`;
-   geometry, in `reprise-layout`: `LayoutSnapshot::span_geometry(pieces)` and
    `annotation_geometry(doc, id)` give rectangles per line fragment with page,
    frame, frame-space rectangle, page-space bounds and the four page-space
    corners (so rotated, mirrored, vertical and spiral frames hit-test exactly),
    plus `top`: the first page and smallest page-space y, for a sidebar.
    An orphan has one zero-width rectangle at its point.

## Display, export and import

-   **Link areas:** `LayoutSnapshot::link_areas(doc, page)` gives each link's
    rectangles on a page with its ID. The facade's `DisplayPage` carries them
    as `links`, beside the frozen display list, which is unchanged (so backends
    and existing goldens are untouched). The app hit-tests them for hover
    cards. Underline and colour are not forced: Cmd+K is one transaction of
    `AddAnnotation` and a `FormatText` with the app's link style (underline
    and colour), so exports draw what the editor shows and the author can
    restyle a link.
-   **HTML:** links export as `<a href>` around the linked text (internal ones
    as `#r<node>`, with that id on the target paragraph). `<a href>` on import
    goes through the same validation; refused links keep their text and report
    `clipboard.html-approximated` Warning. Comments are not exported (Info).
-   **PDF:** each linked run part is a `Link` structure element holding its
    content and a link annotation with `/Contents` alt text (PDF/UA), a URI
    action or a destination for internal links.
-   **Plain text:** link text only; the loss is reported.
-   **Native:** annotations travel in the fragment (see the contract proposal).

## Package

Any range with `annot1`, or a restore hint, sets the required package bit
**13** (`REQUIRED_ANNOTATIONS`), including tombstoned and unreadable records,
so an older engine refuses the file rather than show bare ranges. Documents
without annotations are unchanged.

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

Suggestion mode would add a `suggestion` kind:

-   **Insertion:** typed text gets an annotation over it with payload
    `{ id, op: "insert", author }`, edge policy expanding at the end so
    continued typing joins it. Accept removes the annotation; reject deletes the
    text and the annotation.
-   **Deletion:** the text is not deleted but covered by `{ op: "delete" }`.
    Accept deletes the text and the annotation; reject removes the annotation.

The engine would additionally need:

1.  **Hidden-but-present text in layout:** a per-span display mode (struck,
    or collapsed to zero width) resolved like formatting runs, with the
    composer still producing caret stops and source mappings for it.
2.  **Accept/reject as one transaction:** a kernel command that applies one or
    all suggestions atomically, so undo restores the whole review action.
3.  Overlap rules between suggestions (a deletion inside an insertion), and
    copy that pastes the accepted view by default.

## How later features plug in

-   **Links in the app (hover cards, Cmd+K):** `DisplayPage.links` for
    hit-testing; `annotation(id)` for the card; `AddAnnotation` +
    `FormatText` for Cmd+K; `UpdateAnnotation` to edit; `RemoveAnnotation` to
    unlink.
-   **Comment threads and the sidebar:** the app creates a thread ID, then
    `AddAnnotation { comment { thread } }`; the sidebar lists
    `annotations_of_kind("comment")`, orders by `geometry.top`, and shows
    orphans in a separate section. Thread storage is a later root (above).
-   **Suggestion mode:** a new kind with the needs listed above.
-   **Bookmarks and internal links:** a bookmark is a small new kind (`POINT`
    policy, orphan on empty, payload a name). Internal links already target a
    node, a heading or a range; a bookmark is a range target.
-   **Search highlights:** local-only overlays, not authored annotations. They
    are per-user, transient and recomputed per query, so storing them would
    pollute history and sync. The app computes matches over block text and
    asks `span_geometry` for rectangles; nothing touches the CRDT.
