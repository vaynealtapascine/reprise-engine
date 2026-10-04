//! Shared, reproducible test fixtures (decisions 39 and 40).
//!
//! Every fixture pins its inputs: the bundled font in `fixtures/fonts/` and
//! peer ID 1 (peer 2 for the second replica in concurrent fixtures). Nothing
//! here reads system fonts, the clock or randomness.
//!
//! -   [`spike`]: the end-to-end spike document.
//! -   [`hostile`]: documents built to break things. Every workstream's work
//!     must keep them laying out without panics, deterministically, with the
//!     diagnostics `tests/hostile.rs` expects.

use reprise_font::{Face, FontStore};
use reprise_layout::Engine;

pub mod hostile;
pub mod plugins;
pub mod spike;
pub mod templates;

/// The bundled test font: Source Serif Pro (OFL). It covers Latin only, so
/// Hebrew, Arabic and emoji fall back to `.notdef` until more fonts are bundled.
pub const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");

/// The pinned peer for fixtures.
pub const PEER: u64 = 1;
/// The second peer, for fixtures with two replicas.
pub const OTHER_PEER: u64 = 2;

pub fn fonts() -> FontStore {
    let mut fonts = FontStore::default();
    fonts.add(Face::from_bytes(SERIF).expect("the fixture font loads"));
    fonts
}

/// The default engine over the bundled font.
pub fn engine() -> Engine {
    Engine::new(fonts())
}
