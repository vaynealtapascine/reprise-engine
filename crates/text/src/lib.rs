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
use std::sync::Arc;

use loro::cursor::{Cursor, Side};
use loro::{ContainerTrait, LoroText};
use serde::{Deserialize, Serialize};

pub mod orientation;
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    end_after: bool,
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
    /// The anchor is in the same flow but now in another paragraph of it
    /// (see `docs/flow.md`). `reprise-doc` locates such anchors.
    #[error("the anchor is in another paragraph of the same flow")]
    Elsewhere,
    #[error("the text is not part of a document, or no longer exists")]
    TextGone,
    #[error("U+FDD0 marks paragraph breaks and can't be inserted as text")]
    Reserved,
    #[error("the text store refused the edit: {0}")]
    Store(String),
}

impl From<loro::LoroError> for TextError {
    fn from(e: loro::LoroError) -> Self {
        TextError::Store(e.to_string())
    }
}

/// The character that marks a paragraph break inside a flow (see
/// `docs/flow.md`). A noncharacter, so it is never authored text: views
/// never show it and [`Text::insert`] refuses it.
pub const BREAK: char = '\u{FDD0}';

/// One visible stretch of a shared text, in UTF-8 bytes and in Unicode
/// scalars (Loro's unit), both counted from the start of the shared text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub bytes: Range<usize>,
    pub chars: Range<usize>,
}

/// Where a paragraph's text is inside a shared text. Implemented by
/// `reprise-doc`, which knows where the breaks are.
pub trait Span: Send + Sync {
    /// The paragraph's visible stretches, in order and not overlapping. Never
    /// empty: an empty paragraph is one empty segment at the place where text
    /// typed into it goes. Characters between segments are hidden.
    fn segments(&self) -> Result<Vec<Segment>, TextError>;
}

/// One text sequence, such as the text of a paragraph. Offsets are UTF-8 bytes.
///
/// A `Text` is a handle: clones refer to the same text, and edits through any
/// handle are visible through all of them.
///
/// A text is either a whole Loro text or a **view** of one paragraph of a
/// shared flow text. A view's offsets count only that paragraph's visible
/// characters; it finds its span again on every call, so it stays correct
/// while the shared text changes.
#[derive(Clone)]
pub struct Text {
    text: LoroText,
    span: Option<Arc<dyn Span>>,
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Text")
            .field("container", &self.text.id())
            .field("view", &self.span.is_some())
            .finish()
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.span {
            None => f.write_str(&self.text.to_string()),
            Some(_) => f.write_str(&self.view().map(|v| v.s).unwrap_or_default()),
        }
    }
}

/// A view's current visible text and where each piece of it is.
struct View {
    segs: Vec<Segment>,
    s: String,
}

impl View {
    fn len(&self) -> usize {
        self.s.len()
    }

    /// The shared-text scalar index of the gap at view byte `at`. Where hidden
    /// characters separate two segments, the gap is before them.
    fn gap(&self, at: usize) -> usize {
        let mut acc = 0;
        for seg in &self.segs {
            let n = seg.bytes.len();
            if at <= acc + n {
                return seg.chars.start + self.s[acc..at].chars().count();
            }
            acc += n;
        }
        self.segs.last().map_or(0, |s| s.chars.end)
    }

    /// The shared-text scalar index of the visible character at view byte `at`.
    fn char_at(&self, at: usize) -> usize {
        let mut acc = 0;
        for seg in &self.segs {
            let n = seg.bytes.len();
            if at < acc + n {
                return seg.chars.start + self.s[acc..at].chars().count();
            }
            acc += n;
        }
        self.segs.last().map_or(0, |s| s.chars.end)
    }

    /// The view byte offset of shared-text scalar index `u`, if it is inside
    /// this view. A position among hidden characters maps to the end of the
    /// segment before them.
    fn offset_of(&self, u: usize) -> Option<usize> {
        let (first, last) = (self.segs.first()?, self.segs.last()?);
        if u < first.chars.start || u > last.chars.end {
            return None;
        }
        let mut acc = 0;
        for seg in &self.segs {
            let n = seg.bytes.len();
            if u < seg.chars.start {
                return Some(acc);
            }
            if u <= seg.chars.end {
                let local: usize = self.s[acc..acc + n]
                    .chars()
                    .take(u - seg.chars.start)
                    .map(char::len_utf8)
                    .sum();
                return Some(acc + local);
            }
            acc += n;
        }
        Some(acc)
    }

