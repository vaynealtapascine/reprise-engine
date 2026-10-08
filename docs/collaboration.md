# Real-time collaboration

This document describes how reprise-engine supports a multiplayer editor (decisions 05, 07,
09, 10, 12, 29, 30, 34, 37 and 38). The engine provides sync *primitives* only. Transport,
authentication and servers belong to the host (see the implementation choices under 41).

The pieces are:

1.  **Sync packets** (format 2): delta updates since a version vector, plus a full form for
    the first join, in a versioned frame that names its features.
2.  **A trust model** that separates what the kernel validates from what a peer can write.
3.  **Post-merge invariants**: what every replica can rely on after any merge, and what
    happens when a merge breaks an invariant.
4.  **Stable positions and presence**: carets and selections anchored to characters, and a
    presence payload that travels in the awareness bytes.
5.  **Change reports**: what an import, undo or redo changed.
6.  **Per-user undo** across remote edits.

The **v1** path (`export_updates`/`import_updates` with self-contained history snapshots)
is still readable for trusted legacy peers. New multiplayer transports use format 2
for both first join and ongoing edits; do not fall back to v1 on untrusted input.

## Trust model

There are three sources of authored operations:

| Source | Example | Checked by | Trusted for |
| --- | --- | --- | --- |
| Local kernel | `Editor::apply`, paste, plugin edits | full transaction validation (29) | everything |
| In-process replica | `Document::merge`, `Editor::merge` | nothing more: both sides are this process's own documents | everything |
| Remote packet | `Document::import_packet`, facade `sync_import` | packet preflight (this document) | **nothing semantic** |

A remote packet is checked for **shape and cost**, never for meaning:

-   its frame, format version and feature set;
-   its size, change count, operation count, counters and Lamport timestamps;
-   that it is causally complete against this replica (see *Missing dependencies*);
-   that it carries no operation under the receiving replica's own peer ID that the receiver
    has not already seen.

After that, every operation the CRDT can represent is accepted, including operations no
kernel would produce: raw map writes with wrong types, unknown keys, tree moves and deletes,
texts written into the wrong container, and so on. This is deliberate:

-   **Concurrent kernel edits can already combine into states the kernel would refuse.**
    For example, one peer joins two paragraphs while another adds a child to the second.
    So readers must tolerate such states anyway. A semantic filter on packets would not
    remove that need.
-   **A filter that rejects a packet makes replicas diverge.** One peer accepts the packet,
    another refuses it, and from then on they hold different CRDT states, forever. CRDT
    convergence is only useful if every replica applies the same operations.
-   **Read-time normalisation is deterministic.** Every replica derives the same view of
    the same CRDT state, so a hostile operation produces the same diagnostics everywhere.

The engine **does not authenticate** peers. Anyone who can deliver a packet can write
anything a peer can write, under any peer ID other than the receiver's. Authorisation,
including read-only participants and per-user rights, belongs to the transport or server,
which must drop packets from unauthorised senders before they reach the engine.

## Post-merge invariants

These are the invariants every state produced by the kernel satisfies. For each, the table
says what happens when a merge breaks it, either through concurrent kernel edits ("C") or
through raw hostile operations ("H"), and why that handling converges.

The guiding rule is **read-time normalisation with a diagnostic**. The CRDT state is never
repaired by writing to it on import. A repair written by one replica on import would be a
new operation that every other replica also writes, so the same repair is applied twice or
N times. Most repairs are not idempotent under merge: for example, moving an orphan, or
renaming a style. And a repair raced by a later edit diverges in intent. Read-time
normalisation needs no agreement, because it is a pure function of the CRDT state.

