# Loro 1.16.2 bounded tree depth

Source: crates.io `loro-internal` 1.16.2 (MIT, as in Cargo.toml).
Four behavioral source changes, none of which alters a stored format. The first three remove one
recursion per tree level, which exhausts the 2 MiB native test stack or the 1 MiB WASM
stack on a valid document nested a few thousand levels deep.

1. `src/state.rs::root_container_is_empty` uses `TreeState::is_empty` (a root-children
   count) for trees rather than recursively constructing `get_value`. This makes both
   `LoroDoc::get_value`'s root catalog and a one-component `get_by_str_path` lookup
   independent of tree depth.
2. `src/state/container_store/container_wrapper.rs`: decoding a fast-snapshot tree no
   longer builds `TreeState::get_value` only to discard it; `decode_value` keeps the
   decoded state and ignores that value.
3. `src/state/tree_state.rs::_bfs_all_nodes`, which orders the nodes of a fast-snapshot
   export, walks an explicit stack in the same order instead of recursing per level.

4. `src/diff_calc/tree.rs::diff` elides an exact `UnCreate`/`Create` replay
   round trip when it is the node's only pair of actions and restores the same
   parent and fractional position. Concurrent tree joins otherwise revive an
   unchanged existing node's metadata and re-emit its text as fresh inserts.
   Loro's undo transform then moves a pending deletion past the end of that text.
   Actual moves, deletions, revivals and other action sequences are retained.
   The historical-style join regression covers native/framed joins, both orders,
   pending text undo/redo, and history/shallow reopen. This bug was exposed while
   checking that N3 migration writes stay outside the user's undo history.

The unused `HasIdSpan` import in `src/encoding.rs` is removed so the pinned path dependency
builds without warnings.
Remove this patch when a verified upstream release provides the same guarantees.
