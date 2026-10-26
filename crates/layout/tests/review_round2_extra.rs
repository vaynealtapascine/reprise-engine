include!("review_b3_fixes.rs");

fn finish_round2(
    session: &mut LayoutSession<'_>,
    doc: &Document,
    mut state: reprise_layout::incremental::LayoutContinuation,
) -> LayoutSnapshot {
    for _ in 0..100_000 {
        let mut job = session
            .resume(doc, state)
            .expect("current continuation must survive");
        let step = job.step(1).unwrap();
        assert_eq!(job.counters().running_inline, 0);
        if step.complete {
            return job.complete().unwrap().unwrap();
        }
        state = job.suspend();
    }
    panic!("did not finish");
}

#[test]
fn round2_old_revision_refusal_must_not_destroy_newer_suspended_job() {
    let (engine, doc) = endless_note();
    let mut session = LayoutSession::new(&engine);
    let old = suspended_in_feedback(&mut session, &doc, 2);
    let body = doc.top(Domain::Body)[0];
    doc.block(body).unwrap().text.insert(0, "edit ").unwrap();
    doc.commit();
    let newer = suspended_in_feedback(&mut session, &doc, 3);
    assert!(matches!(session.resume(&doc, old), Err(JobError::Stale)));
    assert_eq!(
        finish_round2(&mut session, &doc, newer),
        engine.layout(&doc)
    );
}

#[test]
fn round2_multiple_replacements_at_region_and_note_checkpoints() {
    let (engine, doc) = endless_note();
    let expected = engine.layout(&doc);
    for checkpoint in [1, 10, 40] {
        let mut session = LayoutSession::new(&engine);
        let old = suspended_in_feedback(&mut session, &doc, checkpoint);
        let early = session
            .start(&doc, Viewport::Pages(0..usize::MAX))
            .suspend();
        let mut cancelled = session.start(&doc, Viewport::Pages(0..usize::MAX));
        cancelled.step(1).unwrap();
        cancelled.cancel();
        let cancelled = cancelled.suspend();
        let mut newest = session.start(&doc, Viewport::Pages(0..usize::MAX));
        newest.step(1).unwrap();
        let newest = newest.suspend();
        for obsolete in [early, old, cancelled] {
            assert!(matches!(
                session.resume(&doc, obsolete),
                Err(JobError::Stale)
            ));
        }
        assert_eq!(finish_round2(&mut session, &doc, newest), expected);
    }
}
#[test]
fn round2_changed_engine_with_fresh_cache_rejects_old_continuation() {
    let (mut engine, doc) = endless_note();
    let mut session = LayoutSession::new(&engine);
    let old = suspended_in_feedback(&mut session, &doc, 2);
    drop(session);
    engine.flow.max_pages = 3;
    engine.fonts = reprise_font::FontStore::default();
    let mut session = LayoutSession::new(&engine);
    assert!(matches!(session.resume(&doc, old), Err(JobError::Stale)));
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
}

#[test]
fn round2_lookahead_relation_owner_is_part_of_digest_validity() {
    let (engine, doc, _, nodes) =
        kept_header(&[("head".into(), true), ("word ".repeat(1000), true)]);
    let mut session = LayoutSession::new(&engine);
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
    let anchor = doc.top(Domain::Body)[0];
    let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
    let relation = doc
        .add_relation(
            &engine.schemas,
            &Relation::new(builtin::NOTE)
                .owned_by(nodes[1])
                .target("anchor", Target::Range(range))
                .param("placement", Param::Text("page".into())),
        )
        .unwrap();
    doc.commit();
    let cached = session.layout(&doc).unwrap();
    let full = engine.layout(&doc);
    eprintln!(
        "cached keep={:?}, full keep={:?}, counters={:?}",
        keep_unsatisfied(&cached),
        keep_unsatisfied(&full),
        session.counters()
    );
    assert!(
        cached == full,
        "relation ownership read only by look-ahead is absent from digest"
    );
    doc.delete_relation(relation).unwrap();
    doc.commit();
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
}

