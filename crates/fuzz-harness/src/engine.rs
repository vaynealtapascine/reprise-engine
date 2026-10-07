//! The engine a scenario lays out with, as data.
//!
//! `Engine` isn't `Clone` and a `LayoutSession` borrows it, so the executor
//! holds an [`EngineSpec`] and rebuilds the engine (and the sessions) whenever
//! an op changes it. The spec is the whole configuration: fonts, images,
//! composer, medium and page limit.

use reprise_compose::{AuthoredBreak, Greedy, Optimal};
use reprise_doc::Medium;
use reprise_geom::Length;
use reprise_layout::Engine;

use crate::pool;
use crate::scenario::EngineChange;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineSpec {
    pub composer: u8,
    pub medium: u8,
    pub max_pages: u8,
    /// Indices into [`pool::font`], in registration order.
    pub fonts: Vec<u8>,
    /// Indices into [`pool::image`].
    pub images: Vec<u8>,
}

/// The mediums a scenario can pick, including degenerate ones.
fn medium(index: u8) -> Medium {
    let pt = Length::from_pt;
    match index % 6 {
        0 => Medium::new(pt(420), pt(300)),
        1 => Medium::new(pt(100), pt(60)),
        2 => Medium::new(pt(612), pt(792)),
        3 => Medium::new(Length::ZERO, pt(100)),
        4 => Medium::new(pt(-50), pt(-50)),
        _ => Medium::new(Length::MAX, Length::MAX),
    }
}

fn max_pages(index: u8) -> u32 {
    [1, 2, 3, 1000][usize::from(index) % 4]
}

impl EngineSpec {
    pub fn apply(&mut self, change: EngineChange) {
        match change {
            EngineChange::Composer(c) => self.composer = c % 4,
            EngineChange::Medium(m) => self.medium = m % 6,
            EngineChange::MaxPages(p) => self.max_pages = p % 4,
            EngineChange::RegisterFont(f) => {
                if self.fonts.len() < 6 {
                    self.fonts.push(f % 5);
                }
            }
            EngineChange::RegisterImage(i) => {
                if !self.images.contains(&(i % 6)) && self.images.len() < 6 {
                    self.images.push(i % 6);
                }
            }
            EngineChange::RemoveImage(i) => self.images.retain(|&x| x != i % 6),
        }
    }

    /// The configuration without fonts or images, for engines whose stores
    /// come from somewhere else (a reopened package).
    pub fn bare(&self) -> Engine {
        let mut engine = Engine::new(reprise_font::FontStore::default());
        engine.medium = medium(self.medium);
        engine.flow.max_pages = max_pages(self.max_pages);
        engine.composer = match self.composer % 4 {
            0 => Box::new(Greedy),
            1 => Box::new(Optimal::default()),
            2 => Box::new(Optimal::justified()),
            _ => Box::new(AuthoredBreak::default()),
        };
        engine
    }

    pub fn build(&self) -> Engine {
        let mut engine = self.bare();
        // The pinned fixture font, as every fixture engine has it.
        engine.fonts = reprise_fixtures::fonts();
        for &font in &self.fonts {
            let (bytes, declaration) = pool::font(usize::from(font));
            // A damaged font is refused with a diagnostic; the engine carries on.
            let _ = engine.fonts.register(bytes, declaration);
        }
        for &image in &self.images {
            // Not an image: refused, and the references stay placeholders.
            let _ = engine.assets.insert(pool::image(usize::from(image)));
        }
        engine
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_configuration_builds() {
        for composer in 0..4 {
            for medium in 0..6 {
                let mut spec = EngineSpec::default();
                spec.apply(EngineChange::Composer(composer));
                spec.apply(EngineChange::Medium(medium));
                for font in 0..5 {
                    spec.apply(EngineChange::RegisterFont(font));
                }
                for image in 0..6 {
                    spec.apply(EngineChange::RegisterImage(image));
                }
                let engine = spec.build();
                assert_eq!(engine.medium, super::medium(medium));
            }
        }
    }

    #[test]
    fn registration_is_bounded() {
        let mut spec = EngineSpec::default();
        for i in 0..=255u8 {
            spec.apply(EngineChange::RegisterFont(i));
            spec.apply(EngineChange::RegisterImage(i));
        }
        assert!(spec.fonts.len() <= 6 && spec.images.len() <= 6);
    }
}
