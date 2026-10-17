# Loro 1.16.2 bounded root discovery

Source: crates.io `loro-internal` 1.16.2 (MIT, as in Cargo.toml).
Behavioral source change: `src/state.rs::root_container_is_empty` uses TreeState::is_empty
(a root-children count) for trees rather than recursively constructing get_value.
This makes both LoroDoc::get_value's root catalog and a one-component
get_by_str_path lookup independent of tree depth. No stored format changes.
Remove this patch when a verified upstream release provides the same guarantee.

The unused `HasIdSpan` import in `src/encoding.rs` is removed so the pinned path dependency builds without warnings.
