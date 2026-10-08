# Flow text: paragraphs as break markers

This document describes how paragraphs are stored so that Enter, Backspace and
multi-paragraph edits merge like ordinary typing (decisions 06, 07, 09, 11, 12, 29). It
replaces the split/join design that copied text into a new block.

## Why

Before this change every paragraph was its own content-tree node with its own text. Enter
copied the tail of the text into a new node, and Backspace at a paragraph start copied the
second text onto the first. Copying gives the moved characters new identities, so an edit a
collaborator made concurrently to the old characters was lost or landed in the wrong place:
a deleted word reappeared, a typo fix went to the old paragraph, a join discarded a
concurrent fix. Character lineage (`transfers.rs`) let carets follow the copy, but it can't
move a collaborator's concurrent operations.

With break markers, splitting and joining paragraphs never copies text. All paragraphs of a
flow share one CRDT text, and a paragraph boundary is one character in it.

## Model

A **host** is a content node whose kind is `paragraph` or `annotation` and that has no table
role. Its text container holds the whole flow:

```text
head text · M1 · text of paragraph 1 · M2 · text of paragraph 2 · ...
```

-   The **head paragraph** is the text before the first active break. Its identity is the
    host's `NodeId`, so a document written before this change reads exactly as before: a
    host with no breaks is one paragraph.
-   A **break** is the character U+FDD0 (a noncharacter, never authored text) with a
    record in the host's `breaks1` map, keyed by the character's Loro ID
    (`counter@peer`). The paragraph after an active break has the identity
    `NodeId::break(host, mark)`, displayed as `12@1/34@2`.
-   A record is a map: `v` (1), `kind`, `style`, `overrides` (a style map), `active`
    (bool), optional `image`, and succession keys as on tree nodes. Every field is its own
    map entry, so concurrent edits to different fields both survive.
-   An **embed** is a break whose record has `embed` set to a child node of the host. It
    places that node (a pasted block, a table, an image) in the flow at that point. See
    *Embeds*.

Only U+FDD0 characters with a readable, active record are breaks. Every other U+FDD0 —
inactive, unrecorded or unreadable — is hidden: it is never part of a paragraph's text.
Views refuse to insert U+FDD0, so authored text can't contain one.

### Paragraph views

`Document::block(id)` returns a `Block` whose `text` is a **view**: byte offsets count only
that paragraph's visible characters. Reads and writes through the view are translated to the
host text. A view is a handle like any `Text`: it recomputes its span on every call, so it
stays correct while the host changes.

At a gap between two visible characters that contains hidden characters, an insertion goes
before the hidden ones. Offset 0 of a break paragraph is right after its marker; its end is
right before the next active break. Anchors work unchanged: they are cursors in the host
text, and `Document::locate` says which paragraph a cursor is in now.

### Liveness

| Paragraph | Live when |
| --- | --- |
| Head | The host is live and its `head` flag is not `false`. |
| Break | The host is live, the marker character is not deleted, and its record is readable and `active`. |
| Embedded node | The node is live, its tree parent is the host, and the earliest active embed marker in the host text names it. |

When the head is deleted (`head = false`), text before the first active break joins the
first active paragraph. When no paragraph of a host is live, the host is hidden like a
deleted block.

### Order

`children(parent)` expands each host into its paragraphs in text order: the head, then each
active break or embed. An embedded node expands recursively, up to `MAX_FLOW_DEPTH` (32)
levels; deeper embeds are hidden and reported. Tree children of a host that no embed
names keep their old meaning: they are the host's children, not part of the flow.

`parent_of` a break paragraph or embedded node is its host's parent. All these paragraphs
are siblings.

## Operations

### Staging keeps identity through undo

Undo of an insertion deletes the characters, and redo inserts new ones with new IDs. So a
marker is never inserted by an undoable operation. As with blocks (07), a new marker is
**staged**: inserted, with `active = false`, in a commit excluded from the undo history,
before the transaction's undoable commit. Hidden characters don't change any view offset,
so staging is invisible. The undoable commit sets `active = true`. Undo and redo then only
flip the flag, and the paragraph keeps its ID.

Loro's undo manager can't merge one step across an excluded commit that touches the same
text, so all markers of a transaction are staged **before** its first undoable change, at
their positions in the text as it was before the transaction. A text the transaction inserts
on both sides of a new break is written as two insertions, one on each side of the marker.

### The operations

| Operation | Effect |
| --- | --- |
| Split at `at` | Stage a marker at `at`, copy the paragraph's kind, style and overrides into its record, activate. No text moves. |
| Join `first`, `second` | If `second` is a break paragraph directly after `first` in the same host, deactivate its marker and record `second` superseded by `first`. Otherwise (different hosts): the old copying join, with lineage. |
| Delete a break paragraph | Delete its visible text and deactivate its marker. |
| Delete a head paragraph | If the host has live break paragraphs, delete the head's visible text and set `head = false`. Otherwise soft-delete the host as before. |
| Insert or place a block between two paragraphs of a host | Stage the block as a child of the host and stage an embed marker at that boundary; activating both places it. |
| Move a break paragraph | Copy it into a new staged block and delete the original, with lineage. Moving is a copy, as in other collaborative editors. |
| Set overrides on a break paragraph | Write its record's `overrides`. |

Concurrent outcomes:

-   Two peers split the same paragraph at different places: both markers stay; three
    paragraphs.
-   Two peers press Enter at the same place: two markers next to each other; an empty
    paragraph between them, as in any text CRDT.
-   One peer joins while another types in either paragraph: the typing survives in the
    joined paragraph.
-   One peer joins (`active = false`) while another restyles the second paragraph: the
    restyle is kept in the record and has no effect while the break is inactive. Undoing the
    join brings it back.
-   A peer deletes a paragraph while another types in it: the deleted text is gone and the
    typing survives, in the paragraph before it.

## Ranges

A range stores the node it was created in and two anchors. Resolution locates both anchors
with `Document::locate`, so a range follows its characters into the paragraph they are in
now. `RangeState` reports the start's paragraph; when the end is in a later paragraph, the
bytes run to the end of the start's paragraph, and `Document::range_extent` gives both ends.

## Hostile states

Everything is read-time normalisation (see `collaboration.md`): no state is repaired on
import.

-   U+FDD0 without a record, with an unreadable or future record, or inactive: hidden.
-   A record without a marker character, or whose character is deleted: ignored.
-   An embed naming a node that is not a child of the host, not live, or already embedded
    earlier: ignored (the marker is hidden).
-   Embeds nested deeper than `MAX_FLOW_DEPTH`: hidden, reported by `audit`.
-   The number of breaks in a host is bounded only by its text length, which the sync
    limits already bound. Reading a host scans its text once per revision; views are
    cached per host and revision.

## Performance

A host's paragraph boundaries are computed once per document revision and host, then
cached. Layout reads paragraphs by `NodeId` as before, and caches by the paragraph's text and
style, so typing in one paragraph re-lays-out one paragraph even when its host holds the whole
document (see `incremental.md`).
