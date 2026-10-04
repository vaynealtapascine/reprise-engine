# Paragraph base level proposal (30)

This is a proposal only. Workstream 7 does not modify the frozen snapshot or
workstream 3b's `flow.rs`. The orchestrator will add it after that workstream merges.

Proposed addition to `reprise_layout::BlockLayout` in `snapshot.rs`:

```rust
/// Resolved UAX #9 paragraph base level (0 or 1), from paragraph itemization.
#[serde(skip_serializing_if = "zero_base_level")]
pub base_level: u8,
```

The serialization helper would be:

```rust
fn zero_base_level(level: &u8) -> bool { *level == 0 }
```

Proposed wording in the Layout snapshot contract:

> Every block records its resolved paragraph `base_level`. Navigation uses it
> to choose the reading direction when crossing visual line boundaries and the
> inline edge of an empty line. Level 0 is omitted from JSON; level 1 is emitted.
> Run levels remain the resolved levels used for visual ordering within lines.

Today `BlockInfo` infers this using ICU4X Bidi_Class, the first strong character
outside isolates (P2/P3). This works for implicit paragraph direction. It cannot
know a future explicitly authored paragraph direction or a layout direction
override; the snapshot must expose the direction actually used by shaping.

Affected owners: layout/flow (populate the field), shaping (supply resolved base
level), editing (replace inference), and fixtures (update RTL JSON snapshots with
an explained new field). Existing LTR JSON should be byte-for-byte unchanged.

No other snapshot mapping is needed for the current acceptance tests. A point
cannot distinguish coincident carets; the contract therefore compares geometry
and requires exact normalized identity only for unique visual positions.
