//! Stable carets and selections (10, 12, 29, 30): carets anchored to
//! characters, so they follow edits made by any peer.
//!
//! A [`Caret`] is a byte offset, valid only for the document state it was
//! taken from. A [`StableCaret`] is anchored to a character in the CRDT and
//! resolves to a caret in any later state. Resolving is a pure function of
//! the document, so every replica with the same state gets the same caret.
//! See `docs/collaboration.md`, "Stable positions", for the fallback order.

use reprise_doc::text::{self, Anchor, Resolved, Text, segment};
use reprise_doc::{Document, Fallback, NodeId};

use crate::caret::{Affinity, Caret, Selection};

/// Longest encoded anchor accepted from outside. A Loro cursor is a few dozen
/// bytes.
pub const MAX_ANCHOR_BYTES: usize = 256;

/// A caret anchored to a character.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StableCaret {
    /// The block the caret was in when it was anchored.
    pub node: NodeId,
    /// The encoded anchor in that block's text. Opaque.
    pub anchor: Vec<u8>,
    /// The caret's affinity, which also chose the anchor's side: downstream
    /// sticks to the next character, upstream to the previous one.
    pub affinity: Affinity,
}

/// A selection whose ends are both anchored.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StableSelection {
    pub anchor: StableCaret,
    pub focus: StableCaret,
}

/// What a stable caret resolved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// At its anchored character, including authored split/join lineage.
    Exact(Caret),
    /// Somewhere else: its character or its block was deleted, and the
    /// documented fallback chose this caret.
    Moved(Caret),
    /// The document has no live block that can hold a caret.
    Gone,
}

impl Resolution {
    pub fn caret(self) -> Option<Caret> {
        match self {
            Resolution::Exact(c) | Resolution::Moved(c) => Some(c),
            Resolution::Gone => None,
        }
    }

    pub fn moved(self) -> bool {
        !matches!(self, Resolution::Exact(_))
    }
}

/// Why a caret could not be anchored.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StableError {
    #[error("no live caret block {0}")]
    NoBlock(NodeId),
    #[error("offset {0} is not a character boundary in its block")]
    BadOffset(usize),
}

/// Floors `at` to a grapheme boundary of `s`, clamped to its length.
fn floor_grapheme(s: &str, at: usize) -> usize {
    let at = at.min(s.len());
    if segment::is_grapheme_boundary(s, at) {
        at
    } else {
        segment::prev_grapheme(s, at).unwrap_or(0)
    }
}

impl StableCaret {
    /// Anchors a caret in the document as it is now.
    pub fn anchor(doc: &Document, caret: Caret) -> Result<StableCaret, StableError> {
        if !doc.is_caret_block(caret.node) {
            return Err(StableError::NoBlock(caret.node));
        }
        let block = doc
            .block(caret.node)
            .map_err(|_| StableError::NoBlock(caret.node))?;
        let side = match caret.affinity {
            Affinity::Upstream => text::Affinity::Before,
            Affinity::Downstream => text::Affinity::After,
        };
        let anchor = block
            .text
            .anchor_for_transfer(caret.offset, side)
            .map_err(|_| StableError::BadOffset(caret.offset))?;
        Ok(StableCaret {
            node: caret.node,
            anchor: anchor.encode(),
            affinity: caret.affinity,
        })
    }

