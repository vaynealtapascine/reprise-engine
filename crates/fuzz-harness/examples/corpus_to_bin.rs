//! Writes the checked-in hex corpus as raw files libFuzzer can read.
//!
//! cargo run -p reprise-fuzz-harness --example corpus_to_bin -- fuzz/corpus/scenario
//!
//! Also writes a few seeded scenarios, so a fresh fuzzing run starts from
//! inputs that reach every op kind.

use std::fs;
use std::path::{Path, PathBuf};

use reprise_fuzz_harness::input::seed_bytes;

fn hex_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            hex_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "hex") {
            out.push(path);
        }
    }
}

fn main() {
    let target = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "fuzz/corpus/scenario".into()),
    );
    fs::create_dir_all(&target).expect("create the corpus directory");
    let mut files = Vec::new();
    hex_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus"),
        &mut files,
    );
    for path in files {
        let text = fs::read_to_string(&path).expect("read a corpus entry");
        let digits: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .flat_map(|l| l.chars().filter(char::is_ascii_hexdigit))
            .collect();
        let bytes: Vec<u8> = (0..digits.len() / 2)
            .filter_map(|i| u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).ok())
            .collect();
        let name = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        fs::write(target.join(name), bytes).expect("write a corpus file");
    }
    for seed in 0..16 {
        fs::write(target.join(format!("seed-{seed}")), seed_bytes(seed, 400))
            .expect("write a seed");
    }
    println!("wrote the corpus to {}", target.display());
}