    /// The shared-text scalar ranges a view byte range covers, without the
    /// hidden characters between segments.
    fn pieces(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut acc = 0;
        for seg in &self.segs {
            let n = seg.bytes.len();
            let (a, b) = (range.start.max(acc), range.end.min(acc + n));
            if a < b {
                let start = seg.chars.start + self.s[acc..a].chars().count();
                out.push(start..start + self.s[a..b].chars().count());
            }
            acc += n;
        }
        out
    }
}

impl Text {
    /// For `reprise-doc` only: wraps a Loro text it stores.
    pub fn from_loro(text: LoroText) -> Self {
        Text { text, span: None }
    }

    /// For `reprise-doc` only: a view of one paragraph of a shared text.
    pub fn paragraph(text: LoroText, span: Arc<dyn Span>) -> Self {
        Text {
            text,
            span: Some(span),
        }
    }

    /// For `reprise-doc` only: the underlying Loro text. For a view, this is
    /// the whole shared text; see [`Text::segments`].
    pub fn loro(&self) -> &LoroText {
        &self.text
    }

    /// Whether this is a view of one paragraph of a shared text.
    pub fn is_view(&self) -> bool {
        self.span.is_some()
    }

    /// For `reprise-doc` only: where this text's visible characters are in
    /// the underlying Loro text.
    pub fn segments(&self) -> Result<Vec<Segment>, TextError> {
        Ok(self.view()?.segs)
    }

    /// For `reprise-doc` only: the scalar index in the underlying Loro text of
    /// the gap at byte `at` of this text.
    pub fn to_shared(&self, at: usize) -> Result<usize, TextError> {
        let v = self.view()?;
        check(&v.s, at)?;
        Ok(v.gap(at))
    }

    /// For `reprise-doc` only: the byte offset in this text of scalar index
    /// `u` of the underlying Loro text, if it is inside this text.
    pub fn from_shared(&self, u: usize) -> Result<Option<usize>, TextError> {
        Ok(self.view()?.offset_of(u))
    }

    /// The identity of the underlying Loro text. Every paragraph of one flow
    /// shares it.
    pub fn id(&self) -> TextId {
        TextId(self.text.id().to_string())
    }

    pub fn len(&self) -> usize {
        match self.span {
            None => self.text.len_utf8(),
            Some(_) => self.view().map_or(0, |v| v.len()),
        }
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
        if s.contains(BREAK) {
            return Err(TextError::Reserved);
        }
        let v = self.view()?;
        check(&v.s, at)?;
        Ok(self.text.insert(v.gap(at), s)?)
    }

    pub fn delete(&self, range: Range<usize>) -> Result<(), TextError> {
        if range.start > range.end {
            return Err(TextError::BadRange(range));
        }
        let v = self.view()?;
        check(&v.s, range.start)?;
        check(&v.s, range.end)?;
        // Hidden characters inside the range stay: they are inactive breaks
        // that undo may bring back.
        for piece in v.pieces(range).into_iter().rev() {
            self.text.delete(piece.start, piece.len())?;
        }
        Ok(())
    }

    /// Creates an anchor at a byte offset with the given affinity.
    pub fn anchor(&self, at: usize, affinity: Affinity) -> Result<Anchor, TextError> {
        let v = self.view()?;
        check(&v.s, at)?;
        let cursor = if at > 0 && affinity == Affinity::Before {
            self.text.get_cursor(v.char_at(prev(&v.s, at)), Side::Right)
        } else if at == 0 && affinity == Affinity::Before && v.gap(0) > 0 {
            // A paragraph of a flow: stick to the break before it, so the
            // anchor stays at this paragraph's start.
            self.text.get_cursor(v.gap(0) - 1, Side::Right)
        } else if at < v.len() {
            self.text.get_cursor(v.char_at(at), Side::Left)
        } else {
            self.text.get_cursor(v.gap(at), Side::Left)
        };
        cursor
            .map(|cursor| Anchor {
                cursor,
                end_after: false,
            })
            .ok_or(TextError::BadOffset(at))
    }

