//! Anchored, overlapping text-style patches. See `docs/text-formatting.md`.
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;

use loro::Frontiers;

use reprise_diag::{Code, Note};
use reprise_geom::Length;
use serde::{Deserialize, Serialize};

use crate::text::RangePolicy;
use crate::{DocError, Document, NodeId, RangeId, get_str};

pub const FORMAT_UNREADABLE: Code = Code::new("style.format-unreadable");
pub const FORMAT_LIMIT: Code = Code::new("style.format-limit");
pub const MAX_FORMATS_PER_HOST: usize = 4096;
const MAX_RECORD_BYTES: usize = 32 * 1024;
pub const MAX_FORMAT_ORDER: u64 = (1 << 53) - 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextFeature {
    pub tag: [u8; 4],
    pub value: u32,
}

/// A patch over inherited paragraph text styling. Absent fields keep earlier
/// values. `reset` first clears every earlier text-format override.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub families: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<Length>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<TextFeature>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slant: Option<crate::TextSlant>,
    #[serde(skip_serializing_if = "crate::Decoration::is_empty")]
    pub decoration: crate::Decoration,
    /// Red, green, blue and alpha, each in 0..=255.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 4]>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
}

impl TextStyle {
    pub fn validate(&self) -> Result<(), DocError> {
        let valid = self.families.as_ref().is_none_or(|f| {
            !f.is_empty()
                && f.len() <= 64
                && f.iter()
                    .all(|s| !s.is_empty() && s.len() <= 256 && !s.contains('\0'))
        }) && self.weight.is_none_or(|w| (1..=1000).contains(&w))
            && self.size.is_none_or(|s| s > Length::ZERO)
            && self.language.as_ref().is_none_or(|s| {
                !s.is_empty() && s.len() <= 128 && s.is_ascii() && !s.chars().any(char::is_control)
            })
            && self.features.as_ref().is_none_or(|fs| {
                let tags: BTreeSet<_> = fs.iter().map(|f| f.tag).collect();
                fs.len() <= 64
                    && tags.len() == fs.len()
                    && fs
                        .iter()
                        .all(|f| f.tag.iter().all(|b| b.is_ascii_graphic()))
            });
        if valid {
            Ok(())
        } else {
            Err(DocError::Store("invalid text-style patch".into()))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormatRun {
    pub bytes: Range<usize>,
    pub style: TextStyle,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextFormats {
    pub runs: Vec<FormatRun>,
    pub notes: Vec<Note>,
    /// At least one readable action covers visible text, including reset actions.
    pub has_formatting: bool,
}

#[derive(Default)]
pub(crate) struct FormatCache {
    frontiers: Option<Frontiers>,
    index: Option<Arc<FormatIndex>>,
}

#[derive(Default)]
struct FormatIndex {
    hosts: BTreeMap<NodeId, HostFormats>,
    /// The largest readable order in the whole document.
    max_order: u64,
}

#[derive(Default)]
struct HostFormats {
    /// Live records, readable or not.
    count: usize,
    unreadable: bool,
    /// The highest-priority readable records, at most `MAX_FORMATS_PER_HOST`.
    records: BTreeMap<(u64, RangeId), TextStyle>,
    limited: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    order: u64,
    style: TextStyle,
}

fn read(raw: &str) -> Option<Record> {
    if raw.len() > MAX_RECORD_BYTES {
        return None;
    }
    let record: Record = serde_json::from_str(raw).ok()?;
    (record.version == 1 && record.order <= MAX_FORMAT_ORDER && record.style.validate().is_ok())
        .then_some(record)
}

impl Document {
    /// Snapshot a copied slice before changing either text. The result uses
    /// slice-local offsets and masks formatting inherited at its destination.
    pub(crate) fn prepare_format_copy(
        &self,
        source: NodeId,
        bytes: Range<usize>,
        target: Option<NodeId>,
    ) -> Result<Vec<FormatRun>, DocError> {
        let formats = self.text_formats(source)?;
        let (count, order) = self.text_format_capacity(target.unwrap_or(source))?;
        let count = if target.is_some() { count } else { 0 };
        if bytes.is_empty() || (!formats.has_formatting && count == 0) {
            return Ok(Vec::new());
        }
        let runs: Vec<_> = formats
            .runs
            .into_iter()
            .filter_map(|run| {
                let start = run.bytes.start.max(bytes.start);
                let end = run.bytes.end.min(bytes.end);
                (start < end).then(|| FormatRun {
                    bytes: start - bytes.start..end - bytes.start,
                    style: TextStyle {
                        reset: true,
                        ..run.style
                    },
                })
            })
            .collect();
        if count.saturating_add(runs.len()) > MAX_FORMATS_PER_HOST
            || order.saturating_add(runs.len() as u64) > MAX_FORMAT_ORDER
        {
            return Err(DocError::Store("text formatting copy limit".into()));
        }
        Ok(runs)
    }

    pub(crate) fn write_format_copy(
        &self,
        target: NodeId,
        at: usize,
        runs: Vec<FormatRun>,
    ) -> Result<(), DocError> {
        for run in runs {
            self.format_text(
                target,
                at + run.bytes.start..at + run.bytes.end,
                &run.style,
                RangePolicy::EXPANDING,
            )?;
        }
        Ok(())
    }

    /// Includes unreadable and tombstoned formatting records, so packages never
    /// advertise compatibility with readers that would ignore retained styling.
    pub fn has_text_formatting(&self) -> bool {
        if !self.has_ranges_root() {
            return false;
        }
        let tree = self.tree("ranges");
        tree.nodes()
            .into_iter()
            .any(|id| tree.get_meta(id).is_ok_and(|m| m.get("format1").is_some()))
    }

    /// The live formatting records, grouped by flow host. Built in one pass
    /// over the range tree and cached per revision, so resolving every
    /// paragraph costs one scan rather than one per paragraph.
    fn format_index(&self) -> Result<Arc<FormatIndex>, DocError> {
        if self.doc.get_pending_txn_len() > 0 {
            return self.build_format_index().map(Arc::new);
        }
        let frontiers = self.doc.state_frontiers();
        let mut cache = self.formats.lock().unwrap_or_else(|e| e.into_inner());
        if cache.frontiers.as_ref() == Some(&frontiers)
            && let Some(index) = &cache.index
        {
            return Ok(index.clone());
        }
        let index = Arc::new(self.build_format_index()?);
        cache.frontiers = Some(frontiers);
        cache.index = Some(index.clone());
        Ok(index)
    }

    /// Whether the `ranges` root exists. Merely opening a root container with
    /// `get_tree` materialises it in exported state, so reads that must not
    /// change saved bytes check first.
    fn has_ranges_root(&self) -> bool {
        matches!(self.doc.get_value(), loro::LoroValue::Map(m) if m.contains_key("ranges"))
    }

    fn build_format_index(&self) -> Result<FormatIndex, DocError> {
        let mut index = FormatIndex::default();
        if !self.has_ranges_root() {
            return Ok(index);
        }
        let tree = self.tree("ranges");
        for id in tree.nodes() {
            if !self.live(&tree, id) {
                continue;
            }
            let meta = tree.get_meta(id)?;
            let Some(raw) = get_str(&meta, "format1") else {
                // Present but not a string is unreadable too.
                if meta.get("format1").is_some()
                    && let Some(host) = get_str(&meta, "node")
                        .and_then(|s| NodeId::parse(&s))
                        .map(|n| n.host())
                {
                    let host = index.hosts.entry(host).or_default();
                    host.count += 1;
                    host.unreadable = true;
                }
                continue;
            };
            let record = read(&raw);
            if let Some(record) = &record {
                index.max_order = index.max_order.max(record.order);
            }
            let Some(host) = get_str(&meta, "node")
                .and_then(|s| NodeId::parse(&s))
                .map(|n| n.host())
            else {
                continue;
            };
            let host = index.hosts.entry(host).or_default();
            host.count += 1;
            let Some(record) = record else {
                host.unreadable = true;
                continue;
            };
            host.records
                .insert((record.order, RangeId(id)), record.style);
            if host.records.len() > MAX_FORMATS_PER_HOST {
                host.records.pop_first();
                host.limited = true;
            }
        }
        Ok(index)
    }

    /// The largest readable formatting order, and active records in this host.
    /// Count unreadable records too, so malformed state cannot bypass limits.
    pub fn text_format_capacity(&self, node: NodeId) -> Result<(usize, u64), DocError> {
        let index = self.format_index()?;
        let count = index.hosts.get(&node.host()).map_or(0, |h| h.count);
        Ok((count, index.max_order))
    }

    /// Adds an undoable formatting action. Does not end the editing step.
    /// Staging preserves the range ID across undo/redo.
    pub fn format_text(
        &self,
        node: NodeId,
        bytes: Range<usize>,
        style: &TextStyle,
        policy: RangePolicy,
    ) -> Result<RangeId, DocError> {
        style.validate()?;
        let text = self.block(node)?.text;
        text.slice(bytes.clone())?;
        if bytes.is_empty() {
            return Err(DocError::Store("empty formatting range".into()));
        }
        let (count, order) = self.text_format_capacity(node)?;
        if count >= MAX_FORMATS_PER_HOST || order >= MAX_FORMAT_ORDER {
            return Err(DocError::Store("text formatting limit".into()));
        }
        let raw = serde_json::to_string(&Record {
            version: 1,
            order: order + 1,
            style: style.clone(),
        })
        .map_err(|e| DocError::Store(e.to_string()))?;
        let id = self.stage_fragment_range(node, bytes, policy)?;
        // Keep the format envelope in staging too: redo only restores liveness.
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        self.tree("ranges").get_meta(id.0)?.insert("format1", raw)?;
        self.doc.commit();
        self.activate_fragment_range(id)?;
        Ok(id)
    }

    /// Hides one formatting action, revealing any lower-priority formatting.
    pub fn remove_text_format(&self, id: RangeId) -> Result<(), DocError> {
        let tree = self.tree("ranges");
        let meta = tree.get_meta(id.0)?;
        if !self.live(&tree, id.0) || meta.get("format1").is_none() {
            return Err(DocError::Store("no live text formatting action".into()));
        }
        meta.insert("deleted", true)?;
        Ok(())
    }

    /// Disjoint, grapheme-aligned overrides for this paragraph. The first scalar
    /// determines a grapheme's formatting; anchors are never rewritten.
    pub fn text_formats(&self, node: NodeId) -> Result<TextFormats, DocError> {
        let text = self.block(node)?.text.to_string();
        let index = self.format_index()?;
        let mut out = TextFormats::default();
        let empty = HostFormats::default();
        let host = index.hosts.get(&node.host()).unwrap_or(&empty);
        let (unreadable, limited) = (host.unreadable, host.limited);
        let records = &host.records;
        if unreadable {
            out.notes.push(Note::warning(
                FORMAT_UNREADABLE,
                "unreadable text formatting retained without application",
            ));
        }
        if limited {
            out.notes.push(Note::warning(
                FORMAT_LIMIT,
                "text formatting resolution limit",
            ));
        }
        if records.is_empty() {
            if !text.is_empty() {
                out.runs.push(FormatRun {
                    bytes: 0..text.len(),
                    style: TextStyle::default(),
                });
            }
            return Ok(out);
        }
        let bounds = crate::text::segment::grapheme_boundaries(&text);
        let ceil = |n| {
            bounds
                .get(bounds.partition_point(|&b| b < n))
                .copied()
                .unwrap_or(text.len())
        };
        let mut spans = Vec::new();
        let mut cuts = BTreeSet::from([0, text.len()]);
        for (&(_, id), style) in records {
            let Some(extent) = self.range_extent(id) else {
                continue;
            };
            let ((start_node, start), (end_node, end)) = (extent.start, extent.end);
            if start_node.host() != node.host()
                || end_node.host() != node.host()
                || self.flow_cmp(start_node, node).is_gt()
                || self.flow_cmp(node, end_node).is_gt()
            {
                continue;
            }
            let bytes = ceil(if start_node == node { start } else { 0 })
                ..ceil(if end_node == node { end } else { text.len() });
            if bytes.is_empty() {
                continue;
            }
            cuts.insert(bytes.start);
            cuts.insert(bytes.end);
            spans.push((bytes, style.clone()));
        }
        out.has_formatting = !spans.is_empty();
        // Sweep endpoints, maintaining the highest active action for each
        // property. Do not rescan every overlapping patch for every interval.
        let mut events = BTreeMap::<usize, Vec<(usize, bool)>>::new();
        for (i, (bytes, _)) in spans.iter().enumerate() {
            events.entry(bytes.start).or_default().push((i, true));
            events.entry(bytes.end).or_default().push((i, false));
        }
        let mut active: [BTreeSet<usize>; 10] = std::array::from_fn(|_| BTreeSet::new());
        let cuts: Vec<_> = cuts.into_iter().collect();
        for pair in cuts.windows(2) {
            let bytes = pair[0]..pair[1];
            for &(i, insert) in events.get(&bytes.start).into_iter().flatten() {
                let patch = &spans[i].1;
                let present = [
                    patch.families.is_some(),
                    patch.size.is_some(),
                    patch.language.is_some(),
                    patch.features.is_some(),
                    patch.weight.is_some(),
                    patch.slant.is_some(),
                    patch.decoration.underline.is_some(),
                    patch.decoration.strike.is_some(),
                    patch.color.is_some(),
                    patch.reset,
                ];
                for (set, present) in active.iter_mut().zip(present) {
                    if present {
                        if insert {
                            set.insert(i);
                        } else {
                            set.remove(&i);
                        }
                    }
                }
            }
            let reset = active[9].last().copied();
            let winner = |property: usize| {
                active[property]
                    .last()
                    .copied()
                    .filter(|&i| reset.is_none_or(|r| i >= r))
                    .map(|i| &spans[i].1)
            };
            let style = TextStyle {
                families: winner(0).and_then(|s| s.families.clone()),
                size: winner(1).and_then(|s| s.size),
                language: winner(2).and_then(|s| s.language.clone()),
                features: winner(3).and_then(|s| s.features.clone()),
                weight: winner(4).and_then(|s| s.weight),
                slant: winner(5).and_then(|s| s.slant),
                decoration: crate::Decoration {
                    underline: winner(6).and_then(|s| s.decoration.underline),
                    strike: winner(7).and_then(|s| s.decoration.strike),
                },
                color: winner(8).and_then(|s| s.color),
                reset: false,
            };
            if let Some(last) = out.runs.last_mut()
                && last.style == style
            {
                last.bytes.end = bytes.end;
            } else {
                out.runs.push(FormatRun { bytes, style });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockKind, PersistenceMode};

    fn doc(text: &str) -> (Document, NodeId) {
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", text).unwrap();
        doc.commit();
        (doc, node)
    }

    fn size(pt: i32) -> TextStyle {
        TextStyle {
            size: Some(Length::from_pt(pt)),
            ..TextStyle::default()
        }
    }

    #[test]
    fn overlaps_compose_properties_and_reset_reveals_paragraph_defaults() {
        let (doc, node) = doc("abcdef");
        doc.format_text(node, 0..5, &size(20), RangePolicy::EXPANDING)
            .unwrap();
        doc.format_text(
            node,
            2..6,
            &TextStyle {
                language: Some("fr".into()),
                ..TextStyle::default()
            },
            RangePolicy::EXPANDING,
        )
        .unwrap();
        let runs = doc.text_formats(node).unwrap().runs;
        assert_eq!(
            runs.iter().map(|r| r.bytes.clone()).collect::<Vec<_>>(),
            [0..2, 2..5, 5..6]
        );
        assert_eq!(runs[1].style.size, Some(Length::from_pt(20)));
        assert_eq!(runs[1].style.language.as_deref(), Some("fr"));
        let reset = doc
            .format_text(
                node,
                3..4,
                &TextStyle {
                    reset: true,
                    ..TextStyle::default()
                },
                RangePolicy::EXPANDING,
            )
            .unwrap();
        assert_eq!(
            doc.text_formats(node).unwrap().runs[2],
            FormatRun {
                bytes: 3..4,
                style: TextStyle::default()
            }
        );
        doc.remove_text_format(reset).unwrap();
        assert_eq!(doc.text_formats(node).unwrap().runs, runs);
    }

    #[test]
    fn formatting_survives_splits_typing_and_staged_undo_redo() {
        let (doc, node) = doc("abcd");
        let mut undo = doc.undo_stack();
        let id = doc
            .format_text(node, 1..4, &size(20), RangePolicy::EXPANDING)
            .unwrap();
        doc.commit_step();
        assert!(undo.undo().unwrap());
        assert_eq!(
            doc.text_formats(node).unwrap().runs[0].style,
            TextStyle::default()
        );
        assert!(undo.redo().unwrap());
        assert!(doc.range_extent(id).is_some());
        let tail = doc.split_block(node, 2).unwrap();
        doc.block(tail).unwrap().text.insert(0, "X").unwrap();
        assert_eq!(doc.text_formats(node).unwrap().runs[1].bytes, 1..2);
        assert_eq!(
            doc.text_formats(tail).unwrap().runs,
            [FormatRun {
                bytes: 0..3,
                style: size(20)
            }]
        );
        doc.join_blocks(node, tail).unwrap();
        assert_eq!(doc.text_formats(node).unwrap().runs[1].bytes, 1..5);
    }

    #[test]
    fn concurrent_overlaps_converge_and_roundtrip_both_persistence_modes() {
        let (a, node) = doc("abcdef");
        let b = a.fork(2).unwrap();
        a.format_text(node, 1..5, &size(20), RangePolicy::FIXED)
            .unwrap();
        b.format_text(node, 2..6, &size(30), RangePolicy::EXPANDING)
            .unwrap();
        a.commit();
        b.commit();
        a.merge(&b).unwrap();
        b.merge(&a).unwrap();
        assert_eq!(a.text_formats(node).unwrap(), b.text_formats(node).unwrap());
        for mode in [PersistenceMode::History, PersistenceMode::Shallow] {
            let reopened = Document::import(&a.try_export(mode).unwrap(), 3).unwrap();
            assert_eq!(
                a.text_formats(node).unwrap(),
                reopened.text_formats(node).unwrap()
            );
        }
        // A later action observes both peers and outranks both.
        a.format_text(node, 0..6, &size(40), RangePolicy::EXPANDING)
            .unwrap();
        assert_eq!(
            a.text_formats(node).unwrap().runs,
            [FormatRun {
                bytes: 0..6,
                style: size(40)
            }]
        );
    }

    #[test]
    fn graphemes_use_the_style_of_their_first_scalar_without_mutating_anchors() {
        let (doc, node) = doc("e\u{301}x");
        let id = doc
            .format_text(node, 1..4, &size(20), RangePolicy::FIXED)
            .unwrap();
        doc.commit();
        let before = doc.revision();
        assert_eq!(
            doc.text_formats(node).unwrap().runs,
            [
                FormatRun {
                    bytes: 0..3,
                    style: TextStyle::default()
                },
                FormatRun {
                    bytes: 3..4,
                    style: size(20)
                },
            ]
        );
        assert_eq!(doc.range_extent(id).unwrap().start, (node, 1));
        assert_eq!(doc.revision(), before);
    }

    #[test]
    fn malformed_envelopes_are_reported_and_kept_verbatim() {
        let (doc, node) = doc("abc");
        let id = doc
            .format_text(node, 0..3, &size(20), RangePolicy::FIXED)
            .unwrap();
        let raw = r#"{"version":99,"order":1,"style":{}}"#;
        let meta = doc.tree("ranges").get_meta(id.0).unwrap();
        meta.insert("format1", raw).unwrap();
        doc.commit();
        let before = doc.revision();
        let formats = doc.text_formats(node).unwrap();
        assert_eq!(formats.notes[0].code, FORMAT_UNREADABLE);
        assert_eq!(formats.runs[0].style, TextStyle::default());
        assert_eq!(get_str(&meta, "format1").as_deref(), Some(raw));
        assert_eq!(doc.revision(), before);
    }

    #[test]
    fn invalid_authoring_does_not_stage_or_change_revision() {
        let (doc, node) = doc("éabc");
        let before = doc.revision();
        for (bytes, style) in [
            (1..3, size(20)),
            (0..2, size(0)),
            (0..0, size(20)),
            (0..99, size(20)),
        ] {
            assert!(
                doc.format_text(node, bytes, &style, RangePolicy::EXPANDING)
                    .is_err()
            );
            assert_eq!(doc.revision(), before);
        }
        assert!(!doc.has_text_formatting());
    }

    #[test]
    fn excess_imported_records_are_bounded_without_repairing_state() {
        let (doc, node) = doc("abc");
        for order in 0..=MAX_FORMATS_PER_HOST {
            let id = doc.add_range(node, 0..3, RangePolicy::FIXED).unwrap();
            let raw = serde_json::to_string(&Record {
                version: 1,
                order: order as u64,
                style: size(if order == MAX_FORMATS_PER_HOST {
                    30
                } else {
                    20
                }),
            })
            .unwrap();
            doc.tree("ranges")
                .get_meta(id.0)
                .unwrap()
                .insert("format1", raw)
                .unwrap();
        }
        doc.commit();
        let before = doc.revision();
        let value = doc.text_formats(node).unwrap();
        assert!(value.notes.iter().any(|n| n.code == FORMAT_LIMIT));
        assert_eq!(
            value.runs,
            [FormatRun {
                bytes: 0..3,
                style: size(30)
            }]
        );
        assert_eq!(doc.revision(), before);
        assert!(
            doc.format_text(node, 0..3, &size(40), RangePolicy::EXPANDING)
                .is_err()
        );
        assert_eq!(doc.revision(), before);
    }
}
