//! Header rows and spans through the clipboard and exports (24, 33, 35).
use reprise_clipboard::{
    Disposition, ExportOptions, Exporter, Feature, Html, ImportLimits, Native, PlainText,
    import_html,
};
use reprise_doc::{Document, GridCell, SchemaRegistry, TableGrid};
use reprise_edit::Editor;

fn pasted(html: &str) -> (Editor, Vec<reprise_diag::Note>) {
    let import = import_html(html, ImportLimits::default()).unwrap();
    let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
    import.fragment.paste(&mut editor, None, "target").unwrap();
    (editor, import.notes)
}

fn grid(doc: &Document) -> TableGrid {
    let table = doc.blocks()[0];
    doc.table_structure(table).unwrap().unwrap()
}

fn shape(grid: &TableGrid) -> Vec<(usize, usize, usize, usize, bool)> {
    grid.cells
        .iter()
        .map(|c: &GridCell| (c.row, c.column, c.colspan, c.rowspan, c.header))
        .collect()
}

fn export<E: Exporter>(e: E, doc: &Document) -> reprise_clipboard::ExportResult {
    let schemas = SchemaRegistry::builtin();
    e.export(
        doc,
        None,
        &ExportOptions {
            source_namespace: "t",
            schemas: &schemas,
            fonts: None,
        },
    )
    .unwrap()
}

#[test]
fn th_thead_and_spans_import_into_the_authored_grid() {
    let (editor, _) = pasted(
        "<table><thead><tr><th colspan=2>Title</th><th>C</th></tr></thead>\
         <tr><td rowspan=2>A</td><td>B</td><td>C</td></tr>\
         <tr><td>D</td><td>E</td></tr></table>",
    );
    let g = grid(editor.document());
    assert_eq!(g.repeating_header_rows, 1);
    assert_eq!(
        shape(&g),
        [
            (0, 0, 2, 1, true),
            (0, 2, 1, 1, true),
            (1, 0, 1, 2, false),
            (1, 1, 1, 1, false),
            (1, 2, 1, 1, false),
            // Row two skips the column the row span already claims.
            (2, 1, 1, 1, false),
            (2, 2, 1, 1, false),
        ]
    );
    assert!(g.issues.is_empty());
}

#[test]
fn a_row_of_only_th_cells_is_a_header_and_a_mixed_row_is_approximated() {
    let (editor, notes) =
        pasted("<table><tr><th>a</th><th>b</th></tr><tr><th>c</th><td>d</td></tr></table>");
    let g = grid(editor.document());
    assert_eq!(
        g.rows.iter().map(|r| r.header).collect::<Vec<_>>(),
        [true, false]
    );
    assert!(
        notes
            .iter()
            .any(|n| n.code == reprise_clipboard::codes::HTML_APPROXIMATED)
    );
}

#[test]
fn hostile_spans_are_clamped_or_read_as_one_and_never_fail() {
    for html in [
        "<table><tr><td colspan=0 rowspan=0>x</td></tr></table>",
        "<table><tr><td colspan=-3 rowspan=abc>x</td></tr></table>",
        "<table><tr><td colspan=99999999999 rowspan=4294967295>x</td></tr></table>",
        "<table><tr><td rowspan=4096>x</td><td rowspan=4096>y</td></tr></table>",
        "<table><tr><td colspan=128>x</td></tr></table>",
    ] {
        let imported = import_html(html, ImportLimits::default());
        let Ok(imported) = imported else {
            // A column over the bound is a typed limit, not a panic.
            assert!(html.contains("99999999999"));
            continue;
        };
        let mut editor = Editor::new(Document::new(2).unwrap(), SchemaRegistry::builtin());
        imported.fragment.paste(&mut editor, None, "t").unwrap();
        let g = grid(editor.document());
        assert!(g.cells.iter().all(|c| c.colspan >= 1 && c.rowspan >= 1));
    }
    // Span bomb: many tall spans stop claiming grid positions at the bound.
    let mut bomb = String::from("<table>");
    for _ in 0..200 {
        bomb.push_str("<tr><td rowspan=4096>x</td></tr>");
    }
    bomb.push_str("</table>");
    assert!(import_html(&bomb, ImportLimits::default()).is_ok());
}

#[test]
fn html_export_writes_th_and_spans_and_round_trips() {
    let (editor, _) = pasted(
        "<table><tr><th colspan=2>T</th></tr><tr><td rowspan=2>a</td><td>b</td></tr>\
         <tr><td>c</td></tr></table>",
    );
    let doc = editor.document();
    let out = export(Html, doc);
    let html = String::from_utf8(out.bytes).unwrap();
    assert!(html.contains("<th colspan=\"2\">"), "{html}");
    assert!(html.contains("<td rowspan=\"2\">"), "{html}");
    assert!(!html.contains("<th rowspan"));
    let (again, _) = pasted(&html);
    assert_eq!(shape(&grid(again.document())), shape(&grid(doc)));
    let tables = out
        .losses
        .features
        .iter()
        .find(|l| l.feature == Feature::Tables)
        .unwrap();
    assert_eq!(tables.disposition, Disposition::Approximated);
    assert!(tables.detail.contains("colspan") && tables.detail.contains("repeated header"));
}

#[test]
fn plain_text_reports_that_headers_and_spans_flatten() {
    let (editor, _) =
        pasted("<table><tr><th colspan=2>T</th></tr><tr><td>a</td><td>b</td></tr></table>");
    let out = export(PlainText, editor.document());
    let tables = out
        .losses
        .features
        .iter()
        .find(|l| l.feature == Feature::Tables)
        .unwrap();
    assert!(tables.detail.contains("spans are flattened"));
}

#[test]
fn native_copy_keeps_spans_and_headers_verbatim() {
    let (editor, _) = pasted(
        "<table><tr><th colspan=2>T</th></tr><tr><td rowspan=1>a</td><td>b</td></tr></table>",
    );
    let doc = editor.document();
    let native = export(Native, doc);
    let fragment = reprise_clipboard::NativeFragment::decode(&native.bytes).unwrap();
    let mut target = Editor::new(Document::new(3).unwrap(), SchemaRegistry::builtin());
    fragment.paste(&mut target, None, "t").unwrap();
    assert_eq!(shape(&grid(target.document())), shape(&grid(doc)));
}
