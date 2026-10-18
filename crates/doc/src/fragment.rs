//! Portable authored fragments. IDs in this envelope are source labels, never allocations.
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use loro::{Container, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};

use crate::annotation::OnEmpty;
use crate::relation::{CopyAction, CopySet, plan_copy};
use crate::text::{Affinity, Empty, RangePolicy};
use crate::{
    BlockKind, DocError, Document, NodeId, RangeId, Relation, RelationId, SchemaRegistry, Style,
    get_str,
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
    /// The block's role, when it is not body (see `docs/roles-and-lists.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<crate::Role>,
    /// Its list membership, naming a definition in [`Fragment::lists`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<FragmentItem>,
    /// The inline objects of `text`'s U+FFFC characters, one each, in order
    /// (see `docs/inline.md`). Pasting gives them new identity. Older engines
    /// refuse a fragment that has them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inline: Vec<crate::inline::InlineObject>,
}

/// A copied block's list membership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentItem {
    /// A source label: a key of [`Fragment::lists`].
    pub list: crate::ListId,
    pub level: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<i64>,
}

/// The most list definitions one fragment carries.
pub const MAX_FRAGMENT_LISTS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentRange {
    pub id: RangeId,
    pub node: NodeId,
    pub bytes: Range<usize>,
    pub policy: RangePolicy,
    /// Optional last endpoint for a span crossing copied paragraphs. The
    /// legacy node/bytes describe its first piece and encode identically when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<FragmentEndpoint>,
    /// The annotation this range carries (see `docs/annotations.md`).
    /// Absent from fragments without annotations; older strict readers
    /// refuse a fragment that has one rather than paste a bare range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation: Option<crate::annotation::AnnotationData>,
}

/// A copied range's final paragraph-local UTF-8 endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentEndpoint {
    pub node: NodeId,
    pub at: usize,
}

/// What a copy is for. It decides which annotations come along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyScope {
    /// Text the user copies to paste somewhere: annotation kinds whose
    /// [`CopyPolicy`] is `Drop` (a comment thread has one anchor) stay behind.
    Clipboard,
    /// The whole document rebuilt as another (a duplicate): every readable
    /// annotation comes along, orphaned ones included, with new identities.
    Document,
}

