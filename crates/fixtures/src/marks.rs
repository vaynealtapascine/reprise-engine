//! Poetry target: authored line positions, not renderer decorations.
use reprise_doc::{
    Basis, BlockKind, Dim, DocError, Document, LengthExpr, PageTemplate, RelationId, Style,
    marks::{Alignment, AnchorEdge, LineEdge, TabStop, TabStops},
};
use reprise_geom::Length;

pub fn poem() -> Result<Document, DocError> {
    let doc = Document::new(crate::PEER)?;
    let pt = Length::from_pt;
    doc.define_style(
        "verse",
        &Style {
            family: Some("Source Serif Pro".into()),
            size: Some(LengthExpr::Pt(pt(16))),
            line_height: Some(LengthExpr::Pt(pt(28))),
            tabs: Some(TabStops {
                interval: pt(60),
                stops: vec![
                    TabStop {
                        position: Some(pt(160)),
                        alignment: Alignment::Start,
                        leader: None,
                    },
                    TabStop {
                        position: Some(pt(240)),
                        alignment: Alignment::Start,
                        leader: None,
                    },
                    TabStop {
                        position: None,
                        alignment: Alignment::End,
                        leader: None,
                    },
                ],
            }),
            ..Default::default()
        },
    )?;
    let template = PageTemplate::new(
        "poem",
        Dim::fraction(Basis::MediumWidth, 1000),
        Dim::fraction(Basis::MediumHeight, 1000),
    )
    .with_frame(crate::templates::flow_frame(
        "verse",
        Dim::pt(52),
        Dim::pt(20),
        Dim::fraction(Basis::PageWidth, 1000).plus(pt(-104)),
        Dim::fraction(Basis::PageHeight, 1000).plus(pt(-40)),
    ));
    doc.define_page_template(&template)?;
    doc.use_page_template("poem")?;
    let title = doc.append_block(BlockKind::Paragraph, "verse", "II")?;
    doc.set_overrides(
        title,
        &Style {
            alignment: Some(Alignment::Centre),
            ..Default::default()
        },
    )?;
    doc.append_block(BlockKind::Paragraph, "verse", "")?;
    let stanza = "i’ve returned to my room and it is two am. and i know\nit is real because\tyou are not there.\nthis is how i tell now\nthat i’ve not fallen for another dream.\nso sure is tomorrow without you that i’ve come to recognize in the shadow your death with me.\ni don’t suppose that is a crime. to\nfeel the world is empty of you and know that world is real.";
    let p = doc.append_block(BlockKind::Paragraph, "verse", stanza)?;
    doc.align_line(p, stanza.find("feel the").unwrap_or(0), Alignment::End)?;
    doc.pin_line(
        p,
        stanza.find("that i’ve").unwrap_or(0),
        LineEdge::Start,
        p,
        stanza.find(" now").unwrap_or(0).saturating_add(4),
        AnchorEdge::Position,
    )?;
    doc.pin_line(
        p,
        stanza.find("i don’t").unwrap_or(0),
        LineEdge::End,
        p,
        stanza.rfind(" real.").unwrap_or(0),
        AnchorEdge::Position,
    )?;
    doc.append_block(BlockKind::Paragraph, "verse", "")?;
    let gaps = "the gaps where you fit\tthey are emptied &\ni find myself\tyour apparition there to\tremind me\nof all the things i once noticed yours.";
    let p = doc.append_block(BlockKind::Paragraph, "verse", gaps)?;
    doc.set_overrides(
        p,
        &Style {
            tabs: Some(TabStops {
                interval: pt(60),
                stops: vec![
                    TabStop {
                        position: Some(pt(213)),
                        alignment: Alignment::Start,
                        leader: None,
                    },
                    TabStop {
                        position: Some(pt(490)),
                        alignment: Alignment::End,
                        leader: None,
                    },
                ],
            }),
            ..Default::default()
        },
    )?;
    doc.align_line(p, gaps.find("of all").unwrap_or(0), Alignment::End)?;
    doc.pin_line(
        p,
        gaps.find("of all").unwrap_or(0),
        LineEdge::Start,
        p,
        gaps.find("remind me").unwrap_or(0).saturating_add(9),
        AnchorEdge::Position,
    )?;
    doc.append_block(BlockKind::Paragraph, "verse", "")?;
    let p = doc.append_block(
        BlockKind::Paragraph,
        "verse",
        "reality collapses back in on itself and i’ve\tmade it forget you and me.",
    )?;
    doc.set_overrides(
        p,
        &Style {
            tabs: Some(TabStops {
                interval: pt(36),
                stops: vec![TabStop {
                    position: None,
                    alignment: Alignment::End,
                    leader: None,
                }],
            }),
            ..Default::default()
        },
    )?;
    doc.commit();
    Ok(doc)
}

pub fn cycle(
    doc: &Document,
    a: reprise_doc::NodeId,
    b: reprise_doc::NodeId,
) -> Result<[RelationId; 2], DocError> {
    Ok([
        doc.pin_line(a, 0, LineEdge::Start, b, 0, AnchorEdge::Position)?,
        doc.pin_line(b, 0, LineEdge::Start, a, 0, AnchorEdge::Position)?,
    ])
}
