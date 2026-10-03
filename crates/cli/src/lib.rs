//! Headless output for the `reprise` command: every output format for a
//! layout snapshot.

use std::path::Path;

use reprise_layout::{DisplayOptions, Engine, LayoutSnapshot};

/// Writes every output format for one snapshot as `<dir>/<name>.*`: layout
/// and display list JSON, plus SVG and PNG of the first page with the debug
/// overlay, and a PDF of every page without it.
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
    let first = snapshot.to_display_list(
        0,
        DisplayOptions {
            debug: true,
            ..DisplayOptions::default()
        },
    );
    let fail = |e: &dyn std::fmt::Display| std::io::Error::other(e.to_string());
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{name}.layout.json")), snapshot.to_json())?;
    std::fs::write(dir.join(format!("{name}.display.json")), first.to_json())?;
    std::fs::write(
        dir.join(format!("{name}.svg")),
        reprise_display::svg::render(&first, &engine.fonts).map_err(|e| fail(&e))?,
    )?;
    std::fs::write(
        dir.join(format!("{name}.png")),
        reprise_display::png::render(&first, &engine.fonts, 3.0).map_err(|e| fail(&e))?,
    )?;
    std::fs::write(
        dir.join(format!("{name}.pdf")),
        reprise_display::pdf::render(&content, &engine.fonts).map_err(|e| fail(&e))?,
    )?;
    Ok(())
}
