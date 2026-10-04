//! The editing kernel (29, 30, 31, 33): what every UI edits through.
//!
//! -   **Commands and transactions** ([`Command`], [`Transaction`], [`Editor`]):
//!     small typed edits, grouped into one validated, atomic commit and one undo
//!     step. An invalid transaction is refused whole with a typed [`EditError`].
//! -   **Per-user undo:** [`Editor::undo`] undoes this replica's transactions only,
//!     keeping collaborators' concurrent edits. A deleted block comes back with the
//!     same ID, because deletion changes its metadata flag (see `reprise-doc`).
//!
//! -   **Carets, selection and navigation** ([`Navigator`]): hit testing, caret
//!     geometry, logical and visual movement and selection geometry over a
//!     layout snapshot, with the gesture operations a UI needs.
//!
//! Positions are UTF-8 byte offsets throughout.

mod caret;
pub mod codes;
mod command;
mod editor;
mod error;
mod model;
mod movement;
mod navigator;
mod paste;
mod plan;
mod select;

pub use caret::{
    Affinity, BlockRange, Caret, CaretRect, Cursor, GraphemeCell, Hit, Movement, Selection,
    SelectionRect,
};
pub use command::{Command, Effect};
pub use editor::{Applied, Bias, Editor, Transaction};
pub use error::{EditError, Reason};
pub use navigator::Navigator;
pub use paste::Pasted;
pub use plan::{MAX_ANCESTORS, MAX_COMMANDS, MAX_INSERT_BYTES, MAX_TRANSACTION_BYTES};
