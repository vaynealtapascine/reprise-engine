fn listing() -> String {
    let mut out = String::new();
    for (name, source) in [
        ("lib", include_str!("../src/lib.rs")),
        ("session", include_str!("../src/session.rs")),
        ("error", include_str!("../src/error.rs")),
        ("typescript", include_str!("../src/typescript.rs")),
        ("dto", include_str!("../src/dto.rs")),
        ("collab", include_str!("../src/session/collab.rs")),
    ] {
        out.push_str(name);
        out.push('\n');
        let lines: Vec<_> = source.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].trim();
            if line.starts_with("pub fn ")
                || line.starts_with("pub const ")
                || line.starts_with("pub use ")
                || line.starts_with("pub type ")
            {
                let mut signature = line.to_owned();
                while !signature.contains('{') && !signature.ends_with(';') && i + 1 < lines.len() {
                    i += 1;
                    signature.push(' ');
                    signature.push_str(lines[i].trim());
                }
                let sig = if line.starts_with("pub fn ") {
                    signature.split('{').next().unwrap_or_default().trim()
                } else {
                    signature.trim()
                };
                out.push_str(sig);
                out.push('\n');
            }
            i += 1;
        }
    }
    out
}
#[test]
fn native_api_surface_is_pinned() {
    let got = listing();
    if std::env::var_os("REPRISE_RECORD_API").is_some() {
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("API.txt"),
            &got,
        )
        .unwrap();
    }
    assert_eq!(got, include_str!("../API.txt").replace("\r\n", "\n"));
}
