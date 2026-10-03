//! The collaborative text store (decisions 09, 10 and 12).
//!
//! Text lives in a Loro CRDT, so every character has an identity and deleted
//! characters leave tombstones. The rest of the engine sees byte offsets and
//! [`Anchor`]s, never Loro types. Loro positions count Unicode scalars, so every
//! conversion between the two happens here.
//!
//! There are three kinds of position (09), with conversions between them:
//!
//! -   **Storage offsets:** UTF-8 byte offsets (`usize`). Every other crate uses these.
//! -   **Navigation positions:** grapheme cluster and word boundaries, from
//!     [`segment`]. Carets move between grapheme boundaries.
//! -   **Persistent positions:** [`Anchor`]s, which survive edits by any peer and
//!     resolve back to a storage offset.
//!
//! [`Text::from_loro`] and [`Text::loro`] are the only places a Loro type
//! crosses this crate's boundary. They exist for `reprise-doc`, which stores
//! the texts; nothing else may use them.

use std::fmt;
use std::ops::Range;

use loro::cursor::{Cursor, PosType, Side};
use loro::{ContainerTrait, LoroText};
use serde::{Deserialize, Serialize};

pub mod segment;

/// What happens at an anchor's boundary when text is inserted exactly there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Affinity {
    /// The anchor sticks to the character before it, so insertions land after it.
    Before,
    /// The anchor sticks to the character after it, so insertions land before it.
    After,
}

/// What a range becomes when nothing is left between its ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Empty {
    /// The range is gone: it resolves as missing. Right for ranges that are
    /// about their content, such as a marked word.
    Missing,
    /// The range survives as a point. Right for ranges that mark a place, such
    /// as a footnote reference.
    Keep,
}

/// How a range's boundaries behave under edits (12). Each range type picks one,
/// usually a preset.
///
/// Deleting the character an end is attached to moves that end to the nearest
/// surviving position, and the range reports itself as rebound. Replacing text
/// is a deletion followed by an insertion, so each part follows its rule.
/// How moving text affects ranges is not decided yet; the editing kernel
/// (decision 29) will add it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RangePolicy {
    pub start: Affinity,
    pub end: Affinity,
    pub empty: Empty,
}

impl RangePolicy {
    /// Text typed at either boundary joins the range, like typing at the end of
    /// bold text. Gone when emptied.
    pub const EXPANDING: RangePolicy = RangePolicy {
        start: Affinity::Before,
        end: Affinity::After,
        empty: Empty::Missing,
    };
    /// The range keeps only what it covered; text typed at a boundary stays
    /// outside. Gone when emptied.
    pub const FIXED: RangePolicy = RangePolicy {
        start: Affinity::After,
        end: Affinity::Before,
        empty: Empty::Missing,
    };
    /// A place in the text, such as a note reference. It sticks to the
    /// character before it and survives with nothing in it.
    pub const POINT: RangePolicy = RangePolicy {
        start: Affinity::Before,
        end: Affinity::Before,
        empty: Empty::Keep,
    };
}

/// The identity of one text, stable across edits, peers and sessions.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TextId(String);

impl fmt::Debug for TextId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A persistent position in one text, stable across edits by any peer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    cursor: Cursor,
}

/// Where an anchor resolved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolved {
    /// The anchor's character is still there.
    Live(usize),
    /// The anchor's character was deleted; this is where it used to be.
    Tombstoned(usize),
}

impl Resolved {
    pub fn offset(self) -> usize {
        match self {
            Resolved::Live(o) | Resolved::Tombstoned(o) => o,
        }
    }

    pub fn is_live(self) -> bool {
        matches!(self, Resolved::Live(_))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("byte offset {0} is out of bounds or not on a character boundary")]
    BadOffset(usize),
    #[error("the range {0:?} is reversed or out of bounds")]
    BadRange(Range<usize>),
    #[error("the anchor belongs to another text")]
    OtherText,
    #[error("the text is not part of a document, or no longer exists")]
    TextGone,
    #[error("the text store refused the edit: {0}")]
    Store(String),
}

impl From<loro::LoroError> for TextError {
    fn from(e: loro::LoroError) -> Self {
        TextError::Store(e.to_string())
    }
}

/// One text sequence, such as the text of a paragraph. Offsets are UTF-8 bytes.
///
/// A `Text` is a handle: clones refer to the same text, and edits through any
/// handle are visible through all of them.
#[derive(Clone, Debug)]
pub struct Text {
    text: LoroText,
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text.to_string())
    }
}

