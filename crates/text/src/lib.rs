//! The collaborative text store (decisions 09, 10 and 12).
//!
//! Text lives in a Loro CRDT, so every character has an identity and deleted
//! characters leave tombstones. The rest of the engine sees byte offsets and
//! [`Anchor`]s, never Loro types. Loro positions count Unicode scalars, so every
//! conversion between the two happens here.

use loro::cursor::{Cursor, PosType, Side};
use loro::{ContainerID, ContainerTrait, LoroDoc, LoroText};
use serde::{Deserialize, Serialize};

/// What happens at an anchor's boundary when text is inserted exactly there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Affinity {
    /// The anchor sticks to the character before it, so insertions land after it.
    Before,
    /// The anchor sticks to the character after it, so insertions land before it.
    After,
}

/// How a range's boundaries behave under edits (12). Each range type picks one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RangePolicy {
    pub start: Affinity,
    pub end: Affinity,
}

impl RangePolicy {
    /// Text typed at either boundary joins the range, like typing at the end of bold text.
    pub const EXPANDING: RangePolicy = RangePolicy {
        start: Affinity::Before,
        end: Affinity::After,
    };
    /// The range keeps only what it covered; text typed at a boundary stays outside.
    pub const FIXED: RangePolicy = RangePolicy {
        start: Affinity::After,
        end: Affinity::Before,
    };
}

/// A persistent position in one text, stable across edits by any peer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    cursor: Cursor,
}

/// Where an anchor resolved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("byte offset {0} is out of bounds or not on a character boundary")]
    BadOffset(usize),
    #[error("the anchor's text no longer exists")]
    TextGone,
    #[error(transparent)]
    Loro(#[from] loro::LoroError),
}

impl std::fmt::Display for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text.to_string())
    }
}

/// One text sequence, such as the text of a paragraph. Offsets are UTF-8 bytes.
#[derive(Clone, Debug)]
pub struct Text {
    text: LoroText,
}

impl Text {
    pub fn new(text: LoroText) -> Self {
        Text { text }
    }

    pub fn id(&self) -> ContainerID {
        self.text.id()
    }

    pub fn len(&self) -> usize {
        self.text.len_utf8()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn insert(&self, at: usize, s: &str) -> Result<(), TextError> {
        self.check_boundary(at)?;
        Ok(self.text.insert_utf8(at, s)?)
    }

    pub fn delete(&self, range: std::ops::Range<usize>) -> Result<(), TextError> {
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

/// Resolves an anchor to a byte offset in the current state of `doc`.
pub fn resolve(doc: &LoroDoc, anchor: &Anchor) -> Result<Resolved, TextError> {
    let found = doc
        .get_cursor_pos(&anchor.cursor)
        .map_err(|_| TextError::TextGone)?;
    let text = doc.get_text(anchor.cursor.container.clone());
    let tombstoned = found.update.is_some();
    // Loro reports the anchored character's index; a cursor on its right side
    // sits one past it.
    let after = found.current.side == Side::Right && anchor.cursor.id.is_some() && !tombstoned;
    let unicode = found.current.pos + usize::from(after);
    let bytes = if unicode >= text.len_unicode() {
        text.len_utf8()
    } else {
        text.convert_pos(unicode, PosType::Unicode, PosType::Bytes)
            .ok_or(TextError::TextGone)?
    };
    // `update` is set when the anchored character is gone and Loro had to find
    // a replacement position.
    Ok(if tombstoned {
        Resolved::Tombstoned(bytes)
    } else {
        Resolved::Live(bytes)
    })
}

impl Anchor {
    /// The text this anchor belongs to.
    pub fn text_id(&self) -> &ContainerID {
        &self.cursor.container
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
    use super::*;

    fn setup(s: &str) -> (LoroDoc, Text) {
        let doc = LoroDoc::new();
        let text = Text::new(doc.get_text("t"));
        text.insert(0, s).unwrap();
        (doc, text)
    }

    fn range(doc: &LoroDoc, a: &Anchor, b: &Anchor) -> (usize, usize) {
        (
            resolve(doc, a).unwrap().offset(),
            resolve(doc, b).unwrap().offset(),
        )
    }

    #[test]
    fn anchors_follow_edits_before_them() {
        let (doc, text) = setup("one two three");
        let a = text.anchor(4, Affinity::After).unwrap();
        text.insert(0, "zero ").unwrap();
        assert_eq!(resolve(&doc, &a).unwrap(), Resolved::Live(9));
    }

    #[test]
    fn expanding_range_takes_boundary_insertions() {
        let (doc, text) = setup("one two three");
        let p = RangePolicy::EXPANDING;
        let (s, e) = (
            text.anchor(4, p.start).unwrap(),
            text.anchor(7, p.end).unwrap(),
        );
        text.insert(7, "!").unwrap();
        text.insert(4, "<").unwrap();
        assert_eq!(range(&doc, &s, &e), (4, 9));
        assert_eq!(&text.to_string()[4..9], "<two!");
    }

    #[test]
    fn fixed_range_excludes_boundary_insertions() {
        let (doc, text) = setup("one two three");
        let p = RangePolicy::FIXED;
        let (s, e) = (
            text.anchor(4, p.start).unwrap(),
            text.anchor(7, p.end).unwrap(),
        );
        text.insert(7, "!").unwrap();
        text.insert(4, "<").unwrap();
        assert_eq!(range(&doc, &s, &e), (5, 8));
        assert_eq!(&text.to_string()[5..8], "two");
    }

    #[test]
    fn offsets_are_bytes() {
        let (doc, text) = setup("héllo wörld");
        let a = text.anchor("héllo ".len(), Affinity::After).unwrap();
        text.insert(0, "ä").unwrap();
        assert_eq!(resolve(&doc, &a).unwrap().offset(), "ähéllo ".len());
        assert!(
            text.anchor("äh".len() + 1, Affinity::After).is_err(),
            "inside the é"
        );
    }

    #[test]
    fn deleted_anchor_reports_tombstone() {
        let (doc, text) = setup("one two three");
        let a = text.anchor(4, Affinity::After).unwrap();
        text.delete(3..8).unwrap();
        assert_eq!(resolve(&doc, &a).unwrap(), Resolved::Tombstoned(3));
    }
}
