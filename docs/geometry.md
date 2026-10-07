# Geometry and reading order hand-off

Authored `FrameTemplate` geometry is saved in envelope v2, with v1 read
compatibility and unknown-field rejection. Its width and height are physical
page dimensions. Horizontal composition uses those dimensions; vertical
composition swaps them. Writing mode maps logical axes first, then physical
mirrors and rotation act about the authored physical origin, then placement
at `(x,y)` maps the frame to its page. Quarter turns and mirrors are exact.
`Rotation::Direction` normalises integer direction components with exact
rounding to 16.16; `Rotation::Matrix` stores explicit fixed rational coefficients.
A noninvertible or numerically unusable linear map falls back to identity with
`layout.transform-unusable` (Warning), retaining the text.

Latin vertical-rl reads down with columns to the left. Vertical-lr deliberately
uses sideways-lr: inline goes up and columns go right, preserving glyph
handedness under a single frame rotation. Downward vertical-lr and upright CJK
are follow-ups. Add vertical direction and `vert`/`vrt2` shaping in
`shape/src/paragraph.rs` and the shaping request/adapter, then record glyph
orientation in layout so editing and display agree. The current bundled font
is Latin-only; Hebrew in the RTL fixture displays .notdef glyphs but retains
its source bytes for extraction.

`Spiral` describes an Archimedean spiral in parent-local physical coordinates;
`(x,y)` is its centre. Integer millidegree sampling yields endpoint chords.
Each chord becomes an ordinary tangent frame, with chord length as its inline
measure and authored height as depth. For one text line per strip, choose depth
between one and two used line heights. Larger depths intentionally allow a
strip to contain several lines. Negative sweeps reverse traversal; signed growth
changes radius by that length per turn of either traversal direction. Invalid
radii/coincident samples are reported, and entirely invalid paths fall back to
an ordinary frame. Expansion caps at 1024 strips; excess geometry is reported
and omitted, while remaining text threads onto later pages. Text changes reflow
through the same strips. Many short strips approximate a curve; glyphs remain
straight within each strip. Tight radii/large glyphs can overlap, just as ordinary
frames can; automatic collision avoidance is outside this workstream.

For editing, a line caret is a `Point<LineSpace>`. `line_to_page` composes its
baseline translation with its frame map, and the inverse maps a page hit back
to that same logical line. `line_bounds` is the axis-aligned envelope of all four
transformed box corners. Path strips have independent frame indices and maps,
so selection may visit many frames while its byte ranges stay contiguous. There
are no hidden per-glyph path transforms. `follow` already maps through the
receiving frame inverse; a test covers rotated source and margin frames.

`reprise.reading-order` supplies whole-block `before`/`after` constraints. Default
reading is document-tree preorder, then logical line order within each block.
`reading_order(&Document)` takes the same-revision document explicitly because
the frozen snapshot stores placement order, not semantic ranks. Mismatched
revisions report and fall back to placement order. Stable topological sorting
completes partial orders semantically (Info). Opposite constraints (Warning),
actual cycles (Warning), and missing/unplaced endpoints (Warning, in addition
to shared relation-resolution diagnostics) are reported. Cycles break at the
first semantic member of a discovered cycle; blocked downstream nodes are not
mistaken for cycle members. At most 4096 distinct edges are honoured; excess
constraints report a Warning. Iterative traversal avoids recursion, and every
iteration removes one node, yielding a total deterministic order.

Use `to_display_lists(DisplayOptions::default())`, then `pdf_reading_order(doc)`
and `pdf::render_ordered(lists, fonts, order)`. Run addresses index the original
page/group items; every glyph item must be named exactly once. Runs sort by source
byte start even where visual bidi order differs. Ordered PDF keeps transforms
and clips, paints nontext first, and wraps each run in an ActualText span. This
changes painting order where content overlaps. The output is a tagged PDF aiming at
PDF/UA-1: `pdf::render_tagged` takes the structure (`pdf::tags`) built from this
reading order, and `render_ordered` writes a flat one. Physical pages are not
reordered, but the structure may name a later page first, so a cross-page reading
override is carried by the tree. Existing `pdf::render` remains unchanged.
The CLI still uses the frozen renderer unless a caller selects ordered export.

`tests/geometry.rs` includes a manually invoked SVG/PNG export for all six new
hostile fixtures. Run it with `cargo test -p reprise-fixtures --test geometry
export_geometry_previews -- --ignored --nocapture`. Files go to the OS temporary
`reprise-ws5-geometry` directory. Both the source SVGs and their actual browser
rendering were inspected. Existing snapshots remain unchanged; each new fixture
adds its layout and content-only display golden.
