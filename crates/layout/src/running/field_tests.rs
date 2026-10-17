//! S8a: fields in running regions (sections §10). A running instance is its
//! own placement of its story, so a field in it resolves for its page and its
//! slot; a page number reads its page's label directly where the page is known;
//! a `page-of` that cannot find its target says so with `?`.
use super::layout_tests::*;
use crate::incremental::LayoutSession;
use crate::{AtomLayout, BlockLayout, FieldInstance, LayoutSnapshot, RunningInstance, SlotId};
use reprise_doc::sections::BreakKind;
use reprise_doc::{BlockKind, Document, Domain, InlineObject, NewBlock, NodeId};

pub(super) enum P<'a> {
    T(&'a str),
    F(&'a str),
}

/// Puts `parts` at the end of `node`, a paragraph of the body or of a story.
pub(super) fn fill(doc: &Document, node: NodeId, parts: &[P<'_>]) {
    let mut text = String::new();
    let mut objects = Vec::new();
    for part in parts {
        match part {
            P::T(t) => text.push_str(t),
            P::F(f) => {
                text.push(reprise_text::OBJECT);
                objects.push(InlineObject::field(f));
            }
        }
    }
    let end = doc.block(node).unwrap().text.len();
    doc.insert_with_inline(node, end, &text, &objects).unwrap();
}

/// A story of one paragraph made of `parts`.
pub(super) fn story_of(doc: &Document, parts: &[P<'_>]) -> NodeId {
    let story = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "s", "")])
        .unwrap();
    fill(doc, doc.top(Domain::Story(story))[0], parts);
    story
}

pub(super) fn atoms(blocks: &[BlockLayout]) -> Vec<&AtomLayout> {
    blocks
        .iter()
        .flat_map(|b| &b.lines)
        .flat_map(|l| &l.atoms)
        .collect()
}

/// The texts of the fields of `instance`, in order.
pub(super) fn shown(instance: &RunningInstance) -> Vec<String> {
    atoms(&instance.blocks).iter().map(|a| a.text()).collect()
}

pub(super) fn on<'a>(s: &'a LayoutSnapshot, page: usize, slot: &str) -> &'a RunningInstance {
    s.sections
        .as_ref()
        .unwrap()
        .running
        .iter()
        .find(|r| r.page == page && r.slot == slot)
        .unwrap_or_else(|| panic!("no {slot} on page {page}"))
}

#[test]
fn a_footer_shows_each_pages_own_label_and_the_total() {
    let (engine, doc) = setup();
    let foot = story_of(
        &doc,
        &[
            P::T("Page "),
            P::F("page-number"),
            P::T(" of "),
            P::F("page-count"),
        ],
    );
    first(
        &doc,
        &[
            ("region.footer.default", bind(foot)),
            ("numbering.restart", serde_json::json!("7")),
        ],
    );
    pages(&doc, 3);
    doc.commit();
    let s = engine.layout(&doc);
    assert_eq!(s.pages.len(), 3);
    assert!(
        s.diagnostics_with("layout.field-unsettled")
            .next()
            .is_none(),
        "{:?}",
        s.diagnostics
    );
    for (page, label) in ["7", "8", "9"].into_iter().enumerate() {
        let footer = on(&s, page, "footer");
        // The label, not the physical number; the total is physical.
        assert_eq!(shown(footer), [label, "3"], "page {page}");
        // Each page's copy is its own placement, in the footer slot.
        for atom in atoms(&footer.blocks) {
            assert_eq!(
                atom.instance,
                FieldInstance::Running {
                    page: page as u32,
                    slot: SlotId::parse("footer").unwrap(),
                    repeat: 0
                }
            );
        }
    }
    // Full layout and the incremental session agree.
    let mut session = LayoutSession::new(&engine);
    assert_eq!(session.layout(&doc).unwrap(), s);
}

#[test]
fn a_body_page_number_uses_the_label_too() {
    let (engine, doc) = setup();
    first(&doc, &[("numbering.restart", serde_json::json!("10"))]);
    let a = body(&doc, "");
    fill(&doc, a, &[P::T("on page "), P::F("page-number")]);
    let b = body(&doc, "");
    fill(&doc, b, &[P::T("then "), P::F("page-number")]);
    boundary(&doc, b, BreakKind::Page, &[]);
    doc.commit();
    let s = engine.layout(&doc);
    let text = |node| {
        s.block(node)
            .unwrap()
            .lines
            .iter()
            .flat_map(|l| &l.atoms)
            .map(|a| a.text())
            .collect::<Vec<_>>()
    };
    assert_eq!(text(a), ["10"]);
    assert_eq!(text(b), ["11"]);
}

