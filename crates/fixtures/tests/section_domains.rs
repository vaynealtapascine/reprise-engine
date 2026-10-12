//! A story in every hostile document catches accidental body-domain leaks.
use reprise_doc::{BlockKind, Domain, NewBlock};

#[test]
fn every_hostile_body_is_unchanged_by_an_unbound_story() {
    for fixture in reprise_fixtures::hostile::all().unwrap() {
        let body = fixture.doc.order(Domain::Body);
        let before = fixture.engine.layout(&fixture.doc);
        let story = fixture
            .doc
            .create_story(&[
                NewBlock::new(
                    BlockKind::Paragraph,
                    "body",
                    "Invisible to body flow: café 👩‍👩‍👧‍👦",
                ),
                NewBlock::new(BlockKind::Paragraph, "body", "A second paragraph."),
            ])
            .unwrap();
        fixture.doc.commit();
        assert_eq!(fixture.doc.order(Domain::Body), body, "{}", fixture.name);
        assert_eq!(fixture.doc.top(Domain::Story(story)).len(), 2);
        let mut after = fixture.engine.layout(&fixture.doc);
        after.revision = before.revision.clone();
        assert_eq!(after.to_json(), before.to_json(), "{}", fixture.name);
    }
}
