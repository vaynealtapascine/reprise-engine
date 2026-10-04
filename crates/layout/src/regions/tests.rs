use super::*;
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    Dim, FrameRole, FrameTemplate, LengthExpr, PageTemplate, Param, SchemaRegistry, Style, Target,
};
use reprise_font::{Face, FontStore};

fn setup(depth: Length) -> (Engine, Document) {
    let mut fonts = FontStore::default();
    fonts.add(
        Face::from_bytes(
            include_bytes!("../../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
        )
        .unwrap(),
    );
    let engine = Engine::new(fonts);
    let doc = Document::new(1).unwrap();
    doc.define_style(
        "s",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(10))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(12))),
            ..Default::default()
        },
    )
    .unwrap();
    doc.set_page_template(
        &PageTemplate::new("r", Dim::pt(100), Dim::pt(60))
            .with_frame(FrameTemplate::new(
                "body",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(100), Dim::pt(60)),
            ))
            .with_frame(FrameTemplate::new(
                "notes",
                FrameRole::Notes,
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(100), Dim::Pt(depth)),
            )),
    )
    .unwrap();
    (engine, doc)
}

fn relation(doc: &Document, owner: NodeId, anchor: NodeId) -> Relation {
    let range = doc.add_range(anchor, 1..2, RangePolicy::FIXED).unwrap();
    Relation::new(builtin::NOTE)
        .owned_by(owner)
        .target("anchor", Target::Range(range))
}

#[test]
fn oscillation_freezes_geometry_that_actually_composed_the_body() {
    let (engine, doc) = setup(Length::from_pt(60));
    let body = doc
        .append_block(BlockKind::Paragraph, "s", "anchor")
        .unwrap();
    let float = doc
        .append_block(BlockKind::Annotation, "s", "float")
        .unwrap();
    let r = relation(&doc, float, body);
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::FLOAT)
            .owned_by(float)
            .target("anchor", r.first("anchor").unwrap().clone())
            .param("width", Param::Length(LengthExpr::Pt(Length::from_pt(100))))
            .param("margin", Param::Length(LengthExpr::Pt(Length::ZERO))),
    )
    .unwrap();
    let s = engine.layout(&doc);
    assert_eq!(s.diagnostics_with("layout.region-cycle").count(), 1);
    let b = &s.block(body).unwrap().lines[0];
    let f = &s.block(float).unwrap().lines[0];
    assert!(
        b.rect.origin.y + b.rect.height <= f.rect.origin.y
            || f.rect.origin.y + f.rect.height <= b.rect.origin.y
    );
}

#[test]
fn reverse_order_dependencies_and_actual_depth_are_both_bounded() {
    let (engine, doc) = setup(Length::from_pt(60));
    let body = doc
        .append_block(BlockKind::Paragraph, "s", "anchor")
        .unwrap();
    let nodes: Vec<_> = (0..40)
        .map(|_| {
            doc.append_block(BlockKind::Annotation, "s", "nested note")
                .unwrap()
        })
        .collect();
    let mut rs = Vec::new();
    let mut anchor = body;
    for node in &nodes {
        rs.push(relation(&doc, *node, anchor));
        anchor = *node;
    }
    for r in rs.into_iter().rev() {
        doc.add_relation(&engine.schemas, &r).unwrap();
    }
    let s = engine.layout(&doc);
    assert_eq!(
        s.blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Annotation)
            .count(),
        MAX_NOTE_DEPTH
    );
    assert_eq!(s.diagnostics_with("layout.note-depth").count(), 8);
    assert!(s.pages.len() <= engine.flow.max_pages as usize);
}

#[test]
fn dependency_cycles_missing_frames_and_extreme_notes_are_partial() {
    for depth in [Length::MIN, Length::ZERO, Length(1), Length::MAX] {
        let (mut engine, doc) = setup(depth);
        engine.flow.max_pages = 3;
        let body = doc
            .append_block(BlockKind::Paragraph, "s", "anchor")
            .unwrap();
        let note = doc
            .append_block(
                BlockKind::Annotation,
                "s",
                "aa\nbb\ncc\ndd\nee\nff\ngg\nhh\nii",
            )
            .unwrap();
        doc.add_relation(&engine.schemas, &relation(&doc, note, body))
            .unwrap();
        let s = engine.layout(&doc);
        assert!(s.block(body).is_some());
        assert!(s.pages.len() <= 3);
        assert_eq!(s, engine.layout(&doc));
        if depth <= Length::ZERO {
            assert!(s.diagnostics_with("relation.no-frame").next().is_some());
        }
    }
    let (engine, doc) = setup(Length::from_pt(60));
    let a = doc.append_block(BlockKind::Annotation, "s", "aa").unwrap();
    let b = doc.append_block(BlockKind::Annotation, "s", "bb").unwrap();
    for r in [relation(&doc, a, b), relation(&doc, b, a)] {
        doc.add_relation(&engine.schemas, &r).unwrap();
    }
    assert_eq!(
        engine
            .layout(&doc)
            .diagnostics_with("layout.note-depth")
            .count(),
        2
    );
}

