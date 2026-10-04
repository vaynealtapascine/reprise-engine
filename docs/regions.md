# Region composition

Decisions 05, 11, 13, 19, 24, 25, 26, 37 and 38 govern these subsystems.
The document stores declarations. Allocation, fragmentation and backtracking
belong to layout; none of their results are saved in the content tree.

## Ownership and phases

`regions.rs` coordinates complete layout passes. `flow.rs` owns the body frame
thread, page creation and frame fills. `floats.rs` owns float placement and
exclusions. `notes.rs` owns note cursors, area allocation and body reservations.
`table.rs` owns column measurement and synchronous row fragmentation. `solver.rs`
only allocates an explicitly declared `SolverDomain`; it never observes the
whole document or participates in line breaking.

Every pass begins from an empty snapshot and the preceding complete region
plan. Body paragraphs use `Bounded<Runaround<Measure>>`. The note reservation
reduces the body's effective depth without changing authored frame geometry.
Table cells use the same shaping and composition entry points as paragraphs.
After flow, region relations are resolved against body lines; newly composed
notes and floats become available to subsequent dependent region relations.
Finally all relations are reported against the complete snapshot. References
can therefore query lines inside notes, including continued notes.

Snapshots keep their frozen shape: a note is one `BlockLayout` with contiguous
source-byte lines across several `Notes` frames. A float's lines sit in its
body frame; its exclusion is derived pass state. Table container nodes have no
visual lines; cell text blocks retain their own identities, styles and ranges.
No new snapshot field is required.

## Floats

An annotation owned by `reprise.float` has one `anchor` range or a layout query
resolving to one body line. Its side is `left`, `right`, `top` or `bottom`.
Left/right default to one third of the candidate frame; top/bottom span its
width. The optional width and margin use authored `LengthExpr` values.
Malformed sides omit the owner and report `layout.region-parameter` (Error).

Candidates follow the main frame thread. Placement starts at the anchor's line
top for side floats, at the top for a top float, and at the bottom for a bottom
float. Floats stack in relation-ID order, using a conservative shared vertical
track across sides. Padding is represented directly in exclusion rectangles;
`Runaround` subtracts them for the entire line band, not just its baseline.

A candidate that does not fit advances to the next frame or page. At most two
template cycles plus one candidate are tried: enough to test the occupied
anchor page and a fresh repetition of every frame. Deferral is a Warning
(`layout.float-deferred`); impossibility omits the owner with an Error
(`layout.float-unplaceable`). Float blocks do not fragment.

## Notes

An annotation owned by `reprise.note` anchors to a persistent range. `page`
placement is a footnote; `end` starts after the last page containing body lines.
A template declares a `FrameRole::Notes` frame: its full extent is the maximum
notes area, not permanently reserved blank space. The first usable notes frame
on each page is selected. An overlapping full-height area permits whole pages
to be occupied by notes. A separate area below the body causes no reservation
when it does not intersect the body.

Notes stack in dependency order, with relation-ID order breaking ties. Each
note shapes once for that pass, then composes from a byte cursor into successive
notes frames. Finished bytes are never repeated. Notes are aligned together
at the bottom of each area's used extent. Floats allocate first, so notes keep
room above themselves for intersecting float boxes and their margins. A projected bounding box of that
extent reserves the intersected bottom depth of each body frame. Projection is
conservative for rotated frames and uses only integer transforms.

The first page keeps room for the anchor's body line. The first fragment uses
only the area below that line; a last-line anchor can therefore put all of its
note on the following page. Continuation pages may use the entire notes area.
This is the convergence rule that prevents a full-page note from repeatedly
moving its own anchor to a later page. Continuation is requested behavior and
reported as `layout.note-continued` (Info). A line taller than a whole notes
frame is placed overflowing on a continuation page, with
`layout.frame-overflow` (Warning). Reaching the page limit omits remaining bytes
with `layout.text-unplaced` and `layout.page-limit` (Error).

