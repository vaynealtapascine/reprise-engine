//! Portable authored fragments. IDs in this envelope are source labels, never allocations.
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use loro::{Container, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};

use crate::relation::{CopyAction, CopySet, plan_copy};
use crate::text::{Affinity, Empty, RangePolicy};
use crate::{
    BlockKind, DocError, Document, NodeId, RangeId, RangeState, Relation, RelationId,
    SchemaRegistry, Style, get_str,
};

pub const FRAGMENT_VERSION: u32 = 1;
pub const MAX_FRAGMENT_BLOCKS: usize = 4096;
pub const MAX_FRAGMENT_BYTES: usize = 16 << 20;
pub const MAX_FRAGMENT_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentBlock {
    pub id: NodeId,
    /// Preorder parent: it must occur before this block.
    pub parent: Option<NodeId>,
    pub kind: BlockKind,
    pub style: String,
    pub overrides: Style,
    pub text: String,
    /// The versioned table metadata, including unsupported forms, kept verbatim.
    pub table: Option<String>,
    /// Versioned image metadata, kept verbatim even when unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Effective paragraph-local text overrides, flattened from anchored actions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formatting: Vec<crate::formatting::FormatRun>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentRange {
    pub id: RangeId,
    pub node: NodeId,
    pub bytes: Range<usize>,
    pub policy: RangePolicy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentRelation {
    pub id: RelationId,
    pub relation: Relation,
}

/// A source namespace supplied by the host (normally the package's document ID).
/// Equality authorizes retaining external targets; different peer IDs alone don't.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fragment {
    pub version: u32,
    pub source: String,
    pub blocks: Vec<FragmentBlock>,
    pub styles: BTreeMap<String, Style>,
    pub ranges: Vec<FragmentRange>,
    pub relations: Vec<FragmentRelation>,
    /// Copy-all carries authored page setup; partial copies leave this empty.
    pub templates: BTreeMap<String, String>,
    pub template_choice: Option<String>,
    #[serde(default)]
    pub notes: Vec<reprise_diag::Note>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FragmentError {
    #[error("unsupported fragment version {0}")]
    Version(u32),
    #[error("fragment exceeds the {0} limit")]
    Limit(&'static str),
    #[error("invalid fragment: {0}")]
    Invalid(String),
}

impl Fragment {
    /// Reject all structural and byte-boundary errors before allocating target IDs.
    pub fn validate(&self) -> Result<(), FragmentError> {
        if self.version != FRAGMENT_VERSION {
            return Err(FragmentError::Version(self.version));
        }
        if self.source.is_empty() {
            return Err(FragmentError::Invalid("empty source namespace".into()));
        }
        if self.blocks.len() > MAX_FRAGMENT_BLOCKS
            || self.ranges.len() > 8192
            || self.relations.len() > 8192
            || self.styles.len() > 1024
            || self.templates.len() > 1024
        {
            return Err(FragmentError::Limit("items"));
        }
        let mut nodes = BTreeMap::new();
        let mut formatting_count = 0usize;
        for block in &self.blocks {
            let mut previous = 0;
            for run in &block.formatting {
                formatting_count = formatting_count.saturating_add(1);
                if formatting_count > crate::formatting::MAX_FORMATS_PER_HOST {
                    return Err(FragmentError::Limit("text formatting runs"));
                }
                if run.bytes.is_empty()
                    || run.bytes.start < previous
                    || block.text.get(run.bytes.clone()).is_none()
                {
                    return Err(FragmentError::Invalid(
                        "invalid text formatting range".into(),
                    ));
                }
                run.style
                    .validate()
                    .map_err(|e| FragmentError::Invalid(e.to_string()))?;
                previous = run.bytes.end;
            }
            if block.text.contains(crate::text::BREAK) {
                return Err(FragmentError::Invalid(
                    "reserved flow marker in authored text".into(),
                ));
            }
            let depth = match block.parent {
                Some(p) => nodes.get(&p).map(|(_, d)| d + 1).ok_or_else(|| {
                    FragmentError::Invalid("parent is not earlier in preorder".into())
                })?,
                None => 0usize,
            };
            if depth > MAX_FRAGMENT_DEPTH {
                return Err(FragmentError::Limit("tree depth"));
            }
            if nodes.insert(block.id, (block, depth)).is_some() {
                return Err(FragmentError::Invalid("duplicate node ID".into()));
            }
        }
        let mut ranges = BTreeSet::new();
        for range in &self.ranges {
            let (block, _) = nodes
                .get(&range.node)
                .ok_or_else(|| FragmentError::Invalid("range node is absent".into()))?;
            if !ranges.insert(range.id)
                || range.bytes.start > range.bytes.end
                || block.text.get(range.bytes.clone()).is_none()
            {
                return Err(FragmentError::Invalid(
                    "duplicate range or invalid UTF-8 range".into(),
                ));
            }
        }
        let mut relations = BTreeSet::new();
        for relation in &self.relations {
            if !relations.insert(relation.id)
                || relation
                    .relation
                    .owner
                    .is_some_and(|n| !nodes.contains_key(&n))
            {
                return Err(FragmentError::Invalid(
                    "duplicate relation or owner outside fragment".into(),
                ));
            }
        }
        let bytes = serde_json::to_vec(self).map_err(|e| FragmentError::Invalid(e.to_string()))?;
        if bytes.len() > MAX_FRAGMENT_BYTES {
            return Err(FragmentError::Limit("payload bytes"));
        }
        Ok(())
    }
}

/// Whole blocks expand to their entire subtree. Partial ranges don't include children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyBlock {
    pub node: NodeId,
    pub bytes: Option<Range<usize>>,
}

impl Document {
    /// Supplies inferred columns after a conservative HTML table import.
    /// Does not commit; callers include this authored metadata in their step.
    pub fn set_fragment_table_columns(
        &self,
        node: NodeId,
        columns: crate::TableColumns,
    ) -> Result<(), DocError> {
        if !matches!(self.table_role(node)?, Some(crate::TableRole::Table(_))) {
            return Err(DocError::Malformed(node, "table role"));
        }
        #[derive(Serialize)]
        struct Envelope {
            version: u32,
            role: crate::TableRole,
        }
        let raw = serde_json::to_string(&Envelope {
            version: 1,
            role: crate::TableRole::Table(columns),
        })
        .map_err(|e| DocError::Store(e.to_string()))?;
        self.tree("content")
            .get_meta(node.node)?
            .insert("table1", raw)?;
        Ok(())
    }

    /// Extract live authored content deterministically. The caller reports policy losses.
    pub fn copy_fragment(
        &self,
        source: &str,
        selection: &[CopyBlock],
        schemas: &SchemaRegistry,
    ) -> Result<Fragment, FragmentError> {
        let bad = |e: DocError| FragmentError::Invalid(e.to_string());
        let mut fragment = Fragment {
            version: FRAGMENT_VERSION,
            source: source.into(),
            blocks: Vec::new(),
            styles: BTreeMap::new(),
            ranges: Vec::new(),
            relations: Vec::new(),
            templates: BTreeMap::new(),
            template_choice: None,
            notes: Vec::new(),
        };
        let mut cuts = BTreeMap::new();
        let mut todo: Vec<_> = selection
            .iter()
            .rev()
            .map(|s| (s.node, s.bytes.clone(), None, 0usize))
            .collect();
        while let Some((node, cut, parent, depth)) = todo.pop() {
            if depth > MAX_FRAGMENT_DEPTH || fragment.blocks.len() >= MAX_FRAGMENT_BLOCKS {
                return Err(FragmentError::Limit("tree"));
            }
            if cuts.contains_key(&node) {
                return Err(FragmentError::Invalid("overlapping block selection".into()));
            }
            let block = self.block(node).map_err(bad)?;
            let bytes = cut.clone().unwrap_or(0..block.text.len());
            let text = block
                .text
                .slice(bytes.clone())
                .map_err(|e| FragmentError::Invalid(e.to_string()))?;
            let meta = self
                .meta_of(node)
                .map_err(|e| FragmentError::Invalid(e.to_string()))?;
            let style = block.style.unwrap_or_default();
            for name in [Some(style.clone()), block.overrides.parent.clone()]
                .into_iter()
                .flatten()
            {
                let mut next = Some(name);
                for _ in 0..1024 {
                    let Some(name) = next.take() else { break };
                    if fragment.styles.contains_key(&name) {
                        break;
                    }
                    let Some(style) = self.style(&name) else {
                        break;
                    };
                    next = style.parent.clone();
                    fragment.styles.insert(name, style);
                    if fragment.styles.len() > 1024 {
                        return Err(FragmentError::Limit("styles"));
                    }
                }
                if next.is_some() {
                    return Err(FragmentError::Limit("style chain"));
                }
            }
            let formats = self.text_formats(node).map_err(bad)?;
            fragment.notes.extend(formats.notes);
            let formatting = if formats.has_formatting {
                formats
                    .runs
                    .into_iter()
                    .filter_map(|run| {
                        let start = run.bytes.start.max(bytes.start);
                        let end = run.bytes.end.min(bytes.end);
                        (start < end).then_some(crate::formatting::FormatRun {
                            bytes: start.saturating_sub(bytes.start)
                                ..end.saturating_sub(bytes.start),
                            style: run.style,
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            fragment.blocks.push(FragmentBlock {
                id: node,
                parent,
                kind: block.kind,
                style,
                overrides: block.overrides,
                text,
                table: get_str(&meta, "table1"),
                image: get_str(&meta, "image1"),
                formatting,
            });
            cuts.insert(node, bytes);
            if cut.is_none() {
                todo.extend(
                    self.children(Some(node))
                        .into_iter()
                        .rev()
                        .map(|n| (n, None, Some(node), depth + 1)),
                );
            }
        }
        let tree = self.tree("ranges");
        let mut ids = tree.nodes();
        ids.sort();
        for id in ids {
            if !self.live(&tree, id) {
                continue;
            }
            let range_id = RangeId(id);
            let meta = tree
                .get_meta(id)
                .map_err(|e| FragmentError::Invalid(e.to_string()))?;
            // Formatting actions travel flattened in `FragmentBlock::formatting`;
            // copying their records too would duplicate them as bare ranges.
            if meta.get("format1").is_some() {
                continue;
            }
            let node = get_str(&meta, "node").and_then(|n| NodeId::parse(&n));
            let resolved = self.resolve_range(range_id);
            let resolved_node = match &resolved {
                RangeState::Valid { node, .. } | RangeState::Rebound { node, .. } => Some(*node),
                RangeState::Missing { .. } => None,
            };
            if !node.is_some_and(|n| cuts.contains_key(&n))
                && !resolved_node.is_some_and(|n| cuts.contains_key(&n))
            {
                continue;
            }
            // Read before resolution: an unreadable policy must refuse the copy,
            // rather than silently omit the range as missing.
            let authored_policy = self.range_policy(range_id).map_err(bad)?;
            let (RangeState::Valid { node, bytes } | RangeState::Rebound { node, bytes }) =
                resolved
            else {
                continue;
            };
            let Some(cut) = cuts.get(&node) else { continue };
            // Intersect nonempty ranges; point ranges at either edge belong to the copy.
            let start = bytes.start.max(cut.start);
            let end = bytes.end.min(cut.end);
            if start > end || (start == end && !bytes.is_empty()) {
                continue;
            }
            let meta = tree
                .get_meta(id)
                .map_err(|e| FragmentError::Invalid(e.to_string()))?;
            let affinity = |key| -> Option<Affinity> {
                let b = crate::ranges::stored_anchor(meta.get(key))?;
                let cursor = loro::cursor::Cursor::decode(&b).ok()?;
                Some(
                    if cursor.side == loro::cursor::Side::Right && cursor.id.is_some() {
                        Affinity::Before
                    } else {
                        Affinity::After
                    },
                )
            };
            let policy = if let Some(policy) = authored_policy {
                policy
            } else {
                RangePolicy {
                    start: affinity("start")
                        .ok_or_else(|| FragmentError::Invalid("range start".into()))?,
                    end: affinity("end")
                        .ok_or_else(|| FragmentError::Invalid("range end".into()))?,
                    empty: if get_str(&meta, "empty").as_deref() == Some("keep") {
                        Empty::Keep
                    } else {
                        Empty::Missing
                    },
                }
            };
            if authored_policy.is_none()
                && (bytes.start == 0 || bytes.end == self.block(node).map_err(bad)?.text.len())
                && !fragment
                    .notes
                    .iter()
                    .any(|n| n.code == "clipboard.range-affinity")
            {
                fragment.notes.push(reprise_diag::Note::warning(reprise_diag::Code::new("clipboard.range-affinity"), "the legacy range has no authored endpoint affinities; fragment uses observable cursor behavior"));
            }
            fragment.ranges.push(FragmentRange {
                id: range_id,
                node,
                bytes: start - cut.start..end - cut.start,
                policy,
            });
        }
        let copied = CopySet {
            nodes: cuts.keys().copied().collect(),
            ranges: fragment.ranges.iter().map(|r| r.id).collect(),
        };
        let relations = self.relations();
        for entry in plan_copy(schemas, &relations, &copied) {
            if !matches!(entry.action, CopyAction::Drop(_))
                && let Some((_, Ok(relation))) =
                    relations.iter().find(|(id, _)| *id == entry.relation)
            {
                fragment.relations.push(FragmentRelation {
                    id: entry.relation,
                    relation: relation.clone(),
                });
            }
        }
        fragment.validate()?;
        Ok(fragment)
    }

    pub fn copy_all_fragment(
        &self,
        source: &str,
        schemas: &SchemaRegistry,
    ) -> Result<Fragment, FragmentError> {
        let selection: Vec<_> = self
            .blocks()
            .into_iter()
            .map(|node| CopyBlock { node, bytes: None })
            .collect();
        let mut fragment = self.copy_fragment(source, &selection, schemas)?;
        let map = self.doc.get_map("page_templates");
        for key in map.keys() {
            if let Some(ValueOrContainer::Value(LoroValue::String(raw))) = map.get(&key) {
                fragment.templates.insert(key.to_string(), raw.to_string());
            }
        }
        fragment.template_choice = get_str(&self.doc.get_map("page_setup"), "template");
        fragment.validate()?;
        Ok(fragment)
    }

    /// Adds metadata to an invisible staged block, outside the undo step.
    pub fn stage_fragment_table(&self, node: NodeId, raw: &str) -> Result<(), DocError> {
        if !self.is_soft_deleted(node) {
            return Err(DocError::NoNode(node));
        }
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        self.tree("content")
            .get_meta(node.node)?
            .insert("table1", raw)?;
        self.doc.commit();
        Ok(())
    }

    /// Stages a range on already-staged text; clearing its flag is the undoable part.
    pub fn stage_fragment_range(
        &self,
        node: NodeId,
        bytes: Range<usize>,
        policy: RangePolicy,
    ) -> Result<RangeId, DocError> {
        if bytes.start > bytes.end {
            return Err(crate::text::TextError::BadRange(bytes).into());
        }
        // A live block is read through its view (a paragraph of a flow has
        // offsets of its own); a staged one is a whole text.
        let text = match self.block(node) {
            Ok(block) => block.text,
            Err(_) => {
                let meta = self.tree("content").get_meta(node.node)?;
                let Some(ValueOrContainer::Container(Container::Text(text))) = meta.get("text")
                else {
                    return Err(DocError::Malformed(node, "text"));
                };
                crate::text::Text::from_loro(text)
            }
        };
        let start = text.anchor(bytes.start, policy.start)?;
        let end = text.anchor(bytes.end, policy.end)?;
        self.commit_part();
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        let tree = self.tree("ranges");
        let id = tree.create(None)?;
        let meta = tree.get_meta(id)?;
        meta.insert("deleted", true)?;
        meta.insert("node", node.to_string())?;
        meta.insert("start", crate::ranges::anchor_value(&start))?;
        meta.insert("end", crate::ranges::anchor_value(&end))?;
        crate::ranges::write_policy(&meta, policy)?;
        self.doc.commit();
        Ok(RangeId(id))
    }

    pub fn activate_fragment_range(&self, id: RangeId) -> Result<(), DocError> {
        self.tree("ranges")
            .get_meta(id.0)?
            .insert("deleted", false)?;
        Ok(())
    }

    /// Moves an existing range onto replacement text without changing its identity.
    /// This is authored and undoable; the operation does not commit.
    pub fn reanchor_fragment_range(
        &self,
        id: RangeId,
        node: NodeId,
        bytes: Range<usize>,
        policy: RangePolicy,
    ) -> Result<(), DocError> {
        if bytes.start > bytes.end {
            return Err(crate::text::TextError::BadRange(bytes).into());
        }
        let tree = self.tree("ranges");
        if !self.live(&tree, id.0) {
            return Err(DocError::Store(format!("no live range {id}")));
        }
        // Never overwrite a policy this engine cannot understand (34).
        self.range_policy(id)?;
        let text = self.block(node)?.text;
        let start = text.anchor(bytes.start, policy.start)?;
        let end = text.anchor(bytes.end, policy.end)?;
        let meta = tree.get_meta(id.0)?;
        meta.insert("node", node.to_string())?;
        meta.insert("start", crate::ranges::anchor_value(&start))?;
        meta.insert("end", crate::ranges::anchor_value(&end))?;
        crate::ranges::write_policy(&meta, policy)?;
        Ok(())
    }
}