#[test]
fn round2_admitted_lookahead_text_styles_and_fields_invalidate() {
    let (engine, doc, _, nodes) =
        kept_header(&[("head".into(), true), ("word ".repeat(1000), false)]);
    let mut session = LayoutSession::new(&engine);
    assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
    for edit in 0..5 {
        match edit {
            0 => {
                doc.block(nodes[1])
                    .unwrap()
                    .text
                    .insert(0, "more text ")
                    .unwrap();
            }
            1 => {
                doc.set_overrides(
                    nodes[1],
                    &Style {
                        keep_together: Some(true),
                        orphans: Some(5),
                        widows: Some(5),
                        ..Default::default()
                    },
                )
                .unwrap();
            }
            2 => {
                doc.set_overrides(
                    nodes[1],
                    &Style {
                        keep_with_next: Some(true),
                        ..Default::default()
                    },
                )
                .unwrap();
            }
            3 => {
                doc.insert_inline(nodes[1], 0, &InlineObject::field("page-count"))
                    .unwrap();
            }
            _ => {
                doc.define_style(
                    "s",
                    &Style {
                        size: Some(reprise_doc::LengthExpr::Pt(reprise_geom::Length::from_pt(
                            13,
                        ))),
                        ..Default::default()
                    },
                )
                .unwrap();
            }
        }
        doc.commit();
        assert_eq!(
            session.layout(&doc).unwrap(),
            engine.layout(&doc),
            "edit {edit}"
        );
    }
}

#[test]
fn round2_decode_at_the_exact_host_boundary_and_above() {
    const MAX: usize = (1 << 20) + 3 * 4096;
    for bytes in [MAX - 1, MAX, MAX + 1] {
        let (engine, doc) = setup();
        let story = doc
            .create_story(&[
                NewBlock::new(BlockKind::Paragraph, "s", "head"),
                NewBlock::new(BlockKind::Paragraph, "s", &"z".repeat(bytes)),
            ])
            .unwrap();
        bind_header(&doc, story);
        block(&doc, "body");
        doc.commit();
        let mut session = LayoutSession::new(&engine);
        let mut job = session.start(&doc, Viewport::Pages(0..usize::MAX));
        let mut large_decodes = 0;
        for _ in 0..100 {
            let step = job.step(1).unwrap();
            let w = job.step_work();
            if w.digest_decoded > 100 {
                large_decodes += 1;
                assert!(bytes <= MAX);
                assert_eq!(w.digest_decoded, bytes);
                assert_eq!((step.used, w.units, w.region_units), (5, 5, 5));
                assert_eq!(
                    (
                        w.digest_blocks,
                        w.digest_inspected,
                        w.composer_calls,
                        w.adapter_requests,
                        w.pages_made
                    ),
                    (0, 0, 0, 0, 0)
                );
            } else {
                assert_eq!(step.used, 1);
            }
            if step.complete {
                break;
            }
        }
        assert_eq!(large_decodes, usize::from(bytes <= MAX));
        assert_eq!(job.complete().unwrap().unwrap(), engine.layout(&doc));
    }
}

#[test]
fn round2_table_beyond_digest_cut_is_not_read_by_keeps() {
    let (engine, doc) = setup();
    let story = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "s", "head")])
        .unwrap();
    let head = doc.top(Domain::Story(story))[0];
    doc.set_overrides(
        head,
        &Style {
            keep_with_next: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let table = doc
        .append_table(two_columns([
            reprise_doc::ColumnWidth::Content,
            reprise_doc::ColumnWidth::Content,
        ]))
        .unwrap();
    let row = doc.append_table_row(table, false).unwrap();
    let cell = doc.append_table_cell(row, 0).unwrap();
    let tail = doc
        .append_cell_block(cell, BlockKind::Paragraph, "s", &"z".repeat(1_100_000))
        .unwrap();
    doc.move_block(table, Some(story), 1).unwrap();
    bind_header(&doc, story);
    block(&doc, "body");
    doc.commit();
    let mut session = LayoutSession::new(&engine);
    let first = session.layout(&doc).unwrap();
    assert_eq!(first, engine.layout(&doc));
    assert!(keep_unsatisfied(&first).is_empty());
    for edit in 0..2 {
        if edit == 0 {
            doc.set_overrides(
                tail,
                &Style {
                    keep_together: Some(true),
                    orphans: Some(8),
                    widows: Some(8),
                    ..Default::default()
                },
            )
            .unwrap();
        } else {
            doc.set_table_row_header(row, true).unwrap();
        }
        doc.commit();
        assert_eq!(session.layout(&doc).unwrap(), engine.layout(&doc));
    }
    let (snapshot, works) = stepped(&engine, &doc, 1);
    assert_eq!(snapshot, engine.layout(&doc));
    assert!(works.iter().all(|w| w.digest_decoded < 1_100_000
        && w.shaped_bytes < 1_100_000
        && w.composer_calls <= 1));
}
