include!("review_b3_fixes.rs");

// Independent add/remove matrix: both memo paths, every block kind, all
// relation kinds consulted by flow classification, and overlapping owners.
#[test]
fn round3_relation_matrix_cached_equals_full() {
    for running in [false, true] {
        for kind in ["paragraph", "annotation", "image", "table"] {
            for schema in [builtin::NOTE, builtin::FLOAT, builtin::FOLLOW] {
                let (mut engine, doc) = setup();
                doc.set_page_template(
                    &PageTemplate::new("p", Dim::pt(200), Dim::pt(120))
                        .with_frame(frame("main", 0, 0, 200, 120)),
                )
                .unwrap();
                let anchor = block(&doc, &["fill"; 8].join("\n"));
                let head = block(&doc, "head");
                let middle = match kind {
                    "paragraph" => block(&doc, "middle"),
                    "annotation" => doc
                        .append_block(BlockKind::Annotation, "s", "annotation")
                        .unwrap(),
                    "image" => plain_image(&mut engine, &doc),
                    _ => {
                        let t = doc
                            .append_table(two_columns([ColumnWidth::Content, ColumnWidth::Content]))
                            .unwrap();
                        let row = doc.append_table_row(t, false).unwrap();
                        let cell = doc.append_table_cell(row, 0).unwrap();
                        doc.append_cell_block(cell, BlockKind::Paragraph, "s", "cell")
                            .unwrap();
                        t
                    }
                };
                let tail = block(&doc, &"word ".repeat(1000));
                for n in [head, middle, tail] {
                    doc.set_overrides(
                        n,
                        &Style {
                            keep_with_next: Some(true),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                }
                if running {
                    let story = doc.create_story(&[]).unwrap();
                    for (i, n) in [head, middle, tail].into_iter().enumerate() {
                        doc.move_block(n, Some(story), i).unwrap();
                    }
                    bind_header(&doc, story);
                }
                doc.commit();
                let mut session = LayoutSession::new(&engine);
                let label = format!("running={running}, kind={kind}, schema={schema:?}");
                laid_out_as_a_full_layout_is(&engine, &doc, &mut session, &label);
                let relation = owned_by(&engine, &doc, schema.clone(), middle, anchor);
                doc.commit();
                laid_out_as_a_full_layout_is(&engine, &doc, &mut session, &label);
                // The unsupported bit may remain true while the keep Link changes.
                if kind == "image" {
                    let second_schema = if schema == builtin::FOLLOW {
                        builtin::NOTE
                    } else {
                        builtin::FOLLOW
                    };
                    let second = owned_by(&engine, &doc, second_schema, middle, anchor);
                    doc.commit();
                    laid_out_as_a_full_layout_is(&engine, &doc, &mut session, &label);
                    doc.delete_relation(relation).unwrap();
                    doc.commit();
                    laid_out_as_a_full_layout_is(&engine, &doc, &mut session, &label);
                    doc.delete_relation(second).unwrap();
                } else {
                    doc.delete_relation(relation).unwrap();
                }
                doc.commit();
                laid_out_as_a_full_layout_is(&engine, &doc, &mut session, &label);
                eprintln!("passed {label}");
            }
        }
    }
}

#[test]
fn round3_running_table_annotation_relation_diagnostics() {
    let mut mismatches = 0;
    for schema in [builtin::NOTE, builtin::FLOAT] {
        for initially_owned in [false, true] {
            let (engine, doc) = setup();
            let anchor = block(&doc, "body");
            let story = doc.create_story(&[]).unwrap();
            let table = doc
                .append_table(two_columns([ColumnWidth::Content, ColumnWidth::Content]))
                .unwrap();
            let row = doc.append_table_row(table, false).unwrap();
            let cell = doc.append_table_cell(row, 0).unwrap();
            doc.append_cell_block(cell, BlockKind::Paragraph, "s", "cell")
                .unwrap();
            let annotation = doc
                .append_cell_block(
                    cell,
                    BlockKind::Annotation,
                    "missing-review-style",
                    "annotation",
                )
                .unwrap();
            doc.move_block(table, Some(story), 0).unwrap();
            bind_header(&doc, story);
            let mut relation = initially_owned
                .then(|| owned_by(&engine, &doc, schema.clone(), annotation, anchor));
            doc.commit();
            let mut session = LayoutSession::new(&engine);
            assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
            for _ in 0..2 {
                if let Some(id) = relation.take() {
                    doc.delete_relation(id).unwrap();
                } else {
                    relation = Some(owned_by(&engine, &doc, schema.clone(), annotation, anchor));
                }
                doc.commit();
                let cached = session.layout(&doc).unwrap();
                let full = engine.layout(&doc);
                eprintln!(
                    "table annotation schema={schema:?} initially_owned={initially_owned} now_owned={} equal={} cached={:?} full={:?} counters={:?}",
                    relation.is_some(),
                    cached == full,
                    cached.diagnostics,
                    full.diagnostics,
                    session.counters()
                );
                mismatches += usize::from(cached != full);
            }
        }
    }
    assert_eq!(
        mismatches, 0,
        "cached/full mismatches across the eight add/remove transitions"
    );
}
