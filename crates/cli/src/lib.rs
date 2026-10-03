//! The end-to-end spike (decision 40): a paragraph with an anchored range, an
//! annotation that follows the line containing it, an edit that reflows the
//! text, and headless output.

use std::path::Path;

use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, DocError, Document, LengthExpr, NodeId, Relation, RelationKind, Style, Target,
};
use reprise_font::{Face, FontStore};
use reprise_geom::Length;
use reprise_layout::{DisplayOptions, Engine, LayoutSnapshot};

pub const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");

pub const OPENING: &str = "The house was a little larger on the inside than on the outside. \
Nobody measured it at first; the difference was a quarter of an inch, and a quarter of an inch \
is easy to forgive.";

pub const HALLWAY: &str = "Later a door appeared in a wall where no door had been, opening onto \
a corridor that went on further than the house could hold. Its walls were smooth and cold and \
the colour of ash.";

/// Inserted at the start of the second paragraph to force a reflow.
pub const EDIT: &str = "Then one evening, without a sound, the plan of the house was wrong. ";

pub fn engine() -> Engine {
    let mut fonts = FontStore::default();
    fonts.add(Face::from_bytes(SERIF).expect("the fixture font loads"));
    Engine::new(fonts)
}

pub struct Spike {
    pub doc: Document,
    pub opening: NodeId,
    pub hallway: NodeId,
    pub hallway_note: NodeId,
}

fn find(text: &str, needle: &str) -> std::ops::Range<usize> {
    let start = text.find(needle).expect("the fixture contains the needle");
    start..start + needle.len()
}

/// Builds the spike document. Peer 1 is pinned so IDs and output are reproducible.
pub fn spike_document() -> Result<Spike, DocError> {
    let doc = Document::new(1)?;
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
    )?;

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
    for (source, range) in [(opening_note, quarter), (hallway_note, corridor)] {
        doc.add_relation(&Relation {
            kind: RelationKind::Follow,
            source,
            target: Target::LineContaining { range },
        })?;
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

/// Writes every output format for one snapshot as `<dir>/<name>.*`.
pub fn write_outputs(
    engine: &Engine,
    snapshot: &LayoutSnapshot,
    dir: &Path,
    name: &str,
) -> std::io::Result<()> {
    let debug = snapshot.to_display_list(DisplayOptions { debug: true });
    let content = debug.content_only();
    let fail = |e: &dyn std::fmt::Display| std::io::Error::other(e.to_string());
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{name}.layout.json")), snapshot.to_json())?;
    std::fs::write(dir.join(format!("{name}.display.json")), debug.to_json())?;
    std::fs::write(
        dir.join(format!("{name}.svg")),
        reprise_display::svg::render(&debug, &engine.fonts).map_err(|e| fail(&e))?,
    )?;
    std::fs::write(
        dir.join(format!("{name}.png")),
        reprise_display::png::render(&debug, &engine.fonts, 3.0).map_err(|e| fail(&e))?,
    )?;
    std::fs::write(
        dir.join(format!("{name}.pdf")),
        reprise_display::pdf::render(&content, &engine.fonts).map_err(|e| fail(&e))?,
    )?;
    Ok(())
}
