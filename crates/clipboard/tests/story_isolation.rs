//! A story's text never reaches the body's reading order or its exports, even
//! when a relation ties the story to a body range (sections B1 review).

use reprise_clipboard::{ExportOptions, Exporter, PlainText};
use reprise_doc::relation::builtin;
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, Dim, Document, Domain, FrameRole, FrameTemplate, NewBlock, PageTemplate, Param,
    Relation, Target,
};
use reprise_fixtures::engine;
use reprise_fixtures::spike::define_styles;

fn plain(doc: &Document, layout: &reprise_layout::LayoutSnapshot) -> String {
    let engine = engine();
    let options = ExportOptions {
        source_namespace: "source",
        schemas: &engine.schemas,
        fonts: None,
    };
    String::from_utf8(PlainText.export(doc, Some(layout), &options).unwrap().bytes).unwrap()
}

#[test]
fn a_stories_note_stays_out_of_reading_order_and_plain_text() {
    for schema in [builtin::NOTE, builtin::FLOAT] {
        let engine = engine();
        let doc = Document::new(1).unwrap();
        define_styles(&doc).unwrap();
        doc.set_page_template(
            &PageTemplate::new("notes", Dim::pt(200), Dim::pt(200))
                .with_frame(FrameTemplate::new(
                    "main",
                    FrameRole::Flow("main".into()),
                    (Dim::pt(0), Dim::pt(0)),
                    (Dim::pt(200), Dim::pt(200)),
                ))
                .with_frame(FrameTemplate::new(
                    "notes",
                    FrameRole::Notes,
                    (Dim::pt(0), Dim::pt(100)),
                    (Dim::pt(200), Dim::pt(100)),
                )),
        )
        .unwrap();
        let anchor = doc
            .append_block(BlockKind::Paragraph, "body", "body text")
            .unwrap();
        let story = doc
            .create_story(&[NewBlock::new(BlockKind::Annotation, "body", "SECRET STORY")])
            .unwrap();
        let owner = doc.top(Domain::Story(story))[0];
        let range = doc.add_range(anchor, 0..1, RangePolicy::FIXED).unwrap();
        let mut relation = Relation::new(schema.clone())
            .owned_by(owner)
            .target("anchor", Target::Range(range));
        relation = if schema == builtin::FLOAT {
            relation.param(
                "width",
                Param::Length(reprise_doc::LengthExpr::Pt(reprise_geom::Length::from_pt(
                    100,
                ))),
            )
        } else {
            relation.param("placement", Param::Text("page".into()))
        };
        doc.add_relation(&engine.schemas, &relation).unwrap();
        doc.commit();
        let layout = engine.layout(&doc);
        assert!(layout.block(owner).is_none(), "{schema}");
        assert!(
            layout
                .reading_order(&doc)
                .iter()
                .all(|step| step.line.node != owner),
            "{schema}"
        );
        let text = plain(&doc, &layout);
        assert!(text.contains("body text"), "{schema}: {text:?}");
        assert!(!text.contains("SECRET"), "{schema}: {text:?}");
    }
}
