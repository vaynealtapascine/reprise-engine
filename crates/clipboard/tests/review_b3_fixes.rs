use reprise_clipboard::{ExportOptions, Exporter, Html, ImportLimits, import_html};
use reprise_doc::{BlockKind, Document, Domain, NewBlock, SchemaRegistry};
use reprise_edit::Editor;

#[test]
fn orphan_links_and_browser_fixture_survive_unbound() {
    let html = "<!doctype html><html><body><p>VISIBLE BODY</p><template data-reprise-story=\"s0\"><p>HIDDEN ORPHAN &lt;script&gt; <a href=\"https://example.com\">link</a></p></template></body></html>";
    let imported = import_html(html, ImportLimits::default()).unwrap();
    let mut e = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    imported.fragment.paste(&mut e, None, "dst").unwrap();
    let d = e.document();
    assert_eq!(d.stories().len(), 1);
    assert!(
        d.section_table()
            .iter()
            .flat_map(|s| &s.bindings)
            .all(|b| b.resolved.is_none())
    );
    let schemas = SchemaRegistry::builtin();
    let out = Html
        .export(
            d,
            None,
            &ExportOptions {
                source_namespace: "src",
                schemas: &schemas,
                fonts: None,
            },
        )
        .unwrap();
    let html = String::from_utf8(out.bytes).unwrap();
    assert!(html.contains("https://example.com"));
    assert!(html.contains("&lt;script&gt;"));
    let round = import_html(&html, ImportLimits::default()).unwrap();
    assert_eq!(
        round
            .fragment
            .fragment
            .envelope()
            .unwrap()
            .unwrap()
            .stories
            .len(),
        1
    );
}

#[test]
fn nested_story_tables_and_ranges_validate_and_stay_owned() {
    let d = Document::new(1).unwrap();
    d.append_block(BlockKind::Paragraph, "", "body").unwrap();
    let s = d
        .create_story(&[NewBlock::new(BlockKind::Paragraph, "", "before")])
        .unwrap();
    let columns = reprise_doc::TableColumns {
        columns: vec![reprise_doc::Column {
            width: reprise_doc::ColumnWidth::Content,
        }],
    };
    let outer = d.append_table(columns.clone()).unwrap();
    let row = d.append_table_row(outer, false).unwrap();
    let cell = d.append_table_cell(row, 0).unwrap();
    let inner = d.append_table(columns).unwrap();
    let ir = d.append_table_row(inner, false).unwrap();
    let ic = d.append_table_cell(ir, 0).unwrap();
    let text = d
        .append_cell_block(ic, BlockKind::Paragraph, "", "nested authored")
        .unwrap();
    d.move_block(inner, Some(cell), 0).unwrap();
    d.move_block(outer, Some(s), 1).unwrap();
    d.add_range(text, 0..6, reprise_doc::text::RangePolicy::FIXED)
        .unwrap();
    d.commit();
    let f = d
        .copy_document_fragment("src", &SchemaRegistry::builtin())
        .unwrap();
    let envelope = f.envelope().unwrap().unwrap();
    envelope.validate_in(&f).unwrap();
    let mut e = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    e.paste(&f, None, "dst").unwrap();
    let back = e.document();
    assert_eq!(back.stories().len(), 1);
    let copied = back
        .copy_document_fragment("dst", &SchemaRegistry::builtin())
        .unwrap();
    let story = &copied.envelope().unwrap().unwrap().stories[0];
    assert_eq!(story.blocks.len(), envelope.stories[0].blocks.len());
    assert!(story.blocks.iter().any(|b| b.text == "nested authored"));
    for b in &story.blocks {
        assert_eq!(back.domain_of(b.id), Some(Domain::Story(back.stories()[0])));
    }
}
