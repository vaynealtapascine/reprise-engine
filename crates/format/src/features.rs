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
/// Older readers must not silently discard anchored character formatting.
pub const REQUIRED_TEXT_FORMATTING: u64 = 1 << 9;

/// Older format1 readers must refuse packages retaining emphasis or text paint.
pub const REQUIRED_TEXT_EMPHASIS: u64 = 1 << 10;
/// Older readers must refuse authored positional properties and characters.
pub const REQUIRED_MARKS: u64 = 1 << 11;
/// Required bits this version understands. Any other required bit refuses.
pub const KNOWN_REQUIRED: u64 =
    REQUIRED_TABLE_SPANS | REQUIRED_TEXT_FORMATTING | REQUIRED_TEXT_EMPHASIS | REQUIRED_MARKS;
/// Optional bits this version sets and clears itself.
pub const KNOWN_OPTIONAL: u64 = OPTIONAL_TABLE_HEADERS;

/// The scan visits at most this many nodes; past it both bits are set, the
/// safe over-declaration.
const MAX_SCANNED_NODES: usize = 1 << 22;

/// The table feature bits `document` needs.
pub fn table_features(document: &Document) -> FeatureFlags {
    let mut flags = FeatureFlags::default();
    if document.has_marks() {
        flags.required |= REQUIRED_MARKS;
    }
    if document.has_text_formatting() {
        flags.required |= REQUIRED_TEXT_FORMATTING;
    }
    if document.has_text_emphasis() {
        flags.required |= REQUIRED_TEXT_EMPHASIS;
    }
    let mut stack: Vec<NodeId> = document.blocks();
    let mut visited = 0usize;
    while let Some(node) = stack.pop() {
        visited += 1;
        if visited > MAX_SCANNED_NODES {
            return FeatureFlags {
                required: REQUIRED_TABLE_SPANS | flags.required,
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
        if flags.required & REQUIRED_TABLE_SPANS != 0 && flags.optional != 0 {
            break;
        }
        stack.extend(document.children(Some(node)));
    }
    flags
}

#[cfg(test)]
mod marks_tests {
    use super::*;
    use reprise_doc::{
        BlockKind, Style,
        marks::{Alignment, AnchorEdge, LineEdge, TabStops},
    };

    #[test]
    fn positional_forms_and_deleted_owners_require_the_marks_reader() {
        let doc = Document::new(1).unwrap();
        let node = doc
            .append_block(BlockKind::Paragraph, "", "ordinary")
            .unwrap();
        assert_eq!(table_features(&doc).required & REQUIRED_MARKS, 0);
        doc.set_alignment(node, Alignment::End).unwrap();
        assert_ne!(table_features(&doc).required & REQUIRED_MARKS, 0);
        doc.set_overrides(node, &Style::default()).unwrap();
        assert_ne!(
            table_features(&doc).required & REQUIRED_MARKS,
            0,
            "new operations remain in history"
        );

        for text in ["a\tb", "a\nb", "a\r\nb", "a\u{2028}b"] {
            let doc = Document::new(1).unwrap();
            let node = doc.append_block(BlockKind::Paragraph, "", text).unwrap();
            doc.delete_block(node).unwrap();
            assert_ne!(table_features(&doc).required & REQUIRED_MARKS, 0);
        }
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "plain").unwrap();
        let id = doc
            .pin_line(node, 0, LineEdge::Start, node, 1, AnchorEdge::Position)
            .unwrap();
        doc.delete_relation(id).unwrap();
        assert_ne!(table_features(&doc).required & REQUIRED_MARKS, 0);
        let doc = Document::new(1).unwrap();
        doc.define_style(
            "tabs",
            &Style {
                tabs: Some(TabStops::default()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_ne!(table_features(&doc).required & REQUIRED_MARKS, 0);
    }
}
