use reprise_display::{AssetStore, DisplayList, Item, Layer, Path, pdf, png, svg};
use reprise_font::FontStore;
use reprise_geom::{Fixed, Length, Matrix, Point, Rect};

fn image_list(hash: String) -> DisplayList {
    let pt = Length::from_pt;
    DisplayList {
        width: pt(60),
        height: pt(60),
        items: vec![Item::Group {
            transform: Matrix {
                xx: Fixed::ZERO,
                xy: Fixed(-65536),
                yx: Fixed::ONE,
                yy: Fixed::ZERO,
                tx: pt(40),
                ty: pt(10),
            },
            clip: Some(Path::rect(Rect::new(
                Point::new(pt(0), pt(0)),
                pt(10),
                pt(5),
            ))),
            items: vec![Item::Image {
                asset: hash,
                rect: Rect::new(Point::new(pt(0), pt(0)), pt(10), pt(10)),
                alt: "<red & rotated>".into(),
                layer: Layer::Content,
            }],
        }],
    }
}

#[test]
fn png_and_jpeg_draw_through_transform_and_clip_in_every_backend() {
    for input in [
        include_bytes!("../../../fixtures/images/red-1x1.png").as_slice(),
        include_bytes!("../../../fixtures/images/red-2x1.jpg").as_slice(),
    ] {
        let mut assets = AssetStore::default();
        let hash = assets.insert(input).unwrap();
        let list = image_list(hash);
        let fonts = FontStore::default();
        let vector = svg::render_with_assets(&list, &fonts, &assets).unwrap();
        assert!(vector.contains("data:image/"));
        assert!(vector.contains("&lt;red &amp; rotated&gt;"));
        assert!(!vector.contains("aria-label=\"<"));
        let raster = png::render_with_assets(&list, &fonts, &assets, 1.0).unwrap();
        assert_eq!(
            raster,
            png::render_with_assets(&list, &fonts, &assets, 1.0).unwrap()
        );
        let decoded = tiny_skia::Pixmap::decode_png(&raster).unwrap();
        let pixel = decoded.pixel(37, 15).unwrap();
        assert!(
            pixel.red() > 200 && pixel.green() < 25 && pixel.blue() < 25,
            "transformed pixel is red"
        );
        assert_eq!(
            decoded.pixel(32, 15).unwrap().green(),
            255,
            "clip removes lower half"
        );
        let pdf_bytes =
            pdf::render_with_assets(std::slice::from_ref(&list), &fonts, &assets).unwrap();
        let parsed = lopdf::Document::load_mem(&pdf_bytes).unwrap();
        assert!(
            parsed
                .objects
                .values()
                .any(|o| o.as_stream().is_ok_and(|s| s
                    .dict
                    .get(b"Subtype")
                    .is_ok_and(|o| o.as_name().ok() == Some(b"Image"))))
        );
        assert!(
            pdf::render_ordered_with_assets(
                &[list],
                &fonts,
                &assets,
                &[pdf::ReadingRun {
                    page: 0,
                    path: vec![0, 0]
                }]
            )
            .is_ok()
        );
    }
}

#[test]
fn missing_corrupt_and_extreme_pixel_payloads_have_bounded_placeholders() {
    let fonts = FontStore::default();
    let mut assets = AssetStore::default();
    for hash in [
        "0".repeat(64),
        assets.insert(b"corrupt".as_slice()).unwrap(),
        assets
            .insert(include_bytes!("../../../fixtures/images/header-65535.png").as_slice())
            .unwrap(),
    ] {
        let list = image_list(hash);
        let raster = png::render_with_assets(&list, &fonts, &assets, 1.0).unwrap();
        let decoded = tiny_skia::Pixmap::decode_png(&raster).unwrap();
        assert_eq!(decoded.pixel(37, 15).unwrap().red(), 221);
        pdf::render_with_assets(&[list], &fonts, &assets).unwrap();
    }
    assert!(!reprise_display::image_renderable(include_bytes!(
        "../../../fixtures/images/header-65535.png"
    )));
}

#[test]
fn truncated_and_mutated_pixel_payloads_never_panic() {
    for input in [
        include_bytes!("../../../fixtures/images/red-1x1.png").as_slice(),
        include_bytes!("../../../fixtures/images/red-2x1.jpg").as_slice(),
    ] {
        for end in 0..input.len() {
            let _ = reprise_display::image_renderable(&input[..end]);
        }
        for at in 0..input.len() {
            let mut mutated = input.to_vec();
            mutated[at] ^= 255;
            let _ = reprise_display::image_renderable(&mutated);
        }
    }
}
