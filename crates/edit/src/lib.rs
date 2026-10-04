//! The editing kernel (29, 30, 31, 33): what every UI edits through.
//!
//! -   **Commands and transactions** ([`Command`], [`Transaction`], [`Editor`]):
//!     small typed edits, grouped into one validated, atomic commit and one undo
//!     step. An invalid transaction is refused whole with a typed [`EditError`].
//! -   **Per-user undo:** [`Editor::undo`] undoes this replica's transactions only,
//!     keeping collaborators' concurrent edits. A deleted block comes back with the
//!     same ID, because deletion moves it into a trash (see `reprise-doc`).
//!
//! Positions are UTF-8 byte offsets throughout.

mod command;
mod editor;
mod error;
mod plan;

pub use command::{Command, Effect};
pub use editor::{Applied, Bias, Editor, Transaction};
pub use error::{EditError, Reason};
pub use plan::{MAX_COMMANDS, MAX_INSERT_BYTES};
