//! What a flow transition read from the frozen plan (sections §11).
//!
//! Today's memo key clones the whole [`Plan`], so a reservation or a field
//! value on page 40 invalidates page 1. Instead a transition records the plan
//! entries it *read*, with the provenance of the frame each was read for, and a
//! memo hit requires every one of them to be unchanged. This is decision 27's
//! dependency-driven evaluation, and it is exact: a transition can only be
//! affected by what it read.
//!
//! An entry is read *for a frame*: `Entry::for_frame` answers only when the
//! entry's provenance equals the frame's. So a read records the frame's
//! address and provenance, and what the plan answered (`None` for no entry, or
//! for an entry of another provenance). An entry whose provenance has changed
//! is then simply a read that still answers `None`, as it did before, and the
//! memo stays valid.

use crate::inline::FieldKey;
use crate::pages::{FrameAddr, Provenance};
use crate::regions::Plan;
use reprise_geom::{FrameSpace, Length, Rect};

/// One thing a transition read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlanRead {
    /// The float exclusions the plan holds for a frame.
    Exclusions {
        addr: FrameAddr,
        provenance: Provenance,
        value: Option<Vec<Rect<FrameSpace>>>,
    },
    /// The depth the plan reserves from a frame.
    Reservation {
        addr: FrameAddr,
        provenance: Provenance,
        value: Option<Length>,
    },
    /// A field's text from the previous pass.
    Field {
        key: FieldKey,
        value: Option<String>,
    },
    /// What the running regions of a page this transition made came to.
    Regions {
        spec: crate::running::attach::PageSpec,
        outcome: Option<crate::running::attach::Outcome>,
    },
}

impl PlanRead {
    /// Whether the plan still answers what was read. `None` for a read that
    /// needs more than the plan: the flow checks regions itself.
    pub fn holds(&self, plan: &Plan) -> Option<bool> {
        Some(match self {
            PlanRead::Exclusions {
                addr,
                provenance,
                value,
            } => {
                plan.exclusions
                    .get(addr)
                    .filter(|e| e.provenance == *provenance)
                    .map(|e| &e.value)
                    == value.as_ref()
            }
            PlanRead::Reservation {
                addr,
                provenance,
                value,
            } => {
                plan.reservations
                    .get(addr)
                    .filter(|e| e.provenance == *provenance)
                    .map(|e| &e.value)
                    == value.as_ref()
            }
            PlanRead::Field { key, value } => plan.fields.get(key) == value.as_ref(),
            PlanRead::Regions { .. } => return None,
        })
    }
}

/// Pushes `read` unless the same one is already there.
pub(crate) fn note(log: &mut Vec<PlanRead>, read: PlanRead) {
    if !log.contains(&read) {
        log.push(read);
    }
}
