//! Restoring annotation anchors after undo (see `docs/annotations.md`).
//!
//! Loro's undo recreates deleted characters under new IDs, so the cursors of
//! an annotation whose text was deleted would stay collapsed. Before text is
//! deleted, each annotation with an end in the deleted span records a hint,
//! outside the undo history: the live character before the span, the span's
//! length and digest, the end's offset in it and its stored anchor. After an
//! undo or redo, an end whose anchor is still that stored anchor and is
//! tombstoned is rebound to the recreated text, but only when the text there
//! has the same digest and was created by this peer after the hint.
use std::ops::Range;

use loro::cursor::{PosType, Side};
use loro::{LoroText, TreeID};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::text::{Affinity, Anchor, Resolved, Text};
use crate::{DocError, Document, NodeId, RangeId, get_str};

/// The `ranges` metadata key holding an annotation's restore hints.
pub(crate) const HINTS: &str = "annot-restore1";
/// Hints kept per annotation, newest first.
const MAX_HINTS: usize = 8;
const MAX_HINTS_BYTES: usize = 16 * 1024;
/// Annotations given hints by one deletion.
const MAX_HINTED: usize = 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct End {
    /// The stored anchor value at the time of the hint.
    anchor: String,
    /// Scalars from the start of the deleted span.
    offset: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hint {
    version: u8,
    /// The live character before the span, as a stored anchor; `None` for
    /// the start of the text.
    base: Option<String>,
    /// The deleted span's length in scalars.
    count: u32,
    /// SHA-256 of the deleted span's UTF-8, in hex.
    digest: String,
    /// The deleting peer, as a decimal string, and its next op counter then:
    /// recreated characters are this peer's, with counters from here.
    peer: String,
    after: i32,
    start: Option<End>,
    end: Option<End>,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn read_hints(raw: &str) -> Vec<Hint> {
    if raw.len() > MAX_HINTS_BYTES {
        return Vec::new();
    }
    let hints: Vec<Hint> = serde_json::from_str(raw).unwrap_or_default();
    hints
        .into_iter()
        .filter(|h| h.version == 1)
        .take(MAX_HINTS)
        .collect()
}

impl Document {
    /// Deletes `bytes` of `node`'s text, first recording restore hints for
    /// the annotations whose ends are in it. Use this rather than deleting
    /// through the text view, so undo restores those annotations. Does not
    /// commit; the hints are written in an excluded staging commit.
    pub fn delete_text(&self, node: NodeId, bytes: Range<usize>) -> Result<(), DocError> {
        let text = self.block(node)?.text;
        text.slice(bytes.clone())?;
        self.record_annotation_restore(node, bytes.clone())?;
        text.delete(bytes)?;
        Ok(())
    }

    /// The whole text a block's anchors live in: its host's for a flow.
    fn anchor_text(&self, node: NodeId) -> Option<LoroText> {
        Some(self.text_of_any(node)?.loro().clone())
    }

    pub(crate) fn record_annotation_restore(
        &self,
        node: NodeId,
        bytes: Range<usize>,
    ) -> Result<(), DocError> {
        if bytes.is_empty() || !self.has_ranges_root() {
            return Ok(());
        }
        let mut ids = std::collections::BTreeSet::new();
        {
            let index = self.annotation_index();
            for (piece, id) in index.by_node.get(&node).into_iter().flatten() {
                if piece.start <= bytes.end && bytes.start <= piece.end {
                    ids.insert(*id);
                }
            }
        }
        if ids.is_empty() {
            return Ok(());
        }
        let Some(raw) = self.anchor_text(node) else {
            return Ok(());
        };
        // Record each visible segment separately. Hidden atom/break markers
        // survive deletion and provide stable bases for subsequent segments.
        let text = self.block(node)?.text;
        let mut at = 0usize;
        for segment in text.segments()? {
            let next = at.saturating_add(segment.bytes.len());
            let a = bytes.start.max(at);
            let b = bytes.end.min(next);
            if a < b {
                self.record_annotation_restore_span(
                    &raw,
                    segment.bytes.start.saturating_add(a - at),
                    segment.bytes.start.saturating_add(b - at),
                    &ids,
                )?;
            }
            at = next;
        }
        Ok(())
    }

    fn record_annotation_restore_span(
        &self,
        raw: &LoroText,
        start: usize,
        end: usize,
        ids: &std::collections::BTreeSet<RangeId>,
    ) -> Result<(), DocError> {
        let (Some(u0), Some(u1)) = (
            raw.convert_pos(start, PosType::Bytes, PosType::Unicode),
            raw.convert_pos(end, PosType::Bytes, PosType::Unicode),
        ) else {
            return Ok(());
        };
        let whole = Text::from_loro(raw.clone());
        let deleted = whole.slice(start..end)?;
        let base = if start > 0 {
            Some(crate::ranges::anchor_value(
                &whole.anchor(start, Affinity::Before)?,
            ))
        } else {
            None
        };
        let peer = self.doc.peer_id();
        let after = self.doc.oplog_vv().get(&peer).copied().unwrap_or_default();
        let template = Hint {
            version: 1,
            base,
            count: u32::try_from(u1 - u0).unwrap_or(u32::MAX),
            digest: hex(&Sha256::digest(deleted.as_bytes())),
            peer: peer.to_string(),
            after,
            start: None,
            end: None,
        };
        let tree = self.tree("ranges");
        let mut writes = Vec::new();
        for &id in ids.iter().take(MAX_HINTED) {
            let Ok(meta) = tree.get_meta(id.0) else {
                continue;
            };
            let end_at = |key: &str| -> Option<End> {
                let stored = get_str(&meta, key)?;
                let anchor = Anchor::decode(&crate::ranges::stored_anchor(meta.get(key))?)?;
                let Ok(Resolved::Live(u)) = whole.resolve_shared(&anchor) else {
                    return None;
                };
                (u0..=u1).contains(&u).then(|| End {
                    anchor: stored,
                    offset: u32::try_from(u - u0).unwrap_or(u32::MAX),
                })
            };
            let hint = Hint {
                start: end_at("start"),
                end: end_at("end"),
                ..template.clone()
            };
            if hint.start.is_none() && hint.end.is_none() {
                continue;
            }
            let mut hints = get_str(&meta, HINTS)
                .map(|raw| read_hints(&raw))
                .unwrap_or_default();
            hints.insert(0, hint);
            hints.truncate(MAX_HINTS);
            if let Ok(raw) = serde_json::to_string(&hints)
                && raw.len() <= MAX_HINTS_BYTES
            {
                writes.push((id, raw));
            }
        }
        if writes.is_empty() {
            return Ok(());
        }
        self.commit_part();
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        for (id, raw) in writes {
            tree.get_meta(id.0)?.insert(HINTS, raw)?;
        }
        self.doc.commit();
        Ok(())
    }

    /// After an undo or redo: rebinds tombstoned annotation ends whose text
    /// was recreated (see the module documentation). Writes in an excluded
    /// staging commit, so it never becomes an undo step of its own.
    pub(crate) fn restore_annotation_anchors(&self) {
        if !self.has_ranges_root() {
            return;
        }
        let tree = self.tree("ranges");
        let mut ids: Vec<TreeID> = tree.nodes();
        ids.sort();
        let mut writes: Vec<(TreeID, &'static str, String)> = Vec::new();
        for id in ids {
            let Ok(meta) = tree.get_meta(id) else {
                continue;
            };
            let Some(raw) = get_str(&meta, HINTS) else {
                continue;
            };
            if !self.live(&tree, id) {
                continue;
            }
            let Some(node) = get_str(&meta, "node").and_then(|s| NodeId::parse(&s)) else {
                continue;
            };
            let Ok(Some(policy)) = self.range_policy(RangeId(id)) else {
                continue;
            };
            let Some(loro) = self.anchor_text(node) else {
                continue;
            };
            let whole = Text::from_loro(loro.clone());
            let mut hints = read_hints(&raw);
            let mut changed = false;
            for (key, affinity) in [("start", policy.start), ("end", policy.end)] {
                let Some(stored) = get_str(&meta, key) else {
                    continue;
                };
                let tombstoned = crate::ranges::stored_anchor(meta.get(key))
                    .and_then(|b| Anchor::decode(&b))
                    .is_some_and(|a| {
                        matches!(whole.resolve_shared(&a), Ok(Resolved::Tombstoned(_)))
                    });
                if !tombstoned {
                    continue;
                }
                for hint in &mut hints {
                    let end = if key == "start" {
                        &mut hint.start
                    } else {
                        &mut hint.end
                    };
                    let Some(end) = end.as_mut().filter(|e| e.anchor == stored) else {
                        continue;
                    };
                    let (base, count, digest, peer, after, offset) = (
                        hint.base.clone(),
                        hint.count,
                        hint.digest.clone(),
                        hint.peer.clone(),
                        hint.after,
                        end.offset,
                    );
                    let Some(anchor) = Self::restored_end(
                        &loro, &whole, base, count, &digest, &peer, after, offset, affinity,
                    ) else {
                        continue;
                    };
                    let value = crate::ranges::anchor_value(&anchor);
                    end.anchor = value.clone();
                    writes.push((id, key, value));
                    changed = true;
                    break;
                }
            }
            if changed && let Ok(raw) = serde_json::to_string(&hints) {
                writes.push((id, HINTS, raw));
            }
        }
        if writes.is_empty() {
            return;
        }
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        for (id, key, value) in writes {
            if let Ok(meta) = tree.get_meta(id) {
                let _ = meta.insert(key, value);
            }
        }
        self.doc.commit();
    }

    /// Where one hinted end belongs in the recreated text, if it is there.
    #[allow(clippy::too_many_arguments)]
    fn restored_end(
        loro: &LoroText,
        whole: &Text,
        base: Option<String>,
        count: u32,
        digest: &str,
        peer: &str,
        after: i32,
        offset: u32,
        affinity: Affinity,
    ) -> Option<Anchor> {
        let p = match base {
            Some(raw) => {
                let bytes = crate::ranges::stored_anchor(Some(loro::ValueOrContainer::Value(
                    loro::LoroValue::String(raw.into()),
                )))?;
                match whole.resolve_shared(&Anchor::decode(&bytes)?).ok()? {
                    Resolved::Live(u) => u,
                    Resolved::Tombstoned(_) => return None,
                }
            }
            None => 0,
        };
        let count = count as usize;
        if count == 0 || offset as usize > count || p.checked_add(count)? > loro.len_unicode() {
            return None;
        }
        // The characters at both ends of the span must be this peer's, made
        // after the hint: recreated by its undo, not text that was already
        // there or that a collaborator typed.
        let peer: u64 = peer.parse().ok()?;
        for at in [p, p + count - 1] {
            let id = loro.get_cursor(at, Side::Left)?.id?;
            if id.peer != peer || id.counter < after {
                return None;
            }
        }
        let a = loro.convert_pos(p, PosType::Unicode, PosType::Bytes)?;
        let b = loro.convert_pos(p + count, PosType::Unicode, PosType::Bytes)?;
        let s = whole.slice(a..b).ok()?;
        if hex(&Sha256::digest(s.as_bytes())) != digest {
            return None;
        }
        let at = loro.convert_pos(p + offset as usize, PosType::Unicode, PosType::Bytes)?;
        whole.anchor(at, affinity).ok()
    }
}