    /// Resolves the caret in the document as it is now. Never fails: bad or
    /// foreign anchors take the fallback, and a document without caret blocks
    /// gives [`Resolution::Gone`].
    pub fn resolve(&self, doc: &Document) -> Resolution {
        let anchor = (self.anchor.len() <= MAX_ANCHOR_BYTES)
            .then(|| Anchor::decode(&self.anchor))
            .flatten();
        // Follow authored relocation only when the original character or
        // block is gone. Undo revives the original; concurrent competing
        // transfers have a deterministic order. Bound cycles and chain depth.
        if let Some(current) = anchor.clone() {
            let mut seen = std::collections::BTreeSet::from([(self.node, current.encode())]);
            let mut queue = std::collections::VecDeque::from([(self.node, current)]);
            let mut queued = 1usize;
            while let Some((node, mut current)) = queue.pop_front() {
                if doc.is_caret_block(node)
                    && let Some(text) = doc.text_of_any(node)
                    && matches!(text.resolve(&current), Ok(Resolved::Tombstoned(_)))
                    && let Some(restored) = doc.restored_transfer_anchor(node, &current)
                {
                    current = restored;
                }
                if doc.is_caret_block(node)
                    && let Some(text) = doc.text_of_any(node)
                    && let Ok(Resolved::Live(offset)) = text.resolve(&current)
                {
                    let s = text.to_string();
                    let offset = floor_grapheme(&s, offset);
                    if node != self.node || current.encode() != self.anchor {
                        return Resolution::Exact(Caret {
                            node,
                            offset,
                            affinity: self.affinity,
                        });
                    }
                    break;
                }
                for candidate in doc.transfer_anchors(node, &current) {
                    if queued >= 256 {
                        break;
                    }
                    if seen.insert((candidate.0, candidate.1.encode())) {
                        queue.push_back(candidate);
                        queued += 1;
                    }
                }
            }
        }
        let caret = |node, offset| Caret {
            node,
            offset,
            affinity: self.affinity,
        };
        let in_text = |text: &Text| -> Option<(usize, bool)> {
            let anchor = anchor.as_ref()?;
            match text.resolve(anchor).ok()? {
                Resolved::Live(o) => Some((o, true)),
                Resolved::Tombstoned(o) => Some((o, false)),
            }
        };
        match doc.caret_fallback(self.node) {
            Fallback::Successor(node) if node == self.node => {
                let Ok(block) = doc.block(node) else {
                    return Resolution::Gone;
                };
                let s = block.text.to_string();
                match in_text(&block.text) {
                    Some((o, live)) => {
                        let floored = floor_grapheme(&s, o);
                        let at = caret(node, floored);
                        if live && floored == o {
                            Resolution::Exact(at)
                        } else {
                            Resolution::Moved(at)
                        }
                    }
                    // A live block whose anchor doesn't resolve: its start.
                    None => Resolution::Moved(caret(node, 0)),
                }
            }
            Fallback::Successor(node) => {
                let Ok(block) = doc.block(node) else {
                    return Resolution::Gone;
                };
                let s = block.text.to_string();
                let old = doc.text_of_any(self.node);
                let offset = match (old.as_ref(), old.as_ref().and_then(in_text)) {
                    (Some(old), Some((o, _))) => {
                        let from_end = old.len().saturating_sub(o);
                        s.len().saturating_sub(from_end)
                    }
                    _ => s.len(),
                };
                let mut offset = offset.min(s.len());
                while !s.is_char_boundary(offset) {
                    offset -= 1;
                }
                Resolution::Moved(caret(node, floor_grapheme(&s, offset)))
            }
            Fallback::EndOf(node) => match doc.block(node) {
                Ok(b) => Resolution::Moved(caret(node, b.text.len())),
                Err(_) => Resolution::Gone,
            },
            Fallback::StartOf(node) => Resolution::Moved(caret(node, 0)),
            Fallback::Nowhere => Resolution::Gone,
        }
    }
}

impl StableSelection {
    pub fn anchor(doc: &Document, selection: &Selection) -> Result<StableSelection, StableError> {
        Ok(StableSelection {
            anchor: StableCaret::anchor(doc, selection.anchor)?,
            focus: StableCaret::anchor(doc, selection.focus)?,
        })
    }

    /// The selection now, and whether either end moved; `None` if the
    /// document has no caret block.
    pub fn resolve(&self, doc: &Document) -> Option<(Selection, bool)> {
        let anchor = self.anchor.resolve(doc);
        let focus = self.focus.resolve(doc);
        Some((
            Selection {
                anchor: anchor.caret()?,
                focus: focus.caret()?,
            },
            anchor.moved() || focus.moved(),
        ))
    }
}
