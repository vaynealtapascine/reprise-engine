//! Who may read the document's relations, and so ownership.
//!
//! What a float, note or follow relation makes of a block reached composition
//! three times by a read nothing keyed (B3 fixes, rounds 2, 3 and 4). Rust
//! cannot say that a module may not call `Document::relations`, so this does:
//! it lists, by file, the places in `crates/layout/src` that read relations,
//! and fails for a place that is not on the list. Composition reads them
//! through `ownership.rs` alone (`Ownership::class`), which the flow of a
//! running story answers from its digest and the body's from one read when it
//! begins; neither the flow nor the table, running or inline code may appear
//! here. A new read that has to be here (a pass over the relations, the key of
//! a memo) is added with the reason it is not composition.

use std::path::{Path, PathBuf};

/// Source files that read relations, and why that is not a composition's read.
const READERS: &[(&str, &str)] = &[
    (
        "incremental.rs",
        "keys of the region and final-pass memos (`PassKey`), the dependency graph, `settled`",
    ),
    (
        "ownership.rs",
        "the door: the body's one read when its flow begins, and a story's digest's",
    ),
    (
        "regions.rs",
        "the region pass: whether it is active, and the relations it places",
    ),
    ("relations/alignment.rs", "the relation pass: alignment"),
    ("relations/mod.rs", "the relation pass"),
    (
        "relations/resolve.rs",
        "the relation pass: resolving targets",
    ),
    (
        "sections/mod.rs",
        "`FloatSpaces`: whether a continuous section needs float spaces (page construction, in the position key by its frames)",
    ),
];

/// Words that read the relations of a document.
const READS: &[&str] = &[".relations()", "resolve_relation(", "dead_relations("];

/// Names that must not come back: a set of owners held by the flow.
const GONE: &[&str] = &["region_owners", "regions::owners("];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            out.push(path);
        }
    }
}

/// `(path relative to src, code lines with comments dropped)`.
fn code() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(files.len() > 30, "found {} sources", files.len());
    files
        .into_iter()
        .map(|path| {
            let rel = path
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            (rel, code)
        })
        .collect()
}

#[test]
fn only_listed_places_read_the_relations_of_a_document() {
    let mut unlisted = Vec::new();
    let mut reading = Vec::new();
    for (file, code) in code() {
        if READS.iter().any(|read| code.contains(read)) {
            if !READERS.iter().any(|(name, _)| *name == file) {
                unlisted.push(file.clone());
            }
            reading.push(file);
        }
    }
    assert!(
        unlisted.is_empty(),
        "{unlisted:?} read the relations of a document and are not in READERS. If it is \
         composition (the flow, a table, a running story, inline), it must ask \
         `Ownership::class` (`ownership.rs`) instead, which is keyed; if not, list it \
         with the reason."
    );
    let stale: Vec<_> = READERS
        .iter()
        .filter(|(name, _)| !reading.iter().any(|file| file == name))
        .map(|(name, _)| *name)
        .collect();
    assert!(
        stale.is_empty(),
        "{stale:?} no longer read relations: drop them from READERS"
    );
}

#[test]
fn no_flow_holds_a_set_of_owners() {
    for (file, code) in code() {
        for gone in GONE {
            assert!(
                !code.contains(gone),
                "{file} uses `{gone}`: composition learns ownership from `Ownership` (`ownership.rs`) alone"
            );
        }
        if file != "ownership.rs" && file != "running/compose.rs" {
            assert!(
                !code.contains("Classes::read("),
                "{file} reads the classes of the document: only the body's flow (`Ownership::of`) \
                 and a story's digest may"
            );
        }
        if file != "flow.rs" && file != "ownership.rs" {
            assert!(
                !code.contains("Ownership::of("),
                "{file} makes an `Ownership` of the document: only the flow's constructor does"
            );
        }
    }
}
