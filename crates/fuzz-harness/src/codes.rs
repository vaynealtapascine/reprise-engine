//! The diagnostic codes `docs/contracts.md` publishes.
//!
//! Codes are public (renaming one is a contract change), and the table is the
//! only list of them. The oracle reads the table itself, so a code that ships
//! without being documented fails a scenario.

use std::collections::BTreeSet;
use std::sync::OnceLock;

const CONTRACTS: &str = include_str!("../../../docs/contracts.md");

/// Every code in the "Codes in use" table.
pub fn documented() -> &'static BTreeSet<String> {
    static CODES: OnceLock<BTreeSet<String>> = OnceLock::new();
    CODES.get_or_init(|| parse(CONTRACTS))
}

/// Pulls the backticked codes out of the second column of the codes table.
pub(crate) fn parse(markdown: &str) -> BTreeSet<String> {
    let mut codes = BTreeSet::new();
    let mut in_table = false;
    for line in markdown.lines() {
        if line.starts_with("| Library | Codes |") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if !line.starts_with('|') {
            break;
        }
        let Some(cell) = line.trim_end_matches('|').rsplit('|').next() else {
            continue;
        };
        for piece in cell.split('`').skip(1).step_by(2) {
            if piece.contains('.') && !piece.contains(' ') {
                codes.insert(piece.to_owned());
            }
        }
    }
    codes
}

/// Whether `code` is in the table.
pub fn is_documented(code: &str) -> bool {
    documented().contains(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_found_and_covers_every_library() {
        let codes = documented();
        for prefix in [
            "font.", "shape.", "compose.", "layout.", "relation.", "style.", "format.", "edit.",
            "plugin.", "clipboard.", "export.", "bindings.",
        ] {
            assert!(
                codes.iter().any(|c| c.starts_with(prefix)),
                "no {prefix} codes parsed"
            );
        }
        assert!(codes.len() > 120, "parsed {} codes", codes.len());
        assert!(is_documented("layout.page-limit"));
        assert!(!is_documented("layout.made-up"));
    }

    #[test]
    fn rows_after_the_table_are_ignored() {
        let codes = parse("| Library | Codes |\n| --- | --- |\n| a | `x.y` |\n\n| b | `no.no` |\n");
        assert_eq!(codes, BTreeSet::from(["x.y".to_owned()]));
    }
}
