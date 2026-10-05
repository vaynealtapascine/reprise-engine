# Vertical text

How reprise-engine sets real vertical typography: upright CJK, UAX #50 glyph
orientation, vertical shaping, downward `vertical-lr`, and tate-chū-yoko
(decisions 20, 21, 22, 23, 30, 33 and 38). [geometry.md](geometry.md) covers frame
transforms; this document covers what happens to glyphs inside vertical frames.

## Principles

-   **Composition stays one-dimensional.** Composers, geometry providers, carets
    and hit testing keep working on a frame's logical axes: inline along x, block
    along y (20). A vertical line is an ordinary line whose frame transform turns
    it. Nothing in `compose` knows the page orientation.
-   **Glyph orientation is a fact of the layout, not of the display.** Shaping
    decides which runs are upright, records it on every run, and layout,
    editing and display read the same record. No backend recomputes it.
-   **Every frame decides for itself.** A paragraph that threads from a
    horizontal frame into a vertical one is set sideways in the first and upright
    where UAX #50 says so in the second. Orientation never depends on a frame
    the line is not in.
-   **Integer arithmetic throughout (19, 38).** Vertical advances, origins,
    synthesised metrics, central baselines and tate-chū-yoko compression are
    `Length` arithmetic on font design units, rounding half away from zero and
    saturating. All glyph transforms are exact quarter turns or reflections.
-   **Everything still mirrors, rotates and spirals.** A vertical frame may carry
    any authored rotation or mirror. Glyph orientation is relative to the line,
    so an upright column inside a rotated frame rotates with it, and an authored
    mirror still mirrors its glyphs.

## Writing modes

`WritingMode` (in `reprise-doc::page`) has four values. Each maps a frame's
logical axes onto its physical, unrotated box.

| Mode | Inline | Block (next line) | Frame map | Line-over side | Glyphs |
| --- | --- | --- | --- | --- | --- |
| `horizontal-tb` | right | down | identity | block-start | upright (horizontal) |
| `vertical-rl` | down | left | quarter turn clockwise | block-start (right) | UAX #50 |
| `vertical-lr` | down | right | transposition (x↔y) | **block-end** (right) | UAX #50 |
| `sideways-lr` | up | right | quarter turn anticlockwise | block-start (left) | all sideways |

`vertical-lr` is new and follows CSS: text runs top to bottom and columns
progress left to right. Its frame map is a reflection (determinant −1). The
previous `vertical-lr`, which ran upwards, is now called `sideways-lr`, as in
CSS. It is still available and lays out exactly as before.

### Storage and compatibility (34)

Templates are stored in a versioned JSON envelope. Version 2 wrote the upward
mode as `"vertical-lr"`. This engine:

-   reads `"vertical-lr"` in a version 1 or 2 envelope as `sideways-lr`, so a
    stored document lays out exactly as it did;
-   writes version 3 only when a template uses `vertical-lr` or `sideways-lr`,
    and version 2 otherwise, so a template without them stays readable by older
    engines;
-   reads `"vertical-lr"` in a version 3 envelope as the downward mode.

An older engine reports a version 3 template as unreadable
(`layout.template-unreadable`) and keeps it verbatim. That is the right outcome,
because it cannot honour the mode.

### Line-over and the reflected frame

Glyphs are drawn in line space and then through the frame map. In
`vertical-lr`, that map is a reflection, so drawing glyphs unchanged would
mirror them. Layout therefore records each frame's `writing_mode` on
`FrameLayout`, and every glyph run in a `vertical-lr` frame is drawn through a
compensating reflection. Only the writing mode's own reflection is compensated:
an authored `mirror_x` or `mirror_y` still mirrors the text, as intended (20).

CSS puts the line-over side on the right in both vertical modes. In
`vertical-lr`, that is the block-end side, so layout places the alphabetic
baseline `descent` below the line's top (plus half-leading), not `ascent`. The
ascenders of sideways Latin then point right in both modes.

## Authored properties (08)

Two keyword properties join the style system. Both are inherited through the
named-style chain, overridden directly like every other property, explained in
`ComputedStyle.explain`, and left out of JSON when no layer sets them.

| Property | Values | Default |
| --- | --- | --- |
| `text-orientation` | `mixed`, `upright`, `sideways` | `mixed` |
| `text-combine-upright` | `none`, `all`, `digits`, `digits 2`, `digits 3`, `digits 4` | `none` |

They are stored as plain strings under those names in the style's Loro map. A
stored value this engine can't read (a newer keyword, `digits 9`, damage) is
kept verbatim, written back unchanged, reported as `style.unparsed` (Warning),
and ignored, so the property keeps the value it inherited (34, 37).