| # | Invariant | Broken by | Handling (all read-time, deterministic) |
| --- | --- | --- | --- |
| I1 | **Tree shape:** the content tree is acyclic and each node has one parent. | C: concurrent moves that would create a cycle. H: raw tree moves. | Loro's movable tree resolves cyclic moves deterministically: the later move in its total order is ignored. The invariant always holds; there is no diagnostic. |
| I2 | **Block envelope:** each live content node has a known `kind` string, a `text` container and a string `style`. | H: raw meta writes such as a missing or ill-typed kind, text as a value, or an unknown kind. | `Document::block` returns `DocError::Malformed`. Layout omits the block with `layout.malformed-block` (Error); its children still lay out. `audit` reports `collab.malformed-node`. An unknown kind written by a newer engine is kept (34). |
| I3 | **Deletion flag:** `deleted` is a boolean; a node is live when no ancestor carries `true`, and Loro's tree has not physically deleted it. | C: concurrent delete and restore (map last-writer-wins). H: a non-boolean flag, or a raw tree delete. | A non-boolean flag reads as *not deleted*. A physically deleted node and its subtree are not live; relations and ranges use their deletion policies (15). `audit` reports `collab.tree-tombstone` (Warning) for a physically deleted node that still has content: the kernel never deletes physically, so its content is hidden, not deleted by an author. |
| I4 | **Live content under a deleted parent** is not expected: the kernel only adds children under live parents, and a join requires the second block to have no live children. | C: a block inserted or moved under a parent that another peer deletes or joins at the same time. | The subtree is hidden (the ancestor flag wins), deterministically. `audit` reports `collab.hidden-content` (Warning) on the live-flagged child, so a UI can offer to restore or move it. Undo of the deletion reveals it again. |
| I5 | **Table grid:** a table's children are rows, a row's children are cells, a cell's `column` is inside the table's column list, and the role record is readable. | C: row deletion against cell edits, concurrent column changes and cell inserts. H: raw role records. | Layout's table reader reports `layout.table-invalid` and lays out the readable part, as the tables workstream defines. A deleted row hides its cells and their edits; undoing the deletion brings them back with the concurrent edits. |
| I6 | **Relation ownership:** an owned relation's owner is live; its JSON matches a registered schema. | C: owner deleted concurrently, or a relation added to a node deleted concurrently. H: raw JSON. | Resolution reports `relation.owner-deleted` (Info), `relation.target-deleted`, `relation.missing-target`, `relation.unreadable` or `relation.unknown-schema`, per 14 and 15. Nothing is written. |
| I7 | **Range endpoints:** both anchors are in the range's node's text, start ≤ end, and the policy record is readable. | C: endpoint text deleted, or ends crossed by concurrent edits. H: raw anchors or policies. | `resolve_range` gives `Rebound`, `Missing`, or empty at the start for crossed ends (12). An unreadable policy resolves as `Missing`. Nothing is written. |
| I8 | **Style references:** a block's named style and every parent in a chain exist, with no cycles. | C: a style deleted while another peer applies it; two peers each making the other's style its parent. H: raw style maps. | `style.parent-missing`, `style.parent-cycle` and `style.chain-too-long` at resolution. Values fall back to defaults (08). |
| I9 | **Image records:** an image block has a readable `image1` record whose asset hash is canonical. | C: an image block deleted while its alt text is edited (fine: the alt text survives in the tombstone and comes back with undo). H: raw records. | `layout.image-record` with a placeholder (the image workstream's rule). |
| I10 | **Succession:** succession links point at nodes, and rebinding terminates. | C: two peers join A into B and B into A at the same time, making a cycle. | Rebinding follows at most `MAX_SUCCESSION_DEPTH` generations and reports `relation.rebind-limit`. |
| I11 | **Identity:** IDs are never reused (07). | H: a peer claiming the receiver's own peer ID. | Refused at the packet boundary with `sync.local-peer`: the receiver would otherwise later reuse counters that the hostile peer used. Other peers' IDs can be forged; that is the transport's problem (see the trust model). |
| I12 | **Unknown data:** keys and root containers that this engine does not know. | H: arbitrary writes. | Kept and ignored (34). They are saved and travel with sync. |

`reprise_doc::Document::audit()` checks I2–I4 over the whole tree and returns notes in
document order. Layout and resolution already report I5–I10 when they read the affected
data. The audit is O(nodes). A host may run it after imports, for example to show a
"some content is hidden" banner. The engine never runs it implicitly.

### Concurrency matrix

`crates/edit/tests/concurrency.rs` runs every pair below from a common base, on two peers.
It runs both merge orders, and seeds the operands. For every pair it checks that the peers
converge, that layout, display, save and reopen do not panic, and that the audit and layout
diagnostics use only documented codes.

| Pair | Converged result |
| --- | --- |
| delete block / edit inside it | Block deleted; the edit is kept in the tombstone. Undo of the delete shows it. |
| split / join of the same paragraph | Both happen. The joined text is a copy, so text can appear twice. That is an intent anomaly, not a broken invariant. |
| table row delete / cell edit in that row | Row deleted; the cell edit is kept. |
| concurrent moves that would form a cycle | Loro skips the later move (I1). |
| relation added / its target or owner deleted | Relation stored; resolution reports the deletion policy's status. |
| style deleted / style applied | Block refers to a missing style: `style.parent-missing`, with defaults. |
| image block deleted / alt text edited | Image deleted; the alt text edit is kept in the tombstone. |
| range endpoint text deleted / text inserted at the endpoint | `Rebound` or `Missing`, following the range's policy. |
| undo on one peer / edit of the same text on another | Only the undoing peer's step is reverted; the other peer's text stays. |
| insert child / delete or join its parent | The child is hidden (I4) with `collab.hidden-content`. |
| two joins forming a succession cycle | Rebinding stops at the limit (I10). |

### Hostile-peer fuzz

`crates/doc/src/hostile_tests.rs` writes seeded random raw Loro operations into the
engine's containers from a separate `LoroDoc`:

-   wrong value types, unknown keys and kinds;
-   texts in the wrong place, and huge or negative numbers;
-   tree moves, creates and physical deletes;
-   policy, relation, image and table records that are garbage or almost right.

It exports a delta packet and imports it into a kernel-built document, then runs layout,
display, save and reopen. Nothing may panic, and every diagnostic code must appear in the
contracts table. The fuzz is bounded and seeded, so it runs in `cargo test`.

## Sync packets, format 2

### Frame

A packet is one byte string. Every integer is little-endian.

| Field | Size | Meaning |
| --- | --- | --- |
| magic | 4 | `RSYN` |
| format | u16 | `2`. Version 1 is the facade's `SyncUpdate` snapshot path. |
| features | u64 | A bit set of features the receiver must understand. Any unknown bit is refused. |
| kind | u8 | `0` = delta, `1` = full snapshot |
| since | vector | What the sender assumed the receiver had. It is empty for a snapshot. |
| until | vector | The sender's version vector at export |
| body length | u32 | Then the body bytes; nothing may follow them. |

A vector is a `u16` count followed by `(u64 peer, i32 counter)` pairs. The pairs are sorted
by peer, peers are distinct, counters are positive, and there are at most 4,096 entries.

Feature bits in format 2:

-   bit 0, `DELTA_JSON`: the delta body is Loro's JSON change schema, version 1, in UTF-8.
-   bit 1, `SNAPSHOT_LORO_1_16`: reserved legacy binary snapshot bit, refused with
    `sync.feature` at the untrusted packet boundary.
-   bit 2, `SNAPSHOT_JSON`: a complete history in the same bounded JSON schema and
    operation vocabulary as deltas, with an empty `since` vector.

A packet sets only the bit its kind needs. A receiver refuses a packet with an unknown
format (`sync.format`) or an unknown feature bit (`sync.feature`) **before** reading the
body. Nothing is applied from a refused packet.

### Why deltas are JSON, not Loro's binary updates

Loro 1.16's binary update blocks are columnar and run-length encoded, and they are not
compressed the way snapshot KV blocks are. Their decoder sizes vectors from counts in the
header before checking them against the input. For example, it calls
`Vec::with_capacity(n_changes)` with an untrusted `u32`, and RLE runs repeat values. A blob
of a few bytes can therefore ask for gigabytes, and an allocation failure aborts the
process: there is no error to catch. Bounding that format would mean re-implementing its
decoder.

Loro's JSON change schema expands at most linearly. `serde_json` allocates in proportion to
its input and bounds nesting at 128 levels. Every count can be checked after parsing and
before Loro sees the changes. Loro's own JSON import also validates counters, op lengths
and created container IDs. One keystroke is a few hundred bytes of JSON, so delta size is
not the bottleneck.

The **snapshot** kind also uses JSON and passes every delta preflight check, including
causal text positions, operation vocabulary, receiver peer protection, and vector agreement.
Binary history snapshots are refused before decoding at this boundary. Package persistence
and the trusted v1 compatibility API remain binary; their separate limits are below.

### Delta preflight

`Document::import_packet` checks the following before anything reaches Loro:

1.  The frame, as above. The body is at most `MAX_PACKET_BYTES` (32 MiB).
2.  The JSON parses as Loro's `JsonSchema`, with `schema_version` 1. Before that, a
    walk over the body without building it refuses any non-ASCII `fractional_index`
    string: Loro's reader slices those strings by byte offset and would panic.
3.  There are at most `MAX_PACKET_CHANGES` changes, and their operations' total atom
    length is at most `MAX_PERSIST_OPS`. Each operation's length is at most
    `MAX_PACKET_OP_LEN`.
4.  Every peer index resolves, and no change uses the reserved peer `u64::MAX`.
5.  **Agreement with the frame:** each change lies inside `[since[p], until[p])` for its
    peer. For each peer, the changes cover that interval exactly, contiguously and without
    overlap. A packet whose vectors misdescribe its content is refused (`sync.invalid`).
6.  Counters and Lamport timestamps stay below `2^30` after adding operation lengths, so no
    later local commit can overflow them.
7.  **Causal completeness:** for every peer `p` in the packet, `since[p] ≤ local[p]`. Every
    dependency `(p, c)` must satisfy `c < max(local[p], until[p])`. Otherwise the packet is
    refused with `sync.missing`.
8.  **Local peer:** the packet carries nothing under the receiver's peer beyond `local[p]`
    (`sync.local-peer`).
9.  **Tree positions:** every tree create and move carries a fractional index that ends
    with Loro's terminator byte `0x80` and is at most `MAX_TREE_POSITION_BYTES` long.
    Loro accepts any bytes on import, but it unwraps when it later generates a position
    beside one without the terminator (an empty, all-`00` or all-`FF` index). That
    aborted the receiver's next local block insertion and poisoned its store, so the
    replica could never edit the document again. With the terminator, every generation
    path terminates and two distinct neighbours always have a position between them.
    `Document::import` applies the same check to every content node of a saved package
    or v1 snapshot, and refuses the document.

If any check fails, nothing is imported. When the checks pass, Loro imports the changes.
`ImportStatus.pending` must then be `None`. If it is not, the checks above have a bug: the
import is reported as `sync.missing`, and the pending changes stay in Loro's own buffer,
invisible to the state.

### Missing dependencies: refuse and report

When a packet depends on operations the receiver lacks, the receiver **refuses** it. The
`sync.missing` error tells the transport to fetch a delta since the receiver's vector
(`sync_info`). The engine does **not** buffer early packets, for three reasons:

-   Loro's own pending buffer is unbounded and invisible to the state, so a peer could fill
    memory with packets that never apply.
-   A bounded buffer in the engine would have to choose what to evict. Whatever it chose,
    the transport would still need the resync path.
-   Refusing keeps a simple invariant: the replica's state is exactly its version vector.

Tests apply packets in both orders. Applying them in order succeeds. Applying the later one
first is refused, then succeeds after the earlier one arrives.

### Duplicates and replays

A delta whose changes the receiver already has, completely or in part, is accepted. Loro
skips the operations it already knows. Importing the same packet twice leaves the state and
revision unchanged, and the second import reports no changes.

### In-process merge

`Document::merge(&other)` now exports only `updates(self.oplog_vv())`: what `self` lacks.
Before, it exported the whole history. Both documents belong to the same process, so the
binary update encoding is safe here. The receiver's own vector bounds what it asks for.

### Facade

The v1 methods (`sync_info`, `export_updates`, `import_updates`) are unchanged. The
additions are:

-   `sync_export(Payload<SyncRequest { since: Option<Vec<Clock>> }>) -> Payload<SyncPacket>`:
    `None` gives a full snapshot packet, a vector gives a delta.
-   `sync_import(Payload<SyncPacket>) -> Payload<SyncReport>`.

`SyncPacket` carries `document_id`, `from_peer`, `format`, `features` (names) and `kind`,
mirroring the frame so a transport can route without decoding, plus the frame in
`content`. Import checks that the mirrored fields agree with the frame.

`SyncReport` carries the new `SyncInfo`, whether anything changed, the change report, and
the transformed local selection (see below).

## Stable positions

### Stable carets

A `StableCaret` is `(node, anchor, affinity)`:

-   `node` is the caret's block when it was anchored.
-   `anchor` is an encoded Loro cursor in that block's text (`reprise_text::Anchor`).
-   `affinity` is the caret's `Affinity`.

The caret's affinity also chooses the anchor's side:

-   **Downstream** sticks to the next character (`text::Affinity::After`). Text inserted
    exactly at the caret lands *before* it, so the caret moves right.
-   **Upstream** sticks to the previous character (`text::Affinity::Before`). Text inserted
    at the caret lands after it, so the caret stays.

A `StableSelection` holds two stable carets, anchor and focus.

**Resolving** a stable caret is a pure function of the CRDT state, so every replica with
the same state resolves it to the same caret. It works in this order:

1.  **The node is live:** resolve the cursor. If its character was deleted, Loro gives the
    position where it was, between its surviving neighbours. The offset is then clamped to
    the text, and floored to a grapheme boundary (a concurrent combining mark can split a
    cluster).
2.  **The node is not live** (soft-deleted, under a deleted ancestor, physically deleted,
    or unreadable):
    1.  If succession (15) gives live successors, use the first in document order. The
        offset is where the old text would end in it,
        `successor.len - old.len + offset_in_old`, clamped and floored. This is exact for
        a join with no later edits, and close otherwise.
    2.  Otherwise, use the nearest live text block **before** the deleted node, at its end.
        The search walks the full tree, including hidden nodes, in document order.
    3.  Otherwise, the nearest live text block after it, at its start.
    4.  Otherwise, `None`: the document has no live text block.

A resolved caret that is not in the original node, or not at its anchored character, is
reported as *moved* (`Resolution::Moved`), so a UI can tell the user.

### The local selection across sync, undo and redo

`DocumentSession::set_selection(Option<Selection>)` anchors the host's current selection.
`sync_import`, `undo_report` and `redo_report` resolve it after their change and return the
transformed selection, and keep it anchored. The kernel's own `Applied.effects` remain the
way to move carets through **local** transactions. Stable anchors are for edits the local
UI did not make.

`anchor_selection` and `resolve_selection` expose the same conversions explicitly, for
hosts that keep several selections, for example per view.

## Presence

Presence is a versioned JSON payload inside the existing awareness bytes, so transports
need no change:

```json
{ "presence": 1, "selection": { "anchor": StableCaret, "focus": StableCaret } | null,
  "meta": { "name": "Ada", "color": "#a0f" } }
```

-   `meta` is opaque to the engine. It is bounded to 16 entries, with keys of at most 64
    bytes and values of at most 1 KiB.
-   The whole payload stays inside the 64 KiB awareness bound.
-   `presence(Payload<Presence>)` builds awareness bytes from a selection and metadata.
-   `resolve_presence(Payload<Awareness>)` returns the peer, its metadata, its resolved
    selection, and a caret rect and selection rects on the current layout.

Stale or garbage presence **degrades to nothing**: an empty view, never an error. That
covers unknown versions, bad JSON, an anchor from another document, a deleted node, or no
current layout. Only an unsupported facade envelope version is an error, as everywhere.

## Change reports

`Document::import_packet`, `Editor::undo_report` and `Editor::redo_report` return a
`ChangeReport`. It is collected from Loro's container events during the operation:

-   `blocks`: every block whose metadata, text or tree position changed. If a node's
    deletion flag or position changed, its descendants are added, because their visibility
    may have changed with it. The list may over-approximate. It never under-approximates.
-   `structure`: whether any tree node was created, moved or deleted.
-   `styles`: whether any named style changed. A UI should then treat every block as
    restyled.
-   `relations` and `ranges`: the IDs whose records changed.
-   `other`: whether an unknown root container changed.

Layout memos are keyed by content (27), so a merge does not invalidate untouched
paragraphs. `crates/reprise/tests/collab.rs` checks that a one-character remote edit
reshapes one paragraph in a many-paragraph document.

## Per-user undo

Undo is Loro's `UndoManager`, scoped to this peer's `reprise:step` commits. Remote changes
are never on the stack, and undo inverts only local steps, transforming them over remote
operations. The tests show that:

-   undo after interleaved remote edits removes only local text;
-   redo after a remote edit restores only local text;
-   the stack stays bounded (`DEFAULT_UNDO_STEPS`) across thousands of merges.

## Transport guide

What the Reprise side sends:

-   **On connect (first join):** send your `sync_info().vector` to the server or peers.
    They reply with `sync_export({ since: yourVector })`. A brand-new replica can instead
    ask for `sync_export({ since: null })`, a full snapshot.
-   **On each local edit:** call `sync_export({ since: lastAckedVector })` for each
    destination, or one shared "since my last broadcast" vector for a relay server. Send
    the packet. Small deltas can be batched; order does not matter, because a delta whose
    predecessors are missing is refused, not misapplied.
-   **On receiving a packet:** call `sync_import`. On success, refresh the blocks named in
    `changes`. Redraw with `selection` if you set one.
-   **On `sync.missing`:** ask the sender for `sync_export({ since: sync_info().vector })`
    and drop the refused packet. Retrying it later is harmless but unnecessary.
-   **On reconnect after offline:** exchange vectors and send each other deltas since the
    other's vector. Both sides then converge. A delta larger than the packet bound fails
    with `sync.limit`: request a full JSON snapshot packet. If that also exceeds the bound,
    the host must retain the session and report the limit; v1 is not an untrusted fallback.
-   **On `sync.format` or `sync.feature`:** the peer runs a newer engine. Keep the session
    and tell the user. Do not retry.
-   **On `sync.invalid` or `sync.local-peer`:** the sender is buggy or hostile. Drop the
    packet and consider disconnecting it.
-   **After compaction:** a replica can send deltas only from its retained base.
    If the requested vector predates that base, `sync.invalid` refuses the export;
    request the missing history from a full-history replica. Keep such a replica
    available when the host uses shallow packages.
-   **Presence:** send `presence(...)` bytes on selection change, throttled by the host.
    On receipt, call `resolve_presence` after each layout.

## Bounds

| Bound | Value |
| --- | --- |
| Packet body | 32 MiB (`MAX_PACKET_BYTES`) |
| Changes per delta | 1,000,000 (`MAX_PACKET_CHANGES`) |
| Total operation atoms per packet | 4,000,000 (`MAX_PERSIST_OPS`) |
| Single operation length | 16 Mi atoms (`MAX_PACKET_OP_LEN`) |
| Counters and Lamport timestamps | below 2^30 |
| Vector entries | 4,096 |
| Tree position (fractional index) | 16 KiB (`MAX_TREE_POSITION_BYTES`) |
| Presence metadata | 16 entries, 64-byte keys, 1 KiB values, 64 KiB in total |

## Known limits

-   **Binary compatibility** remains the trusted v1/package path. Its snapshot
    preflight now bounds expanded KV bytes and the internal columnar change and
    counter counts, summed across blocks, before Loro allocates them. Unknown
    encodings fail closed. Use format 2 for peer transport and its operation
    vocabulary/preflight, rather than treating binary compatibility as a fallback.
-   **Intent anomalies** such as a split concurrent with a join, which duplicates text,
    converge but are not merged semantically.
-   **Remote splits/joins** carry compact authored character lineage. Stable
    carets follow recreated characters across blocks, including chained splits
    and joins. Resolution prefers a still-live original; competing transfers use
    sorted record keys. Breadth-first search visits at most 256 distinct identities.
    Identity history survives undo; undo/redo records aliases for recreated IDs,
    so fresh and old carets follow later ordinary edits without relying on unchanged
    text. Alias discovery requires a live insertion prefix and matching SHA-256
    span digest. Dead intermediate identities remain searchable. Authored ranges
    retain their original block semantics.
-   **Authentication and authorisation** are out of scope (see the trust model).

## End-to-end verification

`crates/reprise/tests/collab.rs::three_peer_partition_rejoin_soak` exercises three
sessions across partitions, shuffled delivery, refused dependencies, replay, per-user
undo/redo, anti-entropy, stable selections, identical layout and package reopen.
`REPRISE_COLLAB_STEPS` controls its length. The WASM smoke test exercises the same
sync, selection, presence and undo APIs through actual generated JavaScript bindings.

`crates/reprise/tests/soak.rs` adds structural edits and block deletion to shuffled
partition/rejoin delivery; `REPRISE_SOAK_EDITS` controls its length. The native
packet-byte golden in `tests/sync_delta.hex` is checked by actual WASM bindings.
Diverged packet validation reads live lengths only for containers the store proves
unchanged since the packet dependencies; concurrently edited texts still use the
isolated shadow checkout.