    /// A UI caret anchor that retains character identity through structural
    /// transfers, including the end of the text. Authored ranges use `anchor`
    /// and retain their raw-cursor representation and block-local policy.
    pub fn anchor_for_transfer(&self, at: usize, affinity: Affinity) -> Result<Anchor, TextError> {
        let v = self.view()?;
        check(&v.s, at)?;
        let len = v.len();
        // A Loro cursor attaches to the character at `pos`. Side::Left sits before
        // it and Side::Right after it, so "stick to the previous character" is
        // the character before `at` with Side::Right.
        let cursor = if at > 0 && (affinity == Affinity::Before || at == len) {
            self.text.get_cursor(v.char_at(prev(&v.s, at)), Side::Right)
        } else if at < len {
            self.text.get_cursor(v.char_at(at), Side::Left)
        } else {
            self.text.get_cursor(v.gap(at), Side::Left)
        };
        cursor
            .map(|cursor| Anchor {
                cursor,
                end_after: at == len && affinity == Affinity::After,
            })
            .ok_or(TextError::BadOffset(at))
    }

    /// Resolves an anchor in this text to a byte offset in its current state.
    /// For a view, [`TextError::Elsewhere`] when the anchor is now in another
    /// paragraph of the same flow.
    pub fn resolve(&self, anchor: &Anchor) -> Result<Resolved, TextError> {
        let unicode = self.resolve_shared(anchor)?;
        let v = self.view()?;
        let tombstoned = matches!(unicode, Resolved::Tombstoned(_));
        let bytes = if self.span.is_none() {
            // The whole text: positions past its end clamp to it.
            let u = unicode.offset().min(self.text.len_unicode());
            v.offset_of(u).unwrap_or(v.len())
        } else {
            v.offset_of(unicode.offset()).ok_or(TextError::Elsewhere)?
        };
        Ok(if anchor.end_after && !tombstoned {
            Resolved::Live(v.len())
        } else if tombstoned {
            Resolved::Tombstoned(bytes)
        } else {
            Resolved::Live(bytes)
        })
    }

    /// For `reprise-doc` only: where an anchor is in the underlying Loro
    /// text, as a scalar index. A cursor on a character's right side is one
    /// past it.
    pub fn resolve_shared(&self, anchor: &Anchor) -> Result<Resolved, TextError> {
        if anchor.cursor.container != self.text.id() {
            return Err(TextError::OtherText);
        }
        let doc = self.text.doc().ok_or(TextError::TextGone)?;
        if self.is_unrecoverable(&doc, anchor) {
            // The anchored character was deleted before the shallow snapshot
            // this document came from, so its history is gone. Loro can't say
            // where it was (it panics when asked), so it resolves to the start
            // of the text, as a tombstone: the author's range reports as
            // moved, never as live.
            return Ok(Resolved::Tombstoned(0));
        }
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
        Ok(if tombstoned {
            Resolved::Tombstoned(unicode)
        } else {
            Resolved::Live(unicode)
        })
    }

