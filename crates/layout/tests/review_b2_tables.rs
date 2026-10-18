//! Astra's review of Stage B part 2, finding 2: the running memo of a story
//! that holds a table depends on the table's structure and every cell's
//! content, not only on the container block. Each test edits a table in place
//! in one persistent session and compares the session's next layout with a
//! full one; a fresh session would not test reuse.
mod common;
use common::*;
use reprise_doc::{ColumnWidth, LengthExpr, Style};
use reprise_geom::Length;

#[test]
fn running_table_cell_edit_invalidates_memo() {
    let (engine, doc) = setup();
    let g = grid(&doc, 1);
    doc.commit();
    let inside = g.blocks[0][0];
    let (_, after, full) = edited(&engine, &doc, || {
        doc.block(inside)
            .unwrap()
            .text
            .insert(0, "CHANGED ")
            .unwrap();
    });
    assert_eq!(running_text(&full, inside), ["CHANGED r0c0"]);
    assert_eq!(running_text(&after, inside), ["CHANGED r0c0"]);
    assert!(
        after == full,
        "running table cell edit must invalidate memo"
    );
}

#[test]
fn a_cell_blocks_style_edit_invalidates_the_running_table() {
    let (engine, doc) = setup();
    let g = grid(&doc, 2);
    doc.commit();
    let victim = g.blocks[1][1];
    assert_invalidated(
        "cell style override",
        edited(&engine, &doc, || {
            doc.set_overrides(
                victim,
                &Style {
                    size: Some(LengthExpr::Pt(Length::from_pt(30))),
                    line_height: Some(LengthExpr::Pt(Length::from_pt(36))),
                    ..Default::default()
                },
            )
            .unwrap();
        }),
    );
}

#[test]
fn a_row_inserted_or_deleted_invalidates_the_running_table() {
    let (engine, doc) = setup();
    let mut g = grid(&doc, 2);
    doc.commit();
    assert_invalidated(
        "row insert",
        edited(&engine, &doc, || add_row(&doc, &mut g, 2)),
    );
    // And from the changed table, the other way, in a session of its own.
    let victim = g.rows[1];
    assert_invalidated(
        "row delete",
        edited(&engine, &doc, || doc.delete_block(victim).unwrap()),
    );
}

#[test]
fn table_geometry_edits_invalidate_the_running_table() {
    // Column widths: a fixed first column narrower than its text.
    let (engine, doc) = setup();
    let g = grid(&doc, 2);
    doc.commit();
    assert_invalidated(
        "column width",
        edited(&engine, &doc, || {
            doc.set_fragment_table_columns(
                g.table,
                two_columns([ColumnWidth::Fixed(Length::from_pt(4)), ColumnWidth::Content]),
            )
            .unwrap();
        }),
    );
    // A cell span.
    let (engine, doc) = setup();
    let g = grid(&doc, 2);
    doc.commit();
    let cell = g.cells[0][0];
    assert_invalidated(
        "cell span",
        edited(&engine, &doc, || {
            doc.set_table_cell_span(cell, 2, 1).unwrap();
        }),
    );
}

#[test]
fn a_header_row_flag_edit_is_followed_by_the_running_table() {
    // Whether the layout shows it or not, the session must agree with a full
    // layout: the flag is part of the table's structure.
    let (engine, doc) = setup();
    let g = grid(&doc, 2);
    doc.commit();
    let row = g.rows[0];
    let (_, after, full) = edited(&engine, &doc, || {
        doc.set_table_row_header(row, true).unwrap();
    });
    assert!(after == full);
}