Notes on notes are ordinary note relations. Their anchors resolve to the parent
note's lines. No recursive call stack is used. Dependency rounds handle reverse
relation order as well as forward order; nesting depth is checked separately.
An unresolved dependency, relation cycle or excessive depth omits the dependent
owner with `layout.note-depth` (Error); final target resolution supplies the
specific missing-target diagnostics too.

## Tables and solver domains

`Document::append_table`, `append_table_row`, `append_table_cell` and
`append_cell_block` create a table -> rows -> cells -> ordinary blocks in the
movable tree. A versioned `table1` metadata entry declares container roles.
Unsupported metadata remains stored and is reported, never rewritten by layout.
Each cell declares its column index; duplicate or out-of-range indices and
misplaced containers are omitted with `layout.table-invalid` (Error).

Column declarations are fixed widths, proportional weights or content widths.
Min content is the widest shaped unbreakable segment; max content is the widest
forced-line segment. Layout declares a width domain with the current frame's
budget. Fixed columns have equal lower/upper bounds; proportional columns have
zero minimum and weighted surplus; content columns have measured closed bounds.

The solver uses bounded water filling with half-away-from-zero division and
stable residual subunit allocation. If bounds are infeasible, they are relaxed
to equal columns that exactly fit the nonnegative budget
(`layout.solver-infeasible`, Warning). No declared share causes equal sharing;
surplus beyond all upper bounds remains unused
(`layout.solver-underconstrained`, Warning). The public solver is intentionally
a width-allocation solver, not a general linear-programming API.

Rows compose cells in parallel from separate block/byte cursors. A row fragment
ends at the deepest cell fragment; unfinished cells continue together in the
next body frame. Finished cells stay blank in later fragments. Cell lines retain
their source block identity and inline column offset. A taller-than-any-frame
line uses the established overflow fallback. Column widths are frozen for the
table after its starting frame, like the starting-frame style rule for a
paragraph. A narrower continuation frame is reported with `layout.frame-overflow`.
Rectangular tables start below float exclusions. Repeated header rows, spanning cells, borders and nested tables are
follow-ups; header authorship is retained but not repeated.

## Bounded cycles and limits

| Class | Limit | Convergence / fallback |
| --- | --- | --- |
| Body -> float anchor -> runaround -> body | 16 complete passes | Identical exclusions, placements and allocations converge. A previously seen complete plan is oscillation. Freeze the input plan used to compose the published body; report `layout.region-cycle` (Warning). |
| Body -> note anchor -> notes depth -> body | Same 16-pass coordinator | Preserve the first-page anchor line; compare complete plans. On exhaustion freeze the last input plan and report `layout.region-limit` (Warning). |
| Note -> dependent note | 32 dependency rounds and 32 actual levels | Resolve available anchors in stable order; no progress or depth exhaustion omits owners with `layout.note-depth` (Error). |
| Domain water filling | 256 iterations, 256 variables | An incomplete allocation keeps unused width (`layout.solver-limit`, Warning). Extra variables are omitted (`layout.solver-limit`, Error). |
| Region relation fanout | 4096 relations | Later region owners omitted with `layout.region-limit` (Error). |
| Table fanout | 4096 rows, 256 columns, 65536 content blocks | Excess rows/blocks omitted with `layout.table-limit` (Error); invalid column count omits the table with `layout.table-invalid` (Error). |
| Pagination | Existing `FlowSettings::max_pages` (0 means 1) | Unplaced bytes reported; no hunting for another page after the limit. |

Backtracking is a complete reflow, bounded by the coordinator. Local text
breaking stays in composers. Table and note cursors advance monotonically inside
a pass. On feedback failure, placements may differ from their final resolved
anchors, explicitly reported by the coordinator; geometry and body reservations
still correspond to the same input plan. There is no cache of provisional output
presented as converged output.