    /// Whether `anchor` names a character that is deleted *and* older than the
    /// shallow start of `doc`. Only then is its position unknowable: live
    /// characters resolve without history, and deletions after the shallow
    /// start still have it.
    fn is_unrecoverable(&self, doc: &loro::LoroDoc, anchor: &Anchor) -> bool {
        let Some(id) = anchor.cursor.id else {
            return false;
        };
        if !doc.is_shallow() {
            return false;
        }
        let trimmed = doc
            .shallow_since_vv()
            .get(&id.peer)
            .is_some_and(|&start| id.counter < start);
        trimmed
            && !(0..self.text.len_unicode()).any(|pos| {
                self.text
                    .get_cursor(pos, Side::Left)
                    .is_some_and(|c| c.id == Some(id))
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

    fn view(&self) -> Result<View, TextError> {
        match &self.span {
            None => {
                let s = self.text.to_string();
                Ok(View {
                    segs: vec![Segment {
                        bytes: 0..s.len(),
                        chars: 0..self.text.len_unicode(),
                    }],
                    s,
                })
            }
            Some(span) => {
                let segs = span.segments()?;
                if segs.is_empty() {
                    return Err(TextError::TextGone);
                }
                let mut s = String::new();
                for seg in &segs {
                    if !seg.chars.is_empty() {
                        s.push_str(&self.text.slice(seg.chars.start, seg.chars.end)?);
                    }
                }
                Ok(View { segs, s })
            }
        }
    }
}

fn check(s: &str, at: usize) -> Result<(), TextError> {
    if s.is_char_boundary(at) {
        Ok(())
    } else {
        Err(TextError::BadOffset(at))
    }
}

/// The start of the character before byte `at` (`at > 0`, on a boundary).
fn prev(s: &str, at: usize) -> usize {
    s[..at].char_indices().next_back().map_or(0, |(i, _)| i)
}

impl Anchor {
    /// The text this anchor belongs to.
    pub fn text_id(&self) -> TextId {
        TextId(self.cursor.container.to_string())
    }

    /// Bytes for storing the anchor inside the document.
    pub fn encode(&self) -> Vec<u8> {
        if self.end_after {
            [b"RPAE\x01".as_slice(), self.cursor.encode().as_slice()].concat()
        } else {
            self.cursor.encode()
        }
    }

    /// Raw cursor identity for authored character-lineage records.
    pub fn cursor_bytes(&self) -> Vec<u8> {
        self.cursor.encode()
    }

    /// Retain end insertion affinity while remapping a character identity.
    pub fn remapped(&self, bytes: &[u8]) -> Option<Anchor> {
        Some(Anchor {
            cursor: Cursor::decode(bytes).ok()?,
            end_after: self.end_after,
        })
    }

    pub fn decode(bytes: &[u8]) -> Option<Anchor> {
        let (bytes, end_after) = if bytes.starts_with(b"RPAE") {
            if bytes.get(4) != Some(&1) {
                return None;
            }
            (bytes.get(5..)?, true)
        } else {
            (bytes, false)
        };
        Cursor::decode(bytes)
            .ok()
            .map(|cursor| Anchor { cursor, end_after })
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
    fn end_affinity_survives_encoding_and_remote_append() {
        let (doc, text) = setup("abc");
        doc.commit();
        let before = Anchor::decode(&text.anchor(3, Affinity::Before).unwrap().encode()).unwrap();
        let after = Anchor::decode(&text.anchor(3, Affinity::After).unwrap().encode()).unwrap();
        let transferred = Anchor::decode(
            &text
                .anchor_for_transfer(3, Affinity::After)
                .unwrap()
                .encode(),
        )
        .unwrap();
        assert!(
            !text
                .anchor(3, Affinity::After)
                .unwrap()
                .encode()
                .starts_with(b"RPAE")
        );
        let remote = LoroDoc::new();
        remote
            .import(&doc.export(loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        remote.get_text("t").insert(3, "d").unwrap();
        remote.commit();
        doc.import(&remote.export(loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        assert_eq!(text.resolve(&before).unwrap(), Resolved::Live(3));
        assert_eq!(text.resolve(&after).unwrap(), Resolved::Live(4));
        assert_eq!(text.resolve(&transferred).unwrap(), Resolved::Live(4));
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

    /// Found by the cross-crate fuzzer: asking Loro where a character was
    /// deleted before a shallow snapshot panics (and poisons the document), so
    /// a document saved without history and reopened aborted on layout.
    #[test]
    fn anchors_on_text_deleted_before_a_shallow_snapshot_resolve_without_panicking() {
        let (doc, text) = setup("abcdef");
        let start = text.anchor(0, Affinity::Before).unwrap();
        let live = text.anchor(4, Affinity::After).unwrap();
        doc.commit();
        text.delete(0..2).unwrap();
        doc.commit();
        let bytes = doc
            .export(loro::ExportMode::shallow_snapshot(&doc.state_frontiers()))
            .unwrap();
        let reopened = LoroDoc::new();
        reopened.import(&bytes).unwrap();
        let text = Text::from_loro(reopened.get_text("t"));
        assert_eq!(text.to_string(), "cdef");
        assert_eq!(text.resolve(&start).unwrap(), Resolved::Tombstoned(0));
        // A character that survived still resolves exactly, history or not.
        assert_eq!(text.resolve(&live).unwrap(), Resolved::Live(2));
    }
}
