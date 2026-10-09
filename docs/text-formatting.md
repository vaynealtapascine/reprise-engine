# Anchored text formatting

Formatting is authored data attached to persistent ranges, separate from paragraph
styles. A range's `format1` envelope contains version 1, an ordering number, and a
text-style patch. The range uses the existing anchor/policy storage, so ordinary
flow splits and joins retain formatting without copying text.

The first implementation supports font-family chains, absolute font size,
language, and OpenType feature settings. Paragraph geometry and writing mode stay
paragraph properties. Weight/slant selection, decoration and paint are separate
extensions; unsupported properties must not be accepted and silently ignored.

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