Both properties only affect `vertical-rl` and `vertical-lr` frames. In a
horizontal frame they have no effect and nothing is reported, as in CSS. A
`sideways-lr` frame always sets every glyph sideways; if the style asks for
`upright` or for a combination, layout reports `layout.orientation-ignored`
(Warning) once per block.

## Orientation (UAX #50)

`reprise-text::orientation` holds a generated range table of the Unicode
`Vertical_Orientation` property, `U`, `R`, `Tu` and `Tr`, for Unicode 17.0.0,
the version ICU4X 2.3 pins elsewhere in the engine. It is generated from
`VerticalOrientation-17.0.0.txt` by `crates/text/src/generate_orientation.py`,
and a test checks every code point against ICU4X's compiled data, so the two
can't drift apart. Lookup is a binary search over 181 merged ranges.

Orientation is decided per grapheme cluster, from its first scalar, so a
combining mark never gets a different orientation from its base.

| `text-orientation` | `U`, `Tu` | `Tr` | `R` |
| --- | --- | --- | --- |
| `mixed` | upright | upright if the face has a vertical alternate, else sideways | sideways |
| `upright` | upright | upright | upright |
| `sideways` | sideways | sideways | sideways |

-   `Tu` characters are upright, and get their `vert` alternate when the face has
    one.
-   `Tr` characters (brackets, the long-vowel mark ー, dashes) depend on the
    face. Layout shapes the grapheme vertically and horizontally with the
    configured adapter. If the glyphs differ, the face has a vertical
    alternate and the grapheme is upright. Otherwise it is rotated, which is
    UAX #50's fallback. This uses only the pinned face and adapter, so it is
    deterministic (38).
-   With `upright`, every character is treated as strong left to right for bidi,
    as CSS requires: the paragraph's levels are all set to 0 in that frame.
    With `mixed`, bidi runs as usual. Right-to-left runs are sideways (Hebrew and
    Arabic are `R`), so a rotated RTL run reads bottom to top inside the column.
    An upright run at an odd level, such as CJK punctuation inside a
    right-to-left embedding, has its glyphs reversed like any right-to-left run.

Items split wherever orientation changes. `Item.orientation` and
`ShapedRun.orientation` record the result (`sideways`, `upright` or
`combined`). Both default to `sideways`, the old behaviour, and are left out of
serialised output then.

## Vertical shaping (22)

`ShapingAdapter` gains `shape_vertical`, a provided method, so existing adapters
still compile. It shapes a run top to bottom and returns glyphs in logical
order, with:

-   `advance`: the inline advance, down the column (positive);
-   `x_offset`: the glyph origin's offset along the inline axis;
-   `y_offset`: the glyph origin's offset towards line-over, relative to the
    run's central baseline.

The glyph origin is the glyph's horizontal origin, drawn rotated a quarter turn
anticlockwise relative to the line, so that its top points to the line's start.

The provided implementation synthesises vertical positions from horizontal
shaping, so any adapter can set upright text, but without vertical
substitutions. `HarfRust` overrides it: it shapes with HarfBuzz's
`TopToBottom` direction, which applies `vert` and the font's vertical GPOS
features (`vkrn`, `vpal` when requested), and takes metrics from the callbacks
below. `vrt2` is not applied. It is meant for applications that do not rotate
sideways glyphs themselves, and this engine does rotate them, so applying it
would turn Latin twice. That follows HarfBuzz.

### Metrics

`reprise-shape::vertical::VerticalMetrics` reads `vhea`, `vmtx` and `VORG`. All
values are in font units, which the adapter scales with
`Length::from_font_units`. It is a harfrust `FontFuncs` implementation, so
HarfBuzz uses exactly these numbers.

-   **Advance:** `vmtx` when the face has one. Otherwise it is synthesised as
    one em, `units_per_em`: the ideographic em box.
-   **Origin x:** half the glyph's horizontal advance, rounded half away from
    zero. The column's centre line runs through every glyph's centre.
-   **Origin y:**
    1.  `VORG` when the face has one: the glyph's entry, or its default.
    2.  Otherwise, with `vmtx`: the glyph's top side bearing plus its bounding
        box top (`yMax + tsb`), as OpenType defines.
    3.  Otherwise it is synthesised as the top of an em box centred on the face's
        ascent and descent: `(ascent − descent + units_per_em) / 2`, rounded
        half away from zero. Here `descent` is the positive distance below the
        baseline.

A face without `vmtx` is reported once per block with `shape.vertical-metrics`
(Info): the text is upright as asked, on synthesised metrics.

### Central baseline

