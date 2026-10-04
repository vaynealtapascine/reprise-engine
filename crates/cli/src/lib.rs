//! Headless output for the `reprise` command: every output format for a
//! layout snapshot.

use std::path::Path;

use reprise_layout::{DisplayOptions, Engine, LayoutSnapshot};

/// Writes every output format for one snapshot into `dir`:
///
/// -   `<name>.layout.json`: the layout snapshot
/// -   `<name>.page-<n>.display.json`, `.svg` and `.png` for every page, from 1,
///     with the debug overlay
/// -   `<name>.pdf`: every page, without the overlay
pub fn write_outputs(
    engine: &Engine,
    snapshot: &LayoutSnapshot,
    dir: &Path,
    name: &str,
) -> std::io::Result<()> {
    let pages = snapshot.to_display_lists(DisplayOptions {
        debug: true,
        ..DisplayOptions::default()
    });
    let content: Vec<_> = pages.iter().map(|p| p.content_only()).collect();
    let fail = |e: &dyn std::fmt::Display| std::io::Error::other(e.to_string());
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{name}.layout.json")), snapshot.to_json())?;
    for (index, page) in pages.iter().enumerate() {
        let n = index + 1;
        std::fs::write(
            dir.join(format!("{name}.page-{n}.display.json")),
            page.to_json(),
        )?;
        std::fs::write(
            dir.join(format!("{name}.page-{n}.svg")),
            reprise_display::svg::render(page, &engine.fonts).map_err(|e| fail(&e))?,
        )?;
        std::fs::write(
            dir.join(format!("{name}.page-{n}.png")),
            reprise_display::png::render(page, &engine.fonts, 3.0).map_err(|e| fail(&e))?,
        )?;
    }
    std::fs::write(
        dir.join(format!("{name}.pdf")),
        reprise_display::pdf::render(&content, &engine.fonts).map_err(|e| fail(&e))?,
    )?;
    Ok(())
}
