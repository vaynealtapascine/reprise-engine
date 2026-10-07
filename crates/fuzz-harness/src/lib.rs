//! Cross-crate fuzzing for reprise-engine (decision 39).
//!
//! A scenario is a byte string. It decodes (totally) to a starting document
//! and a list of ops: edits, undo and redo, syncing two peers, copy and
//! paste, style and template changes, font and image registration, layout
//! through `Engine::layout` and through budgeted jobs, saving and reopening,
//! rendering and exporting. After every op, oracles check the invariants
//! that cross crate boundaries. See `docs/fuzzing.md`.

pub mod authored;
pub mod codes;
pub mod engine;
pub mod input;
pub mod minimize;
pub mod oracle;
pub mod pool;
pub mod run;
pub mod scenario;
pub mod start;
pub mod violation;

pub use run::{Report, check_bytes, hex, run_bytes, run_scenario, run_twice};
pub use scenario::Scenario;
pub use violation::Violation;
