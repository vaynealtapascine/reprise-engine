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
/// Older readers must refuse retained authored page-property history.
pub const REQUIRED_PAGE_SETUP: u64 = 1 << 12;
/// Older readers must refuse paragraph properties, flat named-style records
/// and clearing formatting actions (`docs/paragraph-styles.md`).
pub const REQUIRED_PARAGRAPH_STYLES: u64 = 1 << 13;
/// Older readers must refuse paragraph roles, list membership and list
/// definitions, retained history included (see `docs/roles-and-lists.md`).
pub const REQUIRED_BLOCK_SEMANTICS: u64 = 1 << 14;
/// Older readers must refuse inline object records, live or retired, since
/// they would drop the objects and their U+FFFC characters (`docs/inline.md`).
pub const REQUIRED_INLINE_OBJECTS: u64 = 1 << 15;
/// Older readers must refuse annotations (links, comment anchors), which they
/// would otherwise show as bare ranges and copy without their data. Bits 14
/// and 15 belong to lists and inline objects (fixed at integration).
pub const REQUIRED_ANNOTATIONS: u64 = 1 << 16;
/// Durable style owners, editable labels and versioned built-in references.
pub const REQUIRED_STYLE_IDENTITIES: u64 = 1 << 18;
/// Retained portable range endpoints in distinct text hosts.
pub const REQUIRED_RANGE_ENDPOINTS: u64 = 1 << 20;
/// Required bits this version understands. Any other required bit refuses.
pub const KNOWN_REQUIRED: u64 = REQUIRED_TABLE_SPANS
    | REQUIRED_TEXT_FORMATTING
    | REQUIRED_TEXT_EMPHASIS
    | REQUIRED_MARKS
    | REQUIRED_PAGE_SETUP
    | REQUIRED_PARAGRAPH_STYLES
    | REQUIRED_BLOCK_SEMANTICS
    | REQUIRED_INLINE_OBJECTS
    | REQUIRED_ANNOTATIONS
    | REQUIRED_STYLE_IDENTITIES
    | REQUIRED_RANGE_ENDPOINTS;
/// Optional bits this version sets and clears itself.
pub const KNOWN_OPTIONAL: u64 = OPTIONAL_TABLE_HEADERS;

/// The scan visits at most this many nodes; past it both bits are set, the
/// safe over-declaration.
const MAX_SCANNED_NODES: usize = 1 << 22;

/// The table feature bits `document` needs.
pub fn table_features(document: &Document) -> FeatureFlags {
    let mut flags = FeatureFlags::default();
    if document.has_style_identities() {
        flags.required |= REQUIRED_STYLE_IDENTITIES;
    }
    if document.has_cross_paragraph_ranges() {
        flags.required |= REQUIRED_RANGE_ENDPOINTS;
    }
    if document.has_annotations() {
        flags.required |= REQUIRED_ANNOTATIONS;
    }
    if document.has_page_setup() {
        flags.required |= REQUIRED_PAGE_SETUP;
    }
    if document.has_paragraph_styles() {
        flags.required |= REQUIRED_PARAGRAPH_STYLES;
    }
    if document.has_block_semantics() {
        flags.required |= REQUIRED_BLOCK_SEMANTICS;
    }
    if document.has_inline_objects() {
        flags.required |= REQUIRED_INLINE_OBJECTS;
    }
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

#[cfg(test)]
mod semantics_tests {
    use super::*;
    use reprise_doc::{BlockKind, ListDefinition, Role};

    #[test]
    fn roles_and_lists_require_the_semantics_reader() {
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "plain").unwrap();
        doc.commit();
        assert_eq!(table_features(&doc).required & REQUIRED_BLOCK_SEMANTICS, 0);
        doc.set_role(node, Role::Heading(1)).unwrap();
        doc.commit();
        assert_ne!(table_features(&doc).required & REQUIRED_BLOCK_SEMANTICS, 0);
        doc.set_role(node, Role::Body).unwrap();
        doc.commit();
        assert_ne!(
            table_features(&doc).required & REQUIRED_BLOCK_SEMANTICS,
            0,
            "the stored role stays in the record"
        );
        let doc = Document::new(1).unwrap();
        doc.append_block(BlockKind::Paragraph, "", "plain").unwrap();
        let list = doc.create_list(&ListDefinition::bulleted()).unwrap();
        doc.delete_list(list).unwrap();
        doc.commit();
        assert_ne!(table_features(&doc).required & REQUIRED_BLOCK_SEMANTICS, 0);
    }

    #[test]
    fn an_old_reader_refuses_lists_and_reads_documents_without_them() {
        use crate::container::{Container, Limits};
        let old = KNOWN_REQUIRED & !REQUIRED_BLOCK_SEMANTICS;
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "item").unwrap();
        doc.commit();
        let save = |doc: &Document| {
            crate::Package::new(
                doc,
                crate::DocumentId([7; 16]),
                reprise_doc::PersistenceMode::History,
            )
            .unwrap()
            .save()
            .unwrap()
        };
        Container::decode_known(&save(&doc), Limits::default(), old).unwrap();
        let list = doc.create_list(&ListDefinition::numbered()).unwrap();
        doc.set_list_item(node, list, 0).unwrap();
        doc.commit();
        let bytes = save(&doc);
        assert!(matches!(
            Container::decode_known(&bytes, Limits::default(), old),
            Err(crate::FormatError::RequiredFeatures(bits)) if bits == REQUIRED_BLOCK_SEMANTICS
        ));
        let reopened = crate::Package::open(
            &bytes,
            2,
            Limits::default(),
            &crate::MigrationRegistry::builtin(),
        )
        .unwrap()
        .into_document()
        .unwrap();
        assert_eq!(reopened.list_item(node).unwrap().marker, "1.");
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    #[test]
    fn an_old_reader_refuses_retained_cross_host_endpoints() {
        use crate::container::{Container, Limits};
        let doc = Document::new(1).unwrap();
        let first = doc
            .append_block(reprise_doc::BlockKind::Paragraph, "", "first")
            .unwrap();
        let last = doc
            .append_block(reprise_doc::BlockKind::Paragraph, "", "last")
            .unwrap();
        let save = || {
            crate::Package::new(
                &doc,
                crate::DocumentId([9; 16]),
                reprise_doc::PersistenceMode::History,
            )
            .unwrap()
            .save()
            .unwrap()
        };
        let old = KNOWN_REQUIRED & !REQUIRED_RANGE_ENDPOINTS;
        assert!(!doc.has_cross_paragraph_ranges());
        Container::decode_known(&save(), Limits::default(), old).unwrap();
        let policy = reprise_doc::text::RangePolicy::FIXED;
        let range = doc.add_range(first, 1..3, policy).unwrap();
        doc.stage_fragment_range_end(range, last, 2, policy)
            .unwrap();
        for deleted in [false, true] {
            if deleted {
                doc.delete_block(first).unwrap();
            }
            doc.commit();
            assert!(doc.has_cross_paragraph_ranges());
            let bytes = save();
            assert!(
                matches!(Container::decode_known(&bytes, Limits::default(), old), Err(crate::FormatError::RequiredFeatures(bits)) if bits == REQUIRED_RANGE_ENDPOINTS)
            );
            Container::decode_known(&bytes, Limits::default(), KNOWN_REQUIRED).unwrap();
        }
    }
}
