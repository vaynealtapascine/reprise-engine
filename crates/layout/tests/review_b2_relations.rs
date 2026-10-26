//! Astra's review of Stage B part 2, finding 4: whether a running story's image
//! is laid out or left out as unsupported (a float or note owner, or a block
//! placed by following a line) depends on the relations that own it, so a
//! relation edit invalidates the running memo, in both directions. Each test
//! edits one document through a single persistent session.
mod common;
use common::*;
use reprise_doc::relation::builtin;
use reprise_doc::text::RangePolicy;
use reprise_doc::{Document, LayoutQuery, NodeId, Param, Relation, RelationId, Target};
use reprise_layout::incremental::LayoutSession;
use reprise_layout::{Engine, LayoutSnapshot};

/// A running header holding one image, and a body paragraph to anchor to.
fn header_image(engine: &mut Engine, doc: &Document) -> (NodeId, NodeId) {
    let hash = engine
        .assets
        .insert(&include_bytes!("../../../fixtures/images/red-1x1.png")[..])
        .unwrap();
    let story = doc.create_story(&[]).unwrap();
    let img = doc
        .append_image("s", &reprise_doc::image::ImageData::new(hash), "image")
        .unwrap();
    doc.move_block(img, Some(story), 0).unwrap();
    bind_header(doc, story);
    let anchor = block(doc, "body");
    doc.commit();
    (img, anchor)
}

fn shown(s: &LayoutSnapshot, img: NodeId) -> bool {
    s.sections
        .as_ref()
        .unwrap()
        .running
        .iter()
        .flat_map(|r| &r.blocks)
        .any(|b| b.node == img)
}

/// Adds a relation owned by `img` in the session's document, then deletes it,
/// checking the session against a full layout after each edit, and that each
/// edit really changed whether the image is laid out.
fn through_a_session(
    engine: &Engine,
    doc: &Document,
    img: NodeId,
    add: impl FnOnce() -> RelationId,
) {
    let mut session = LayoutSession::new(engine);
    let first = session.layout(doc).unwrap();
    assert!(shown(&first, img), "a plain image in a header is laid out");
    let id = add();
    doc.commit();
    let owned = session.layout(doc).unwrap();
    let full = engine.layout(doc);
    assert!(!shown(&full, img), "an owned image is left out of a story");
    assert!(
        owned == full,
        "adding the relation must invalidate the running memo"
    );
    doc.delete_relation(id).unwrap();
    doc.commit();
    let back = session.layout(doc).unwrap();
    let full = engine.layout(doc);
    assert!(
        shown(&full, img),
        "without the relation it is laid out again"
    );
    assert!(
        back == full,
        "deleting the relation must invalidate the running memo"
    );
}

#[test]
fn running_image_relation_edit_invalidates_memo() {
    let (mut engine, doc) = setup();
    let (img, anchor) = header_image(&mut engine, &doc);
    let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
    through_a_session(&engine, &doc, img, || {
        doc.add_relation(
            &engine.schemas,
            &Relation::new(builtin::FLOAT)
                .owned_by(img)
                .target("anchor", Target::Range(range))
                .param("side", Param::Text("top".into())),
        )
        .unwrap()
    });
}

#[test]
fn a_follow_relation_on_a_running_image_invalidates_memo() {
    let (mut engine, doc) = setup();
    let (img, anchor) = header_image(&mut engine, &doc);
    let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
    through_a_session(&engine, &doc, img, || {
        doc.add_relation(
            &engine.schemas,
            &Relation::new(builtin::FOLLOW).owned_by(img).target(
                "line",
                Target::Layout(LayoutQuery::LineContaining { range }),
            ),
        )
        .unwrap()
    });
}

#[test]
fn a_note_relation_on_a_running_image_invalidates_memo() {
    let (mut engine, doc) = setup();
    let (img, anchor) = header_image(&mut engine, &doc);
    let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
    through_a_session(&engine, &doc, img, || {
        doc.add_relation(
            &engine.schemas,
            &Relation::new(builtin::NOTE)
                .owned_by(img)
                .target("anchor", Target::Range(range))
                .param("placement", Param::Text("page".into())),
        )
        .unwrap()
    });
}
