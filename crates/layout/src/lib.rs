//! Layout: from an authored [`Document`] to a derived [`LayoutSnapshot`] and
//! [`DisplayList`]s (decisions 05, 24, 26, 28 and 37).
//!
//! Layout runs in two passes:
//! 1. Flow ([`flow`]): the page template is resolved against the medium
//!    ([`template`]), and paragraphs thread down its frames, page after page.
//!    A paragraph that doesn't fit the rest of a frame continues in the next.
//! 2. Relations ([`relations`]): blocks placed by relations, such as notes
//!    beside the line they follow, which is only known after pass 1. That
//!    ordering is the staged-pass rule (26).
//!
//! Layout never fails as a whole. Whatever can't be laid out is left out and
//! reported in `diagnostics` (37).
//!
//! [`Document`]: reprise_doc::Document
//! [`DisplayList`]: reprise_display::DisplayList

pub mod codes;
mod display;
mod flow;
mod query;
mod region;
mod relations;
mod snapshot;
mod template;

use reprise_compose::{Composer, Greedy};
use reprise_doc::{Document, FunctionRegistry, Medium, SchemaRegistry};
use reprise_font::FontStore;
use reprise_geom::Length;
use reprise_shape::{HarfRust, ShapingAdapter};
use serde::{Deserialize, Serialize};

pub use display::DisplayOptions;
pub use snapshot::{
    BlockLayout, Diagnostic, FrameLayout, LayoutSnapshot, LineLayout, LineRef, PageLayout,
    PositionedRun, RelationLayout, RelationStatus, Resolution, Subject, TargetLayout,
    TemplateSource, TemplateUsed,
};

/// Engine settings for the flow. Like the rest of the engine configuration
/// they are an input to the determinism guarantee, so the snapshot records them (38).
///
/// Spacing lives here, as engine defaults, until styles can carry it (08):
/// paragraph spacing is a property of a block's style (space before and
/// after), and the annotation gap belongs to the frame or relation that stacks
/// annotations. When those exist these values become the lowest layer, like
/// `default_style()`, and nothing that reads them changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowSettings {
    /// The most pages layout makes. Text that needs more is left out and
    /// reported with `layout.page-limit`; it never loops. At least one page is
    /// always made, so 0 means 1.
    pub max_pages: u32,
    /// The gap between consecutive paragraphs in a frame.
    pub paragraph_spacing: Length,
    /// The gap between annotations stacked in one frame.
    pub annotation_spacing: Length,
}

impl Default for FlowSettings {
    fn default() -> Self {
        FlowSettings {
            max_pages: 1000,
            paragraph_spacing: Length::from_pt(8),
            annotation_spacing: Length::from_pt(4),
        }
    }
}

/// The engine configuration: fonts, the shaping adapter, the composer, the
/// relation schemas, style functions, the medium and the flow settings. All of it is an input
/// to the determinism guarantee (38).
pub struct Engine {
    pub fonts: FontStore,
    pub shaper: Box<dyn ShapingAdapter>,
    pub composer: Box<dyn Composer>,
    pub schemas: SchemaRegistry,
    /// Pure functions available to authored style expressions.
    pub functions: FunctionRegistry,
    /// What the document is laid out for. Page templates may size themselves
    /// from it. Not authored state: the same document lays out differently on
    /// a different medium.
    pub medium: Medium,
    pub flow: FlowSettings,
}

impl Engine {
    pub fn new(fonts: FontStore) -> Engine {
        Engine {
            fonts,
            shaper: Box::new(HarfRust),
            composer: Box::new(Greedy),
            schemas: SchemaRegistry::builtin(),
            functions: FunctionRegistry::builtin(),
            medium: Medium::new(Length::from_pt(420), Length::from_pt(300)),
            flow: FlowSettings::default(),
        }
    }

    pub fn layout(&self, doc: &Document) -> LayoutSnapshot {
        let mut snapshot = LayoutSnapshot {
            revision: doc.revision(),
            adapter: self.shaper.info(),
            composer: self.composer.name().into(),
            medium: self.medium,
            settings: self.flow,
            template: TemplateUsed {
                name: String::new(),
                source: TemplateSource::Builtin,
            },
            pages: Vec::new(),
            frames: Vec::new(),
            blocks: Vec::new(),
            relations: Vec::new(),
            diagnostics: Vec::new(),
        };
        let template = template::resolve(self, doc, &mut snapshot.diagnostics);
        snapshot.template = TemplateUsed {
            name: template.name.clone(),
            source: template.source,
        };
        let pending = flow::run(self, doc, &template, &mut snapshot);
        relations::run(self, doc, &mut snapshot, pending);
        snapshot
    }
}
