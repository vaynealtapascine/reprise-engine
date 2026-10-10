//! Diagnostic codes reported by layout itself. Fonts, shaping and
//! composition report their own (`font.*`, `shape.*`, `compose.*`).

use reprise_diag::Code;

/// Merged or unreadable page properties could not be applied.
pub const PAGE_SETUP_INVALID: Code = Code::new("layout.page-setup-invalid");

pub const IMAGE_RECORD: Code = Code::new("layout.image-record");
pub const IMAGE_MISSING: Code = Code::new("layout.image-missing");
pub const IMAGE_HEADER: Code = Code::new("layout.image-header");
pub const IMAGE_LIMIT: Code = Code::new("layout.image-limit");
pub const IMAGE_SIZE: Code = Code::new("layout.image-size");

/// A block couldn't be read from the document.
pub const MALFORMED_BLOCK: Code = Code::new("layout.malformed-block");
/// A block's style couldn't be resolved.
pub const STYLE: Code = Code::new("layout.style");
/// A style property resolved to a negative length and was clamped to zero.
pub const STYLE_CLAMPED: Code = Code::new("layout.style-clamped");
/// Composition stopped before the end of a block's text.
pub const TEXT_UNPLACED: Code = Code::new("layout.text-unplaced");
/// A block runs past the bottom of its frame.
pub const FRAME_OVERFLOW: Code = Code::new("layout.frame-overflow");
/// A block that only a relation can place had no relation placing it.
pub const UNPLACED: Code = Code::new("layout.unplaced");
/// A relation couldn't be read from the document; it is kept there.
pub const RELATION_UNREADABLE: Code = Code::new("relation.unreadable");
/// A relation's schema isn't registered with this engine.
pub const RELATION_UNKNOWN_SCHEMA: Code = Code::new("relation.unknown-schema");
/// A relation is registered but layout has no behaviour for it.
pub const RELATION_NOT_APPLIED: Code = Code::new("relation.not-applied");
/// A relation's target is gone.
pub const RELATION_MISSING_TARGET: Code = Code::new("relation.missing-target");
/// A relation's target was rebound after part of it was deleted.
pub const RELATION_REBOUND: Code = Code::new("relation.rebound");
/// A relation's targets don't match its schema.
pub const RELATION_BAD_TARGET: Code = Code::new("relation.bad-target");
/// A relation's owner isn't a block the relation can place.
pub const RELATION_OWNER: Code = Code::new("relation.owner-not-placeable");
/// An owned relation's owner was deleted, so the relation went with it (07, 14).
pub const RELATION_OWNER_DELETED: Code = Code::new("relation.owner-deleted");
/// A layout query matched nothing.
pub const RELATION_NO_MATCH: Code = Code::new("relation.no-match");
/// A placed block was moved to avoid overlapping an earlier one.
pub const RELATION_PUSHED: Code = Code::new("relation.pushed");
/// A role that takes one target resolved to several equally good candidates,
/// and layout didn't choose (15).
pub const RELATION_AMBIGUOUS: Code = Code::new("relation.ambiguous");
/// A target was deleted, and the schema's policy is to delete the relation (14).
pub const RELATION_TARGET_DELETED: Code = Code::new("relation.target-deleted");
/// A snapshot target's version isn't available: not in the document's
/// history, compacted away, or malformed (13).
pub const RELATION_SNAPSHOT_UNAVAILABLE: Code = Code::new("relation.snapshot-unavailable");
/// A relation's target resolved to the relation's own owner.
pub const RELATION_SELF_REFERENCE: Code = Code::new("relation.self-reference");
/// Rebinding followed a chain of successors to its limit without finding a
/// live node (37).
pub const RELATION_REBIND_LIMIT: Code = Code::new("relation.rebind-limit");
/// A stored page template can't be read by this engine; it is kept in the
/// document. A warning when it is the template in use, so the built-in one
/// stood in; otherwise info.
pub const TEMPLATE_UNREADABLE: Code = Code::new("layout.template-unreadable");
/// The page template can't be laid out (its page size isn't positive or
/// can't be resolved, or it has no usable frame for the main flow), so the
/// built-in template stood in.
pub const TEMPLATE_UNUSABLE: Code = Code::new("layout.template-unusable");
/// A template frame has a zero or negative size. A negative size is clamped
/// to zero (warning). A frame with no depth takes no text; one with no width
/// takes text only as overflow (info).
pub const DEGENERATE_FRAME: Code = Code::new("layout.degenerate-frame");
/// The text needs more pages than the engine's page limit; the rest of it was
/// left out.
pub const PAGE_LIMIT: Code = Code::new("layout.page-limit");
/// The page of the line a relation targets has no frame to place the owner in.
pub const RELATION_NO_FRAME: Code = Code::new("relation.no-frame");

pub const SOLVER_INFEASIBLE: Code = Code::new("layout.solver-infeasible");
pub const SOLVER_UNDERCONSTRAINED: Code = Code::new("layout.solver-underconstrained");
pub const SOLVER_LIMIT: Code = Code::new("layout.solver-limit");
pub const REGION_CYCLE: Code = Code::new("layout.region-cycle");
pub const REGION_LIMIT: Code = Code::new("layout.region-limit");
pub const REGION_PARAMETER: Code = Code::new("layout.region-parameter");
pub const FLOAT_DEFERRED: Code = Code::new("layout.float-deferred");
pub const FLOAT_UNPLACEABLE: Code = Code::new("layout.float-unplaceable");
pub const NOTE_CONTINUED: Code = Code::new("layout.note-continued");
pub const NOTE_DEPTH: Code = Code::new("layout.note-depth");
pub const TABLE_INVALID: Code = Code::new("layout.table-invalid");
pub const TABLE_LIMIT: Code = Code::new("layout.table-limit");
/// A cell span was clamped (zero, past the table edge, over an earlier cell
/// or out of its header rows); the table is laid out with what remains.
pub const TABLE_SPAN: Code = Code::new("layout.table-span");
/// Rows joined by a row span do not fit one frame and were split across frames.
pub const TABLE_ROWSPAN_SPLIT: Code = Code::new("layout.table-rowspan-split");
/// Header rows are not repeated on a continuation frame.
pub const TABLE_HEADER_UNREPEATED: Code = Code::new("layout.table-header-unrepeated");

/// Authored inline constraints that cannot be fulfilled as requested (Warning).
pub const ALIGNMENT_INVALID: Code = Code::new("relation.alignment-invalid");
pub const ALIGNMENT_CONFLICT: Code = Code::new("relation.alignment-conflict");
pub const ALIGNMENT_LIMIT: Code = Code::new("relation.alignment-limit");
pub const ALIGNMENT_CYCLE: Code = Code::new("relation.alignment-cycle");
pub const ALIGNMENT_OUTSIDE: Code = Code::new("relation.alignment-outside");

pub const TAB_LEADER_LIMIT: Code = Code::new("layout.tab-leader-limit");
pub const TAB_LEADER_MISSING: Code = Code::new("layout.tab-leader-missing");
