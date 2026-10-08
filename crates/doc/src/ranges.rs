//! Authored range policies, stored atomically beside their cursors (10, 12, 34).
use loro::{LoroMap, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};

use crate::text::{Affinity, Empty, RangePolicy};
use crate::{DocError, Document, RangeId};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyEnvelope {
    version: u32,
    start: Affinity,
    end: Affinity,
    empty: Empty,
}

/// The longest stored anchor read back. A Loro cursor is a few dozen bytes.
const MAX_STORED_ANCHOR: usize = 256;

/// How a range end is stored: hex text, so that it survives every sync path
/// unchanged. (Bytes, as written before, turn into a list of numbers in the
/// JSON delta encoding, so the receiver held a different value.)
pub(crate) fn anchor_value(anchor: &crate::text::Anchor) -> String {
    use std::fmt::Write;
    let bytes = anchor.encode();
    let mut out = String::with_capacity(2 + 2 * bytes.len());
    out.push_str("a1");
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The bytes of a stored range end, from any form it has been stored in:
/// hex text, bytes, or a list of byte values (what the JSON delta encoding
/// made of bytes).
pub(crate) fn stored_anchor(value: Option<ValueOrContainer>) -> Option<Vec<u8>> {
    match value? {
        ValueOrContainer::Value(LoroValue::String(s)) => {
            let hex = s.strip_prefix("a1")?;
            if hex.len() % 2 != 0 || hex.len() > 2 * MAX_STORED_ANCHOR {
                return None;
            }
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
                .collect()
        }
        ValueOrContainer::Value(LoroValue::Binary(b)) if b.len() <= MAX_STORED_ANCHOR => {
            Some(b.to_vec())
        }
        ValueOrContainer::Value(LoroValue::List(items)) if items.len() <= MAX_STORED_ANCHOR => {
            items
                .iter()
                .map(|v| match v {
                    LoroValue::I64(n) => u8::try_from(*n).ok(),
                    _ => None,
                })
                .collect()
        }
        _ => None,
    }
}

pub(super) fn write_policy(meta: &LoroMap, policy: RangePolicy) -> Result<(), DocError> {
    let raw = serde_json::to_string(&PolicyEnvelope {
        version: 1,
        start: policy.start,
        end: policy.end,
        empty: policy.empty,
    })
    .map_err(|e| DocError::Store(e.to_string()))?;
    meta.insert("policy1", raw)?;
    Ok(())
}

impl Document {
    /// Returns the authored policy; `None` identifies a legacy range with only cursors.
    /// Unsupported or malformed envelopes are refused and kept verbatim in the store.
    /// A missing or deleted range is an error, rather than a legacy range.
    pub fn range_policy(&self, id: RangeId) -> Result<Option<RangePolicy>, DocError> {
        let tree = self.tree("ranges");
        if !self.live(&tree, id.0) {
            return Err(DocError::Store(format!("no live range {id}")));
        }
        let meta = tree.get_meta(id.0)?;
        let Some(value) = meta.get("policy1") else {
            return Ok(None);
        };
        let ValueOrContainer::Value(LoroValue::String(raw)) = value else {
            return Err(DocError::Store(format!("invalid policy for range {id}")));
        };
        // A v1 policy is under 100 bytes. Refuse oversized or deeply nested future
        // payloads before parsing; persistence still retains the original bytes.
        if raw.len() > 1024 {
            return Err(DocError::Store(format!(
                "range {id} policy exceeds byte limit"
            )));
        }
        let envelope: PolicyEnvelope = serde_json::from_str(&raw)
            .map_err(|_| DocError::Store(format!("unreadable policy for range {id}")))?;
        if envelope.version != 1 {
            return Err(DocError::Store(format!(
                "unsupported range policy version {} for range {id}",
                envelope.version
            )));
        }
        Ok(Some(RangePolicy {
            start: envelope.start,
            end: envelope.end,
            empty: envelope.empty,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockKind, PersistenceMode, RangeState, SchemaRegistry};

    fn range(doc: &Document) -> (crate::NodeId, RangeId) {
        let node = doc
            .append_block(BlockKind::Paragraph, "", "\u{e9}\u{5d0}\u{5d1}")
            .unwrap();
        let id = doc.add_range(node, 0..6, RangePolicy::FIXED).unwrap();
        (node, id)
    }

    #[test]
    fn all_policies_survive_history_shallow_fork_and_merge() {
        let doc = Document::new(1).unwrap();
        let node = doc
            .append_block(BlockKind::Paragraph, "", "\u{e9}\u{5d0}\u{5d1}")
            .unwrap();
        let mut ranges = Vec::new();
        for start in [Affinity::Before, Affinity::After] {
            for end in [Affinity::Before, Affinity::After] {
                for empty in [Empty::Keep, Empty::Missing] {
                    let policy = RangePolicy { start, end, empty };
                    let id = doc.add_range(node, 0..6, policy).unwrap();
                    ranges.push((id, policy));
                }
            }
        }
        for mode in [PersistenceMode::History, PersistenceMode::Shallow] {
            let restored = Document::import(&doc.try_export(mode).unwrap(), 3).unwrap();
            for &(id, policy) in &ranges {
                assert_eq!(restored.range_policy(id).unwrap(), Some(policy));
            }
        }
        let other = doc.fork(2).unwrap();
        let id = ranges[0].0;
        doc.reanchor_fragment_range(id, node, 0..6, RangePolicy::POINT)
            .unwrap();
        other
            .reanchor_fragment_range(id, node, 0..6, RangePolicy::EXPANDING)
            .unwrap();
        doc.merge(&other).unwrap();
        other.merge(&doc).unwrap();
        assert_eq!(
            doc.range_policy(id).unwrap(),
            other.range_policy(id).unwrap()
        );
        let chosen = doc.range_policy(id).unwrap().unwrap();
        assert!([RangePolicy::POINT, RangePolicy::EXPANDING].contains(&chosen));
        for &(id, policy) in ranges.iter().skip(1) {
            assert_eq!(other.range_policy(id).unwrap(), Some(policy));
        }
    }

    #[test]
    fn legacy_only_ranges_use_cursor_policy_and_warn_at_endpoints() {
        let doc = Document::new(1).unwrap();
        let (node, id) = range(&doc);
        let meta = doc.tree("ranges").get_meta(id.0).unwrap();
        meta.delete("policy1").unwrap();
        meta.insert("empty", "keep").unwrap();
        assert_eq!(doc.range_policy(id).unwrap(), None);
        let fragment = doc
            .copy_all_fragment("legacy", &SchemaRegistry::builtin())
            .unwrap();
        assert_eq!(fragment.ranges.len(), 1);
        assert_eq!(fragment.ranges[0].policy.empty, Empty::Keep);
        assert_eq!(fragment.notes[0].code.as_str(), "clipboard.range-affinity");
        assert_eq!(fragment.notes[0].severity, reprise_diag::Severity::Warning);
        doc.block(node).unwrap().text.delete(0..6).unwrap();
        assert!(
            matches!(doc.resolve_range(id), RangeState::Rebound { bytes, .. } if bytes.is_empty())
        );
        let restored =
            Document::import(&doc.try_export(PersistenceMode::History).unwrap(), 2).unwrap();
        assert_eq!(restored.range_policy(id).unwrap(), None);
    }

    #[test]
    fn future_and_malformed_policies_refuse_without_mutating_raw_payload() {
        let doc = Document::new(1).unwrap();
        let (node, id) = range(&doc);
        let meta = doc.tree("ranges").get_meta(id.0).unwrap();
        for raw in [
            r#"{"version":2,"start":"after","end":"before","empty":"missing"}"#.to_owned(),
            r#"{"version":1,"start":"after","end":"before","empty":"missing","future":true}"#
                .to_owned(),
            r#"{"version":1,"start":"unknown","end":"before","empty":"keep"}"#.to_owned(),
            "{".into(),
            "[".repeat(512),
            "x".repeat(1 << 20),
        ] {
            meta.insert("policy1", raw.clone()).unwrap();
            doc.commit();
            let before = doc.revision();
            assert!(doc.range_policy(id).is_err());
            assert!(matches!(doc.resolve_range(id), RangeState::Missing { .. }));
            assert!(
                doc.copy_all_fragment("future", &SchemaRegistry::builtin())
                    .is_err()
            );
            assert!(
                doc.reanchor_fragment_range(id, node, 0..6, RangePolicy::FIXED)
                    .is_err()
            );
            assert_eq!(before, doc.revision());
            let restored =
                Document::import(&doc.try_export(PersistenceMode::History).unwrap(), 2).unwrap();
            assert!(restored.range_policy(id).is_err());
            assert_eq!(
                crate::get_str(&restored.tree("ranges").get_meta(id.0).unwrap(), "policy1"),
                Some(raw)
            );
        }
        meta.insert("policy1", 42_i64).unwrap();
        assert!(doc.range_policy(id).is_err());
    }

    #[test]
    fn empty_policy_is_authoritative_and_invalid_creation_writes_nothing() {
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "").unwrap();
        let point = doc.add_range(node, 0..0, RangePolicy::POINT).unwrap();
        let missing = doc.add_range(node, 0..0, RangePolicy::FIXED).unwrap();
        doc.tree("ranges")
            .get_meta(point.0)
            .unwrap()
            .insert("empty", "missing")
            .unwrap();
        doc.tree("ranges")
            .get_meta(missing.0)
            .unwrap()
            .insert("empty", "keep")
            .unwrap();
        assert!(
            matches!(doc.resolve_range(point), RangeState::Valid { bytes, .. } if bytes.is_empty())
        );
        assert!(matches!(
            doc.resolve_range(missing),
            RangeState::Missing { .. }
        ));
        doc.commit();
        let before = doc.revision();
        assert!(
            doc.add_range(node, 0..usize::MAX, RangePolicy::FIXED)
                .is_err()
        );
        assert!(
            doc.add_range(
                node,
                std::ops::Range { start: 1, end: 0 },
                RangePolicy::FIXED
            )
            .is_err()
        );
        doc.commit();
        assert_eq!(before, doc.revision());
        let absent = RangeId::parse("2147483646@999").unwrap();
        assert!(doc.range_policy(absent).is_err());
    }
}
