//! Authored character lineage for split/join edits. Compact ID spans connect
//! recreated characters to their originals, replicated and undone with the edit.
use loro::cursor::{Cursor, PosType, Side};
use loro::{LoroMap, LoroValue, ValueOrContainer};
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

impl Document {
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
        let old = self.text_of_any(source).ok_or(DocError::NoNode(source))?;
        let new = self.text_of_any(target).ok_or(DocError::NoNode(target))?;
        let from = old.loro();
        let to = new.loro();
        let begin = from
            .convert_pos(start, PosType::Bytes, PosType::Unicode)
            .ok_or_else(|| DocError::Store("invalid transfer source".into()))?;
        let end = from
            .convert_pos(start + bytes, PosType::Bytes, PosType::Unicode)
            .ok_or_else(|| DocError::Store("invalid transfer source end".into()))?;
        let dest = to
            .convert_pos(at, PosType::Bytes, PosType::Unicode)
            .ok_or_else(|| DocError::Store("invalid transfer destination".into()))?;
        let meta = self.tree("content").get_meta(source.0)?;
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
        let mut span: Option<(Cursor, Cursor, u32, usize)> = None;
        let write = |(a, b, count, offset): (Cursor, Cursor, u32, usize)| -> Result<(), DocError> {
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
                source_offset: u32::try_from(offset)
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
                .push((target, reverse_key, reverse_raw));
            map.insert(&key, value.clone())?;
            self.lineage_pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((source, key, value));
            Ok(())
        };
        for index in begin..end {
            let a = from
                .get_cursor(index, Side::Left)
                .ok_or_else(|| DocError::Store("missing source cursor".into()))?;
            let b = to
                .get_cursor(dest + index - begin, Side::Left)
                .ok_or_else(|| DocError::Store("missing target cursor".into()))?;
            let contiguous = span.as_ref().is_some_and(|(x, y, n, _)| {
                x.id.zip(a.id).is_some_and(|(x, a)| {
                    x.peer == a.peer && i64::from(x.counter) + i64::from(*n) == i64::from(a.counter)
                }) && y.id.zip(b.id).is_some_and(|(y, b)| {
                    y.peer == b.peer && i64::from(y.counter) + i64::from(*n) == i64::from(b.counter)
                })
            });
            if contiguous {
                if let Some((_, _, count, _)) = &mut span {
                    *count += 1;
                }
            } else {
                if let Some(previous) = span.take() {
                    write(previous)?;
                }
                span = Some((a, b, 1, index - begin));
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
            let Ok(meta) = self.tree("content").get_meta(node.0) else {
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
            if !self.is_caret_block(node) {
                continue;
            }
            let Some(text) = self.text_of_any(node) else {
                continue;
            };
            let s = text.to_string();
            for (_, value) in self.lineage_entries(node) {
                let Ok(record) = serde_json::from_str::<Transfer>(&value) else {
                    continue;
                };
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
                let Some(start) = Self::source_span(&text, &record, &s) else {
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

    fn lineage_entries(&self, node: NodeId) -> Vec<(String, String)> {
        let Ok(meta) = self.tree("content").get_meta(node.0) else {
            return vec![];
        };
        let mut entries = std::collections::BTreeMap::new();
        for field in ["transfer-history1", "transfers1"] {
            if let Some(ValueOrContainer::Container(loro::Container::Map(map))) = meta.get(field) {
                map.for_each(|key, value| {
                    if let ValueOrContainer::Value(LoroValue::String(raw)) = value
                        && raw.len() <= 2048
                    {
                        entries.insert(key.to_string(), raw.to_string());
                    }
                });
            }
        }
        entries.into_iter().collect()
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
        for (_, value) in self.lineage_entries(node) {
            let candidate = (|| {
                let record: Transfer = serde_json::from_str(&value).ok()?;
                if record.version != 1 || record.count == 0 {
                    return None;
                }
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
        let entries = self.lineage_entries(node);
        let mut live = Vec::new();
        let mut rest = Vec::new();
        for (_, raw) in entries {
            let candidate = (|| {
                let Ok(record) = serde_json::from_str::<Transfer>(&raw) else {
                    return None;
                };
                if record.version != 1 || record.count == 0 {
                    return None;
                }
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
    fn malformed_lineage_does_not_partially_fail_a_join() {
        let doc = Document::new(1).unwrap();
        let first = doc
            .append_block(crate::BlockKind::Paragraph, "", "abc")
            .unwrap();
        let second = doc
            .append_block(crate::BlockKind::Paragraph, "", "def")
            .unwrap();
        doc.tree("content")
            .get_meta(second.0)
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
        let meta = doc.tree("content").get_meta(second.0).unwrap();
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
        record.node = NodeId(loro::TreeID {
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
