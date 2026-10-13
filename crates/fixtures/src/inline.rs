//! Documents for inline objects and fields (`docs/inline.md`). Each scenario
//! pins what one behaviour of the model needs: a tall image growing its line,
//! an object that wraps, an object wider than its frame, page numbers over
//! several pages, a forward `page-of`, right-to-left and vertical lines, and a
//! field in a repeated table header row. `tests/inline.rs` checks each and
//! records its layout.

use reprise_doc::{
    BlockKind, Column, ColumnWidth, Dim, DocError, Document, InlineObject, InlineSize, LengthExpr,
    NodeId, PageTemplate, Style, TableColumns, WritingMode,
};
use reprise_geom::Length;
use reprise_layout::Engine;

use crate::templates::flow_frame;
use crate::{PEER, engine};

/// A one-pixel PNG: a real image, whose size the objects always set.
pub const RED: &[u8] = include_bytes!("../../../fixtures/images/red-1x1.png");

/// A document and the engine that lays it out.
pub struct Scenario {
    pub name: &'static str,
    pub engine: Engine,
    pub doc: Document,
    /// The paragraph the scenario is about.
    pub subject: NodeId,
}

/// Part of a paragraph: text, or an object.
pub enum Part<'a> {
    Text(&'a str),
    Object(InlineObject),
}

/// A size parameter of `points`, as an object stores it.
pub fn pt(points: i32) -> String {
    InlineSize::Pt(Length::from_pt(points)).to_string()
}

/// An image object of `width` by `height` points.
pub fn image(asset: &str, width: i32, height: i32, alt: &str) -> InlineObject {
    InlineObject::image(asset, alt)
        .with("width", pt(width))
        .with("height", pt(height))
}

/// A `page-number` field.
pub fn page_number() -> InlineObject {
    InlineObject::field("page-number")
}

/// The engine, a document with one `body` style (10 pt on 12 pt lines) and a
/// page that is one main-flow frame of `width` by `height` points in `mode`,
/// and the hash of the image asset the engine holds.
pub fn page(
    width: i32,
    height: i32,
    mode: WritingMode,
) -> Result<(Engine, Document, String), DocError> {
    let mut engine = engine();
    let asset = engine
        .assets
        .insert(RED)
        .map_err(|e| DocError::Store(e.to_string()))?;
    let doc = Document::new(PEER)?;
    doc.define_style(
        "body",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(10))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(12))),
            ..Default::default()
        },
    )?;
    let mut frame = flow_frame(
        "body",
        Dim::pt(0),
        Dim::pt(0),
        Dim::pt(width),
        Dim::pt(height),
    );
    frame.writing_mode = mode;
    doc.set_page_template(
        &PageTemplate::new("inline", Dim::pt(width), Dim::pt(height)).with_frame(frame),
    )?;
    Ok((engine, doc, asset))
}

/// Appends a paragraph made of `parts`.
pub fn paragraph(doc: &Document, parts: Vec<Part<'_>>) -> Result<NodeId, DocError> {
    let p = doc.append_block(BlockKind::Paragraph, "body", "")?;
    extend(doc, p, parts)?;
    Ok(p)
}

/// Adds `parts` at the end of `p` in one insertion. Every edit of a paragraph
/// reads the records of all its objects, so adding them one by one costs
/// time quadratic in their number (twelve seconds for the 200 fields of
/// `inline_storm` in a debug build, in every test that builds all fixtures).
pub fn extend(doc: &Document, p: NodeId, parts: Vec<Part<'_>>) -> Result<(), DocError> {
    let mut text = String::new();
    let mut objects = Vec::new();
    for part in parts {
        match part {
            Part::Text(run) => text.push_str(run),
            Part::Object(object) => {
                text.push(reprise_doc::text::OBJECT);
                objects.push(object);
            }
        }
    }
    let end = doc.block(p)?.text.len();
    doc.insert_with_inline(p, end, &text, &objects)
}

/// A 120 pt wide column; a 36 pt tall image on a line of 12 pt text.
pub fn tall_image() -> Result<Scenario, DocError> {
    let (engine, doc, asset) = page(120, 300, WritingMode::default())?;
    let subject = paragraph(
        &doc,
        vec![
            Part::Text("Words before the "),
            Part::Object(image(&asset, 24, 36, "a tall picture")),
            Part::Text(" and then the words after it that wrap onto a few more lines."),
        ],
    )?;
    doc.commit();
    Ok(Scenario {
        name: "inline_tall_image",
        engine,
        doc,
        subject,
    })
}

/// A line that is almost full, then an image that cannot fit after it.
pub fn image_wraps_at_line_end() -> Result<Scenario, DocError> {
    let (engine, doc, asset) = page(100, 300, WritingMode::default())?;
    let subject = paragraph(
        &doc,
        vec![
            Part::Text("one two three four "),
            Part::Object(image(&asset, 40, 12, "wide")),
            Part::Text(" five"),
        ],
    )?;
    doc.commit();
    Ok(Scenario {
        name: "inline_image_wraps",
        engine,
        doc,
        subject,
    })
}

/// An image three times as wide as its frame.
pub fn image_wider_than_frame() -> Result<Scenario, DocError> {
    let (engine, doc, asset) = page(100, 200, WritingMode::default())?;
    let subject = paragraph(
        &doc,
        vec![
            Part::Text("Before "),
            Part::Object(image(&asset, 300, 30, "too wide")),
            Part::Text(" after"),
        ],
    )?;
    doc.commit();
    Ok(Scenario {
        name: "inline_image_wider_than_frame",
        engine,
        doc,
        subject,
    })
}

