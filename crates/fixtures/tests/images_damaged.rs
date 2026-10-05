//! Orchestrator review: hosts hand the engine arbitrary image bytes. This
//! damages every test image (truncated at many points, bytes flipped across
//! the file, including the header and the first chunks) and checks that
//! header reading never panics, that layout stays total and repeatable, and
//! that the SVG, PNG and PDF backends, which decode pixels, never panic.

use reprise_display::AssetStore;
use reprise_display::assets::image_header;
use reprise_doc::image::ImageData;
use reprise_doc::{BlockKind, Document};
use reprise_fixtures::{PEER, engine};
use reprise_layout::DisplayOptions;

const IMAGES: [&[u8]; 4] = [
    include_bytes!("../../../fixtures/images/red-1x1.png"),
    include_bytes!("../../../fixtures/images/density.png"),
    include_bytes!("../../../fixtures/images/header-65535.png"),
    include_bytes!("../../../fixtures/images/red-2x1.jpg"),
];

fn variants(bytes: &[u8]) -> Vec<Vec<u8>> {
    let len = bytes.len();
    let mut out: Vec<Vec<u8>> = (0..24).map(|i| bytes[..len * i / 24].to_vec()).collect();
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..40 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let mut damaged = bytes.to_vec();
        damaged[(state as usize) % len] ^= 0xA5;
        let early = ((state >> 32) as usize) % len.min(64);
        damaged[early] = damaged[early].wrapping_add(0x3C);
        out.push(damaged);
    }
    out
}

#[test]
fn damaged_images_never_panic_from_header_to_pixels() {
    for (i, image) in IMAGES.iter().enumerate() {
        for (v, bytes) in variants(image).into_iter().enumerate() {
            let _ = image_header(&bytes);
            let mut engine = engine();
            let Ok(hash) = engine.assets.insert(bytes) else {
                continue;
            };
            let doc = Document::new(PEER).unwrap();
            reprise_fixtures::spike::define_styles(&doc).unwrap();
            doc.append_image("body", &ImageData::new(hash), "damaged image")
                .unwrap();
            doc.append_block(BlockKind::Paragraph, "body", "After the image.")
                .unwrap();
            doc.commit();
            let snapshot = engine.layout(&doc);
            assert_eq!(snapshot, engine.layout(&doc), "image {i} variant {v}");
            let lists = snapshot.to_display_lists(DisplayOptions::default());
            let assets: &AssetStore = &engine.assets;
            for list in &lists {
                let _ = reprise_display::svg::render_with_assets(list, &engine.fonts, assets);
                let _ = reprise_display::png::render_with_assets(list, &engine.fonts, assets, 1.0);
            }
            let _ = reprise_display::pdf::render_with_assets(&lists, &engine.fonts, assets);
        }
    }
}
