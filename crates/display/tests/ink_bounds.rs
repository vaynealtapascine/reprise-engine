//! Ink envelopes must enclose the unrounded font rectangle, even when its
//! coordinates put scaling and shearing on the same inward rounding boundary.
use reprise_display::{Color, Glyph, GlyphRun, Layer};
use reprise_font::{Face, FontStore};
use reprise_geom::Length;

fn with_box(bbox: [i16; 4]) -> Face {
    let original = Face::from_bytes(
        include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
    )
    .unwrap();
    let head = original
        .font_ref()
        .table_directory()
        .table_records()
        .iter()
        .find(|r| r.tag() == skrifa::raw::types::Tag::new(b"head"))
        .unwrap()
        .offset() as usize;
    let mut bytes = original.data().to_vec();
    for (index, coordinate) in bbox.into_iter().enumerate() {
        let at = head + 36 + index * 2;
        bytes[at..at + 2].copy_from_slice(&coordinate.to_be_bytes());
    }
    Face::from_bytes(bytes).unwrap()
}

#[test]
fn shear_bounds_enclose_unrounded_metadata_at_inward_rounding_boundaries() {
    for [x0, y0, x1, y1] in [[1, 3, 10, 20], [-10, -20, -1, -3]] {
        let face = with_box([x0, y0, x1, y1]);
        let denominator = i64::from(face.metrics().units_per_em).saturating_mul(4);
        let mut fonts = FontStore::default();
        let mut run = GlyphRun {
            synthetic_oblique: true,
            face: fonts.add(face),
            size: Length::ZERO,
            color: Color::BLACK,
            text: "a".into(),
            glyphs: vec![Glyph {
                id: 0,
                x: Length::ZERO,
                y: Length::ZERO,
                text: 0..1,
            }],
            layer: Layer::Content,
        };
        for size in [1, 501, 999, 1001, 65535, Length::MAX.0] {
            run.size = Length(size);
            let bounds = run.ink_bounds(&fonts).unwrap();
            // Compare with exact rational design-coordinate edges. The 501
            // case needs x <= 3507/4000, rather than an inward-rounded 1.
            let exact_x = |x: i16, y: i16| {
                i64::from(size)
                    .saturating_mul(i64::from(x).saturating_mul(4).saturating_add(i64::from(y)))
            };
            let exact_y = |y: i16| {
                i64::from(size)
                    .saturating_mul(i64::from(y))
                    .saturating_mul(-4)
            };
            assert!(
                i64::from(bounds.origin.x.0).saturating_mul(denominator) <= exact_x(x0, y0),
                "size {size}: {bounds:?}"
            );
            assert!(
                i64::from(bounds.max_x().0).saturating_mul(denominator) >= exact_x(x1, y1),
                "size {size}: {bounds:?}"
            );
            assert!(i64::from(bounds.origin.y.0).saturating_mul(denominator) <= exact_y(y1));
            assert!(i64::from(bounds.max_y().0).saturating_mul(denominator) >= exact_y(y0));
        }
    }
}
