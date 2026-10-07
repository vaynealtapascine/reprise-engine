# Vertical text

Vertical typography is implemented on main: UAX #50 orientation, upright CJK,
vertical OpenType shaping, downward columns in both directions, short
text-combine-upright runs, editing geometry and SVG/PNG/PDF output.

## Writing modes and compatibility

| Mode | Inline progression | Column progression | Glyph orientation |
| --- | --- | --- | --- |
| horizontal-tb | right | down | horizontal |
| vertical-rl | down | left | authored orientation / UAX #50 |
| vertical-lr | down | right | authored orientation / UAX #50 |
| sideways-lr | up | right | sideways |

Template versions 1 and 2 used `vertical-lr` for the upward mode. They still
read as `sideways-lr` and keep their geometry. New templates using either LR
mode use version 3; other templates retain version 2. An older engine preserves
an unreadable version 3 template and reports its existing template diagnostic.

## Shaping and layout

The inherited `text-orientation` property accepts `mixed` (default), `upright`
and `sideways`. Explicit upright treats characters as strong LTR; mixed orientation uses the pinned Unicode 17 UAX #50 table at
grapheme boundaries. Combining marks stay with their base. Orientation splits
retain font fallback, script and bidi metadata.

Upright runs use HarfRust's top-to-bottom direction and `vert`/`vrt2`. Its pinned
integer OpenType implementation reads vertical metrics/origins or synthesizes
missing metrics. Layout converts downward advances and offsets to logical
inline/block axes using integer font-unit scaling. The Noto Sans JP subset pins
real vmtx/vhea/VORG and vertical alternates for regression tests.

The current frame's writing mode is part of preparation identity. Shaping cache
keys include vertical orientation and combination style. A paragraph continuing
from a horizontal frame into a vertical one prepares for the new writing mode;
table cells cache preparation per destination mode while retaining solved columns;
its percentage-resolution context otherwise retains the paragraph's starting
frame. Reference and stepped layout use the same preparation and composition.

## Short horizontal combinations

`text-combine-upright` accepts `none`, `all`, and `digits 2` through `digits 4`
(`digits` means `digits 2`). Each eligible maximal digit run fits in one em.
Longer digit runs remain sideways. `all` is deliberately bounded to maximal
non-whitespace runs of at most four scalars within one fallback/script item;
longer runs retain normal orientation. No combination crosses a font or script
boundary. In horizontal/sideways frames these properties have no effect.

A combination shapes horizontally and retains original source clusters. Integer
advances total exactly one em; horizontal glyph compression fits its width
without shrinking glyph height. Normal line-break opportunities inside the run
are suppressed. Source offsets remain usable for selection and PDF text.

## Geometry, display and exports

Combined groups are atomic visual caret units: pointer hits and keyboard movement
land before or after the one-em box. Internal source offsets normalize to an edge;
authored ranges and text-edit commands still use the original byte offsets.

Carets and hit testing use logical line coordinates; the frame transform makes
their bars horizontal in a vertical column. Upright glyph groups counter-rotate
inside vertical-rl frames. Vertical-lr uses an exact transposition, compensated
with a glyph reflection for sideways runs and transposition for upright runs.
Authored frame rotation/mirroring remains applied outside that compensation.
SVG, PNG and PDF consume the same display groups. PDF reading paths address the
nested glyph item, including repeated headers as artifacts.

`crates/fixtures/tests/vertical.rs` covers upright CJK, Latin, bounded digits,
both downward modes, legacy storage, stepped/reference parity, mode changes
within paragraphs and tables, explicit upright RTL direction, deterministic rasterization and PDF reading addresses.
Set `REPRISE_VERTICAL_PREVIEW` to write the tested SVG/PNG artifacts.
