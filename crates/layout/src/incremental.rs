//! Incremental evaluation beside the frozen full-layout reference.
use crate::{Engine, LayoutSnapshot};
use reprise_doc::Document;

/// Computation state bound to one immutable engine configuration.
pub struct LayoutSession<'engine> {
    engine: &'engine Engine,
}
impl<'engine> LayoutSession<'engine> {
    pub fn new(engine: &'engine Engine) -> Self {
        Self { engine }
    }
    pub fn layout(&mut self, doc: &Document) -> LayoutSnapshot {
        self.engine.layout(doc)
    }
}