#[test]
fn page_count_settles_across_the_digit_boundary() {
    // Nine pages of body: "of 9" is short. A header that wraps at two digits
    // takes room from the body, which can add a page, which changes the count.
    // Whatever it settles on, the header says what the layout has.
    let (engine, doc) = setup();
    let head = story_of(&doc, &[P::T("a header that is "), P::F("page-count")]);
    first(&doc, &[("region.header.default", bind(head))]);
    pages(&doc, 9);
    doc.commit();
    let s = engine.layout(&doc);
    assert!(
        s.diagnostics_with("layout.field-unsettled")
            .next()
            .is_none()
    );
    let total = s.pages.len().to_string();
    for page in 0..s.pages.len() {
        assert_eq!(shown(on(&s, page, "header")), [total.clone()], "{page}");
    }
    // One page more crosses the boundary.
    let more = body(&doc, "one more");
    boundary(&doc, more, BreakKind::Page, &[]);
    doc.commit();
    let s = engine.layout(&doc);
    assert_eq!(s.pages.len(), 10);
    assert!(
        s.diagnostics_with("layout.field-unsettled")
            .next()
            .is_none()
    );
    for page in 0..10 {
        assert_eq!(shown(on(&s, page, "header")), ["10"], "{page}");
    }
}

#[test]
fn a_field_in_a_table_inside_a_story_bound_to_two_slots_resolves_per_slot() {
    let (engine, doc) = setup();
    let story = doc
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "s", "lead")])
        .unwrap();
    let table = doc
        .append_table(reprise_doc::TableColumns {
            columns: vec![reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Content,
            }],
        })
        .unwrap();
    let row = doc.append_table_row(table, false).unwrap();
    let cell = doc.append_table_cell(row, 0).unwrap();
    let inside = doc
        .append_cell_block(cell, BlockKind::Paragraph, "s", "")
        .unwrap();
    fill(&doc, inside, &[P::T("n="), P::F("page-number")]);
    doc.move_block(table, Some(story), 1).unwrap();
    first(
        &doc,
        &[
            ("region.header.default", bind(story)),
            ("region.footer.default", bind(story)),
            ("numbering.restart", serde_json::json!("5")),
        ],
    );
    pages(&doc, 2);
    doc.commit();
    let s = engine.layout(&doc);
    assert!(
        s.diagnostics_with("layout.story-unsupported")
            .next()
            .is_none(),
        "{:?}",
        s.diagnostics
    );
    for (page, label) in ["5", "6"].into_iter().enumerate() {
        for slot in ["header", "footer"] {
            let instance = on(&s, page, slot);
            // The table's cell kept the containing instance: this slot's, this page's.
            let fields = atoms(&instance.blocks);
            assert_eq!(fields.len(), 1, "{slot} {page}");
            assert_eq!(fields[0].text(), label);
            assert_eq!(
                fields[0].instance,
                FieldInstance::Running {
                    page: page as u32,
                    slot: SlotId::parse(slot).unwrap(),
                    repeat: 0
                },
                "{slot} {page}"
            );
        }
    }
    let mut session = LayoutSession::new(&engine);
    assert_eq!(session.layout(&doc).unwrap(), s);
}

