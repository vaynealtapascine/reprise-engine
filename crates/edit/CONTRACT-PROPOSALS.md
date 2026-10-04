# Paragraph base level integration (22, 30)

The proposal was accepted and implemented on main in `1183221`, after workstream
3b merged. Workstream 7 imports that contract; it does not edit layout or flow.

The approved field in `reprise_layout::BlockLayout` is:

```rust
/// Resolved UAX #9 paragraph base level (0 or 1), from paragraph itemization.
#[serde(skip_serializing_if = "is_zero")]
pub base_level: u8,
```

The serialization helper is:

```rust
fn is_zero(level: &u8) -> bool { *level == 0 }
```

The approved Layout snapshot contract says:

> Each `BlockLayout` records its paragraph's resolved bidi `base_level` (0 left to
> right, 1 right to left; left out of JSON when 0). Carets, visual movement and
> alignment use it instead of re-deriving it.

`BlockInfo` now uses this field. The former ICU4X first-strong inference and
the editing crate's direct `icu_properties` dependency have been removed.
This keeps navigation aligned with the direction actually used by shaping,
including a base level that differs from the source's first strong character.

Layout/flow populates the field from shaping. The orchestrator's commit adds
the field to three RTL layout goldens; zero-valued/LTR serialization is unchanged.

There is no outstanding snapshot mapping proposal. A point
cannot distinguish coincident carets; the contract therefore compares geometry
and requires exact normalized identity only for unique visual positions.
