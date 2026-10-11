# Engine plumbing: transactions and host seams

Status: design note for `ws/plumbing`. It is committed before the code and
updated when the code settles. Decisions it leans on: 07 (identity), 29
(mutation and undo), 33 (reading order), 34 (file format), 35 (clipboard).

The web editor's workstreams each logged a place where the app works around the
engine. This note covers the seven that sit on the facade (`crates/reprise`),
the paste kernel and the marks query. None adds an authored form, so **no
package feature bit is taken**: documents written by this branch stay readable
by every older engine, and no existing snapshot changes.

## 1. Atomic replace and the batch

`Editor::apply` already runs a list of commands as one validated transaction
and one undo step. What it cannot do is anything that needs the document as it
looks *after* an earlier step: a paste is exclusive because it reads the
document, and a stable selection is only meaningful against the document that
exists when it is resolved. The app therefore pastes first and deletes second
(two undo steps, with a window in which a remote edit can land).

**Replace** is one kernel operation, `Editor::replace_selection`:

1.  *Stage* (`Editor::stage_replacement`): decode and validate everything that
    does not depend on where the text goes. That is the fragment's own
    validation, style renaming, relation policy, size bounds, and a
    conservative bound on the text that stays around the selection. It writes
    nothing. A refusal here leaves the selection untouched, so a bad clipboard
    can never erase selected text.
2.  *Commit* (`Editor::commit_replacement`): resolve the stable selection **in
    the current document**, derive the deletion from the resolved ends, write
    it, write the paste at the start of the range, and end the undo step once.

Because step 2 resolves the selection, a remote packet imported between 1 and 2
is simply part of the document the selection is resolved in. The facade
performs both stages inside one call; the split exists so the kernel (and its
tests) can prove the property. A single `commit_step` closes the delete and the
paste together, so there is one undo step.

The range between the ends is taken in **document order** (the preorder of the
content tree, the order `State.blocks` lists). A selection that spans blocks
becomes: delete the tail of the first block, delete the head of the last block,
delete the blocks strictly between whose subtrees hold neither end, delete the
text of in-between ancestors of an end (their subtree must survive), and join
the last block onto the first when the kernel allows it (same kind, no
children). When it does not, the blocks stay separate and the paste lands at
the start of the range. Reading-order overrides are not consulted: they are a
layout notion, and replace must work without a layout.

**Batch** is the facade's `batch`. It is `apply` made safe for hosts that edit
concurrently and track a selection:

-   `commands` is the same list `apply` takes. It is all or nothing, validated
    whole before anything is written, with the index of a refused command in
    the error. A failing third command changes nothing, not even the revision.
-   `expected_revision` is a precondition. If the document has moved, the
    batch is refused with `bindings.stale` and nothing is written (the app's
    `editAnchored` proposal).
-   `selection` is mapped through the batch's effects and returned, so the host
    never recomputes a caret from a stale position.
-   `Command::ReplaceText { selection, text }` is a new **selection-addressed
    command**. It is resolved and expanded against the document *and the earlier
    commands of the same batch* (positions are mapped through the effects so
    far), so "make heading" or "accept a suggestion" can replace text and change
    a style in one step. Rich (native or HTML) replacement carries bytes and
    stays a separate call, `replace_selection`, because it needs the document
    after the deletion.

Extension points: a new selection-addressed command is one more `Command`
variant whose expansion produces ordinary commands; a new precondition is one
more optional field on `Batch`.

## 2. Import as a copy

`Workspace::import_copy(request, bytes)` opens a package and returns a session
with a **new document identity**. The copy is rebuilt, not cloned:

-   *Kept:* all text and formatting, the block tree, named styles (including
    unreferenced ones), page setup and templates, relations, ranges, tables,
    image blocks, bundled assets and fonts, extension bytes and unknown
    package sections.
-   *Dropped:* the CRDT history, undo stacks, presence, and the source's
    document id and peers.

The content is replayed (through the native fragment of the whole document,
plus every named style) into a fresh CRDT document authored by the copy's own
peer. A clone with history would share operation ids with the source: any peer
that later held both would merge them. A fresh history has no operation the
source has, so honest traffic cannot cross:

-   the copy refuses a packet that names its source with
    `bindings.copy-source`; the source refuses the copy's with `bindings.id`
    (a foreign document), as for any other document;
-   a source *delta* with a rewritten document id still cannot apply, because it
    depends on operations the copy lacks.

*Known gap.* A relay that rewrites the document id on a source **snapshot**
packet makes the copy merge it, because a snapshot carries all its own history.
Packets are not authenticated (see `collaboration.md`), so the relay must keep
one room per document id. Closing the gap in-band needs a lineage marker in the
document, which is a new stored form and so a package feature bit; it is left
as a follow-up.

The copy may use the same peer id as the source. The two histories are
independent, so their operation ids can coincide; nothing ever merges them.

## 3. Fonts once per workspace

`Workspace` gains a shared font set (`Workspace::declare_font`,
`Workspace::set_font_settings`). A declared face is parsed once and held as
`Arc<Face>`; every session the workspace opens copies the `Arc` pointers, not
the bytes. Sessions opened earlier pick up later declarations the next time
they lay out or save (a generation counter decides; nothing is polled).
Per-session `declare_font` stays and is documented as the fallback for a face
that belongs to one document. The same face declared both ways is one identity
(the hash), so the session's own copy wins harmlessly.

Settings: workspace settings are the default for every session of the
workspace. A session's own `set_font_settings` detaches that session from the
workspace settings from then on (the document's explicit choice wins). Settings
are still host state and are never stored in a package.

*Binary path.* The WASM `Workspace.declare_font(request, content: Uint8Array)`
takes the bytes as a typed array, like `DocumentSession.declare_font`. The
Tauri host should keep one engine thread owning one `Workspace`, receive font
bytes over Tauri's binary IPC (a raw request body, not a JSON array of
numbers), and call `Workspace::declare_font`; it then opens every session from
that workspace.

## 4. Mark facts

`Mark` gains `edge` (the authored `start`/`end`), `target_node` and
`target_offset`, and `diagnostic` (the stable code explaining an unapplied or
suspect relation: `relation.alignment-cycle`, `-limit`, `-outside`, `-conflict`
or `-invalid`). They come from the same resolved guide the mark already uses,
so a host needs no hit test. `DocumentSession::diagnostics_for(filter)`
returns diagnostics filtered by code prefix, severity floor and subject, with a
hard cap, so a host can ask about one relation without a full dump.

## 5. Reading order with byte ranges

`ReadingStep` gains `start` and `end`, the UTF-8 byte range of the step's line
in its block's text, from the layout's line. A step whose line the layout no
longer holds is an error, not a guess.

## 6. One packet bound

`MAX_PACKET_BYTES` (facade and TS) is the engine's bound, 32 MiB. Rationale: it
is the largest snapshot packet the sync layer accepts, and a document whose
snapshot is larger cannot be joined by a new peer. 16 MiB would make documents
the engine and relay accept unstoreable locally; 64 MiB (the persistence cap)
would let one packet dwarf the 32 MiB font and 16 MiB asset bounds that travel
beside it. The relay and the browser store should both import the constant
rather than carry their own number: the relay's 32 MiB already matches;
browser persistence should raise from 16 MiB to the constant.

## 7. Facade review

`docs/facade-review.md`, prose only.

## How later features plug in

Written with the final report and kept here for the next wave.