impl Text {
    /// For `reprise-doc` only: wraps a Loro text it stores.
    pub fn from_loro(text: LoroText) -> Self {
        Text { text }
    }

    /// For `reprise-doc` only: the underlying Loro text.
    pub fn loro(&self) -> &LoroText {
        &self.text
    }

    pub fn id(&self) -> TextId {
        TextId(self.text.id().to_string())
    }

    pub fn len(&self) -> usize {
        self.text.len_utf8()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The text in `range`.
    pub fn slice(&self, range: Range<usize>) -> Result<String, TextError> {
        let s = self.to_string();
        s.get(range.clone())
            .map(str::to_owned)
            .ok_or(TextError::BadRange(range))
    }

    pub fn insert(&self, at: usize, s: &str) -> Result<(), TextError> {
        self.check_boundary(at)?;
        Ok(self.text.insert_utf8(at, s)?)
    }

    pub fn delete(&self, range: Range<usize>) -> Result<(), TextError> {
        if range.start > range.end {
            return Err(TextError::BadRange(range));
        }
        self.check_boundary(range.start)?;
        self.check_boundary(range.end)?;
        Ok(self
            .text
            .delete_utf8(range.start, range.end - range.start)?)
    }

    /// Creates an anchor at a byte offset with the given affinity.
    pub fn anchor(&self, at: usize, affinity: Affinity) -> Result<Anchor, TextError> {
        self.check_boundary(at)?;
        let pos = self.to_unicode(at)?;
        // A Loro cursor attaches to the character at `pos`. Side::Left sits before
        // it and Side::Right after it, so "stick to the previous character" is
        // the character at pos - 1 with Side::Right.
        let cursor = match affinity {
            Affinity::Before if pos > 0 => self.text.get_cursor(pos - 1, Side::Right),
            _ => self.text.get_cursor(pos, Side::Left),
        };
        cursor
            .map(|cursor| Anchor { cursor })
            .ok_or(TextError::BadOffset(at))
    }

    /// Resolves an anchor in this text to a byte offset in its current state.
    pub fn resolve(&self, anchor: &Anchor) -> Result<Resolved, TextError> {
        if anchor.cursor.container != self.text.id() {
            return Err(TextError::OtherText);
        }
        let doc = self.text.doc().ok_or(TextError::TextGone)?;
        let found = doc
            .get_cursor_pos(&anchor.cursor)
            .map_err(|_| TextError::TextGone)?;
        // `update` is set when the anchored character is gone and Loro had to
        // find a replacement position.
        let tombstoned = found.update.is_some();
        // Loro reports the anchored character's index; a cursor on its right
        // side sits one past it.
        let after = found.current.side == Side::Right && anchor.cursor.id.is_some() && !tombstoned;
        let unicode = found.current.pos + usize::from(after);
        let bytes = if unicode >= self.text.len_unicode() {
            self.text.len_utf8()
        } else {
            self.text
                .convert_pos(unicode, PosType::Unicode, PosType::Bytes)
                .ok_or(TextError::TextGone)?
        };
        Ok(if tombstoned {
            Resolved::Tombstoned(bytes)
        } else {
            Resolved::Live(bytes)
        })
    }

    /// The grapheme boundary after `at`, or `None` at the end.
    pub fn next_grapheme(&self, at: usize) -> Option<usize> {
        segment::next_grapheme(&self.to_string(), at)
    }

    /// The grapheme boundary before `at`, or `None` at the start.
    pub fn prev_grapheme(&self, at: usize) -> Option<usize> {
        segment::prev_grapheme(&self.to_string(), at)
    }

    /// The word containing byte `at`, derived by segmentation now (10).
    pub fn word_at(&self, at: usize) -> Option<Range<usize>> {
        segment::word_at(&self.to_string(), at)
    }

    fn to_unicode(&self, at: usize) -> Result<usize, TextError> {
        self.text
            .convert_pos(at, PosType::Bytes, PosType::Unicode)
            .ok_or(TextError::BadOffset(at))
    }

    fn check_boundary(&self, at: usize) -> Result<(), TextError> {
        let s = self.to_string();
        if s.is_char_boundary(at) {
            Ok(())
        } else {
            Err(TextError::BadOffset(at))
        }
    }
}

impl Anchor {
    /// The text this anchor belongs to.
    pub fn text_id(&self) -> TextId {
        TextId(self.cursor.container.to_string())
    }

    /// Bytes for storing the anchor inside the document.
    pub fn encode(&self) -> Vec<u8> {
        self.cursor.encode()
    }