Upright glyphs sit on the line's central baseline, at the middle of the line
box: `top + height / 2`. Sideways glyphs keep the alphabetic baseline, with
half-leading as in horizontal lines, so a line of only sideways text is
unchanged. Layout turns each upright glyph's `y_offset` into an offset from the
line's alphabetic baseline, as for every other glyph. `LineLayout.baseline`
keeps meaning the alphabetic baseline.

## Tate-chū-yoko (`text-combine-upright`)

A combination is set horizontally and stands upright in the column, taking one
em of the inline axis.

-   `digits N` (N from 2 to 4, `digits` alone means 2) combines every maximal run
    of ASCII digits `0`–`9` of at most N characters. Longer runs are left as they
    are.
-   `all` combines each maximal run of non-whitespace graphemes, because styles
    apply to whole blocks and a combination of a whole paragraph would be
    useless. A run of more than 16 graphemes is not combined; it is set upright
    and reported with `shape.combine-limit` (Warning). This is a narrowing of
    CSS, where `all` applies to an inline element.

Compression is integer arithmetic. The run is shaped horizontally at the style
size `S` and its width `W` is measured. If `W > S`, it is shaped again at
`S′ = ⌊S · S / W⌋`, which shrinks it uniformly, as the CSS fallback allows.
Glyphs are not stretched, so outlines stay undistorted. The composition is
centred on the central baseline, and its horizontal em box (ascent plus descent
at `S′`) is centred in the em along the inline axis. The run's inline advances
are split exactly into `S` across its glyphs, `S·(i+1)/n − S·i/n`, so carets
step across the combination in proportion and `LineLayout.width` is exact.
`ShapedRun.size` is `S′`, the size the glyphs are drawn at.

## Lines (23)

-   **Breaking:** UAX #14 already breaks between ideographs and keeps closing
    punctuation (CL, CP), small kana (CJ, strict rules aside) and full stops off
    the start of a line. Combinations never break inside, because digit
    sequences have no break opportunities.
-   **Justification:** `Optimal::justified()` stretches word spaces. A justified
    line with no word space, but with an upright or combined run, distributes
    the gap as letter spacing after each grapheme instead (inter-character
    justification, as CJK needs). Lines with word spaces behave as before.
-   **Bidi:** lines are reordered with rules L1 and L2 as in horizontal text.
    Orientation is per run, so reordering keeps it.

## Editing (30)

Carets are zero-width rectangles across the line box at an inline position,
mapped through the frame transform. In a vertical frame they are horizontal
bars, whether the glyphs are sideways or upright, so upright runs need no
special case. Their inline positions come from the vertical advances. Hit
testing inverts the same transform, and visual movement maps page arrows
through it: Down moves forward in `vertical-rl` and `vertical-lr`, and Left and
Right move between lines. A combination is one cluster per grapheme, with
proportional caret stops across its em.

## Display (32, 33)

A run is drawn through a matrix `G` from glyph space into line space, chosen by
orientation and frame. Each `G` is an exact quarter turn or reflection.

| Run | Not reflected (`vertical-rl`, …) | `vertical-lr` |
| --- | --- | --- |
| sideways | identity (no group) | `mirror_y` |
| upright, combined | quarter turn anticlockwise | transposition |

A run whose `G` is not the identity becomes `Group { transform: G about the
line's baseline origin }` holding its `GlyphRun`. Inside the group, a sideways
glyph sits at `(pen + x_offset, −y_offset)` and an upright one at
`(y_offset, pen + x_offset)`. SVG, PNG and PDF already draw nested groups, so
backends need no new code.

Every run still carries its source text and per-glyph byte ranges. Ordered PDF
run addresses point into the group, so upright text extracts in logical order
with ToUnicode and ActualText, as horizontal text does.

## Diagnostics

| Code | Severity | When |
| --- | --- | --- |
| `shape.vertical-metrics` | Info | an upright run's face has no `vmtx`; metrics were synthesised |
| `shape.combine-limit` | Warning | a `text-combine-upright: all` run was longer than 16 graphemes and was set upright instead |
| `layout.orientation-ignored` | Warning | `upright` or a combination was asked for in a `sideways-lr` frame |

## Known gaps

-   **Table cells** in vertical frames are still set sideways: their composition
    lives in `layout/src/table.rs`, which this workstream does not own.
-   **Inline styles** do not exist yet, so `text-orientation` and
    `text-combine-upright` apply to whole blocks.
-   **`vrt2`, `hwid`/`twid`/`qwid`** are not applied. Combinations fit by uniform
    scaling.
-   **Spiral strips** remain horizontal (geometry.md).
-   **Ruby, emphasis marks and `text-orientation: sideways-right`** are out of
    scope.
