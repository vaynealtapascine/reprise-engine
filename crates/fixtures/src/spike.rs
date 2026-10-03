//! The end-to-end spike (decision 40): a paragraph with an anchored range, an
//! annotation that follows the line containing it, an edit that reflows the
//! text, and headless output.

use reprise_doc::relation::builtin::FOLLOW;
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, DocError, Document, LayoutQuery, LengthExpr, NodeId, Relation, SchemaRegistry,
    Style, Target,
};
use reprise_geom::Length;

use crate::PEER;

pub const OPENING: &str = "The house was a little larger on the inside than on the outside. \
Nobody measured it at first; the difference was a quarter of an inch, and a quarter of an inch \
is easy to forgive.";

pub const HALLWAY: &str = "Later a door appeared in a wall where no door had been, opening onto \
a corridor that went on further than the house could hold. Its walls were smooth and cold and \
the colour of ash.";

/// Inserted at the start of the second paragraph to force a reflow.
pub const EDIT: &str = "Then one evening, without a sound, the plan of the house was wrong. ";

pub struct Spike {
    pub doc: Document,
    pub opening: NodeId,
    pub hallway: NodeId,
    pub hallway_note: NodeId,
}

pub(crate) fn find(text: &str, needle: &str) -> std::ops::Range<usize> {
    let start = text.find(needle).expect("the fixture contains the needle");
    start..start + needle.len()
}

/// The styles every fixture uses: `body` and a smaller `note`.
pub fn define_styles(doc: &Document) -> Result<(), DocError> {
    doc.define_style(
        "body",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(11))),
            ..Default::default()
        },
    )?;
    doc.define_style(
        "note",
        &Style {
            parent: Some("body".into()),
            size: Some(LengthExpr::Em(750)),
            line_height: Some(LengthExpr::Em(1250)),
            ..Default::default()
        },
    )
}

/// A `reprise.follow` relation from `owner` to the line containing `range`.
pub fn follow(owner: NodeId, range: reprise_doc::RangeId) -> Relation {
    Relation::new(FOLLOW).owned_by(owner).target(
        "line",
        Target::Layout(LayoutQuery::LineContaining { range }),
    )
}

/// Builds the spike document. Peer 1 is pinned so IDs and output are reproducible.
pub fn document() -> Result<Spike, DocError> {
    let doc = Document::new(PEER)?;
    let schemas = SchemaRegistry::builtin();
    define_styles(&doc)?;

    let opening = doc.append_block(BlockKind::Paragraph, "body", OPENING)?;
    let hallway = doc.append_block(BlockKind::Paragraph, "body", HALLWAY)?;
    let opening_note = doc.append_block(
        BlockKind::Annotation,
        "note",
        "A quarter of an inch, measured from where?",
    )?;
    let hallway_note = doc.append_block(
        BlockKind::Annotation,
        "note",
        "The corridor is never the same length twice.",
    )?;

    let quarter = doc.add_range(
        opening,
        find(OPENING, "a quarter of an inch"),
        RangePolicy::FIXED,
    )?;
    let corridor = doc.add_range(hallway, find(HALLWAY, "a corridor"), RangePolicy::FIXED)?;
    for (owner, range) in [(opening_note, quarter), (hallway_note, corridor)] {
        doc.add_relation(&schemas, &follow(owner, range))?;
    }
    doc.commit();
    Ok(Spike {
        doc,
        opening,
        hallway,
        hallway_note,
    })
}

impl Spike {
    /// The spike's edit: a sentence typed at the start of the second paragraph.
    pub fn edit(&self) -> Result<(), DocError> {
        self.doc.block(self.hallway)?.text.insert(0, EDIT)?;
        self.doc.commit();
        Ok(())
    }
}