    pub fn decode(bytes: &[u8]) -> Option<Anchor> {
        Cursor::decode(bytes).ok().map(|cursor| Anchor { cursor })
    }
}

#[cfg(test)]
mod tests {
    use loro::LoroDoc;

    use super::*;

    fn setup(s: &str) -> (LoroDoc, Text) {
        let doc = LoroDoc::new();
        let text = Text::from_loro(doc.get_text("t"));
        text.insert(0, s).unwrap();
        (doc, text)
    }

    fn range(text: &Text, a: &Anchor, b: &Anchor) -> (usize, usize) {
        (
            text.resolve(a).unwrap().offset(),
            text.resolve(b).unwrap().offset(),
        )
    }

    #[test]
    fn anchors_follow_edits_before_them() {
        let (_doc, text) = setup("one two three");
        let a = text.anchor(4, Affinity::After).unwrap();
        text.insert(0, "zero ").unwrap();
        assert_eq!(text.resolve(&a).unwrap(), Resolved::Live(9));
    }

    #[test]
    fn expanding_range_takes_boundary_insertions() {
        let (_doc, text) = setup("one two three");
        let p = RangePolicy::EXPANDING;
        let (s, e) = (
            text.anchor(4, p.start).unwrap(),
            text.anchor(7, p.end).unwrap(),
        );
        text.insert(7, "!").unwrap();
        text.insert(4, "<").unwrap();
        assert_eq!(range(&text, &s, &e), (4, 9));
        assert_eq!(text.slice(4..9).unwrap(), "<two!");
    }

    #[test]
    fn fixed_range_excludes_boundary_insertions() {
        let (_doc, text) = setup("one two three");
        let p = RangePolicy::FIXED;
        let (s, e) = (
            text.anchor(4, p.start).unwrap(),
            text.anchor(7, p.end).unwrap(),
        );
        text.insert(7, "!").unwrap();
        text.insert(4, "<").unwrap();
        assert_eq!(range(&text, &s, &e), (5, 8));
        assert_eq!(text.slice(5..8).unwrap(), "two");
    }

    #[test]
    fn point_ranges_stick_to_the_previous_character() {
        let (_doc, text) = setup("word next");
        let p = RangePolicy::POINT;
        let (s, e) = (
            text.anchor(4, p.start).unwrap(),
            text.anchor(4, p.end).unwrap(),
        );
        text.insert(4, "s").unwrap(); // typed at the marker: lands after it
        assert_eq!(range(&text, &s, &e), (4, 4));
        text.delete(0..5).unwrap(); // the character it sticks to goes too
        assert_eq!(range(&text, &s, &e), (0, 0));
    }

    #[test]
    fn offsets_are_bytes() {
        let (_doc, text) = setup("héllo wörld");
        let a = text.anchor("héllo ".len(), Affinity::After).unwrap();
        text.insert(0, "ä").unwrap();
        assert_eq!(text.resolve(&a).unwrap().offset(), "ähéllo ".len());
        assert!(
            text.anchor("äh".len() + 1, Affinity::After).is_err(),
            "inside the é"
        );
    }

    #[test]
    fn deleted_anchor_reports_tombstone() {
        let (_doc, text) = setup("one two three");
        let a = text.anchor(4, Affinity::After).unwrap();
        text.delete(3..8).unwrap();
        assert_eq!(text.resolve(&a).unwrap(), Resolved::Tombstoned(3));
    }

    #[test]
    fn anchors_only_resolve_in_their_own_text() {
        let doc = LoroDoc::new();
        let a = Text::from_loro(doc.get_text("a"));
        let b = Text::from_loro(doc.get_text("b"));
        a.insert(0, "aaa").unwrap();
        b.insert(0, "bbb").unwrap();
        let anchor = a.anchor(1, Affinity::After).unwrap();
        assert!(matches!(b.resolve(&anchor), Err(TextError::OtherText)));
        assert_eq!(anchor.text_id(), a.id());
    }

    #[test]
    fn reversed_and_misaligned_ranges_are_errors_not_panics() {
        let (_doc, text) = setup("héllo");
        assert!(matches!(
            text.delete(Range { start: 3, end: 1 }),
            Err(TextError::BadRange(_))
        ));
        assert!(matches!(text.slice(0..2), Err(TextError::BadRange(_))));
        assert!(matches!(
            text.insert(99, "x"),
            Err(TextError::BadOffset(99))
        ));
    }
}
