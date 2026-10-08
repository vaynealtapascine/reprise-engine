//! Authored character lineage for split/join edits. Compact ID spans connect
//! recreated characters to their originals, replicated and undone with the edit.
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use loro::cursor::{Cursor, PosType, Side};
use loro::{Frontiers, ID, LoroMap, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{DocError, Document, NodeId, text::Anchor};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transfer {
    version: u8,
    node: NodeId,
    from: Vec<u8>,
    to: Vec<u8>,
    count: u32,
    base: Option<Vec<u8>>,
    source_base: Option<Vec<u8>>,
    source_offset: u32,
    offset: u32,
    digest: Vec<u8>,
}

/// The most lineage records one lookup considers: those whose source spans
/// start nearest at or before the character, then in numeric ID order. A peer can
/// write any number of records; this bounds what each caret resolution reads.
pub const MAX_LINEAGE_CANDIDATES: usize = 16;

/// The most records of one node that are indexed, in numeric ID order.
pub const MAX_LINEAGE_RECORDS: usize = 4096;

#[derive(Default)]
pub(crate) struct LineageCache {
    frontiers: Option<Frontiers>,
    nodes: HashMap<NodeId, Arc<LineageIndex>>,
}

/// A node's readable lineage records, by the ID of the first source
/// character. A record whose key doesn't name its own source and target is
/// not indexed.
#[derive(Default)]
pub(crate) struct LineageIndex {
    by_source: BTreeMap<(u64, i32), Vec<(LineageKey, Transfer)>>,
}

type LineageKey = (u64, i32, u64, i32);

/// The four IDs a record key names: source peer and counter, target peer
/// and counter.
fn key_ids(key: &str) -> Option<LineageKey> {
    if key.len() > 63 {
        return None;
    }
    let mut parts = key.split(':');
    let ids = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    let (peer, counter, target_peer, target_counter) = ids;
    (parts.next().is_none()
        && counter >= 0
        && target_counter >= 0
        && key == format!("{peer}:{counter}:{target_peer}:{target_counter}"))
    .then_some(ids)
}

impl LineageIndex {
    fn build(entries: Vec<(String, String)>) -> LineageIndex {
        let mut index = LineageIndex::default();
        for (key, raw) in entries.into_iter().take(MAX_LINEAGE_RECORDS) {
            let Some((peer, counter, to_peer, to_counter)) = key_ids(&key) else {
                continue;
            };
            let Ok(record) = serde_json::from_str::<Transfer>(&raw) else {
                continue;
            };
            let (Ok(from), Ok(to)) = (Cursor::decode(&record.from), Cursor::decode(&record.to))
            else {
                continue;
            };
            let named =
                |c: &Cursor, p: u64, n: i32| c.id.is_some_and(|id| id.peer == p && id.counter == n);
            if record.version != 1
                || record.count == 0
                || !named(&from, peer, counter)
                || !named(&to, to_peer, to_counter)
            {
                continue;
            }
            index
                .by_source
                .entry((peer, counter))
                .or_default()
                .push(((peer, counter, to_peer, to_counter), record));
        }
        for records in index.by_source.values_mut() {
            records.sort_by_key(|(key, _)| *key);
        }
        index
    }

    /// Records whose source span holds `id`, at most
    /// [`MAX_LINEAGE_CANDIDATES`], in numeric ID order.
    fn candidates(&self, id: ID) -> Vec<Transfer> {
        let mut found: Vec<(LineageKey, Transfer)> = Vec::new();
        for (&(peer, start), records) in self.by_source.range(..=(id.peer, id.counter)).rev() {
            if peer != id.peer || found.len() >= MAX_LINEAGE_CANDIDATES {
                break;
            }
            for (key, record) in records {
                let end = i64::from(start) + i64::from(record.count);
                if i64::from(id.counter) < end && found.len() < MAX_LINEAGE_CANDIDATES {
                    found.push((*key, record.clone()));
                }
            }
        }
        found.sort_by_key(|a| a.0);
        found.into_iter().map(|(_, r)| r).collect()
    }
}

impl Document {
    /// The lineage index of `node`, cached per revision.
    fn lineage_index(&self, node: NodeId) -> Arc<LineageIndex> {
        let node = node.host();
        if self.doc.get_pending_txn_len() > 0 {
            return Arc::new(LineageIndex::build(self.lineage_entries(node)));
        }
        let frontiers = self.doc.state_frontiers();
        let mut cache = self.lineage.lock().unwrap_or_else(|e| e.into_inner());
        if cache.frontiers.as_ref() != Some(&frontiers) {
            cache.nodes.clear();
            cache.frontiers = Some(frontiers);
        }
        cache
            .nodes
            .entry(node)
            .or_insert_with(|| Arc::new(LineageIndex::build(self.lineage_entries(node))))
            .clone()
    }

    pub(crate) fn record_transfer(
        &self,
        source: NodeId,
        start: usize,
        target: NodeId,
        at: usize,
        bytes: usize,
    ) -> Result<(), DocError> {
        if bytes == 0 {
            return Ok(());
        }
        // Lineage is kept in the underlying texts' coordinates; a paragraph
        // of a flow gives offsets in its view.
        let source_view = self.block(source)?.text;
        // Copying a non-flow block stages its destination before activation.
        // Live destinations need paragraph coordinates; staged ones use their
        // entire text container.
        let target_view = match self.block(target) {
            Ok(block) => block.text,
            Err(_) if self.is_soft_deleted(target) => {
                self.text_of_any(target).ok_or(DocError::NoNode(target))?
            }
            Err(error) => return Err(error),
        };
        let copied = source_view.slice(start..start + bytes)?;
        let source_skip = source_view.slice(0..start)?.chars().count();
        let target_skip = target_view.slice(0..at)?.chars().count();
        target_view.slice(at..at + bytes)?;
        let source_segments = source_view.segments()?;
        let target_segments = target_view.segments()?;
        let (start, at) = (
            self.shared_byte(source, start)?,
            self.shared_byte(target, at)?,
        );
        let old = self.text_of_any(source).ok_or(DocError::NoNode(source))?;
        let new = self.text_of_any(target).ok_or(DocError::NoNode(target))?;
        let from = old.loro();
        let to = new.loro();
        let begin = from
            .convert_pos(start, PosType::Bytes, PosType::Unicode)
            .ok_or_else(|| DocError::Store("invalid transfer source".into()))?;
        let dest = to
            .convert_pos(at, PosType::Bytes, PosType::Unicode)
            .ok_or_else(|| DocError::Store("invalid transfer destination".into()))?;
        let meta = self.meta_of(source.host())?;
        let map = match meta.get("transfers1") {
            Some(ValueOrContainer::Container(loro::Container::Map(map))) => map,
            None => meta.insert_container("transfers1", LoroMap::new())?,
            // Keep malformed/future authored data verbatim. It must not make
            // an otherwise valid structural edit return after partial mutation.
            _ => return Ok(()),
        };
        let base = (at > 0)
            .then(|| new.anchor(at, crate::text::Affinity::Before))
            .transpose()?
            .map(|a| a.encode());
        let source_base = (start > 0)
            .then(|| old.anchor(start, crate::text::Affinity::Before))
            .transpose()?
            .map(|a| a.encode());
        let target_string = new.to_string();
        let mut span: Option<(Cursor, Cursor, u32, usize, usize)> = None;
        let write = |(a, b, count, source_offset, offset): (Cursor, Cursor, u32, usize, usize)| -> Result<(), DocError> {
            let start = to
                .convert_pos(dest + offset, PosType::Unicode, PosType::Bytes)
                .ok_or_else(|| DocError::Store("invalid transfer start".into()))?;
            let end = to
                .convert_pos(
                    dest + offset + count as usize,
                    PosType::Unicode,
                    PosType::Bytes,
                )
                .ok_or_else(|| DocError::Store("invalid transfer end".into()))?;
            let id =
                a.id.ok_or_else(|| DocError::Store("missing transfer character".into()))?;
            let target_id =
                b.id.ok_or_else(|| DocError::Store("missing target character".into()))?;
            let key = format!(
                "{}:{}:{}:{}",
                id.peer, id.counter, target_id.peer, target_id.counter
            );
            let value = serde_json::to_string(&Transfer {
                version: 1,
                node: target,
                from: a.encode(),
                to: b.encode(),
                count,
                base: base.clone(),
                source_base: source_base.clone(),
                source_offset: u32::try_from(source_offset)
                    .map_err(|_| DocError::Store("transfer offset limit".into()))?,
                offset: u32::try_from(offset)
                    .map_err(|_| DocError::Store("transfer offset limit".into()))?,
                digest: Sha256::digest(&target_string.as_bytes()[start..end]).to_vec(),
            })
            .map_err(|e| DocError::Store(e.to_string()))?;
            let forward: Transfer =
                serde_json::from_str(&value).map_err(|e| DocError::Store(e.to_string()))?;
            let reverse = Transfer {
                node: source,
                from: forward.to.clone(),
                to: forward.from.clone(),
                base: forward.source_base.clone(),
                source_base: forward.base.clone(),
                offset: forward.source_offset,
                source_offset: forward.offset,
                ..forward
            };
            let reverse_key = format!(
                "{}:{}:{}:{}",
                target_id.peer, target_id.counter, id.peer, id.counter
            );
            let reverse_raw =
                serde_json::to_string(&reverse).map_err(|e| DocError::Store(e.to_string()))?;
            self.lineage_pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((target.host(), reverse_key, reverse_raw));
            map.insert(&key, value.clone())?;
            self.lineage_pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((source.host(), key, value));
            Ok(())
        };
        let source_positions = source_segments
            .iter()
            .flat_map(|s| s.chars.clone())
            .skip(source_skip);
        let target_positions = target_segments
            .iter()
            .flat_map(|s| s.chars.clone())
            .skip(target_skip);
        for (index, target_index) in source_positions
            .zip(target_positions)
            .take(copied.chars().count())
        {
            // Paragraph coordinates omit inactive markers. Map each visible
            // character on both sides; never create a lineage edge for a marker.
            let a = from
                .get_cursor(index, Side::Left)
                .ok_or_else(|| DocError::Store("missing source cursor".into()))?;
            let b = to
                .get_cursor(target_index, Side::Left)
                .ok_or_else(|| DocError::Store("missing target cursor".into()))?;
            let contiguous =
                span.as_ref()
                    .is_some_and(|(x, y, n, source_offset, target_offset)| {
                        begin + source_offset + *n as usize == index
                            && dest + target_offset + *n as usize == target_index
                            && x.id.zip(a.id).is_some_and(|(x, a)| {
                                x.peer == a.peer
                                    && i64::from(x.counter) + i64::from(*n) == i64::from(a.counter)
                            })
                            && y.id.zip(b.id).is_some_and(|(y, b)| {
                                y.peer == b.peer
                                    && i64::from(y.counter) + i64::from(*n) == i64::from(b.counter)
                            })
                    });
            if contiguous {
                if let Some((_, _, count, _, _)) = &mut span {
                    *count += 1;
                }
            } else {
                if let Some(previous) = span.take() {
                    write(previous)?;
                }
                span = Some((a, b, 1, index - begin, target_index - dest));
            }
        }
        if let Some(last) = span {
            write(last)?;
        }
        Ok(())
    }

    // Identity history is retained like staged block IDs, separately from
    // the undoable edit. Undo may recreate text under fresh character IDs.
    pub(crate) fn commit_lineage(&self) {
        let pending = std::mem::take(
            &mut *self
                .lineage_pending
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        if pending.is_empty() {
            return;
        }
        let pending: std::collections::BTreeMap<_, _> = pending
            .into_iter()
            .map(|(node, key, raw)| ((node, key), raw))
            .collect();
        for ((node, key), raw) in pending {
            let index = self.doc.get_map("lineage-index1");
            let node_key = node.to_string();
            if index.get(&node_key).is_none() {
                let _ = index.insert(&node_key, true);
            }
            let Ok(meta) = self.meta_of(node) else {
                continue;
            };
            let map = match meta.get("transfer-history1") {
                Some(ValueOrContainer::Container(loro::Container::Map(map))) => map,
                None => match meta.insert_container("transfer-history1", LoroMap::new()) {
                    Ok(map) => map,
                    Err(_) => continue,
                },
                _ => continue,
            };
            let _ = map.insert(&key, raw);
        }
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        self.doc.commit();
    }

    pub(crate) fn capture_restored_lineage(&self) {
        let mut nodes = Vec::new();
        self.doc.get_map("lineage-index1").for_each(|key, value| {
            if matches!(value, ValueOrContainer::Value(LoroValue::Bool(true)))
                && let Some(node) = NodeId::parse(key)
            {
                nodes.push(node);
            }
        });
        nodes.sort_unstable();
        for node in nodes {
            if !self.live(&self.tree("content"), node.node) {
                continue;
            }
            let Some(text) = self.text_of_any(node) else {
                continue;
            };
            let s = text.to_string();
            for (_, record) in self.lineage_index(node).by_source.values().flatten() {
                if record.version != 1 || record.count == 0 || record.node == node {
                    continue;
                }
                let Some(original) = Anchor::decode(&record.from) else {
                    continue;
                };
                if !matches!(
                    text.resolve(&original),
                    Ok(crate::text::Resolved::Tombstoned(_))
                ) {
                    continue;
                }
                let Some(start) = Self::source_span(&text, record, &s) else {
                    continue;
                };
                let Ok(old) = Cursor::decode(&record.from) else {
                    continue;
                };
                let Some(old_id) = old.id else {
                    continue;
                };
                let mut spans: Vec<(Cursor, u32, usize)> = Vec::new();
                for delta in 0..record.count as usize {
                    let Some(current) = text.loro().get_cursor(start + delta, Side::Left) else {
                        break;
                    };
                    let contiguous = spans.last().is_some_and(|(cursor, count, _)| {
                        cursor.id.zip(current.id).is_some_and(|(a, b)| {
                            a.peer == b.peer
                                && i64::from(a.counter) + i64::from(*count) == i64::from(b.counter)
                        })
                    });
                    if contiguous {
                        if let Some((_, count, _)) = spans.last_mut() {
                            *count += 1;
                        }
                    } else {
                        spans.push((current, 1, delta));
                    }
                }
                for (current, count, delta) in spans {
                    let Some(current_id) = current.id else {
                        continue;
                    };
                    let mut from = old.clone();
                    let Some(counter) =
                        i32::try_from(i64::from(old_id.counter) + delta as i64).ok()
                    else {
                        continue;
                    };
                    if let Some(id) = &mut from.id {
                        id.counter = counter;
                    }
                    let Some(a) =
                        text.loro()
                            .convert_pos(start + delta, PosType::Unicode, PosType::Bytes)
                    else {
                        continue;
                    };
                    let Some(b) = text.loro().convert_pos(
                        start + delta + count as usize,
                        PosType::Unicode,
                        PosType::Bytes,
                    ) else {
                        continue;
                    };
                    let alias = Transfer {
                        version: 1,
                        node,
                        from: from.encode(),
                        to: current.encode(),
                        count,
                        base: record.source_base.clone(),
                        source_base: record.source_base.clone(),
                        offset: record.source_offset.saturating_add(delta as u32),
                        source_offset: record.source_offset.saturating_add(delta as u32),
                        digest: Sha256::digest(&s.as_bytes()[a..b]).to_vec(),
                    };
                    let reverse = Transfer {
                        from: alias.to.clone(),
                        to: alias.from.clone(),
                        ..alias.clone()
                    };
                    let key = format!(
                        "{}:{}:{}:{}",
                        old_id.peer, counter, current_id.peer, current_id.counter
                    );
                    let reverse_key = format!(
                        "{}:{}:{}:{}",
                        current_id.peer, current_id.counter, old_id.peer, counter
                    );
                    for (key, entry) in [(key, alias), (reverse_key, reverse)] {
                        if let Ok(raw) = serde_json::to_string(&entry) {
                            self.lineage_pending
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push((node, key, raw));
                        }
                    }
                }
            }
        }
        self.commit_lineage();
    }

    /// The byte offset in `node`'s underlying text of byte `at` of its
    /// current text (they differ for a paragraph of a flow).
    fn shared_byte(&self, node: NodeId, at: usize) -> Result<usize, DocError> {
        let Ok(block) = self.block(node) else {
            return Ok(at);
        };
        if !block.text.is_view() {
            return Ok(at);
        }
        let u = block.text.to_shared(at)?;
        let raw = block.text.loro();
        Ok(if u >= raw.len_unicode() {
            raw.len_utf8()
        } else {
            raw.convert_pos(u, PosType::Unicode, PosType::Bytes)
                .ok_or_else(|| DocError::Store("invalid transfer offset".into()))?
        })
    }

    fn lineage_entries(&self, node: NodeId) -> Vec<(String, String)> {
        let Ok(meta) = self.meta_of(node) else {
            return vec![];
        };
        let mut entries = std::collections::BTreeMap::new();
        for field in ["transfer-history1", "transfers1"] {
            if let Some(ValueOrContainer::Container(loro::Container::Map(map))) = meta.get(field) {
                map.for_each(|key, value| {
                    if let ValueOrContainer::Value(LoroValue::String(raw)) = value
                        && raw.len() <= 2048
                        && let Some(ids) = key_ids(key)
                    {
                        entries.insert(ids, (key.to_string(), raw.to_string()));
                        if entries.len() > MAX_LINEAGE_RECORDS {
                            entries.pop_last();
                        }
                    }
                });
            }
        }
        entries.into_values().collect()
    }

    fn source_span(text: &crate::text::Text, record: &Transfer, s: &str) -> Option<usize> {
        let base = match &record.source_base {
            Some(bytes) => match text.resolve(&Anchor::decode(bytes)?) {
                Ok(crate::text::Resolved::Live(at)) => at,
                _ => return None,
            },
            None => 0,
        };
        let raw = text.loro();
        let start = raw
            .convert_pos(base, PosType::Bytes, PosType::Unicode)?
            .checked_add(record.source_offset as usize)?;
        let end = start.checked_add(record.count as usize)?;
        let a = raw.convert_pos(start, PosType::Unicode, PosType::Bytes)?;
        let b = raw.convert_pos(end, PosType::Unicode, PosType::Bytes)?;
        (Sha256::digest(s.as_bytes().get(a..b)?).as_slice() == record.digest).then_some(start)
    }

    /// Rebind an original source ID after undo restored the copied span.
    pub fn restored_transfer_anchor(&self, node: NodeId, anchor: &Anchor) -> Option<Anchor> {
        let text = self.text_of_any(node)?;
        let cursor = Cursor::decode(&anchor.cursor_bytes()).ok()?;
        let id = cursor.id?;
        let s = text.to_string();
        for record in self.lineage_index(node).candidates(id) {
            let candidate = (|| {
                let from = Cursor::decode(&record.from).ok()?;
                let source = from.id?;
                let delta = i64::from(id.counter) - i64::from(source.counter);
                if from.container != cursor.container
                    || source.peer != id.peer
                    || delta < 0
                    || delta >= i64::from(record.count)
                {
                    return None;
                }
                let start = Self::source_span(&text, &record, &s)?;
                let mut replacement = text.loro().get_cursor(start + delta as usize, Side::Left)?;
                replacement.side = cursor.side;
                anchor.remapped(&replacement.encode())
            })();
            if candidate.is_some() {
                return candidate;
            }
        }
        None
    }

    /// Follow one split/join lineage edge for a stable character. The caller
    /// bounds chain traversal and prefers an original live character. Invalid
    /// future records are preserved but ignored. Sorted keys make concurrent
    /// competing transfers resolve identically on every replica.
    pub fn transferred_anchor(&self, node: NodeId, anchor: &Anchor) -> Option<(NodeId, Anchor)> {
        self.transfer_anchors(node, anchor).into_iter().next()
    }

    /// Deterministic, bounded alternatives for character-lineage graph traversal.
    pub fn transfer_anchors(&self, node: NodeId, anchor: &Anchor) -> Vec<(NodeId, Anchor)> {
        let Ok(cursor) = Cursor::decode(&anchor.cursor_bytes()) else {
            return vec![];
        };
        let Some(id) = cursor.id else {
            return vec![];
        };
        let Some(old) = self.text_of_any(node) else {
            return vec![];
        };
        if anchor.text_id() != old.id() {
            return vec![];
        }
        let mut live = Vec::new();
        let mut rest = Vec::new();
        for record in self.lineage_index(node).candidates(id) {
            let candidate = (|| {
                let (Ok(from), Ok(mut to)) =
                    (Cursor::decode(&record.from), Cursor::decode(&record.to))
                else {
                    return None;
                };
                let source = from.id?;
                let delta = i64::from(id.counter) - i64::from(source.counter);
                if from.container != cursor.container
                    || source.peer != id.peer
                    || delta < 0
                    || delta >= i64::from(record.count)
                {
                    return None;
                }
                let text = self.text_of_any(record.node)?;
                if Anchor::decode(&record.to)?.text_id() != text.id() {
                    return None;
                }
                let Some(target) = &mut to.id else {
                    return None;
                };
                target.counter = i32::try_from(i64::from(target.counter) + delta).ok()?;
                to.side = cursor.side;
                let mut mapped = anchor.remapped(&to.encode())?;
                // Undo/redo can recreate the copy under fresh IDs. The insertion
                // boundary remains anchored to the untouched prefix. Rebind only
                // if that exact copied span still exists there (SHA-256), never
                // substitute unrelated replacement text at a stale byte offset.
                if matches!(
                    text.resolve(&mapped),
                    Ok(crate::text::Resolved::Tombstoned(_))
                ) {
                    let repaired = (|| {
                        let base = match &record.base {
                            Some(bytes) => match text.resolve(&Anchor::decode(bytes)?) {
                                Ok(crate::text::Resolved::Live(at)) => at,
                                _ => return None,
                            },
                            None => 0,
                        };
                        let target_text = text.loro();
                        let start = target_text
                            .convert_pos(base, PosType::Bytes, PosType::Unicode)?
                            .checked_add(record.offset as usize)?;
                        let end = start.checked_add(record.count as usize)?;
                        if end > target_text.len_unicode() {
                            return None;
                        }
                        let start_byte =
                            target_text.convert_pos(start, PosType::Unicode, PosType::Bytes)?;
                        let end_byte =
                            target_text.convert_pos(end, PosType::Unicode, PosType::Bytes)?;
                        let s = text.to_string();
                        if Sha256::digest(s.as_bytes().get(start_byte..end_byte)?).as_slice()
                            != record.digest
                        {
                            return None;
                        }
                        let mut replacement =
                            target_text.get_cursor(start + delta as usize, Side::Left)?;
                        replacement.side = cursor.side;
                        anchor.remapped(&replacement.encode())
                    })();
                    if let Some(repaired) = repaired {
                        mapped = repaired;
                    }
                }
                Some((record.node, mapped))
            })();
            if let Some((destination, mapped)) = candidate {
                let is_live = self.is_caret_block(destination)
                    && self.text_of_any(destination).is_some_and(|t| {
                        matches!(t.resolve(&mapped), Ok(crate::text::Resolved::Live(_)))
                    });
                let list = if is_live { &mut live } else { &mut rest };
                if list.len() < 256 {
                    list.push((destination, mapped));
                }
            }
        }
        live.extend(rest.into_iter().take(256usize.saturating_sub(live.len())));
        live
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lineage_keys_have_one_canonical_spelling() {
        assert_eq!(key_ids("2:3:10:4"), Some((2, 3, 10, 4)));
        for key in [
            "02:3:10:4",
            "+2:3:10:4",
            "2:-3:10:4",
            "2:3:10:04",
            "2:3:10:4:5",
        ] {
            assert_eq!(key_ids(key), None, "{key}");
        }
    }

    #[test]
    fn hostile_lineage_is_bounded_and_cache_tracks_edits() {
        let doc = Document::new(1).unwrap();
        let first = doc
            .append_block(crate::BlockKind::Paragraph, "", "abc")
            .unwrap();
        let second = doc
            .append_block(crate::BlockKind::Paragraph, "", "def")
            .unwrap();
        let anchor = doc
            .block(second)
            .unwrap()
            .text
            .anchor(1, crate::text::Affinity::After)
            .unwrap();
        doc.join_blocks(first, second).unwrap();
        doc.commit();
        let before = doc.lineage_index(second);
        assert!(Arc::ptr_eq(&before, &doc.lineage_index(second)));
        let entries = doc.lineage_entries(second);
        let (valid_key, valid_raw) = entries.first().unwrap();
        let meta = doc.meta_of(second).unwrap();
        let Some(ValueOrContainer::Container(loro::Container::Map(map))) = meta.get("transfers1")
        else {
            panic!()
        };
        // Arbitrary key aliases must not crowd out a legitimate source span.
        for n in 0..MAX_LINEAGE_RECORDS + 20 {
            map.insert(&format!("!{n}"), valid_raw.as_str()).unwrap();
        }
        assert_eq!(doc.transfer_anchors(second, &anchor).len(), 1);
        doc.commit();
        assert!(!Arc::ptr_eq(&before, &doc.lineage_index(second)));
        assert_eq!(doc.transferred_anchor(second, &anchor).unwrap().0, first);
        // Canonical but forged keys are ignored when their cursor IDs disagree.
        for n in 0..MAX_LINEAGE_RECORDS + 20 {
            map.insert(&format!("99:{n}:99:0"), valid_raw.as_str())
                .unwrap();
        }
        assert_eq!(doc.lineage_entries(second).len(), MAX_LINEAGE_RECORDS);
        assert!(
            doc.lineage_entries(second)
                .iter()
                .any(|(k, _)| k == valid_key)
        );
        assert_eq!(doc.transfer_anchors(second, &anchor).len(), 1);
    }

    #[test]
    fn matching_lineage_candidates_are_capped_in_numeric_order() {
        let doc = Document::new(1).unwrap();
        let first = doc
            .append_block(crate::BlockKind::Paragraph, "", "abc")
            .unwrap();
        let second = doc
            .append_block(crate::BlockKind::Paragraph, "", "def")
            .unwrap();
        doc.join_blocks(first, second).unwrap();
        let (_, raw) = doc.lineage_entries(second).remove(0);
        let original: Transfer = serde_json::from_str(&raw).unwrap();
        let source = Cursor::decode(&original.from).unwrap().id.unwrap();
        let mut entries = Vec::new();
        for peer in 2..42 {
            let mut record = original.clone();
            let mut target = Cursor::decode(&record.to).unwrap();
            target.id.as_mut().unwrap().peer = peer;
            record.to = target.encode();
            let key = format!(
                "{}:{}:{}:{}",
                source.peer,
                source.counter,
                peer,
                target.id.unwrap().counter
            );
            entries.push((key, serde_json::to_string(&record).unwrap()));
        }
        let index = LineageIndex::build(entries);
        let candidates = index.candidates(source);
        assert_eq!(candidates.len(), MAX_LINEAGE_CANDIDATES);
        let peers: Vec<_> = candidates
            .iter()
            .map(|r| Cursor::decode(&r.to).unwrap().id.unwrap().peer)
            .collect();
        assert_eq!(peers, (2..18).collect::<Vec<_>>());
        assert!(
            index
                .candidates(ID {
                    peer: source.peer,
                    counter: source.counter + 100
                })
                .is_empty()
        );
    }
    #[test]
    fn malformed_lineage_does_not_partially_fail_a_join() {
        let doc = Document::new(1).unwrap();
        let first = doc
            .append_block(crate::BlockKind::Paragraph, "", "abc")
            .unwrap();
        let second = doc
            .append_block(crate::BlockKind::Paragraph, "", "def")
            .unwrap();
        doc.tree("content")
            .get_meta(second.node)
            .unwrap()
            .insert("transfers1", "future")
            .unwrap();
        doc.join_blocks(first, second).unwrap();
        assert_eq!(doc.block(first).unwrap().text.to_string(), "abcdef");
        assert!(doc.block(second).is_err());
    }
    #[test]
    fn malformed_matching_record_cannot_mask_a_later_valid_transfer() {
        let doc = Document::new(1).unwrap();
        let first = doc
            .append_block(crate::BlockKind::Paragraph, "", "abc")
            .unwrap();
        let second = doc
            .append_block(crate::BlockKind::Paragraph, "", "def")
            .unwrap();
        let anchor = doc
            .block(second)
            .unwrap()
            .text
            .anchor(1, crate::text::Affinity::After)
            .unwrap();
        doc.join_blocks(first, second).unwrap();
        let meta = doc.tree("content").get_meta(second.node).unwrap();
        let Some(ValueOrContainer::Container(loro::Container::Map(map))) = meta.get("transfers1")
        else {
            panic!()
        };
        let mut raw = None;
        map.for_each(|_, value| {
            if let ValueOrContainer::Value(LoroValue::String(s)) = value {
                raw = Some(s.to_string())
            }
        });
        let mut record: Transfer = serde_json::from_str(&raw.unwrap()).unwrap();
        record.node = NodeId::tree(loro::TreeID {
            peer: 99,
            counter: 999,
        });
        map.insert("!invalid", serde_json::to_string(&record).unwrap())
            .unwrap();
        let (node, mapped) = doc.transferred_anchor(second, &anchor).unwrap();
        assert_eq!(node, first);
        assert_eq!(
            doc.block(first)
                .unwrap()
                .text
                .resolve(&mapped)
                .unwrap()
                .offset(),
            4
        );
    }
}
