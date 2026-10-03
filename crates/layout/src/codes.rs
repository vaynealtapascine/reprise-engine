//! Diagnostic codes reported by layout itself. Fonts, shaping and
//! composition report their own (`font.*`, `shape.*`, `compose.*`).

use reprise_diag::Code;

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
