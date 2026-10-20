//! A layout the incremental session returns from its final-pass cache is the
//! layout a full layout makes (sections §17: section diagnostics are sorted by
//! section order). The cache used to keep the snapshot before the sort, so a
//! job that hit it returned the section diagnostics where the flow had put
//! them: before the body's.

use reprise_doc::{LengthExpr, Style};
use reprise_fixtures::hostile;
use reprise_geom::Length;
use reprise_layout::incremental::LayoutSession;

#[test]
fn a_final_pass_hit_has_the_section_diagnostics_where_a_full_layout_puts_them() {
    for fixture in [
        hostile::sections_geometry_hostile().unwrap(),
        hostile::sections_regions_hostile().unwrap(),
        hostile::sections_bands_hostile().unwrap(),
        hostile::sections_retained_hostile().unwrap(),
    ] {
        let (engine, doc) = (&fixture.engine, &fixture.doc);
        let mut session = LayoutSession::new(engine);
        doc.commit();
        assert_eq!(
            session.layout(doc).unwrap(),
            engine.layout(doc),
            "{}",
            fixture.name
        );
        // An edit that nothing in the layout reads: the final pass is a hit.
        doc.define_style(
            "no-block-has-this-style",
            &Style {
                size: Some(LengthExpr::Pt(Length::from_pt(11))),
                ..Style::default()
            },
        )
        .unwrap();
        doc.commit();
        let again = session.layout(doc).unwrap();
        let counters = session.counters();
        assert_eq!(
            (counters.relation_passes, counters.reading_order_passes),
            (0, 0),
            "{}: the final pass was not a hit: {counters:?}",
            fixture.name
        );
        let full = engine.layout(doc);
        let codes = |s: &reprise_layout::LayoutSnapshot| -> Vec<String> {
            s.diagnostics.iter().map(|d| d.code.to_string()).collect()
        };
        assert_eq!(
            codes(&again),
            codes(&full),
            "{}: diagnostic order",
            fixture.name
        );
        assert_eq!(again, full, "{}", fixture.name);
    }
}
