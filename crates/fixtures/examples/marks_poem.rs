//! Render the authored-position design target for visual review.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = reprise_fixtures::hostile::marks_poem()?;
    let layout = fixture.engine.layout(&fixture.doc);
    let out = std::path::Path::new("out/marks-poem");
    std::fs::create_dir_all(out)?;
    for (page, display) in layout
        .to_display_lists(Default::default())
        .iter()
        .enumerate()
    {
        std::fs::write(
            out.join(format!("page-{page}.svg")),
            reprise_display::svg::render(display, &fixture.engine.fonts)?,
        )?;
        std::fs::write(
            out.join(format!("page-{page}.png")),
            reprise_display::png::render(display, &fixture.engine.fonts, 1.0)?,
        )?;
        std::fs::write(
            out.join(format!("page-{page}.marks.json")),
            serde_json::to_vec_pretty(&layout.marks(page))?,
        )?;
    }
    for diagnostic in &layout.diagnostics {
        println!(
            "{:?} {}: {}",
            diagnostic.severity, diagnostic.code, diagnostic.message
        );
    }
    println!("wrote {}", out.display());
    Ok(())
}
