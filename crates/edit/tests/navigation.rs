//! Caret geometry, hit testing, movement and selection over real layouts (30).

use reprise_doc::{BlockKind, Document, NodeId};
use reprise_edit::{Affinity, Caret, Cursor, Movement, Navigator, Selection};
use reprise_fixtures::{engine, hostile};
use reprise_geom::{Length, PageSpace, Point};
use reprise_layout::{Engine, LayoutSnapshot, LineRef};

fn doc_with(blocks: &[&str]) -> (Document, Vec<NodeId>) {
    let doc = Document::new(1).unwrap();
    reprise_fixtures::spike::define_styles(&doc).unwrap();
    let ids = blocks
        .iter()
        .map(|t| doc.append_block(BlockKind::Paragraph, "body", t).unwrap())
        .collect();
    (doc, ids)
}

fn laid_out(blocks: &[&str]) -> (Document, Vec<NodeId>, LayoutSnapshot) {
    let (doc, ids) = doc_with(blocks);
    let snapshot = engine().layout(&doc);
    (doc, ids, snapshot)
}

fn pt(x: i32, y: i32) -> Point<PageSpace> {
    Point::new(Length(x), Length(y))
}

/// Every caret position of every block, with where it is drawn.
fn all_carets(nav: &Navigator) -> Vec<(Caret, reprise_edit::CaretRect)> {
    nav.reading_order()
        .iter()
        .flat_map(|&n| nav.caret_positions(n))
        .map(|c| {
            (
                c,
                nav.caret_rect(c).expect("every caret position has a rect"),
            )
        })
        .collect()
}

#[test]
fn rect_then_hit_gives_the_caret_back_in_every_hostile_fixture() {
    let (mut exact, mut coincident) = (0, 0);
    for fixture in hostile::all().unwrap() {
        let name = fixture.name;
        let snapshot = fixture.engine.layout(&fixture.doc);
        let nav = Navigator::semantic(&snapshot, &fixture.doc);
        let carets = all_carets(&nav);
        for (caret, rect) in &carets {
            assert_eq!(
                nav.normalize(*caret),
                Some(*caret),
                "{name}: {caret:?} is normal"
            );
            let hit = nav
                .hit(rect.page, rect.point())
                .unwrap_or_else(|| panic!("{name}: {caret:?} is hit"));
            // Carets drawn in exactly the same place (zero-height lines
            // stacked on one another, a note over text) can't be told apart
            // by geometry: the hit must be one of them.
            let twins: Vec<&Caret> = carets
                .iter()
                .filter(|(_, r)| r.page == rect.page && r.rect == rect.rect)
                .map(|(c, _)| c)
                .collect();
            if twins.len() == 1 {
                assert_eq!(hit.caret, *caret, "{name}: rect then hit, {rect:?}");
                assert_eq!(hit.line, rect.line, "{name}: same line");
                exact += 1;
            } else {
                assert!(
                    twins.contains(&&hit.caret),
                    "{name}: {caret:?} -> {:?}",
                    hit.caret
                );
                coincident += 1;
            }
        }
    }
    eprintln!("{exact} exact, {coincident} coincident");
    assert!(exact > 1000, "{exact} exact, {coincident} coincident");
}
