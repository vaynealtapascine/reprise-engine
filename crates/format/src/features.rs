//! Package feature bits for authored table metadata (34, 24).
//!
//! The container header carries a required and an optional feature mask.
//! Table features are declared from what the document actually contains, so
//! a package without header rows or spans stays byte-identical to before.
//!
//! - [`OPTIONAL_TABLE_HEADERS`]: some row is marked as a header row. A reader
//!   that predates repeating headers still lays the table out correctly, only
//!   without the repeated copies, so this degrades and is optional.
//! - [`REQUIRED_TABLE_SPANS`]: some cell spans several rows or columns. A reader
//!   without span support would drop those cells as unreadable metadata and show
//!   a different table, so it must refuse the file rather than degrade.
//!
//! Both masks are recomputed on every save: removing the last span clears the
//! required bit. Bits this crate does not define stay exactly as read.
use reprise_doc::{Document, NodeId, TableRole};

use crate::container::FeatureFlags;

pub const OPTIONAL_TABLE_HEADERS: u64 = 1 << 8;
pub const REQUIRED_TABLE_SPANS: u64 = 1 << 8;

/// Required bits this version understands. Any other required bit refuses.
pub const KNOWN_REQUIRED: u64 = REQUIRED_TABLE_SPANS;
/// Optional bits this version sets and clears itself.
pub const KNOWN_OPTIONAL: u64 = OPTIONAL_TABLE_HEADERS;

/// The scan visits at most this many nodes; past it both bits are set, the
/// safe over-declaration.
const MAX_SCANNED_NODES: usize = 1 << 22;

/// The table feature bits `document` needs.
pub fn table_features(document: &Document) -> FeatureFlags {
    let mut flags = FeatureFlags::default();
    let mut stack: Vec<NodeId> = document.blocks();
    let mut visited = 0usize;
    while let Some(node) = stack.pop() {
        visited += 1;
        if visited > MAX_SCANNED_NODES {
            return FeatureFlags {
                required: REQUIRED_TABLE_SPANS,
                optional: OPTIONAL_TABLE_HEADERS,
            };
        }
        match document.table_role(node) {
            Ok(None) => {}
            Ok(Some(TableRole::Row(row))) => {
                if row.header {
                    flags.optional |= OPTIONAL_TABLE_HEADERS;
                }
            }
            Ok(Some(TableRole::Cell(cell))) => {
                if cell.colspan != 1 || cell.rowspan != 1 {
                    flags.required |= REQUIRED_TABLE_SPANS;
                }
            }
            Ok(Some(TableRole::Table(_))) => {}
            // Metadata this version cannot read may be anything newer.
            Err(_) => flags.required |= REQUIRED_TABLE_SPANS,
        }
        if flags.required != 0 && flags.optional != 0 {
            break;
        }
        stack.extend(document.children(Some(node)));
    }
    flags
}
