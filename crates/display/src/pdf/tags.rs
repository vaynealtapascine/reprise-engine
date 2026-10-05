//! The structure a tagged PDF is built from (decisions 32, 33).
//!
//! A [`Structure`] is the logical tree of a document: headings, paragraphs,
//! figures, notes, tables. Its leaves are the display-list glyph and image
//! items, addressed by [`ReadingRun`] and listed in reading order. The backend
//! paints text in that order, wraps every leaf in marked content with an MCID,
//! and writes the tree, the `ParentTree`, the catalog entries and the XMP
//! metadata that PDF/UA-1 (ISO 14289-1) asks for.
//!
//! The display crate cannot see layout or the document, so whoever owns those
//! builds the tree. `reprise-clipboard` does for the PDF exporter.

use std::ops::Range;

use reprise_diag::Code;
use reprise_geom::{PageSpace, Point, Rect};

use super::ReadingRun;

/// Structure nesting deeper than this is flattened into its ancestor at the limit.
pub const MAX_DEPTH: usize = 64;
/// The `/Lang` written when a structure names none.
pub const DEFAULT_LANG: &str = "en";
/// The title written when a structure names none.
pub const DEFAULT_TITLE: &str = "Untitled document";

/// A figure has no alt text, so it is marked as an artifact instead.
pub const FIGURE_ALT_MISSING: Code = Code::new("pdf.figure-alt-missing");
/// A note reference could not be linked to its note.
pub const REFERENCE_UNLINKED: Code = Code::new("pdf.reference-unlinked");
/// A heading level outside H1 to H6 was clamped.
pub const HEADING_LEVEL: Code = Code::new("pdf.heading-level");
/// Structure nested deeper than [`MAX_DEPTH`] was flattened.
pub const STRUCTURE_DEPTH: Code = Code::new("pdf.structure-depth");
/// A split of one run into parts could not be made, so the run stays whole.
pub const RUN_SPLIT: Code = Code::new("pdf.run-split");
/// The PDF is tagged but does not meet PDF/UA-1, so it does not claim to.
pub const UA_NOT_MET: Code = Code::new("pdf.ua-not-met");

/// A table header cell's scope (ISO 32000-1, table 349).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Row,
    Column,
    Both,
}

/// What a table cell is: the one place that decides `TD` or `TH`, and the spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRole {
    /// `Some` makes a `TH` with that `/Scope`; `None` a `TD`.
    pub header: Option<Scope>,
    /// `/RowSpan`, at least 1.
    pub row_span: u32,
    /// `/ColSpan`, at least 1.
    pub col_span: u32,
}

impl Default for CellRole {
    fn default() -> Self {
        CellRole {
            header: None,
            row_span: 1,
            col_span: 1,
        }
    }
}

/// A link annotation over a note reference, to the note it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The clickable area, in the page space of the reference's first run.
    pub rect: Rect<PageSpace>,
    /// The [`Node::key`] of the node to jump to.
    pub target: u32,
    /// The annotation's text for assistive technology.
    pub alt: String,
}

/// What a structure node is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    /// `P`.
    P,
    /// `H1` to `H6`. `title` names it in the outline and the structure tree.
    Heading { level: u8, title: String },
    /// `Figure`. An empty `alt` makes its content an artifact (reported).
    Figure { alt: String },
    /// `Note`, with a stable `/ID` where `id` is given.
    Note { id: Option<String> },
    /// `Reference`, wrapping a `Link` when `link` is set.
    Reference { link: Option<Link> },
    /// `Div`: a float or another grouping that has no tag of its own.
    Div,
    /// `Table`.
    Table,
    /// `TR`.
    Row,
    /// `TD` or `TH`.
    Cell(CellRole),
}

/// A child of a node: another node or a painted run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Child {
    Node(Node),
    /// A glyph run or image item, or a part of a glyph run.
    Content(Content),
}

/// One leaf of the structure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub run: ReadingRun,
    /// The bytes of the glyph run's `text` this leaf covers. `None` is the
    /// whole item. A run's parts must tile its text; the backend splits the
    /// run's glyphs among them, or keeps the run whole when it cannot (reported).
    pub part: Option<Range<usize>>,
}

impl Content {
    pub fn whole(run: ReadingRun) -> Content {
        Content { run, part: None }
    }
}

/// A node of the structure tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub role: Role,
    pub children: Vec<Child>,
    /// Names the node for [`Link::target`]. Unique within a structure.
    pub key: Option<u32>,
    /// Where a link or an outline entry to this node lands, in the page space
    /// of its first run.
    pub at: Option<Point<PageSpace>>,
}

impl Node {
    pub fn new(role: Role, children: Vec<Child>) -> Node {
        Node {
            role,
            children,
            key: None,
            at: None,
        }
    }
}

/// The whole logical structure, in reading order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Structure {
    /// The document title (XMP `dc:title`, shown by viewers). Empty gets [`DEFAULT_TITLE`].
    pub title: String,
    /// The default language, a BCP 47 tag. Empty gets [`DEFAULT_LANG`].
    pub lang: String,
    pub children: Vec<Node>,
}
