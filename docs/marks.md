# Authored line positions and formatting marks

Hard breaks are LF characters in a paragraph's CRDT text; tabs are TAB characters.
They have character identity, UTF-8 byte offsets, and the same merge/undo behavior
as typing. Paragraph boundaries remain U+FDD0 flow markers. Plain import splits
at two LFs and retains single LF; HTML `<br>` retains an in-paragraph break.
Native clipboard preserves characters, styles, ranges and remapped relations.

`Style.alignment` is start, centre or end in logical inline coordinates. The
default remains legacy positioning when no alignment is authored. Tabs opt into
logical start alignment, including RTL. Justification is deliberately absent;
the existing Optimal composer remains available for ordinary paragraphs.

`Style.tabs` is a versioned `tabs1:` JSON string with a positive default interval
and up to 256 increasing stops. A stop has an integer distance from inline start,
start/centre/end field alignment and an optional single character leader. A null
position is the final interval edge: an end-aligned null stop fills the gap so
the following field ends at the line end. Fields run to the next tab, forced
break or selected soft break. After explicit stops, the default interval applies.
Stops are relative to the available interval, including runaround fragments.
Vertical writing uses the same inline distances; RTL fields progress from the
logical start and use the paragraph's bidi order. Mixed direction fields retain
Unicode L1/L2 order. Unreachable stops clamp the gap without reversing text and
report `compose.tab-unreachable` Warning.

Paragraphs containing tabs use bounded first-fit composition. Hard breaks retain
the configured composer, including Optimal and AuthoredBreak. Fitting
charges at most 1,000,000 tab visits per composition; exceeding it leaves the
tail unplaced and reports `compose.tab-limit` Error. Empty text and trailing hard
breaks receive empty caret lines, including continuation into another frame.
Tab cells have their expanded width in navigation; breaks have zero width.
Leader rendering uses the tab's font and size, bounded to 256 repetitions per
gap and 8,192 glyphs per fragment. Missing or truncated leaders leave blank
space and report `layout.tab-leader-missing` / `layout.tab-leader-limit` Warning.

## Alignment relations

`reprise.alignment` version 1 is owned by its source paragraph, with exactly one
`line` range/layout target and optionally one `to` range/layout target. It uses
Rebind deletion policy, duplicates fully copied relations, and retains crossing
targets only within the clipboard's namespace rules. Commands create POINT
ranges, staged outside undo history; undo/redo activates the same IDs. Range
anchors follow flow splits and ordinary joins. Deleting an owner deactivates its
owned relations; a deleted target is missing or rebound according to range
identity. Layout publishes Valid, Rebound, Ambiguous and Missing outcomes.

With no `to`, the text parameter `alignment` sets the containing visual line's
start/centre/end default. `SetAlignment.at = null` instead updates paragraph
style. Individual-line defaults are needed for the RALIGN/CALIGN labels in the
design target. A point addresses one visual fragment, recomputed after reflow;
it does not create a saved visual line number. Setting again replaces observed
defaults in the same hard-break-delimited authored line; concurrent defaults
remain independent, with the last relation in ID order winning. Style commands
patch just their property, retaining existing formatting and tab settings.
Property commands store scalar keys `node/alignment` and `node/tabs` in the
versioned flat `marks1` root. Independent first edits merge without competing
to create a nested style container. Reading projects these patches into block
overrides; replacement `SetStyleOverride` clears the observed patches. Named
styles and native clipboard continue to use the ordinary style representation.

With `to`, `edge` chooses the source line's start/end content edge.
`target-edge` is position, gap-start, gap-end, line-start or line-end. Gap targets
require a tab at the anchored position. Source/target ranges resolve through the
shared resolver; singleton layout queries work and multiple matches are ambiguous.

The solver is a declared domain of at most 256 constrained lines. After normal
flow and ordinary relation placement, line defaults apply, then pins translate
existing lines and runs along their source frame's inline axis. Target points
transform through page space, supporting vertical, reflected and rotated frames.
Pins re-evaluate after every reflow; they do not change line breaks or page
allocation. Acyclic chains settle in at most 256 rounds. Cycles and their
dependents retain the complete pre-pin geometry and report
`relation.alignment-cycle` Warning. Oversized domains retain all pre-pin geometry
with `relation.alignment-limit` Warning. Multiple pins on one line choose none,
publish Ambiguous and report `relation.alignment-conflict` Warning. A pin outside
its interval is retained with `relation.alignment-outside` Warning. Invalid
vocabulary/gone gap edges are retained with `relation.alignment-invalid` Warning.
Local authoring is bounded to 4,096 live alignment relations.

Required package bit 11 (`REQUIRED_MARKS`) covers retained breaks/tabs, paragraph
alignment, tab stops and alignment relations, including tombstones and unknown
stored style forms. Older readers refuse the package. No derived positions or
guide geometry enters Loro. No new third-party dependency is needed.

## Query and boundary

`LayoutSnapshot::marks(page)` and `DocumentSession::marks(page)` are read-only.
The facade requires a complete current layout and returns `Payload<MarksPage>`
with its layout token, page and marks. WASM exposes `marks(page)`; the reference
worker accepts `{ kind: "marks", page }` and returns a marks response. All points
are integer page units (1/1024 pt). Offsets are UTF-8 bytes. Each mark identifies
its paragraph and visual line. Kinds are paragraph-end, line-break, gap,
alignment and anchor. Gaps have both edges; alignment marks name the alignment
and a label point above the logical start. Anchors have both guide endpoints
when resolved, target page, relation ID/state and whether the constraint applied.
Missing targets retain a source marker with a null guide endpoint. Unplaced
source lines cannot supply a page position and produce no invented geometry.
Images and repeated header copies do not invent authored paragraph marks.

Marks never enter the content/debug display list, rerun layout, mutate document
state or alter rendering. Caret-like points subdivide ligatures by grapheme count
with integer rounding, and use the following cluster at bidi junctions.
`lines_in` includes an empty trailing caret line when its selected range ends
at a paragraph's trailing hard break; an empty selection remains empty.

Commands: insert-line-break, insert-tab, set-alignment, set-tab-stops, add-anchor
and remove-anchor; the latter uses the existing relation tombstone operation.
Facade types are owned DTOs, with regenerated TypeScript and wasm-bindgen
declarations. HTML retains breaks, literal tabs and paragraph alignment, but
reports custom stops/leaders and line defaults as approximated; relation pins
are reported as dropped. Plain text preserves logical characters and reports
layout/style loss. PDF paints shaped leader text and line positions.

## Poem fixture and fidelity

`fixtures::marks::poem` transcribes the design target, retaining the centred II,
hard-broken stanzas, because/you and fit/they gaps, right-aligned individual lines,
now/that pin, upper end-edge pin, yours pin and final line-end fill. The hostile
suite pins layout and content display snapshots. Typography uses the pinned
Source Serif Pro font and a responsive 850-by-620 pt reference medium.

The picture also aligns pieces of a line before and after a gap independently.
This implementation aligns whole visual line fragments and tab fields; it does
not add independently constrained pre-tab field edges. The fixture therefore
approximates the second stanza's pre-tab right alignment and may overflow where
multiple line constraints cannot share the available interval; diagnostics make
that visible. Pins do not reduce the composing measure or trigger another reflow.
Collaborator labels, selections, toolbar and mark styling remain UI work.
