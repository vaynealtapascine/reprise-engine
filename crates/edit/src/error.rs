//! Why a transaction was refused (29, 37).

use std::ops::Range;

use reprise_diag::{Note, Severity};
use reprise_doc::{NodeId, RangeId, RelationId, SchemaError};

/// An editing error: which command, and why. Validation errors change nothing;
/// `Store` identifies an unexpected document/store failure.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", match .command { Some(i) => format!("command {i}: {}", .reason), None => .reason.to_string() })]
pub struct EditError {
    /// Index of the offending command in the transaction, if it was one.
    pub command: Option<usize>,
    pub reason: Reason,
}

impl EditError {
    /// A stable diagnostic for a refused operation. Error severity means the
    /// requested edit was omitted; validation errors leave authored state intact.
    pub fn note(&self) -> Note {
        let code = match self.reason {
            Reason::Fragment(reprise_doc::fragment::FragmentError::Version(_)) => {
                reprise_diag::Code::new("clipboard.version")
            }
            Reason::Fragment(reprise_doc::fragment::FragmentError::Limit(_)) => {
                reprise_diag::Code::new("clipboard.limit")
            }
            Reason::Fragment(_) => reprise_diag::Code::new("clipboard.invalid"),
            Reason::TooManyCommands { .. }
            | Reason::TextTooLong { .. }
            | Reason::TransactionTooLarge { .. }
            | Reason::TreeDepthLimit { .. } => crate::codes::LIMIT,
            Reason::Store(_) => crate::codes::STORE,
            _ => crate::codes::INVALID_COMMAND,
        };
        Note::new(Severity::Error, code, self.to_string())
    }

    pub(crate) fn at(command: usize, reason: Reason) -> EditError {
        EditError {
            command: Some(command),
            reason,
        }
    }
}

/// What was wrong. Tests match on these, never on the messages.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Reason {
    #[error(transparent)]
    Fragment(reprise_doc::fragment::FragmentError),
    #[error("paste must be the only command in its transaction")]
    MixedPaste,
    #[error("a transaction takes at most {max} commands, not {count}")]
    TooManyCommands { count: usize, max: usize },
    #[error("inserted text of {len} bytes is over the limit of {max}")]
    TextTooLong { len: usize, max: usize },
    #[error("transaction payload of {len} bytes is over the limit of {max}")]
    TransactionTooLarge { len: usize, max: usize },
    #[error("ancestry of {node} exceeds the editing limit of {max} nodes")]
    TreeDepthLimit { node: NodeId, max: usize },
    #[error("no live block {0}")]
    NoSuchBlock(NodeId),
    #[error("no live relation {0}")]
    NoSuchRelation(RelationId),
    #[error("offset {offset} is out of range or inside a character of {node}")]
    BadOffset { node: NodeId, offset: usize },
    #[error("range {range:?} is reversed, out of range or inside a character of {node}")]
    BadRange { node: NodeId, range: Range<usize> },
    #[error("index {index} is past the end of {len} children")]
    BadIndex { index: usize, len: usize },
    #[error("{0} can't move into itself or its own subtree")]
    Cycle(NodeId),
    #[error("{0} can't be joined to itself")]
    SameBlock(NodeId),
    #[error("{first} and {second} are different kinds of block")]
    KindMismatch { first: NodeId, second: NodeId },
    #[error("{0} has children, so can't be joined onto another block")]
    HasChildren(NodeId),
    #[error("the relation is invalid: {0}")]
    Schema(SchemaError),
    #[error("the relation names {0}, which is not a live block")]
    DeadBlock(NodeId),
    #[error("the relation names {0}, which doesn't exist")]
    DeadRange(RangeId),
    #[error("the document refused a validated change: {0}")]
    Store(String),
}