/// Page numbers in body text over several pages (a frame holds three lines).
pub fn page_numbers() -> Result<Scenario, DocError> {
    let (engine, doc, _) = page(100, 36, WritingMode::default())?;
    let mut parts = Vec::new();
    for _ in 0..8 {
        parts.push(Part::Text("page "));
        parts.push(Part::Object(page_number()));
        parts.push(Part::Text(" of the text and "));
    }
    let subject = paragraph(&doc, parts)?;
    doc.commit();
    Ok(Scenario {
        name: "inline_page_numbers",
        engine,
        doc,
        subject,
    })
}

/// A `page-of` and a `page-count` at the start of a long document, pointing
/// at a paragraph several pages away: their text changes after the first
/// pass, so a second pass is needed, and then it settles.
pub fn page_of_forward() -> Result<Scenario, DocError> {
    let (engine, doc, _) = page(100, 36, WritingMode::default())?;
    let subject = paragraph(
        &doc,
        vec![Part::Text("See page "), Part::Text(" of "), Part::Text(".")],
    )?;
    for i in 0..8 {
        let text = format!("filler paragraph number {i} that takes up some room");
        doc.append_block(BlockKind::Paragraph, "body", &text)?;
    }
    let target = doc.append_block(BlockKind::Paragraph, "body", "The target.")?;
    let target_at = doc.field_target(target, 0)?;
    let text = doc.block(subject)?.text.to_string();
    let after_see = text.find(" of ").unwrap_or(0);
    doc.insert_inline(
        subject,
        after_see,
        &InlineObject::field("page-of").with("target", target_at),
    )?;
    let at = doc.block(subject)?.text.len() - 1;
    doc.insert_inline(subject, at, &InlineObject::field("page-count"))?;
    doc.commit();
    Ok(Scenario {
        name: "inline_page_of_forward",
        engine,
        doc,
        subject,
    })
}

/// Hebrew around an image and a field in a right-to-left paragraph.
pub fn rtl_with_atoms() -> Result<Scenario, DocError> {
    let (engine, doc, asset) = page(150, 200, WritingMode::default())?;
    let subject = paragraph(
        &doc,
        vec![
            Part::Text("\u{5e9}\u{5dc}\u{5d5}\u{5dd} "),
            Part::Object(image(&asset, 20, 10, "inline picture")),
            Part::Text(" \u{5e2}\u{5d5}\u{5dc}\u{5dd} abc "),
            Part::Object(page_number()),
            Part::Text(" \u{5e9}\u{5dc}\u{5d5}\u{5dd}"),
        ],
    )?;
    doc.commit();
    Ok(Scenario {
        name: "inline_rtl",
        engine,
        doc,
        subject,
    })
}

/// Latin text in a vertical frame with an upright image and field.
pub fn vertical_with_atoms() -> Result<Scenario, DocError> {
    let (engine, doc, asset) = page(120, 200, WritingMode::VerticalRl)?;
    let subject = paragraph(
        &doc,
        vec![
            Part::Text("Some text "),
            Part::Object(image(&asset, 16, 24, "upright picture")),
            Part::Text(" and a number "),
            Part::Object(page_number()),
            Part::Text(" in a vertical column that wraps."),
        ],
    )?;
    doc.commit();
    Ok(Scenario {
        name: "inline_vertical",
        engine,
        doc,
        subject,
    })
}

/// A table whose header row holds a page-number field, over several pages.
pub fn repeated_header_page_numbers() -> Result<Scenario, DocError> {
    let (engine, doc, _) = page(100, 60, WritingMode::default())?;
    let table = doc.append_table(TableColumns {
        columns: (0..2)
            .map(|_| Column {
                width: ColumnWidth::Proportional(1),
            })
            .collect(),
    })?;
    let head = doc.append_table_row(table, true)?;
    let mut subject = None;
    for column in 0..2u32 {
        let cell = doc.append_table_cell(head, column)?;
        let p = doc.append_cell_block(
            cell,
            BlockKind::Paragraph,
            "body",
            if column == 0 { "Page " } else { "Item" },
        )?;
        if column == 0 {
            let end = doc.block(p)?.text.len();
            doc.insert_inline(p, end, &page_number())?;
            subject = Some(p);
        }
    }
    for i in 0..9 {
        let row = doc.append_table_row(table, false)?;
        for column in 0..2u32 {
            let cell = doc.append_table_cell(row, column)?;
            doc.append_cell_block(cell, BlockKind::Paragraph, "body", &format!("row {i}"))?;
        }
    }
    doc.commit();
    Ok(Scenario {
        name: "inline_repeated_header",
        engine,
        doc,
        subject: subject.ok_or(DocError::Store("no header cell".into()))?,
    })
}

/// Every scenario.
pub fn all() -> Result<Vec<Scenario>, DocError> {
    Ok(vec![
        tall_image()?,
        image_wraps_at_line_end()?,
        image_wider_than_frame()?,
        page_numbers()?,
        page_of_forward()?,
        rtl_with_atoms()?,
        vertical_with_atoms()?,
        repeated_header_page_numbers()?,
    ])
}
