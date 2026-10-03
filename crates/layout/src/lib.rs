//! Layout: from an authored [`Document`] to a derived [`LayoutSnapshot`] and a
//! [`DisplayList`] (decisions 05, 24, 26, 28 and 37).
//!
//! The spike lays out one page in two passes:
//! 1. Flow: paragraphs stack down the main column.
//! 2. Relations: annotations are placed beside the line their relation targets,
//!    which is only known after pass 1. That ordering is the staged-pass rule (26).
//!
//! Layout never fails as a whole. Whatever can't be laid out is left out and
//! reported in `diagnostics` (37).

mod display;
mod flow;
mod relations;
mod snapshot;

use reprise_compose::{Composer, Greedy};
use reprise_font::FontStore;
use reprise_geom::Length;
use reprise_shape::{HarfRust, ShapingAdapter};
use serde::{Deserialize, Serialize};

pub use display::DisplayOptions;
pub use snapshot::{
    BlockLayout, Diagnostic, LayoutSnapshot, LineLayout, RelationLayout, RelationStatus,
};

/// Page geometry for the spike: one page, a main column and a margin column.
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

/// The engine configuration: fonts, the shaping adapter and the composer.
/// All of it is an input to the determinism guarantee (38).
pub struct Engine {
    pub fonts: FontStore,
    pub shaper: Box<dyn ShapingAdapter>,
    pub composer: Box<dyn Composer>,
    pub page: PageSettings,
}

impl Engine {
    pub fn new(fonts: FontStore) -> Engine {
        Engine {
            fonts,
            shaper: Box::new(HarfRust),
            composer: Box::new(Greedy),
            page: PageSettings::default(),
        }
    }
}

impl Engine {
    pub fn layout(&self, doc: &reprise_doc::Document) -> LayoutSnapshot {
        let mut snapshot = LayoutSnapshot {
            revision: doc.revision(),
            adapter: self.shaper.info(),
            composer: self.composer.name().into(),
            page: self.page,
            blocks: Vec::new(),
            relations: Vec::new(),
            diagnostics: Vec::new(),
        };
        let annotations = flow::run(self, doc, &mut snapshot);
        relations::run(self, doc, &mut snapshot, annotations);
        snapshot
    }
}