#[test]
fn note_page_limit_reports_exact_unplaced_bytes_and_endnotes_follow_body() {
    let (mut engine, doc) = setup(Length::from_pt(60));
    engine.flow.max_pages = 1;
    let body = doc
        .append_block(BlockKind::Paragraph, "s", "anchor")
        .unwrap();
    let note = doc
        .append_block(BlockKind::Annotation, "s", "aa\nbb\ncc\ndd\nee\nff")
        .unwrap();
    doc.add_relation(&engine.schemas, &relation(&doc, note, body))
        .unwrap();
    let s = engine.layout(&doc);
    let placed = s.block(note).unwrap().lines.last().unwrap().text.end;
    let error = s
        .diagnostics_with("layout.text-unplaced")
        .find(|d| d.subject == Subject::Node(note))
        .unwrap();
    assert_eq!(error.bytes.as_ref().unwrap().start, placed);
    assert_eq!(s.pages.len(), 1);
    let (engine, doc) = setup(Length::from_pt(60));
    let body = doc
        .append_block(BlockKind::Paragraph, "s", "aa\nbb\ncc\ndd\nee\nff")
        .unwrap();
    let note = doc
        .append_block(BlockKind::Annotation, "s", "endnote")
        .unwrap();
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &relation(&doc, note, body).param("placement", Param::Text("end".into())),
    )
    .unwrap();
    let s = engine.layout(&doc);
    assert_eq!(
        s.frame(s.block(note).unwrap().lines[0].frame).unwrap().page,
        2
    );
}

#[test]
fn deferred_floats_stack_and_invalid_parameters_omit_only_the_owner() {
    let (engine, doc) = setup(Length::from_pt(60));
    let body = doc
        .append_block(BlockKind::Paragraph, "s", "aa\nbb\ncc\ndd\nee")
        .unwrap();
    let range = doc.add_range(body, 13..14, RangePolicy::FIXED).unwrap();
    let mut owners = Vec::new();
    for side in ["left", "right", "not-a-side"] {
        let n = doc
            .append_block(BlockKind::Annotation, "s", "aa\nbb")
            .unwrap();
        owners.push(n);
        doc.add_relation(
            &engine.schemas,
            &Relation::new(builtin::FLOAT)
                .owned_by(n)
                .target("anchor", Target::Range(range))
                .param("side", Param::Text(side.into()))
                .param("margin", Param::Length(LengthExpr::Pt(Length::ZERO))),
        )
        .unwrap();
    }
    let s = engine.layout(&doc);
    let left = s.block(owners[0]).unwrap();
    let right = s.block(owners[1]).unwrap();
    assert_eq!(s.frame(left.lines[0].frame).unwrap().page, 1);
    assert_eq!(s.frame(right.lines[0].frame).unwrap().page, 1);
    assert!(
        left.lines.last().unwrap().rect.origin.y + Length::from_pt(12)
            <= right.lines[0].rect.origin.y
    );
    assert!(s.block(owners[2]).is_none());
    assert_eq!(s.diagnostics_with("layout.float-deferred").count(), 2);
    assert_eq!(s.diagnostics_with("layout.region-parameter").count(), 1);
}

