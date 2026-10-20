//! Generate authored packages and native facade output for the sections WASM gate.
use reprise_doc::sections::{BreakKind, SectionId, SectionProperties};
use reprise_doc::{BlockKind, Document, FrameRole, FrameTemplate, LengthExpr, PageTemplate};
use reprise_geom::Length;
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn properties(entries: &[(&str, Value)]) -> SectionProperties {
    entries
        .iter()
        .map(|(k, v)| ((*k).into(), v.clone()))
        .collect()
}
fn styled_document() -> Result<Document> {
    let doc = Document::new(1)?;
    doc.define_style(
        "band",
        &reprise_doc::Style {
            family: Some("Source Serif Pro".into()),
            size: Some(LengthExpr::Pt(Length::from_pt(10))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(12))),
            ..Default::default()
        },
    )?;
    Ok(doc)
}
fn document() -> Result<Document> {
    let doc = styled_document()?;
    doc.set_page_setup_patch(reprise_doc::page_setup::PageSetupPatch {
        width: Some(Length::from_pt(200)),
        height: Some(Length::from_pt(200)),
        top: Some(Length::from_pt(10)),
        right: Some(Length::from_pt(10)),
        bottom: Some(Length::from_pt(10)),
        left: Some(Length::from_pt(10)),
    })?;
    Ok(doc)
}
fn bands(vertical: bool, regions: bool) -> Result<Document> {
    let doc = document()?;
    if regions {
        use reprise_doc::Dim;
        doc.set_page_template(
            &PageTemplate::new("regions", Dim::pt(200), Dim::pt(200))
                .with_frame(FrameTemplate::new(
                    "main",
                    FrameRole::Flow("main".into()),
                    (Dim::pt(10), Dim::pt(10)),
                    (Dim::pt(180), Dim::pt(180)),
                ))
                .with_frame(FrameTemplate::new(
                    "notes",
                    FrameRole::Notes,
                    (Dim::pt(10), Dim::pt(80)),
                    (Dim::pt(180), Dim::pt(110)),
                )),
        )?;
    }
    let mut first = properties(&[
        ("columns.widths", json!("1,2")),
        ("columns.gap", json!(8192)),
    ]);
    if vertical {
        first.insert("writing-mode".into(), json!("vertical-rl"));
    }
    doc.write_section_properties(SectionId::First, "0", &first, &[])?;
    let a = doc.append_block(BlockKind::Paragraph, "band", "a\na\na\na\na\na\na\na")?;
    let b = doc.append_block(BlockKind::Paragraph, "band", "b\nb\nb\nb")?;
    let c = doc.append_block(BlockKind::Paragraph, "band", "c")?;
    for (node, count) in [(b, 2), (c, 1)] {
        let mut own = properties(&[
            ("start", json!("continuous")),
            ("columns.count", json!(count)),
            ("columns.rule", json!(1024)),
            ("numbering.restart", json!("-7")),
        ]);
        if vertical {
            own.insert("writing-mode".into(), json!("vertical-rl"));
        }
        let life = doc.mint_lifetime(node)?;
        doc.activate_start(node, &life, BreakKind::Section, &own)?;
    }
    if regions {
        use reprise_doc::relation::builtin;
        use reprise_doc::text::RangePolicy;
        use reprise_doc::{Param, Relation, Target};
        let schemas = reprise_doc::SchemaRegistry::builtin();
        for (anchor, count) in [(a, 12), (b, 8)] {
            let note = doc.append_block(
                BlockKind::Annotation,
                "band",
                &vec!["note"; count].join("\n"),
            )?;
            let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED)?;
            doc.add_relation(
                &schemas,
                &Relation::new(builtin::NOTE)
                    .owned_by(note)
                    .target("anchor", Target::Range(range))
                    .param("placement", Param::Text("page".into())),
            )?;
        }
        let float = doc.append_block(BlockKind::Annotation, "band", "float\nfloat")?;
        let range = doc.add_range(b, 0..1, RangePolicy::FIXED)?;
        doc.add_relation(
            &schemas,
            &Relation::new(builtin::FLOAT)
                .owned_by(float)
                .target("anchor", Target::Range(range))
                .param("side", Param::Text("right".into()))
                .param("width", Param::Length(LengthExpr::Pt(Length::from_pt(28)))),
        )?;
    }
    doc.commit();
    Ok(doc)
}
fn geometry() -> Result<Document> {
    let doc = document()?;
    doc.write_section_properties(
        SectionId::First,
        "0",
        &properties(&[
            ("width", json!(-1)),
            ("columns.count", json!(1000)),
            ("columns.gap", json!(0)),
            ("template", json!("missing")),
            ("numbering.restart", json!("-2")),
        ]),
        &[],
    )?;
    doc.append_block(BlockKind::Paragraph, "band", "cap")?;
    let b = doc.append_block(BlockKind::Paragraph, "band", "rtl")?;
    let c = doc.append_block(BlockKind::Paragraph, "band", "landscape")?;
    for (node, own) in [
        (
            b,
            properties(&[
                ("start", json!("odd-page")),
                ("columns.widths", json!("1,0,2")),
                ("direction", json!("rtl")),
                ("columns.rule", json!(1024)),
                ("numbering.restart", json!("7")),
            ]),
        ),
        (c, properties(&[("orientation", json!("landscape"))])),
    ] {
        let life = doc.mint_lifetime(node)?;
        doc.activate_start(node, &life, BreakKind::Section, &own)?;
    }
    doc.commit();
    Ok(doc)
}
fn spatial(oversized: bool) -> Result<Document> {
    use reprise_doc::Dim;
    let doc = styled_document()?;
    doc.set_page_template(
        &PageTemplate::new("spatial", Dim::pt(200), Dim::pt(200))
            .with_frame(FrameTemplate::new(
                "short",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(100), Dim::pt(80)),
            ))
            .with_frame(FrameTemplate::new(
                "tall",
                FrameRole::Flow("main".into()),
                (Dim::pt(100), Dim::pt(0)),
                (Dim::pt(100), Dim::pt(120)),
            )),
    )?;
    doc.write_section_properties(
        SectionId::First,
        "0",
        &properties(&[("mode", json!("paged")), ("bottom", json!(0))]),
        &[],
    )?;
    doc.append_block(BlockKind::Paragraph, "band", "a\na\na\na")?;
    let b = doc.append_block(BlockKind::Paragraph, "band", "b")?;
    let life = doc.mint_lifetime(b)?;
    doc.activate_start(
        b,
        &life,
        BreakKind::Section,
        &properties(&[("start", json!("continuous"))]),
    )?;
    if oversized {
        doc.set_overrides(
            b,
            &reprise_doc::Style {
                line_height: Some(LengthExpr::Pt(Length::from_pt(150))),
                ..Default::default()
            },
        )?;
        let c = doc.append_block(BlockKind::Paragraph, "band", "c")?;
        let life = doc.mint_lifetime(c)?;
        doc.activate_start(
            c,
            &life,
            BreakKind::Section,
            &properties(&[("start", json!("continuous"))]),
        )?;
    }
    doc.commit();
    Ok(doc)
}
/// A header with a page number, a footer, a page break and a second section
/// that hides the header: what the section queries and navigation are about.
fn regions() -> Result<Document> {
    use reprise_doc::{InlineObject, NewBlock};
    let doc = document()?;
    let head = doc.create_story(&[NewBlock::new(BlockKind::Paragraph, "band", "Head ")])?;
    let head_block = doc.top(reprise_doc::Domain::Story(head))[0];
    doc.insert_inline(head_block, 5, &InlineObject::field("page-number"))?;
    let foot = doc.create_story(&[NewBlock::new(BlockKind::Paragraph, "band", "Foot")])?;
    doc.write_section_properties(
        SectionId::First,
        "0",
        &properties(&[
            ("region.header.default", json!(head.to_string())),
            ("region.footer.default", json!(foot.to_string())),
            ("numbering.format", json!("lower-roman")),
        ]),
        &[],
    )?;
    let p: Vec<_> = (0..4)
        .map(|i| doc.append_block(BlockKind::Paragraph, "band", &format!("page {i}")))
        .collect::<std::result::Result<_, _>>()?;
    for (node, kind, own) in [
        (p[1], BreakKind::Page, properties(&[])),
        (
            p[2],
            BreakKind::Section,
            properties(&[
                ("region.header.default", json!("")),
                ("numbering.format", json!("decimal")),
                ("numbering.restart", json!(1)),
            ]),
        ),
        (p[3], BreakKind::Page, properties(&[])),
    ] {
        let life = doc.mint_lifetime(node)?;
        doc.activate_start(node, &life, kind, &own)?;
    }
    doc.commit();
    Ok(doc)
}
/// Every section query and placed navigation of a finished layout, as the JSON
/// the WASM facade must reproduce.
fn queries(session: &reprise::DocumentSession) -> Result<Value> {
    use reprise::Payload;
    let state = session.state()?.data;
    let mut pages = Vec::new();
    for page in 0.. {
        let Ok(label) = session.page_label(page) else {
            break;
        };
        pages.push(json!({
            "label": label.data,
            "sections": session.page_sections(page)?.data,
            "regions": session.page_regions(page)?.data,
            "fields": session.page_fields(page)?.data,
        }));
    }
    let mut section_pages = serde_json::Map::new();
    for section in &state.sections {
        section_pages.insert(
            section.id.clone(),
            json!(
                session
                    .section_pages(&Payload::new(section.id.clone()))?
                    .data
            ),
        );
    }
    let mut instances = serde_json::Map::new();
    for story in &state.stories {
        instances.insert(
            story.id.clone(),
            json!(
                session
                    .region_instances(&Payload::new(story.id.clone()))?
                    .data
            ),
        );
    }
    let mut navigation = Value::Null;
    if let Some(region) = session
        .page_regions(0)
        .ok()
        .and_then(|r| r.data.into_iter().next())
    {
        let hit = session
            .hit_placed(&Payload::new(reprise::HitPlaced {
                page: 0,
                x: region.rect.x + region.rect.width / 2,
                y: region.rect.y + region.rect.height / 2,
                mode: None,
            }))?
            .data;
        if let reprise::PlacedHit::Running { position, instance } = &hit {
            let anchor = reprise::Caret {
                offset: 0,
                ..position.clone()
            };
            navigation = json!({
                "hit": hit,
                "caret": session.caret_in(&Payload::new(reprise::CaretIn {
                    instance: instance.clone(),
                    position: position.clone(),
                }))?.data,
                "selection": session.selection_rects_in(&Payload::new(reprise::SelectionIn {
                    instance: instance.clone(),
                    selection: reprise::Selection { anchor, focus: position.clone() },
                }))?.data,
            });
        } else {
            navigation = json!({"hit": hit});
        }
    }
    Ok(
        json!({"pages": pages, "section_pages": section_pages, "instances": instances, "navigation": navigation}),
    )
}
fn main() -> Result<()> {
    let target =
        std::path::PathBuf::from(std::env::args().nth(1).ok_or("output directory required")?);
    std::fs::create_dir_all(&target)?;
    let mut cases = Vec::new();
    for (ordinal, (name, doc)) in [
        ("geometry", geometry()?),
        ("bands", bands(false, false)?),
        ("vertical-bands", bands(true, false)?),
        ("notes-floats", bands(false, true)?),
        ("spatial-bands", spatial(false)?),
        ("spatial-overflow", spatial(true)?),
        ("regions", regions()?),
    ]
    .into_iter()
    .enumerate()
    {
        let bytes = reprise_format::Package::new(
            &doc,
            reprise_format::DocumentId([ordinal as u8; 16]),
            reprise_doc::PersistenceMode::History,
        )?
        .save()?;
        std::fs::write(target.join(format!("{name}.rpr")), &bytes)?;
        let mut session = reprise::Workspace::new().open(
            &reprise::Payload::new(reprise::Open {
                peer_id: "2".into(),
            }),
            &bytes,
        )?;
        let options = reprise::LayoutOptions {
            viewport_end: 1000,
            ..Default::default()
        };
        let mut job = session.start_layout(&reprise::Payload::new(options.clone()))?;
        let mut pages = None;
        for unit in 0..100_000 {
            if unit % 7 == 0 {
                assert_eq!(job.step(&mut session, 0)?.data.used, 0);
            }
            let progress = job.step(&mut session, 1)?.data;
            if progress.complete {
                pages = Some(progress.pages);
                break;
            }
        }
        let display = pages
            .ok_or("bounded native parity job did not finish")?
            .into_iter()
            .map(|page| session.display_json(page).map(|d| d.data))
            .collect::<reprise::Result<Vec<_>>>()?;
        cases.push(json!({"name": name, "options": options, "display": display,
            "diagnostics": session.diagnostics().data, "sections": session.state()?.data.sections,
            "queries": queries(&session)?}));
    }
    std::fs::write(
        target.join("native.json"),
        serde_json::to_vec_pretty(&cases)?,
    )?;
    println!(
        "sections native parity: {} bounded cases generated",
        cases.len()
    );
    Ok(())
}
