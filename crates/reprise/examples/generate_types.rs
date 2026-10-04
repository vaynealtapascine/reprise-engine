fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../reprise-wasm/ts/types.d.ts");
    std::fs::write(path, reprise::typescript::declarations())?;
    Ok(())
}
