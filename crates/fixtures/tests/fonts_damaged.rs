//! Orchestrator review: frontends now hand the engine arbitrary font bytes.
//! A font can pass registration and still be corrupt deeper down, in glyph
//! outlines, CFF charstrings or layout tables, where it hurts shaping or
//! drawing rather than parsing. This damages every bundled default face,
//! truncating it at many points and flipping bytes across the file, and for
//! each variant that registers, lays out text that falls back through it and
//! draws it in SVG, PNG and PDF (which subsets the font). Nothing may panic, and layout must repeat exactly.

use reprise_doc::{BlockKind, Document, Style};
use reprise_fixtures::{PEER, engine};
use reprise_font::{Descriptors, FontDeclaration, FontStore, GenericFamily};
use reprise_layout::DisplayOptions;

fn variants(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let len = bytes.len();
    for i in 1..48 {
        out.push(bytes[..len * i / 48].to_vec());
    }
    let mut state = 0x243F_6A88_85A3_08D3u64;
    for _ in 0..48 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let mut damaged = bytes.to_vec();
        let at = (state as usize) % len;
        damaged[at] ^= 0xFF;
        // Also hit the table directory and early headers, where offsets live.
        let early = (state >> 32) as usize % len.min(512);
        damaged[early] = damaged[early].wrapping_add(0x5A);
        out.push(damaged);
    }
    out
}

#[test]
fn damaged_fonts_register_or_refuse_but_never_panic_downstream() {
    let defaults = FontStore::default();
    let sources: Vec<Vec<u8>> = [
        GenericFamily::Serif,
        GenericFamily::SansSerif,
        GenericFamily::Monospace,
        GenericFamily::Script,
    ]
    .into_iter()
    .map(|g| defaults.generic(g).data().to_vec())
    .collect();
    let mut registered = 0;
    for (s, source) in sources.iter().enumerate() {
        for (v, bytes) in variants(source).into_iter().enumerate() {
            let mut engine = engine();
            let declaration = FontDeclaration {
                family: "Damaged".into(),
                descriptors: Descriptors::default(),
                face_index: 0,
            };
            if engine.fonts.register(bytes, declaration).is_err() {
                continue;
            }
            registered += 1;
            let doc = Document::new(PEER).unwrap();
            doc.define_style(
                "damaged",
                &Style {
                    families: Some(vec!["Damaged".into(), "sans-serif".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
            doc.append_block(
                BlockKind::Paragraph,
                "damaged",
                "office caf\u{e9} \u{5d0}\u{5d1} Z\u{335}\u{322} ffi 0123 \u{1f600}",
            )
            .unwrap();
            doc.commit();
            let snapshot = engine.layout(&doc);
            assert_eq!(
                snapshot,
                engine.layout(&doc),
                "source {s} variant {v}: repeatable"
            );
            let lists = snapshot.to_display_lists(DisplayOptions::default());
            for list in &lists {
                let _ = reprise_display::svg::render(list, &engine.fonts);
                let _ = reprise_display::png::render(list, &engine.fonts, 1.0);
            }
            // The PDF backend subsets the font through krilla.
            let _ = reprise_display::pdf::render(&lists, &engine.fonts);
        }
    }
    // Some damaged variants should still register, or this test proves little.
    assert!(registered > 0, "no damaged font registered at all");
}
