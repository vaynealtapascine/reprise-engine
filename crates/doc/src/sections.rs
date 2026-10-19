//! Retained section starts and independent stories (05, 07, 29, 34).
//!
//! Reads never create roots and never repair hostile authored records. A start's
//! address is stable; each activation has its own excluded operation lifetime.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use loro::{Container, LoroMap, TreeParentId, ValueOrContainer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{DocError, Document, Domain, NewBlock, NodeId, get_bool, get_str};

pub const STARTS: &str = "starts1";
pub const GEOMETRY: &str = "geometry1";
pub const REQUIRED_SECTIONS: u64 = 1 << 17;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SectionId {
    First,
    Block(NodeId),
}

impl fmt::Display for SectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::First => f.write_str("first"),
            Self::Block(id) => id.fmt(f),
        }
    }
}

impl SectionId {
    pub fn parse(s: &str) -> Option<Self> {
        if s == "first" {
            Some(Self::First)
        } else {
            NodeId::parse(s)
                .filter(|id| {
                    id.node.counter >= 0
                        && id.mark.is_none_or(|mark| mark.counter >= 0)
                        && id.to_string() == s
                })
                .map(Self::Block)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BreakKind {
    Section,
    Page,
    Column,
}

impl BreakKind {
    pub fn token(self) -> &'static str {
        match self {
            Self::Section => "section",
            Self::Page => "page",
            Self::Column => "column",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "section" => Some(Self::Section),
            "page" => Some(Self::Page),
            "column" => Some(Self::Column),
            _ => None,
        }
    }
}

/// Current-lifetime authored values, including unknown scalar values. Containers
/// remain unreadable and are never rewritten by interpretation.
pub type SectionProperties = BTreeMap<String, Value>;

/// A start a paste writes: its block (already pasted), its kind, whether the
/// source record was active, and its raw properties with bindings resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct PastedStart {
    pub block: NodeId,
    pub kind: BreakKind,
    pub active: bool,
    pub properties: SectionProperties,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StartRecord {
    pub id: SectionId,
    pub active: bool,
    pub activation_readable: bool,
    pub life: Option<String>,
    pub kind: Option<BreakKind>,
    pub own: SectionProperties,
    pub unreadable: bool,
    pub malformed_keys: Vec<String>,
}

impl Document {
    fn has_section_root(&self, name: &str) -> bool {
        self.doc.get_by_str_path(name).is_some()
    }

    fn retained_map(&self, name: &str) -> Option<LoroMap> {
        match self.doc.get_by_str_path(name)? {
            ValueOrContainer::Container(Container::Map(map)) => Some(map),
            _ => None,
        }
    }

    /// Required even for emptied roots, inactive lifetimes and tombstoned stories.
    pub fn has_sections(&self) -> bool {
        self.has_section_root(STARTS)
            || self.has_section_root(GEOMETRY)
            || self
                .tree("content")
                .get_nodes(true)
                .iter()
                .any(|node| self.is_story_node(node.id))
    }

    /// The raw geometry-mode seam, absent for untouched documents.
    pub fn geometry_mode_record(&self) -> Option<SectionProperties> {
        if !self.has_section_root(GEOMETRY) {
            return None;
        }
        Some(
            self.retained_map(GEOMETRY)
                .map(|map| values(&map))
                .unwrap_or_else(|| BTreeMap::from([("v".into(), Value::Null)])),
        )
    }

    pub fn set_geometry_mode(&self, mode: &str) -> Result<(), DocError> {
        if !matches!(mode, "paged" | "pageless") {
            return Err(DocError::Store("invalid geometry mode".into()));
        }
        let map = self.doc.get_map(GEOMETRY);
        map.insert("v", 1)?;
        map.insert("mode", mode)?;
        Ok(())
    }

    /// All retained addresses, in numeric ID order. Only their current lifetime
    /// is interpreted. Old lifetimes are history, not malformed data.
    pub fn start_records(&self) -> Vec<StartRecord> {
        let Some(map) = self.retained_map(STARTS) else {
            return Vec::new();
        };
        let mut addresses: BTreeMap<SectionId, Vec<String>> = BTreeMap::new();
        for key in map.keys() {
            if let Some((addr, _, _)) = parse_key(&key) {
                addresses.entry(addr).or_default();
            } else {
                // Bounded samples are selected after sorting all retained keys.
                let addr = key
                    .split('/')
                    .next()
                    .and_then(SectionId::parse)
                    .unwrap_or(SectionId::First);
                addresses.entry(addr).or_default().push(key.to_string());
            }
        }
        let mut records: BTreeMap<_, _> = addresses
            .into_iter()
            .map(|(id, mut malformed_keys)| {
                malformed_keys.sort();
                malformed_keys.truncate(8);
                let life = if id == SectionId::First {
                    Some("0".into())
                } else {
                    get_str(&map, &format!("{id}/life")).filter(|s| valid_life(s, id))
                };
                let activation = get_bool(&map, &format!("{id}/active"));
                (
                    id,
                    StartRecord {
                        id,
                        active: id == SectionId::First || activation == Some(true),
                        activation_readable: id == SectionId::First
                            || activation.is_some()
                            || map.get(&format!("{id}/active")).is_none(),
                        life,
                        kind: None,
                        own: BTreeMap::new(),
                        unreadable: false,
                        malformed_keys,
                    },
                )
            })
            .collect();
        // One pass over properties, not a scan per address. Only the selected
        // lifetime's values are materialised; obsolete lifetimes stay inert.
        map.for_each(|key, value| {
            let Some((addr, Some(life), prop)) = parse_key(key) else {
                return;
            };
            let Some(record) = records.get_mut(&addr) else {
                return;
            };
            if record.life.as_deref() == Some(life) && prop != "born" {
                record.unreadable |= matches!(value, ValueOrContainer::Container(_));
                record.own.insert(prop.into(), raw_value(value));
            }
        });
        records
            .into_values()
            .map(|mut record| {
                record.kind = record
                    .own
                    .get("break")
                    .and_then(Value::as_str)
                    .and_then(BreakKind::parse);
                record
            })
            .collect()
    }

    /// Effective block starts, in body order. A start on body index zero is kept.
    pub fn effective_starts(&self) -> Vec<StartRecord> {
        let mut records: BTreeMap<_, _> = self
            .start_records()
            .into_iter()
            .filter(|r| r.active && r.life.is_some() && r.kind.is_some())
            .map(|r| (r.id, r))
            .collect();
        self.top(Domain::Body)
            .into_iter()
            .filter_map(|id| records.remove(&SectionId::Block(id)))
            .collect()
    }

    /// Mint the ID of this excluded write itself, checking the committed counter.
    /// No integer generation is reused by undo, redo, reset or divergent peers.
    pub fn mint_lifetime(&self, address: NodeId) -> Result<String, DocError> {
        if !self.top(Domain::Body).contains(&address) {
            return Err(DocError::NoNode(address));
        }
        self.mint_lifetime_unchecked(address)
    }

    /// [`Document::mint_lifetime`] for an address the caller has already
    /// found in the body, so a paste of thousands of starts reads the body once.
    fn mint_lifetime_unchecked(&self, address: NodeId) -> Result<String, DocError> {
        self.commit_part();
        let peer = self.doc.peer_id();
        let counter = self.doc.oplog_vv().get(&peer).copied().unwrap_or_default();
        let after = counter
            .checked_add(1)
            .ok_or_else(|| DocError::Store("operation lifetime exhausted".into()))?;
        let life = format!("{counter}@{peer}");
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        self.doc
            .get_map(STARTS)
            .insert(&format!("{address}/{life}/born"), true)?;
        self.doc.commit();
        if self.doc.oplog_vv().get(&peer).copied() != Some(after) {
            return Err(DocError::Store(
                "staged lifetime did not use its predicted operation ID".into(),
            ));
        }
        Ok(life)
    }

    /// Activate one complete lifetime; previous lifetimes and unknown keys stay.
    pub fn activate_start(
        &self,
        block: NodeId,
        life: &str,
        kind: BreakKind,
        properties: &SectionProperties,
    ) -> Result<(), DocError> {
        if !self.top(Domain::Body).contains(&block) || !valid_life(life, SectionId::Block(block)) {
            return Err(DocError::NoNode(block));
        }
        validate_properties(properties, &[])?;
        let map = self
            .retained_map(STARTS)
            .ok_or_else(|| DocError::Store("unstaged lifetime".into()))?;
        if get_bool(&map, &format!("{block}/{life}/born")) != Some(true) {
            return Err(DocError::Store("unstaged lifetime".into()));
        }
        self.write_section_properties(SectionId::Block(block), life, properties, &[])?;
        map.insert(&format!("{block}/{life}/break"), kind.token())?;
        map.insert(&format!("{block}/life"), life)?;
        map.insert(&format!("{block}/active"), true)?;
        Ok(())
    }

    /// Writes the starts a paste brings (sections §14): each on its block, in a
    /// lifetime of its own that this call mints, with the `active` flag the
    /// source record had. Nothing of the source's lifetimes is read or kept.
    /// Every start is checked before the first is written.
    pub fn write_pasted_starts(&self, starts: &[PastedStart]) -> Result<(), DocError> {
        let body: BTreeSet<NodeId> = self.top(Domain::Body).into_iter().collect();
        for start in starts {
            if !body.contains(&start.block) {
                return Err(DocError::NoNode(start.block));
            }
            validate_properties(&start.properties, &[])?;
        }
        for start in starts {
            let life = self.mint_lifetime_unchecked(start.block)?;
            self.write_section_properties(
                SectionId::Block(start.block),
                &life,
                &start.properties,
                &[],
            )?;
            let map = self.doc.get_map(STARTS);
            let block = start.block;
            map.insert(&format!("{block}/{life}/break"), start.kind.token())?;
            map.insert(&format!("{block}/life"), life.as_str())?;
            map.insert(&format!("{block}/active"), start.active)?;
        }
        Ok(())
    }

    /// Writes the raw geometry-mode record a copy carried (sections §18): its
    /// scalar keys exactly as they were, unknown ones included, so a record
    /// this reader doesn't understand is still carried to the next one.
    pub fn write_geometry_record(&self, record: &SectionProperties) -> Result<(), DocError> {
        validate_properties(record, &[])?;
        let map = self.doc.get_map(GEOMETRY);
        for (key, value) in record {
            match value {
                Value::String(v) => map.insert(key, v.as_str())?,
                Value::Bool(v) => map.insert(key, *v)?,
                Value::Number(v) => map.insert(
                    key,
                    v.as_i64()
                        .ok_or_else(|| DocError::Store("non-integer geometry value".into()))?,
                )?,
                _ => return Err(DocError::Store("non-scalar geometry value".into())),
            };
        }
        Ok(())
    }

    pub fn deactivate_start(&self, block: NodeId) -> Result<(), DocError> {
        if !self
            .effective_starts()
            .iter()
            .any(|r| r.id == SectionId::Block(block))
        {
            return Err(DocError::NoNode(block));
        }
        self.doc
            .get_map(STARTS)
            .insert(&format!("{block}/active"), false)?;
        Ok(())
    }

    /// Writes a patch to one lifetime. Clears of absent keys and empty patches
    /// do not create the root.
    pub fn write_section_properties(
        &self,
        section: SectionId,
        life: &str,
        patch: &SectionProperties,
        clear: &[String],
    ) -> Result<(), DocError> {
        validate_properties(patch, clear)?;
        if !valid_life(life, section) {
            return Err(DocError::Store("invalid lifetime".into()));
        }
        if patch.is_empty() && (!self.has_section_root(STARTS) || clear.is_empty()) {
            return Ok(());
        }
        let map = self.doc.get_map(STARTS);
        for key in clear {
            map.delete(&format!("{section}/{life}/{key}"))?;
        }
        for (key, value) in patch {
            let key = format!("{section}/{life}/{key}");
            match value {
                Value::String(v) => map.insert(&key, v.as_str())?,
                Value::Bool(v) => map.insert(&key, *v)?,
                Value::Number(v) => map.insert(
                    &key,
                    v.as_i64()
                        .ok_or_else(|| DocError::Store("non-integer section value".into()))?,
                )?,
                _ => return Err(DocError::Store("non-scalar section value".into())),
            };
        }
        Ok(())
    }

    /// Stage a root-level story and its blocks outside undo history. Activation
    /// only clears the story tombstone, so all identities survive redo.
    pub fn stage_story(&self, blocks: &[NewBlock]) -> Result<NodeId, DocError> {
        for block in blocks {
            crate::check_authored(&block.text)?;
        }
        let mut children = Vec::new();
        for block in blocks {
            children.push(self.stage_block(block)?);
        }
        self.commit_part();
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        let tree = self.tree("content");
        let id = tree.create(None)?;
        let meta = tree.get_meta(id)?;
        meta.insert("kind", "story")?;
        meta.insert("v", 1)?;
        meta.insert("deleted", true)?;
        for child in children {
            tree.mov(child.node, Some(id))?;
            tree.get_meta(child.node)?.insert("deleted", false)?;
        }
        self.doc.commit();
        Ok(NodeId::tree(id))
    }

    pub fn activate_story(&self, story: NodeId) -> Result<(), DocError> {
        if story.is_break()
            || !self.is_story_node(story.node)
            || self.tree("content").parent(story.node) != Some(TreeParentId::Root)
        {
            return Err(DocError::NoNode(story));
        }
        self.restore_block(story)
    }

    pub fn create_story(&self, blocks: &[NewBlock]) -> Result<NodeId, DocError> {
        let story = self.stage_story(blocks)?;
        self.activate_story(story)?;
        Ok(story)
    }

    pub(crate) fn section_audit(&self) -> Vec<crate::invariants::Finding> {
        use crate::invariants::Finding;
        use reprise_diag::{Code, Note, Severity};
        let body: BTreeSet<_> = self.top(Domain::Body).into_iter().collect();
        let mut out = Vec::new();
        let mut groups: BTreeMap<(&'static str, Severity), Vec<NodeId>> = BTreeMap::new();
        for record in self.start_records() {
            let SectionId::Block(node) = record.id else {
                continue;
            };
            if !record.malformed_keys.is_empty() {
                groups
                    .entry(("section.orphan-start", Severity::Warning))
                    .or_default()
                    .push(node);
            }
            let (code, severity) = if record.activation_readable && !record.active {
                ("section.inactive", Severity::Info)
            } else if !record.activation_readable
                || record.life.is_none()
                || record.kind.is_none()
                || !body.contains(&node)
            {
                ("section.orphan-start", Severity::Warning)
            } else {
                continue;
            };
            groups.entry((code, severity)).or_default().push(node);
        }
        for ((code, severity), nodes) in groups {
            if let Some(&node) = nodes.first() {
                let sample: Vec<_> = nodes.iter().take(8).map(ToString::to_string).collect();
                out.push(Finding {
                    node,
                    note: Note::new(
                        severity,
                        Code::new(code),
                        format!(
                            "{} retained starts; sample: {}",
                            nodes.len(),
                            sample.join(", ")
                        ),
                    ),
                });
            }
        }
        let tree = self.tree("content");
        let mut nested: Vec<_> = tree
            .nodes()
            .into_iter()
            .filter(|&id| self.is_story_node(id) && tree.parent(id) != Some(TreeParentId::Root))
            .collect();
        nested.sort();
        if let Some(&id) = nested.first() {
            let sample: Vec<_> = nested.iter().take(8).map(ToString::to_string).collect();
            out.push(Finding {
                node: NodeId::tree(id),
                note: Note::new(
                    Severity::Warning,
                    Code::new("story.nested"),
                    format!(
                        "{} nested stories; sample: {}",
                        nested.len(),
                        sample.join(", ")
                    ),
                ),
            });
        }
        // Resolve inheritance in body order, including empty first.
        let mut bindings: BTreeMap<String, String> = BTreeMap::new();
        let mut bound = BTreeSet::new();
        let mut sequence = Vec::new();
        if let Some(first) = self
            .start_records()
            .into_iter()
            .find(|r| r.id == SectionId::First)
        {
            sequence.push(first);
        }
        sequence.extend(
            self.effective_starts()
                .into_iter()
                .filter(|r| r.kind == Some(BreakKind::Section)),
        );
        for record in sequence {
            for (key, value) in record.own {
                if binding_key(&key)
                    && let Some(value) = value.as_str()
                {
                    bindings.insert(key, value.into());
                }
            }
            bound.extend(bindings.values().filter_map(|s| NodeId::parse(s)));
        }
        let orphans: BTreeSet<_> = self
            .stories()
            .into_iter()
            .filter(|s| !bound.contains(s))
            .collect();
        if let Some(&node) = orphans.first() {
            let sample: Vec<_> = orphans.iter().take(8).map(ToString::to_string).collect();
            out.push(Finding {
                node,
                note: Note::new(
                    Severity::Info,
                    Code::new("story.orphan"),
                    format!(
                        "{} unbound stories retained; sample: {}",
                        orphans.len(),
                        sample.join(", ")
                    ),
                ),
            });
        }
        out
    }
}

pub fn binding_key(key: &str) -> bool {
    matches!(
        key.split('.').collect::<Vec<_>>().as_slice(),
        [
            "region",
            "header" | "footer" | "background",
            "default" | "first" | "even"
        ]
    )
}

pub fn validate_properties(patch: &SectionProperties, clear: &[String]) -> Result<(), DocError> {
    for key in patch.keys().chain(clear) {
        if key.is_empty() || key.contains('/') || matches!(key.as_str(), "active" | "life" | "born")
        {
            return Err(DocError::Store("invalid section property key".into()));
        }
    }
    if patch
        .values()
        .any(|v| !matches!(v, Value::String(_) | Value::Bool(_)) && v.as_i64().is_none())
    {
        return Err(DocError::Store("non-scalar section value".into()));
    }
    Ok(())
}

fn valid_life(life: &str, address: SectionId) -> bool {
    if address == SectionId::First {
        life == "0"
    } else {
        NodeId::parse(life)
            .is_some_and(|id| !id.is_break() && id.node.counter >= 0 && id.to_string() == life)
    }
}

fn parse_key(key: &str) -> Option<(SectionId, Option<&str>, &str)> {
    let (prefix, prop) = key.rsplit_once('/')?;
    if matches!(prop, "active" | "life") {
        return Some((SectionId::parse(prefix)?, None, prop));
    }
    let (addr, life) = prefix.rsplit_once('/')?;
    let addr = SectionId::parse(addr)?;
    (valid_life(life, addr) && !prop.is_empty()).then_some((addr, Some(life), prop))
}

fn raw_value(value: ValueOrContainer) -> Value {
    match value {
        ValueOrContainer::Value(value) => serde_json::to_value(value).unwrap_or(Value::Null),
        ValueOrContainer::Container(_) => Value::Null,
    }
}

fn values(map: &LoroMap) -> SectionProperties {
    let mut out = BTreeMap::new();
    map.for_each(|key, value| {
        out.insert(key.into(), raw_value(value));
    });
    out
}

#[cfg(test)]
mod tests;

mod views;
pub use views::{BindingView, SectionView, known_property};
