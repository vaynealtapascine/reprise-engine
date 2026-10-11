# Facade review

An assessment of the `reprise` facade (`DocumentSession`, `Workspace`, the DTOs
in `dto.rs` and the WASM wrapper) written while touching it for `ws/plumbing`.
It contains no code. It feeds the refactor round that follows this wave, so each
item says what is wrong, why it costs something, and what a refactor could do.
Items are ordered by how much they cost a host today.

The web app's wrapper, `app/engine/{protocol,worker}.ts`, is the evidence for
what hosts end up rebuilding. It is read-only context here.

## 1. What the app's wrapper reimplements

1.  **Releasing jobs after every mutation.** `worker.ts` calls `release()` after
    `edit`, `undo`, `redo`, `font`, `asset`, `save`, `paste`, `importText`,
    `image` and a changing `syncImport`, and tracks one `active` job by hand. The
    facade already makes a job stale by token (document, revision, generation),
    so the call is only about freeing a handle. A facade that drops its own
    superseded jobs, or hands the WASM class one retained job rather than a
    numbered map, removes the whole class of "forgot to release" bugs. The same
    applies to the eight-job limit.
2.  **Stepping and fetching pages as two calls.** `step` returns
    `progress.pages` (page numbers) and the wrapper then calls `partial_page` for
    each. One call returning `{ progress, pages: DisplayPage[] }` is what every
    host builds. The native `LayoutJob::step` and the WASM class disagree on this
    shape already (`display_page(session, page)` versus `partial_page(job,
    page)`).
3.  **Re-reading `State` after each edit.** Every mutating operation in the
    wrapper returns `{ applied, state: current().state().data }`. `State` is the
    whole document: every block's text and formatting runs. An edit of one
    character costs a full-document serialisation twice (engine and structured
    clone). The facade should offer a `Changes`-style delta (blocks changed,
    created, deleted; page setup changed) on `Applied`. `sync_import` and
    `undo_report` already return a `Changes`; `apply` should too, and `State`
    should become something a host reads at open and on resynchronisation.
4.  **The `Payload { version, data }` envelope.** The wrapper wraps every input
    with `payload(...)` and unwraps every output with `.data`, on both sides, for
    every operation. The version is per envelope, not per call, and it is
    constant. A WASM method could take and return bare data and check the
    version once at construction, with the envelope kept for the Tauri JSON
    transport only.
5.  **Operation names.** The wrapper renames the facade: `font` for
    `declare_font`, `asset` for `register_asset`, `move` for `move_cursor`, `hit`
    for `hit_test`, `caret` for `caret_rect`, `glyphs` for `glyph_outlines`,
    `importText`, `syncImport` for `sync_import` and the older `import_updates`.
    The facade should have one vocabulary and the wrapper none.
6.  **Selection plumbing.** The app builds its own multi-block delete from `move`
    calls in reading order (`deletionPlan` in `app/input/editing.ts`) and maps a
    caret through `Applied.effects` itself (`mapSelection`). The kernel knows both.
    `replace_selection`, `replace_text` and `batch` (this wave) take the first
    over; `Applied::map_position` is available but has no facade method, only
    `batch` uses it. A `map_selection(applied, selection)` query, or returning
    the mapped selection from every mutation, would remove the second.
7.  **Compare-and-set.** The app serialises its own "expected revision" guard
    around edits. `batch` now has it. `apply`, `paste`, `import_text`,
    `insert_image` and `set_page_setup` still do not, so only a batch is safe
    against a concurrent import. Giving every mutation an optional
    `expected_revision` (or making them all batches) would finish the job.

## 2. Naming that is duplicated or inconsistent

-   **Two sync paths.** `export_updates`/`import_updates` (the legacy v1 snapshot
    update, `SyncUpdate`) and `sync_export`/`sync_import` (format 2, `SyncPacket`).
    Both are public, both documented, one trusted and one validated. The legacy
    pair should be deprecated and then removed; it also needs its own
    `copy-source` check, which this wave had to add twice.
-   **Four verbs for "write bytes out".** `save` (package), `export` (plain,
    HTML, native, PDF), `export_updates` and `copy`/`copy_as`. `copy` returns
    native bytes but `export(Native)` also exists. `copy_as` projects a
    selection; `export` projects the document. A clearer split: `save` for
    packages, `export(format, selection?)` for projections, `sync_*` for
    packets, and `copy` only as the native-clipboard case of `export`.
-   **Declare, register, install, load.** `declare_font`, `register_asset`,
    `load_plugin`, `install_plugin`. Fonts and assets are the same act (give the
    session bytes it cannot name itself), and now fonts exist at two scopes
    (`Workspace::declare_font`, `DocumentSession::declare_font`). Pick one verb.
-   **`Open`, `Create`, `ImportCopy`.** Three request types with overlapping
    fields. `Create` and `ImportCopy` both carry `document_id` and `peer_id`;
    `Open` carries only `peer_id` and takes the id from the package. One
    `OpenRequest { source: Empty | Package(bytes), document_id?: , peer_id }`
    would cover all three, and `import_copy` is then "open with a new id".