#[test]
fn mixed_notes_floats_and_tables_share_space_without_overlapping() {
    let (engine, doc) = setup(Length::from_pt(60));
    let body = doc
        .append_block(BlockKind::Paragraph, "s", "anchor text")
        .unwrap();
    let note = doc
        .append_block(BlockKind::Annotation, "s", "aa\nbb\ncc\ndd\nee\nff")
        .unwrap();
    // Deliberately declare the note first: allocator class order must still
    // preserve the float's box when sizing the notes area.
    doc.add_relation(&engine.schemas, &relation(&doc, note, body))
        .unwrap();
    let float = doc
        .append_block(BlockKind::Annotation, "s", "aa\nbb")
        .unwrap();
    let r = relation(&doc, float, body);
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::FLOAT)
            .owned_by(float)
            .target("anchor", r.first("anchor").unwrap().clone())
            .param("margin", Param::Length(LengthExpr::Pt(Length::ZERO))),
    )
    .unwrap();
    let table = doc
        .append_table(reprise_doc::TableColumns {
            columns: vec![reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Proportional(1),
            }],
        })
        .unwrap();
    let row = doc.append_table_row(table, false).unwrap();
    let cell = doc.append_table_cell(row, 0).unwrap();
    let text = doc
        .append_cell_block(cell, BlockKind::Paragraph, "s", "cell text")
        .unwrap();
    let s = engine.layout(&doc);
    assert!(s.block(text).is_some());
    let float_last = s.block(float).unwrap().lines.last().unwrap();
    let float_page = s.frame(float_last.frame).unwrap().page;
    for l in &s.block(note).unwrap().lines {
        if s.frame(l.frame).unwrap().page == float_page {
            assert!(l.rect.origin.y >= float_last.rect.origin.y + float_last.rect.height);
        }
    }
    for l in &s.block(text).unwrap().lines {
        if s.frame(l.frame).unwrap().page == float_page {
            assert!(l.rect.origin.y >= float_last.rect.origin.y + float_last.rect.height);
        }
    }
}

#[test]
fn content_columns_and_uneven_cell_continuations_keep_every_byte_once() {
    let (engine, doc) = setup(Length::from_pt(60));
    let table = doc
        .append_table(reprise_doc::TableColumns {
            columns: vec![
                reprise_doc::Column {
                    width: reprise_doc::ColumnWidth::Fixed(Length::from_pt(20)),
                },
                reprise_doc::Column {
                    width: reprise_doc::ColumnWidth::Content,
                },
                reprise_doc::Column {
                    width: reprise_doc::ColumnWidth::Proportional(1),
                },
            ],
        })
        .unwrap();
    let row = doc.append_table_row(table, true).unwrap();
    let mut nodes = Vec::new();
    for (column, texts) in [
        (0, vec!["a"]),
        (1, vec!["aa bb\ncc dd\nee ff\ngg hh\nii jj\nkk ll"]),
        (2, vec!["first", "second"]),
    ] {
        let cell = doc.append_table_cell(row, column).unwrap();
        for text in texts {
            nodes.push(
                doc.append_cell_block(cell, BlockKind::Paragraph, "s", text)
                    .unwrap(),
            );
        }
    }
    let s = engine.layout(&doc);
    assert!(s.pages.len() > 1);
    for n in nodes {
        let b = s.block(n).unwrap();
        let mut at = 0;
        for l in &b.lines {
            assert_eq!(l.text.start, at);
            at = l.text.end;
        }
        assert_eq!(at, b.text.len());
    }
    assert!(
        s.diagnostics_with("layout.solver-infeasible")
            .next()
            .is_none()
    );
}

#[test]
fn table_notes_move_anchors_to_later_pages_and_zero_budget_terminates() {
    let (mut engine, doc) = setup(Length::from_pt(60));
    engine.flow.max_pages = 5;
    let table = doc
        .append_table(reprise_doc::TableColumns {
            columns: vec![reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Proportional(1),
            }],
        })
        .unwrap();
    let row = doc.append_table_row(table, false).unwrap();
    let cell = doc.append_table_cell(row, 0).unwrap();
    let text = doc
        .append_cell_block(
            cell,
            BlockKind::Paragraph,
            "s",
            "aa\nbb\ncc\ndd\nee\nff\ngg",
        )
        .unwrap();
    let note = doc
        .append_block(BlockKind::Annotation, "s", "note")
        .unwrap();
    let range = doc.add_range(text, 16..17, RangePolicy::FIXED).unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::NOTE)
            .owned_by(note)
            .target("anchor", Target::Range(range)),
    )
    .unwrap();
    let s = engine.layout(&doc);
    assert!(s.frame(s.block(note).unwrap().lines[0].frame).unwrap().page > 0);
    let table2 = doc
        .append_table(reprise_doc::TableColumns {
            columns: vec![reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Fixed(Length::ZERO),
            }],
        })
        .unwrap();
    let row2 = doc.append_table_row(table2, false).unwrap();
    let cell2 = doc.append_table_cell(row2, 0).unwrap();
    let text2 = doc
        .append_cell_block(cell2, BlockKind::Paragraph, "s", "unbreakable")
        .unwrap();
    let s = engine.layout(&doc);
    assert!(s.block(text2).is_some());
    assert!(s.diagnostics_with("compose.overflow").next().is_some());
    assert!(s.pages.len() <= 5);
}

