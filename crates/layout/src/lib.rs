//! Layout: from an authored [`Document`] to a derived [`LayoutSnapshot`] and
//! [`DisplayList`]s (decisions 05, 24, 26, 28 and 37).
//!
//! The spike lays out one page in two passes:
//! 1. Flow ([`flow`]): paragraphs stack down the main frame.
//! 2. Relations ([`relations`]): blocks placed by relations, such as notes
//!    beside the line they follow, which is only known after pass 1. That
//!    ordering is the staged-pass rule (26).
//!
//! Layout never fails as a whole. Whatever can't be laid out is left out and
//! reported in `diagnostics` (37).
//!
//! [`Document`]: reprise_doc::Document
//! [`DisplayList`]: reprise_display::DisplayList

mod display;
mod flow;
mod relations;
mod snapshot;

use reprise_compose::{Composer, Greedy};
use reprise_doc::{Document, SchemaRegistry};
use reprise_font::FontStore;
use reprise_geom::Length;
use reprise_shape::{HarfRust, ShapingAdapter};
use serde::{Deserialize, Serialize};

pub use display::DisplayOptions;
pub use snapshot::{
    BlockLayout, Diagnostic, FrameLayout, LayoutSnapshot, LineLayout, LineRef, PageLayout,
    PositionedRun, RelationLayout, RelationStatus, Resolution, Subject, TargetLayout,
};

/// Diagnostic codes reported by layout itself. Fonts, shaping and
/// composition report their own (`font.*`, `shape.*`, `compose.*`).
pub mod codes {
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
}

/// Page geometry for the spike: one page, a main column and a margin column.
/// Replaced by page templates and frames in the flow workstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSettings {
    pub width: Length,
    pub height: Length,
    pub margin_top: Length,
    pub margin_left: Length,
    pub column_width: Length,
    pub gutter: Length,
    pub margin_column_width: Length,
    pub paragraph_spacing: Length,
    pub annotation_spacing: Length,
}

impl Default for PageSettings {
    fn default() -> Self {
        PageSettings {
            width: Length::from_pt(420),
            height: Length::from_pt(300),
            margin_top: Length::from_pt(36),
            margin_left: Length::from_pt(36),
            column_width: Length::from_pt(220),
            gutter: Length::from_pt(18),
            margin_column_width: Length::from_pt(110),
            paragraph_spacing: Length::from_pt(8),
            annotation_spacing: Length::from_pt(4),
        }
    }
}

/// The engine configuration: fonts, the shaping adapter, the composer and the
/// relation schemas. All of it is an input to the determinism guarantee (38).
pub struct Engine {
    pub fonts: FontStore,
    pub shaper: Box<dyn ShapingAdapter>,
    pub composer: Box<dyn Composer>,
    pub schemas: SchemaRegistry,
    pub page: PageSettings,
}

impl Engine {
    pub fn new(fonts: FontStore) -> Engine {
        Engine {
            fonts,
            shaper: Box::new(HarfRust),
            composer: Box::new(Greedy),
            schemas: SchemaRegistry::builtin(),
            page: PageSettings::default(),
        }
    }

    pub fn layout(&self, doc: &Document) -> LayoutSnapshot {
        let mut snapshot = LayoutSnapshot {
            revision: doc.revision(),
            adapter: self.shaper.info(),
            composer: self.composer.name().into(),
            page: self.page,
            pages: Vec::new(),
            frames: Vec::new(),
            blocks: Vec::new(),
            relations: Vec::new(),
            diagnostics: Vec::new(),
        };
        let pending = flow::run(self, doc, &mut snapshot);
        relations::run(self, doc, &mut snapshot, pending);
        snapshot
    }
}