-   **`Command` versus kernel `Command`.** The facade `Command::SetStyle` is the
    kernel's `SetStyleOverride`; `AddAnchor`/`RemoveAnchor` are sugar over
    `AddRelation`/`RemoveRelation` with a reserved schema. `InsertImage` is a
    command that must be alone, and so is a native paste; neither can be in a
    batch. The set of commands is split across `apply`, `insert_image`,
    `set_page_setup` (a command too), `paste`, `import_text` and now
    `replace_*`. A refactor should make every one of them a command or a
    clearly separate "document-addressed" operation, not both.
-   **`State.revision` versus `SyncInfo.vector`.** Both are `Vec<Clock>`. One is
    a frontier and one is a version vector, and `bindings.md` has to say "a
    frontier is not a version vector". Distinct types (`Frontier`,
    `VersionVector`) cost nothing on the wire and catch the mix-up.
-   **`Selection`, `StableSelection`, `Caret`, `StableCaret`** are fine, but
    `Paste.at`, `TextImport.at` and `ImageInsert.at` are `Option<Caret>` while
    `replace_*` take a `StableSelection`. A stable caret everywhere would make
    all insertions safe against concurrent edits, and `at: None` ("append")
    would become an explicit variant.
-   **`Applied`, `Replaced.applied`, `BatchReport.applied`, `EditReport`.** Four
    result shapes for "what an edit did". `EditReport` has `changes` and a
    selection; `Applied` has created ids and effects. They should be one record.
-   **Strings with grammar.** `Diagnostic.subject` is `"relation:<id>"`,
    `"node:<id>"`, `"range:<id>"` or `"document"`, parsed by hosts. It should be
    a tagged object. `Block.id` and every other id is an opaque string whose
    validity is checked on each call; a branded type on the TypeScript side is
    free.
-   **Integer widths.** Offsets and counts are `u32`, lengths are `i32`, clock
    counters are `i32`, `LayoutOptions` pages are `u32`. None of it overflows,
    but hosts convert between `number` and `bigint` nowhere, so it only costs
    reading. Document the unit (1/1024 pt, UTF-8 bytes) in the type, not the
    prose.

## 3. Payloads that could be smaller

-   **`State` on every mutation** (item 1.3). The largest single saving.
-   **`Mark` has 15 fields, most `null`.** A paragraph-end mark carries `to`,
    `target_page`, `alignment`, `relation`, `state`, `applied`, `edge`,
    `target_*` and `diagnostic`, all null. A tagged union per `kind` is roughly
    a third of the bytes on a page of ordinary text, and a page can hold 20,000
    marks. This wave added five more optional fields to it, which is the
    argument.
-   **`LayoutProgress.pages` plus every `DisplayPage`** on each step resend pages
    that have not changed. The token already says which revision a page belongs
    to; a `dirty` flag, or sending only pages whose content changed, matches the
    "dirty-page transport" the docs already hint at.
-   **`SyncPacket` snapshots.** A join sends the whole history as JSON updates
    inside a framed packet. A delta is small, but the first join of a large
    document is bounded only by `MAX_PACKET_BYTES`. A compact binary delta (the
    follow-up already named in `bindings.md`) and a state-only join that drops
    history for read-only peers would cut it.
-   **`SyncReport.info`** repeats `sync_info()` after every import, which the
    host needs only when it acknowledges. Make it opt-in.
-   **`PresenceView.rects`** carries selection rectangles for every peer on every
    presence message. Rects can be requested for the visible page only.
-   **Font bytes** were the single biggest avoidable payload: ~4.9 MB per
    session as a JSON array on desktop. Fixed this wave for the web and native
    paths (`Workspace::declare_font`); Tauri still has to use its binary channel.
-   **`ReadingStep`** is three numbers and a string per line, thousands of times.
    Steps could be grouped by block (`{ node, lines: [...] }`); the ranges added
    this wave make the lines of a block tile it, so only boundaries are needed.

## 4. Smaller things worth fixing in the same round

-   `Workspace` was a unit struct and is now a handle on shared fonts. It needs a
    `close`/`free` story for native hosts (it frees on drop; WASM needs `free()`).
-   `DocumentSession::contain` must be called by every host for every call; a
    macro or a `Session` wrapper type that does it by construction would remove
    the "forgot to wrap" failure.
-   `diagnostics()` mixes package notes, layout diagnostics and (in `State`) page
    setup and malformed-node diagnostics, assembled in two places. One
    collector with a stable order would make `diagnostics_for` trivially
    consistent with `State.diagnostics`.
-   The API surface is pinned by a text listing (`API.txt`) produced from source
    lines, which sees `pub fn` but not types, trait impls or visibility changes
    inside modules. Replace it with `cargo public-api` or equivalent.
-   `Workspace::font_handles` is a test hook in the public API. It would be better
    as a `#[doc(hidden)]` method or a feature-gated one.
-   `ReplaceSelection`, `ReplaceText`, `Batch` and `BatchReport` could carry the
    session's `LayoutToken` so a host does not have to infer that its view is
    retired; today any mutation silently clears the snapshot.