#[test]
fn a_second_note_follows_its_reflowed_body_anchor() {
    let (engine, doc) = setup(Length::from_pt(60));
    let body = doc
        .append_block(BlockKind::Paragraph, "s", "aa\nbb\ncc\ndd\nee")
        .unwrap();
    let first = doc
        .append_block(BlockKind::Annotation, "s", "aa\nbb\ncc\ndd\nee\nff\ngg")
        .unwrap();
    doc.add_relation(&engine.schemas, &relation(&doc, first, body))
        .unwrap();
    let second = doc
        .append_block(BlockKind::Annotation, "s", "second note")
        .unwrap();
    let range = doc.add_range(body, 10..11, RangePolicy::FIXED).unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::NOTE)
            .owned_by(second)
            .target("anchor", Target::Range(range)),
    )
    .unwrap();
    let s = engine.layout(&doc);
    let anchor = s.line_containing(body, 10).unwrap();
    let anchor_page = s.page_of(anchor).unwrap();
    assert!(anchor_page > 0);
    let note_page = s
        .frame(s.block(second).unwrap().lines[0].frame)
        .unwrap()
        .page;
    assert!(note_page >= anchor_page);
    assert!(s.diagnostics_with("layout.region-cycle").next().is_none());
    assert!(s.diagnostics_with("layout.region-limit").next().is_none());
}

#[test]
fn large_region_fanout_and_table_domains_have_explicit_limits() {
    let (mut engine, doc) = setup(Length::from_pt(60));
    engine.flow.max_pages = 1;
    let body = doc
        .append_block(BlockKind::Paragraph, "s", "anchor")
        .unwrap();
    let range = doc.add_range(body, 1..2, RangePolicy::FIXED).unwrap();
    // Shared owner keeps this adversarial graph small; size is bounded before
    // duplicate-owner validation, so the size diagnostic is still required.
    let owner = doc
        .append_block(BlockKind::Annotation, "s", "note")
        .unwrap();
    for _ in 0..MAX_REGION_RELATIONS + 1 {
        doc.add_relation(
            &engine.schemas,
            &Relation::new(builtin::NOTE)
                .owned_by(owner)
                .target("anchor", Target::Range(range)),
        )
        .unwrap();
    }
    let s = engine.layout(&doc);
    assert!(
        s.diagnostics_with("layout.region-limit")
            .any(|d| d.severity == Severity::Error)
    );
    assert!(s.pages.len() <= 1);
    let (engine, doc) = setup(Length::from_pt(60));
    doc.append_table(reprise_doc::TableColumns {
        columns: vec![
            reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Content
            };
            10000
        ],
    })
    .unwrap();
    let rest = doc.append_block(BlockKind::Paragraph, "s", "rest").unwrap();
    let s = engine.layout(&doc);
    assert!(s.diagnostics_with("layout.table-invalid").next().is_some());
    assert!(s.block(rest).is_some());
}

#[test]
fn extreme_float_padding_keeps_the_entire_frame_excluded() {
    let (mut engine, doc) = setup(Length::from_pt(60));
    engine.flow.max_pages = 3;
    let body = doc
        .append_block(BlockKind::Paragraph, "s", "anchor")
        .unwrap();
    let float = doc
        .append_block(BlockKind::Annotation, "s", "float")
        .unwrap();
    let r = relation(&doc, float, body);
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::FLOAT)
            .owned_by(float)
            .target("anchor", r.first("anchor").unwrap().clone())
            .param("margin", Param::Length(LengthExpr::Pt(Length::MAX))),
    )
    .unwrap();
    let mut base = engine.layout(&doc);
    let template = crate::template::resolve(&engine, &doc, &mut Vec::new());
    base.blocks.retain(|b| b.node == body);
    let plan = super::allocate(&engine, &doc, &template, &base);
    let excluded = plan.exclusions.values().next().unwrap().first().unwrap();
    assert_eq!(excluded.origin, reprise_geom::Point::origin());
    assert_eq!(excluded.width, Length::from_pt(100));
    assert_eq!(excluded.height, Length::from_pt(60));
}
