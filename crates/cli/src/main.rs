//! `reprise spike [OUT_DIR]` runs the end-to-end spike and writes its output.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("spike") => {
            let out = args
                .next()
                .map_or_else(|| PathBuf::from("out"), PathBuf::from);
            match run_spike(&out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("spike failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("usage: reprise spike [OUT_DIR]");
            ExitCode::from(2)
        }
    }
}

fn run_spike(out: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let engine = reprise_fixtures::engine();
    let spike = reprise_fixtures::spike::document()?;

    let before = engine.layout(&spike.doc);
    reprise_cli::write_outputs(&engine, &before, out, "before")?;
    spike.edit()?;
    let after = engine.layout(&spike.doc);
    reprise_cli::write_outputs(&engine, &after, out, "after")?;

    for (name, snapshot) in [("before", &before), ("after", &after)] {
        let note = snapshot
            .line_containing(spike.hallway_note, 0)
            .and_then(|l| snapshot.line_bounds(l))
            .map(|(_, r)| r.origin.y);
        println!("{name}: hallway note at y = {note:?}");
        for d in &snapshot.diagnostics {
            println!("  {:?} {}: {}", d.severity, d.code, d.message);
        }
    }
    println!("wrote {}", out.display());
    Ok(())
}
