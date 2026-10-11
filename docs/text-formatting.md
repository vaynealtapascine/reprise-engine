# Anchored text formatting

Formatting is authored data attached to persistent ranges, separate from paragraph
styles. A range's `format1` envelope contains version 1, an ordering number, and a
text-style patch. The range uses the existing anchor/policy storage, so ordinary
flow splits and joins retain formatting without copying text.

Supported fields are font-family chains, absolute size, language, OpenType
features, weight (1–1000), slant (normal/italic/oblique), independent underline
and strike patches, and RGBA colour (four bytes). Paragraph styles also provide
weight, slant, decoration and colour defaults. A missing decoration member
inherits; false explicitly removes just that line. Reset reveals paragraph
properties. Alpha zero preserves text and geometry for selection/accessibility.
Paragraph geometry and writing mode stay paragraph properties.

The envelope remains format1 with deny_unknown_fields, including decoration.
Packages using emphasis set REQUIRED_TEXT_EMPHASIS (required bit 10) as well as
bit 9 when anchored formatting is retained. Bit 9 alone is insufficient: its
older reader diagnoses an unknown envelope but continues layout without it.
Named styles, break overrides, tombstones and unreadable formatting conservatively
retain bit 10. Legacy styling keeps its stored/output form.

Descriptor-aware itemisation uses CSS stretch/style/weight order and pinned
identity ties within each family; unavailable descriptors report font.nearest
Warning. Generics search registered variants of the configured default family
alongside its pinned regular face. Legacy entry points remain unchanged. No
synthetic bold or variable-axis instancing is performed. Hosts may opt into
synthetic oblique using `FontSettings.synthetic_oblique`: when the family has
no slanted face, italic/oblique uses the upright glyph with `x += y/4` in y-up
design space. This fixed quarter shear leaves advances and reading mappings
intact, reports `font.synthetic` Info and propagates through reshaping, vertical
compensation, transformed frames, glyph outlines and every backend. Integer
ink envelopes include the shear. Hosts drawing display JSON request
`glyph_outlines_styled` with the run's `synthetic_oblique` flag (absent means
false); outline caches must include the flag alongside face/glyph ID.

Host generic family mappings and ordered universal fallback aliases live in
`FontSettings`, outside document state. Configuring either enables the existing
whole-grapheme coverage path even for a single-family style. The author's chain
is tried first, then host fallback families, then the built-in terminal class.
An authored generic is itself a preferred family. Atkinson Hyperlegible Next
is a host choice, not an assertion that every Unicode scalar has a glyph.
All substitutions/missing glyphs remain diagnosed. Unconfigured defaults keep
the original matching and snapshot representation.

Decorations are filled content Path rectangles over each visual run's inline
extent, including spaces. Integer post underline and OS/2 strike metrics scale
by the selected face and size; missing positions use -0.1/0.3 em and missing or
nonpositive thickness uses 0.05 em (minimum one layout unit after scaling).
Zero-size/zero-width runs draw no line. Vertical underlines sit on the physical
right; upright and combined glyph compensation does not rotate decoration paths.
PDF reading addresses account for paths between glyph items, including repeated
headers. Glyphs and paths share colour, including zero alpha.

HTML emits font-weight, font-style, text-decoration and exact #RRGGBBAA colour
on effective spans. Import accepts b/strong, i/em, u/s and the bounded CSS
subset: absolute numeric weights plus normal/bold, keyword slants, underline/
line-through/none, hex colours, comma rgb/rgba (integer RGB and alpha with at
most six decimal places), transparent and black/white/red/green/blue. Unsupported
relative weights, slant angles, decoration variants and colour syntax are
reported with clipboard.html-approximated. Native fragments retain patches.

Each formatting action adds an independent range. Its order is one greater than
the largest readable formatting order observed in the document. Overlaps apply
in `(order, range ID)` order, field by field. Concurrent actions therefore keep
independent properties and resolve competing properties deterministically. A
reset patch clears earlier text overrides before applying its own fields.
Resetting reveals the paragraph style, including subsequent paragraph-style edits.

Formatting actions use staged ranges and whole editing steps: undo/redo toggles
the range's deletion flag and retains its ID. Removing an action is different
from applying a reset: removing reveals earlier actions; resetting masks them.

Resolution projects both endpoints into the current paragraph, including ranges
spanning several paragraphs of one flow. Each grapheme uses the style of its
first scalar; derived style boundaries round forward to grapheme boundaries.
Stored anchors are not rewritten. Adjacent identical resolved runs coalesce.

Read-time limits and malformed/future envelopes produce diagnostics without
repairing replicated data. Local authoring validates properties and endpoints
before staging. At most 4,096 formatting records per host participate in reads;
local authoring refuses to exceed that limit. The ordering number is bounded to
JavaScript's exact integer range. Envelopes are bounded before JSON parsing.

Layout must include resolved formatting and its diagnostics in paragraph cache
keys. Shaping still receives whole-paragraph context and paragraph-local byte
offsets. Native copy/paste must preserve effective formatting on every selected
paragraph; plain export must report its loss, HTML must express supported styles,
and PDF must use the resulting shaped runs. Package feature declarations must
prevent an older reader silently treating formatted text as unformatted.

Implementation: `reprise-doc/src/formatting.rs` (storage, per-revision index,
resolution), the `FormatText` kernel command, layout style runs and cache keys,
native fragments, HTML `<span>` export/import (`clipboard/src/text_css.rs`), the
`REQUIRED_TEXT_FORMATTING` package bit, and the facade's `FormatText` command and
`State.blocks[].formatting`. Plain text reports formatting as dropped. Mixed
sizes enlarge the paragraph-wide strut to 1.2× the largest inline size; per-line
metrics are a later composer change.

## Clearing one field

A formatting action may list fields in `clear` (`families`, `size`,
`language`, `features`, `weight`, `slant`, `underline`, `strike`, `color`).
It masks earlier actions' values for exactly those fields, revealing the
paragraph style, and leaves every other field to earlier actions: a per-field
`reset`. A field may not be both set and cleared. The envelope stays format1;
`clear` is absent when empty, so other envelopes are byte-identical, and a
retained clearing action declares required package bit 13 (older format1
readers would reject the envelope and drop the action). The facade spells it
`TextStyle.clear: StyleProperty[]` (`docs/paragraph-styles.md`).