/// An annotation was left out of a copy or paste: a kind that is not
/// copied (Info), or a link whose internal target is outside the copy and
/// the target document (Warning: the text is pasted without its link).
pub const ANNOTATION_DROPPED: reprise_diag::Code =
    reprise_diag::Code::new("clipboard.annotation-dropped");

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
    /// Bounded versioned raw page properties; copy-all only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_setup_patch: Option<String>,
    #[serde(default)]
    pub notes: Vec<reprise_diag::Note>,
    /// The list definitions the copied items use, by source label. Paste
    /// gives them fresh IDs. A `base` outside the fragment is dropped.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lists: BTreeMap<crate::ListId, crate::ListDefinition>,
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
        if let Some(raw) = &self.page_setup_patch {
            Document::validate_raw_page_setup(raw)
                .map_err(|e| FragmentError::Invalid(e.to_string()))?;
        }
        if self.lists.len() > MAX_FRAGMENT_LISTS {
            return Err(FragmentError::Limit("lists"));
        }
        for def in self.lists.values() {
            def.validate()
                .map_err(|e| FragmentError::Invalid(e.to_string()))?;
        }
        for block in &self.blocks {
            if block.role.is_some_and(|r| !r.is_valid()) {
                return Err(FragmentError::Invalid("heading levels are 1 to 6".into()));
            }
            if let Some(item) = &block.list
                && (item.level > crate::lists::MAX_LEVEL || !self.lists.contains_key(&item.list))
            {
                return Err(FragmentError::Invalid("list item".into()));
            }
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
            if block.text.matches(crate::text::OBJECT).count() != block.inline.len() {
                return Err(FragmentError::Invalid(
                    "inline objects don't match their markers".into(),
                ));
            }
            for object in &block.inline {
                object.validate().map_err(FragmentError::Invalid)?;
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
        let mut annotations = 0usize;
        for range in &self.ranges {
            if let Some(end) = &range.end {
                let (last, _) = nodes
                    .get(&end.node)
                    .ok_or_else(|| FragmentError::Invalid("range end node is absent".into()))?;
                if !last.text.is_char_boundary(end.at) || end.node == range.node {
                    return Err(FragmentError::Invalid("range end".into()));
                }
                let a = self.blocks.iter().position(|b| b.id == range.node);
                let b = self.blocks.iter().position(|b| b.id == end.node);
                if a >= b {
                    return Err(FragmentError::Invalid(
                        "range endpoints out of order".into(),
                    ));
                }
            }
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
            if let Some(data) = &range.annotation {
                annotations += 1;
                if annotations > crate::annotation::MAX_ANNOTATIONS {
                    return Err(FragmentError::Limit("annotations"));
                }
                if range.bytes.is_empty() && data.kind().spec().on_empty != OnEmpty::Orphan {
                    return Err(FragmentError::Invalid("empty annotation range".into()));
                }
                data.validate()
                    .map_err(|e| FragmentError::Invalid(e.to_string()))?;
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
        self.copy_fragment_scoped(source, selection, schemas, CopyScope::Clipboard)
    }

    /// [`Document::copy_fragment`] for a stated purpose (see [`CopyScope`]).
    pub fn copy_fragment_scoped(
        &self,
        source: &str,
        selection: &[CopyBlock],
        schemas: &SchemaRegistry,
        scope: CopyScope,
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
            page_setup_patch: None,
            notes: Vec::new(),
            lists: BTreeMap::new(),
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
            let style = if scope == CopyScope::Document {
                self.style_reference(node).unwrap_or_default()
            } else {
                block.style.unwrap_or_default()
            };
            let reference = self.style_reference(node).unwrap_or_default();
            let override_parent = block.overrides.parent.clone();
            for (name, reference) in [
                Some((style.clone(), reference)),
                override_parent.map(|name| (name.clone(), self.style_owner_reference(&name))),
            ]
            .into_iter()
            .flatten()
            {
                let mut next = Some((name, reference));
                for _ in 0..1024 {
                    let Some((name, reference)) = next.take() else {
                        break;
                    };
                    if fragment.styles.contains_key(&name) {
                        break;
                    }
                    let Some((style, parent_reference)) = self.style_for_copy_reference(&reference)
                    else {
                        break;
                    };
                    next = style
                        .parent
                        .clone()
                        .map(|parent| (parent.clone(), parent_reference.unwrap_or(parent)));
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
                role: self
                    .stored_role(node)
                    .ok()
                    .map(|r| r.role)
                    .filter(|r| *r != crate::Role::Body),
                list: self.copy_list_item(node, &mut fragment.lists)?,
                inline: self.inline_slice(node, bytes.clone()).map_err(bad)?,
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
        // Convert internal persistent targets to portable source labels/bytes.
        // A paste remaps these after every destination paragraph is activated.
        for block in &mut fragment.blocks {
            for object in &mut block.inline {
                if object.kind != crate::inline::FIELD || object.param("field") != Some("page-of") {
                    continue;
                }
                if let Some((node, at)) = object
                    .param("target")
                    .and_then(|raw| self.resolve_field_target(raw))
                    && let Some(cut) = cuts.get(&node)
                    // Inside the copy; its end belongs to it only when the copy
                    // runs to the end of the paragraph. A target at the cut
                    // end of a partial copy is the next character, which stays.
                    && cut.start <= at
                    && (at < cut.end
                        || (at == cut.end
                            && self
                                .block(node)
                                .is_ok_and(|b| b.text.len() == cut.end)))
                {
                    object.params.insert(
                        "target".into(),
                        format!("copy:{node} {}", at.saturating_sub(cut.start)),
                    );
                }
            }
        }
        let tree = self.tree("ranges");
        let mut ids = tree.nodes();
        ids.sort();
        let order = self.document_order();
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
            let extent = self.range_extent(range_id);
            let pieces: Vec<_> = extent.as_ref().map_or_else(Vec::new, |e| {
                let mut inside = false;
                order
                    .iter()
                    .filter_map(|&n| {
                        if n == e.start.0 {
                            inside = true;
                        }
                        if !inside {
                            return None;
                        }
                        let is_end = n == e.end.0;
                        let piece = cuts.get(&n).and_then(|cut| {
                            let len = self.block(n).ok()?.text.len();
                            let start = if n == e.start.0 { e.start.1 } else { 0 }.max(cut.start);
                            let end = if is_end { e.end.1 } else { len }.min(cut.end);
                            if start < end || (e.start == e.end && start == end) {
                                Some((
                                    n,
                                    start.checked_sub(cut.start)?..end.checked_sub(cut.start)?,
                                ))
                            } else {
                                None
                            }
                        });
                        if is_end {
                            inside = false;
                        }
                        piece
                    })
                    .collect()
            });
            if pieces.is_empty() {
                // Still validate selected malformed policies rather than lose data.
                if node.is_some_and(|n| cuts.contains_key(&n)) {
                    self.range_policy(range_id).map_err(bad)?;
                }
                continue;
            }
            let authored_policy = self.range_policy(range_id).map_err(bad)?;
            let (node, bytes) = pieces
                .first()
                .cloned()
                .ok_or_else(|| FragmentError::Invalid("range piece".into()))?;
            let (start, end) = (bytes.start, bytes.end);
            let last = pieces
                .last()
                .filter(|(n, _)| *n != node)
                .map(|(node, bytes)| FragmentEndpoint {
                    node: *node,
                    at: bytes.end,
                });
            // Annotations travel with their data; kinds that are not copied
            // and records this engine cannot read are left out and reported.
            let annotation = match get_str(&meta, crate::annotation::KEY) {
                None if meta.get(crate::annotation::KEY).is_none() => None,
                raw => match raw.map(|r| crate::annotation::AnnotationData::read(&r)) {
                    Some(Ok(data))
                        if scope == CopyScope::Document
                            || data.kind().spec().copy
                                == crate::annotation::CopyPolicy::Duplicate =>
                    {
                        // An empty span is an orphan: only kinds that keep
                        // their point can be copied that way.
                        if start == end && data.kind().spec().on_empty != OnEmpty::Orphan {
                            continue;
                        }
                        Some(data)
                    }
                    Some(Ok(data)) => {
                        let message = format!("{} annotations are not copied", data.kind().name());
                        if !fragment.notes.iter().any(|n| n.message == message) {
                            fragment
                                .notes
                                .push(reprise_diag::Note::info(ANNOTATION_DROPPED, message));
                        }
                        continue;
                    }
                    _ => {
                        if !fragment
                            .notes
                            .iter()
                            .any(|n| n.code == crate::annotation::UNREADABLE)
                        {
                            fragment.notes.push(reprise_diag::Note::warning(
                                crate::annotation::UNREADABLE,
                                "an unreadable annotation was left out of the copy",
                            ));
                        }
                        continue;
                    }
                },
            };
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
                bytes: start..end,
                policy,
                end: last,
                annotation,
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
        self.copy_all_scoped(source, schemas, CopyScope::Clipboard)
    }

    /// The whole document as a fragment to rebuild it elsewhere (a duplicate):
    /// [`CopyScope::Document`], so every readable annotation comes along.
    pub fn copy_document_fragment(
        &self,
        source: &str,
        schemas: &SchemaRegistry,
    ) -> Result<Fragment, FragmentError> {
        self.copy_all_scoped(source, schemas, CopyScope::Document)
    }

    fn copy_all_scoped(
        &self,
        source: &str,
        schemas: &SchemaRegistry,
        scope: CopyScope,
    ) -> Result<Fragment, FragmentError> {
        let selection: Vec<_> = self
            .blocks()
            .into_iter()
            .map(|node| CopyBlock { node, bytes: None })
            .collect();
        let mut fragment = self.copy_fragment_scoped(source, &selection, schemas, scope)?;
        // A full-document copy also carries unreferenced authored definitions.
        // Tombstones belong to history and are intentionally not replayed.
        for (id, read) in self.lists() {
            let def = read.map_err(FragmentError::Invalid)?;
            fragment.lists.insert(id, def);
            if fragment.lists.len() > MAX_FRAGMENT_LISTS {
                return Err(FragmentError::Limit("lists"));
            }
        }
        let ids: BTreeSet<_> = fragment.lists.keys().copied().collect();
        for def in fragment.lists.values_mut() {
            if def.base.is_some_and(|base| !ids.contains(&base)) {
                def.base = None;
            }
        }
        let map = self.doc.get_map("page_templates");
        for key in map.keys() {
            if let Some(ValueOrContainer::Value(LoroValue::String(raw))) = map.get(&key) {
                fragment.templates.insert(key.to_string(), raw.to_string());
            }
        }
        fragment.template_choice = get_str(&self.doc.get_map("page_setup"), "template");
        fragment.page_setup_patch = self
            .raw_page_setup()
            .map_err(|e| FragmentError::Invalid(e.to_string()))?;
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

    /// Attach a staged cross-paragraph endpoint before activating the range.
    pub fn stage_fragment_range_end(
        &self,
        id: RangeId,
        node: NodeId,
        at: usize,
        policy: RangePolicy,
    ) -> Result<(), DocError> {
        let text = self
            .block(node)
            .map(|b| b.text)
            .ok()
            .or_else(|| self.text_of_any(node))
            .ok_or(DocError::NoNode(node))?;
        let end = text.anchor(at, policy.end)?;
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        let meta = self.tree("ranges").get_meta(id.0)?;
        meta.insert("end-node1", node.to_string())?;
        meta.insert("end", crate::ranges::anchor_value(&end))?;
        self.doc.commit();
        Ok(())
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

impl Document {
    /// A copied block's membership, adding its definition (and the bases it
    /// inherits from, while they are live) to `lists`. Only items of live
    /// definitions are copied.
    fn copy_list_item(
        &self,
        node: NodeId,
        lists: &mut BTreeMap<crate::ListId, crate::ListDefinition>,
    ) -> Result<Option<FragmentItem>, FragmentError> {
        let Some(item) = self.list_item(node) else {
            return Ok(None);
        };
        let restart = self
            .list_membership(node)
            .ok()
            .flatten()
            .and_then(|m| m.restart);
        let mut next = Some(item.list);
        for _ in 0..crate::lists::MAX_BASE_DEPTH {
            let Some(id) = next.take() else { break };
            if lists.contains_key(&id) {
                break;
            }
            let Ok(def) = self.list_definition(id) else {
                break;
            };
            next = def.base;
            lists.insert(id, def);
            if lists.len() > MAX_FRAGMENT_LISTS {
                return Err(FragmentError::Limit("lists"));
            }
        }
        // A base that was not copied (missing or past the depth) is dropped.
        let copied: Vec<crate::ListId> = lists.keys().copied().collect();
        for def in lists.values_mut() {
            if def.base.is_some_and(|b| !copied.contains(&b)) {
                def.base = None;
            }
        }
        Ok(Some(FragmentItem {
            list: item.list,
            level: item.level,
            restart,
        }))
    }
}
