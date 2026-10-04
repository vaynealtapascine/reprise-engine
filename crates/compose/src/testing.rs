//! Shaping with the bundled font, for unit tests.

use reprise_font::{Face, FontStore};
use reprise_geom::Length;
use reprise_shape::{HarfRust, Item, ParagraphInput, Shaper, StyleRun, itemize};

use crate::{ComposeRequest, Composer, Composition, GeometryProvider, break_opportunities};

const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");

pub struct Shaped {
    pub text: String,
    fonts: FontStore,
    items: Vec<Item>,
}

impl Shaped {
    pub fn new(text: &str) -> Shaped {
        let mut fonts = FontStore::default();
        fonts.add(Face::from_bytes(SERIF).unwrap());
        let styles = [StyleRun {
            range: 0..text.len(),
            families: vec!["Source Serif Pro".into()],
            size: Length::from_pt(10),
            language: None,
            features: Vec::new(),
        }];
        let items = itemize(
            &ParagraphInput {
                text,
                styles: &styles,
                direction: None,
            },
            &fonts,
        )
        .items;
        Shaped {
            text: text.into(),
            fonts,
            items,
        }
    }

    pub fn shaper(&self) -> Shaper<'_> {
        Shaper {
            text: &self.text,
            items: &self.items,
            fonts: &self.fonts,
            adapter: &HarfRust,
        }
    }

    /// The fragments' text put back together, whitespace and all.
    pub fn joined(&self, c: &Composition) -> String {
        c.lines.iter().map(|l| &self.text[l.text.clone()]).collect()
    }

    /// Composes with 12pt lines from the start.
    pub fn compose(&self, composer: &dyn Composer, geometry: &dyn GeometryProvider) -> Composition {
        let shaper = self.shaper();
        let shaped = shaper.shape();
        composer.compose(&ComposeRequest {
            text: &self.text,
            shaped: &shaped,
            reshape: &shaper,
            breaks: &break_opportunities(&self.text),
            line_height: Length::from_pt(12),
            geometry,
            start: 0,
            block_start: Length::ZERO,
        })
    }

    /// Each fragment's text, without trailing whitespace.
    pub fn lines<'a>(&'a self, c: &Composition) -> Vec<&'a str> {
        c.lines
            .iter()
            .map(|l| self.text[l.text.clone()].trim_end())
            .collect()
    }
}