#[test]
fn a_page_of_aimed_into_a_story_or_a_gone_story_is_a_question_mark() {
    let (engine, doc) = setup();
    let head = story_of(&doc, &[P::T("heading")]);
    let gone = story_of(&doc, &[P::T("gone")]);
    first(&doc, &[("region.header.default", bind(head))]);
    let in_story = doc
        .field_target(doc.top(Domain::Story(head))[0], 0)
        .unwrap();
    let in_gone = doc
        .field_target(doc.top(Domain::Story(gone))[0], 0)
        .unwrap();
    let in_body = {
        let t = body(&doc, "target");
        doc.field_target(t, 0).unwrap()
    };
    let p = body(&doc, "");
    let objects = [&in_story, &in_gone, &in_body]
        .map(|t| InlineObject::field("page-of").with("target", t.as_str()));
    let text: String = objects.iter().map(|_| reprise_text::OBJECT).collect();
    doc.insert_with_inline(p, 0, &text, &objects).unwrap();
    doc.delete_block(gone).unwrap();
    doc.commit();
    let s = engine.layout(&doc);
    let texts: Vec<_> = s
        .block(p)
        .unwrap()
        .lines
        .iter()
        .flat_map(|l| &l.atoms)
        .map(|a| a.text())
        .collect();
    // A story has many placements or none; a deleted one has none.
    assert_eq!(texts, ["?", "?", "1"]);
    assert!(s.diagnostics_with("layout.field-target").count() >= 1);
}

#[test]
fn a_formerly_resolved_target_that_becomes_unplaced_shows_a_question_mark() {
    let (engine, doc) = setup();
    let target = body(&doc, "target");
    let p = body(&doc, "");
    let aim = doc.field_target(target, 0).unwrap();
    doc.insert_with_inline(
        p,
        0,
        &reprise_text::OBJECT.to_string(),
        &[InlineObject::field("page-of").with("target", aim.as_str())],
    )
    .unwrap();
    doc.commit();
    let s = engine.layout(&doc);
    let atom = |s: &LayoutSnapshot| s.block(p).unwrap().lines[0].atoms[0].clone();
    assert_eq!(atom(&s).text(), "1");
    // The pass places the target no more, as a later pass of the loop might:
    // the field keeps the text it was composed with, "1", unless it says `?`.
    let mut unplaced = s.clone();
    unplaced.blocks.retain(|b| b.node != target);
    let (resolved, failed) = crate::inline::resolve_fields(&doc, &unplaced);
    assert_eq!(failed.len(), 1);
    let key = crate::inline::FieldKey {
        id: atom(&s).id,
        instance: atom(&s).instance,
    };
    assert_eq!(resolved.get(&key).map(String::as_str), Some("?"));
}

#[test]
fn an_annotation_composed_before_its_page_is_known_follows_feedback_to_the_final_label() {
    use reprise_doc::relation::builtin;
    use reprise_doc::text::RangePolicy;
    use reprise_doc::{Dim, FrameRole, FrameTemplate, PageTemplate, Param, Relation, Target};
    let (engine, doc) = setup();
    doc.set_page_template(
        &PageTemplate::new("n", Dim::pt(200), Dim::pt(100))
            .with_frame(FrameTemplate::new(
                "main",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(200), Dim::pt(60)),
            ))
            .with_frame(FrameTemplate::new(
                "notes",
                FrameRole::Notes,
                (Dim::pt(0), Dim::pt(60)),
                (Dim::pt(200), Dim::pt(40)),
            )),
    )
    .unwrap();
    first(&doc, &[("numbering.restart", serde_json::json!("20"))]);
    let a = body(&doc, "first page");
    let b = body(&doc, "second page anchor");
    boundary(&doc, b, BreakKind::Page, &[]);
    let note = doc.append_block(BlockKind::Annotation, "s", "").unwrap();
    fill(&doc, note, &[P::T("note on "), P::F("page-number")]);
    let range = doc.add_range(b, 0..1, RangePolicy::FIXED).unwrap();
    doc.add_relation(
        &engine.schemas,
        &Relation::new(builtin::NOTE)
            .owned_by(note)
            .target("anchor", Target::Range(range))
            .param("placement", Param::Text("page".into())),
    )
    .unwrap();
    doc.commit();
    let s = engine.layout(&doc);
    assert_eq!(s.pages.len(), 2, "{:?}", s.diagnostics);
    let _ = a;
    let placed = s.block(note).expect("the note is placed");
    let frame = s.frame(placed.lines[0].frame).unwrap();
    assert_eq!(frame.page, 1, "on the anchor's page");
    // Not the label of page 0, which an unplaced composition guesses with.
    let texts: Vec<_> = placed
        .lines
        .iter()
        .flat_map(|l| &l.atoms)
        .map(|a| a.text())
        .collect();
    assert_eq!(texts, ["21"]);
    assert!(
        s.diagnostics_with("layout.field-unsettled")
            .next()
            .is_none()
    );
}
