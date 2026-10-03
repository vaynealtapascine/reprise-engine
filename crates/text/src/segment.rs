//! Navigation positions (09, 10): grapheme cluster and word boundaries, from
//! ICU4X segmentation. These work on plain strings so the editing kernel can
//! use them on any text it holds. Every offset is a UTF-8 byte offset.
//!
//! Word boundaries are derived at query time and are never stored: some
//! writing systems have no word boundaries, or segment them in ways an author
//! disagrees with, so anything durable uses author-marked ranges instead (10).

use std::ops::Range;

use icu_segmenter::options::WordBreakInvariantOptions;
use icu_segmenter::{GraphemeClusterSegmenter, WordSegmenter};

/// Every grapheme cluster boundary, including 0 and `text.len()`.
pub fn grapheme_boundaries(text: &str) -> Vec<usize> {
    let mut out: Vec<usize> = GraphemeClusterSegmenter::new().segment_str(text).collect();
    if out.is_empty() {
        out.push(0);
    }
    out
}

pub fn is_grapheme_boundary(text: &str, at: usize) -> bool {
    grapheme_boundaries(text).binary_search(&at).is_ok()
}

/// The first grapheme boundary after `at`, or `None` at or past the end.
pub fn next_grapheme(text: &str, at: usize) -> Option<usize> {
    grapheme_boundaries(text).into_iter().find(|&b| b > at)
}

/// The last grapheme boundary before `at`, or `None` at the start.
pub fn prev_grapheme(text: &str, at: usize) -> Option<usize> {
    grapheme_boundaries(text)
        .into_iter()
        .rev()
        .find(|&b| b < at)
}

/// Word-like segments (letters, numbers, ideographs; not spaces or
/// punctuation), in order.
pub fn words(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut iter = WordSegmenter::new_auto(WordBreakInvariantOptions::default()).segment_str(text);
    let mut start = 0;
    while let Some(end) = iter.next() {
        if end > start && iter.is_word_like() {
            out.push(start..end);
        }
        start = end;
    }
    out
}

/// The word containing byte `at`. A position at a word's end belongs to it,
/// so a caret just after a word finds that word.
pub fn word_at(text: &str, at: usize) -> Option<Range<usize>> {
    words(text)
        .into_iter()
        .find(|w| w.start <= at && at <= w.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphemes_keep_combining_marks_and_zwj_sequences_whole() {
        let text = "e\u{301}👩‍👩‍👧x";
        let b = grapheme_boundaries(text);
        assert_eq!(b, [0, 3, 3 + "👩‍👩‍👧".len(), text.len()]);
        assert_eq!(next_grapheme(text, 0), Some(3));
        assert_eq!(next_grapheme(text, 1), Some(3), "from inside a cluster");
        assert_eq!(prev_grapheme(text, text.len()), Some(b[2]));
        assert_eq!(prev_grapheme(text, 0), None);
        assert_eq!(next_grapheme(text, text.len()), None);
        assert!(!is_grapheme_boundary(text, 1));
    }

    #[test]
    fn empty_text_has_one_boundary() {
        assert_eq!(grapheme_boundaries(""), [0]);
        assert_eq!(next_grapheme("", 0), None);
        assert!(words("").is_empty());
    }

    #[test]
    fn words_skip_spaces_and_punctuation() {
        let text = "Tom's house, 42 rooms.";
        let found: Vec<&str> = words(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["Tom's", "house", "42", "rooms"]);
        assert_eq!(word_at(text, 7).map(|r| &text[r]), Some("house"));
        assert_eq!(
            word_at(text, 11).map(|r| &text[r]),
            Some("house"),
            "at its end"
        );
        assert_eq!(word_at(text, 12), None, "the space after the comma");
    }
}
