//! Explicit deterministic fixture generator; never runs during library compilation.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-plugins");
    for name in ["demo", "loop"] {
        let source = root.join(format!("{name}.wat"));
        let bytes = wat::parse_file(source)?;
        std::fs::write(root.join(format!("{name}.wasm")), bytes)?;
    }
    Ok(())
}
