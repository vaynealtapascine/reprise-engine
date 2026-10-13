//! Immutable property owners and versioned references; names are selectors.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use loro::{Container, LoroMap, ValueOrContainer};
use sha2::{Digest, Sha256};

use crate::{DocError, Document, NodeId, Style, StyleRecord, StyleStatus, get_str};

pub(crate) const ROOT: &str = "styles2";
const ID_PREFIX: &str = "\u{1}style1:";
const BUILTIN_PREFIX: &str = "\u{2}builtin1:";
const MAX_KEYS: usize = 65536;

#[derive(Default)]
pub(crate) struct IdentityCache {
    frontiers: Option<loro::Frontiers>,
    index: Option<Arc<IdentityIndex>>,
}

#[derive(Default)]
struct IdentityIndex {
    labels: BTreeMap<String, String>,
    selectors: BTreeMap<String, String>,
}

fn collision_label(owner: &str, label: &str) -> String {
    let hash = Sha256::digest(owner.as_bytes());
    let suffix = format!(
        " [style {}]",
        hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let mut end = label
        .len()
        .min(crate::MAX_STYLE_NAME.saturating_sub(suffix.len()));
    while !label.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}{suffix}", label.get(..end).unwrap_or_default())
}

fn encode(owner: &str) -> String {
    let mut reference = String::with_capacity(
        ID_PREFIX
            .len()
            .saturating_add(owner.len().saturating_mul(2)),
    );
    reference.push_str(ID_PREFIX);
    let digit = |n: u8| {
        char::from(if n < 10 {
            b'0'.saturating_add(n)
        } else {
            b'a'.saturating_add(n.saturating_sub(10))
        })
    };
    for byte in owner.bytes() {
        reference.push(digit(byte >> 4));
        reference.push(digit(byte & 15));
    }
    reference
}

fn decode(reference: &str) -> Option<String> {
    let hex = reference.strip_prefix(ID_PREFIX)?;
    if hex.len() > 2048 || !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[first, second]| {
            let digit = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                _ => None,
            };
            Some(
                digit(*first)?
                    .saturating_mul(16)
                    .saturating_add(digit(*second)?),
            )
        })
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

impl Document {
    fn identity_map(&self) -> Option<LoroMap> {
        match self.doc.get_by_str_path(ROOT) {
            Some(ValueOrContainer::Container(Container::Map(map))) => Some(map),
            _ => None,
        }
    }

    /// Whether authored identity/built-in-v1 semantics have been retained.
    /// Reads never create this root, including after undo.
    pub fn has_style_identities(&self) -> bool {
        self.has_root(ROOT)
    }

    pub(crate) fn identity_listing_cut(&self) -> bool {
        self.identity_map().is_some_and(|map| map.len() > MAX_KEYS)
    }

    fn check_identity_keys(&self, keys: &[String]) -> Result<(), DocError> {
        let map = self.identity_map();
        if map.is_none() && self.has_root(ROOT) {
            return Err(DocError::Store("unreadable style identity root".into()));
        }
        let count = map.as_ref().map_or(0, LoroMap::len).saturating_add(
            keys.iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter(|key| {
                    map.as_ref()
                        .is_none_or(|map| map.get(key.as_str()).is_none())
                })
                .count(),
        );
        if count > MAX_KEYS {
            return Err(DocError::Store("style identity key limit".into()));
        }
        Ok(())
    }

    pub(crate) fn retain_style_identities(&self) {
        self.doc.get_map(ROOT);
    }

    pub(crate) fn legacy_builtin_context(&self) -> bool {
        if self.has_style_identities() {
            self.identity_map()
                .is_some_and(|map| get_str(&map, "legacy-builtins1").as_deref() == Some("1"))
        } else {
            self.legacy_builtins
        }
    }

    pub(crate) fn author_style_identities(&self) -> Result<(), DocError> {
        let legacy = self.legacy_builtin_context();
        let keys = if legacy {
            vec!["legacy-builtins1".to_owned()]
        } else {
            Vec::new()
        };
        self.check_identity_keys(&keys)?;
        self.retain_style_identities();
        if legacy && get_str(&self.doc.get_map(ROOT), "legacy-builtins1").as_deref() != Some("1") {
            self.doc.get_map(ROOT).insert("legacy-builtins1", "1")?;
        }
        Ok(())
    }

    pub(crate) fn pin_builtin_owner(&self, owner: &str) -> Result<(), DocError> {
        let key = format!("base/{owner}");
        self.check_identity_keys(std::slice::from_ref(&key))?;
        self.author_style_identities()?;
        let map = self.doc.get_map(ROOT);
        if get_str(&map, &key).as_deref() != Some("builtin1") {
            map.insert(&key, "builtin1")?;
        }
        Ok(())
    }

    fn build_identity_index(&self) -> IdentityIndex {
        let mut index = IdentityIndex::default();
        let Some(map) = self.identity_map().filter(|map| map.len() <= MAX_KEYS) else {
            return index;
        };
        let mut desired = BTreeMap::new();
        for key in map.keys() {
            if let Some(owner) = key.strip_prefix("label/") {
                desired.insert(
                    owner.to_owned(),
                    get_str(&map, &key).unwrap_or_else(|| owner.into()),
                );
            }
        }
        // A concurrent first definition or untouched built-in may own the
        // destination label without having any identity metadata of its own.
        let targets: BTreeSet<_> = desired.values().cloned().collect();
        for label in targets {
            if self.raw_style_record(&label, true).is_some() {
                desired.entry(label.clone()).or_insert(label);
            }
        }
        let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (owner, label) in &desired {
            groups
                .entry(label.clone())
                .or_default()
                .insert(owner.clone());
        }
        index.labels = desired.clone();
        let mut pending: BTreeSet<_> = groups
            .iter()
            .filter(|(_, owners)| owners.len() > 1)
            .map(|(label, _)| label.clone())
            .collect();
        let mut suffixed = BTreeSet::new();
        // Each owner is suffixed at most once: hostile authored labels can
        // imitate a generated selector, but cannot trigger quadratic retries.
        while let Some(label) = pending.pop_first() {
            let owners = groups.get(&label).cloned().unwrap_or_default();
            if owners.len() <= 1 {
                continue;
            }
            for owner in owners {
                if !suffixed.insert(owner.clone()) {
                    continue;
                }
                let next = collision_label(
                    &owner,
                    desired.get(&owner).map(String::as_str).unwrap_or(&owner),
                );
                if let Some(group) = groups.get_mut(&label) {
                    group.remove(&owner);
                }
                index.labels.insert(owner.clone(), next.clone());
                let group = groups.entry(next.clone()).or_default();
                group.insert(owner);
                if group.len() > 1 {
                    pending.insert(next);
                }
            }
        }
        for (owner, label) in &index.labels {
            index.selectors.insert(label.clone(), owner.clone());
        }
        index
    }

    fn identity_index(&self) -> Arc<IdentityIndex> {
        if self.doc.get_pending_txn_len() != 0 {
            return Arc::new(self.build_identity_index());
        }
        let frontiers = self.doc.state_frontiers();
        let mut cache = self
            .style_identities
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if cache.frontiers.as_ref() == Some(&frontiers)
            && let Some(index) = &cache.index
        {
            return index.clone();
        }
        let index = Arc::new(self.build_identity_index());
        cache.frontiers = Some(frontiers);
        cache.index = Some(index.clone());
        index
    }

    fn desired_label(&self, owner: &str) -> String {
        self.identity_map()
            .and_then(|map| get_str(&map, &format!("label/{owner}")))
            .unwrap_or_else(|| owner.to_owned())
    }

    pub(crate) fn owner_label(&self, owner: &str) -> String {
        self.identity_index()
            .labels
            .get(owner)
            .cloned()
            .unwrap_or_else(|| owner.into())
    }

    pub(crate) fn style_owner(&self, name: &str) -> String {
        if let Some(owner) = decode(name) {
            return owner;
        }
        if let Some(name) = name.strip_prefix(BUILTIN_PREFIX) {
            return name.to_owned();
        }
        // Disambiguated menu selectors bind to their own owner, even when the
        // unsuffixed alias was concurrently assigned to another owner.
        if name.contains(" [style ")
            && let Some(owner) = self.identity_index().selectors.get(name)
        {
            return owner.clone();
        }
        self.identity_map()
            .and_then(|map| get_str(&map, &format!("alias/{name}")))
            .unwrap_or_else(|| name.to_owned())
    }

    pub(crate) fn style_owner_reference(&self, name: &str) -> String {
        encode(&self.style_owner(name))
    }

    pub(crate) fn style_owner_floor(&self, peer: u64) -> Result<i64, DocError> {
        let Some(map) = self.identity_map() else {
            return Ok(0);
        };
        let key = format!("clock/{peer}");
        if map.get(&key).is_none() {
            return Ok(0);
        }
        get_str(&map, &key)
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|n| *n >= 0)
            .ok_or_else(|| DocError::Store("unreadable style allocation clock".into()))
    }

    /// An opaque, document-scoped owner token, stable across rename and undo.
    /// Copies retain local tokens in their independent document namespace.
    pub fn style_identity(&self, name: &str) -> Option<String> {
        self.style_record(name)
            .map(|_| encode(&self.style_owner(name)))
    }

    /// The underlying block reference for resolution/copy/incremental keys.
    /// Plain legacy strings bind to their original owner, not a reused alias.
    pub fn style_reference(&self, node: NodeId) -> Option<String> {
        let raw = get_str(&self.meta_of(node).ok()?, "style")?;
        if !self.has_style_identities()
            && !crate::BUILTIN_STYLES.contains(&raw.as_str())
            && !raw.starts_with(ID_PREFIX)
            && !raw.starts_with(BUILTIN_PREFIX)
        {
            return Some(raw);
        }
        if !raw.starts_with(ID_PREFIX)
            && !raw.starts_with(BUILTIN_PREFIX)
            && self.legacy_builtin_context()
            && self.raw_style_record(&raw, true).is_some_and(|r| r.builtin)
        {
            return Some(format!("{BUILTIN_PREFIX}{raw}"));
        }
        Some(
            if raw.starts_with(ID_PREFIX) || raw.starts_with(BUILTIN_PREFIX) {
                raw
            } else {
                encode(&raw)
            },
        )
    }

    pub(crate) fn reference_label(&self, reference: &str) -> String {
        if let Some(name) = reference.strip_prefix(BUILTIN_PREFIX) {
            return self.owner_label(name);
        }
        decode(reference).map_or_else(
            || self.owner_label(reference),
            |owner| self.owner_label(&owner),
        )
    }

    pub(crate) fn authored_style_reference(&self, name: &str) -> Result<String, DocError> {
        if name.starts_with(ID_PREFIX) || name.starts_with(BUILTIN_PREFIX) {
            if !self.has_style_identities() {
                self.author_style_identities()?;
            }
            return Ok(name.to_owned());
        }
        let owner = self.style_owner(name);
        if self
            .raw_style_record(&owner, true)
            .is_some_and(|record| record.builtin)
        {
            self.author_style_identities()?;
            return Ok(format!("{BUILTIN_PREFIX}{owner}"));
        }
        if self.has_style_identities() {
            return Ok(encode(&owner));
        }
        Ok(name.to_owned())
    }

    pub(crate) fn authored_parent(&self, style: &Style) -> Result<Style, DocError> {
        let mut style = style.clone();
        if let Some(parent) = &style.parent {
            // Once identities exist, bind every newly authored parent to the
            // selected owner. Legacy parent strings remain original-owner refs.
            let reference = self.authored_style_reference(parent)?;
            style.parent = Some(
                if self.has_style_identities() && !reference.starts_with(BUILTIN_PREFIX) {
                    encode(&self.style_owner(parent))
                } else {
                    reference
                },
            );
        }
        Ok(style)
    }

    pub(crate) fn authored_paragraph_style(&self, style: &Style) -> Result<Style, DocError> {
        if crate::PARAGRAPH_KEYS
            .iter()
            .any(|key| style.stored(key).is_some())
            || style.alignment == Some(crate::marks::Alignment::Justify)
        {
            self.author_style_identities()?;
        }
        self.authored_parent(style)
    }

    pub(crate) fn public_style(&self, mut style: Style) -> Style {
        if let Some(parent) = &style.parent {
            style.parent = Some(self.reference_label(parent));
        }
        style
    }

    pub(crate) fn definition_owner(&self, name: &str) -> Result<String, DocError> {
        let owner = self.style_owner(name);
        let reused = self
            .style_record(name)
            .is_some_and(|record| record.status != StyleStatus::Live);
        if !reused {
            return Ok(owner);
        }
        let peer = self.doc.peer_id();
        let counter = i64::from(self.doc.oplog_vv().get(&peer).copied().unwrap_or_default())
            .checked_add(i64::try_from(self.doc.get_pending_txn_len()).unwrap_or(i64::MAX))
            .and_then(|n| n.checked_add(self.style_owner_floor(peer).ok()?))
            .ok_or_else(|| {
                DocError::Store("style allocation clock exhausted or unreadable".into())
            })?;
        let fresh = format!("\u{3}{peer}:{counter}");
        self.check_identity_keys(&[
            format!("label/{owner}"),
            format!("alias/{name}"),
            format!("label/{fresh}"),
        ])?;
        self.author_style_identities()?;
        let map = self.doc.get_map(ROOT);
        map.insert(&format!("label/{owner}"), self.desired_label(&owner))?;
        map.insert(&format!("alias/{name}"), fresh.clone())?;
        map.insert(&format!("label/{fresh}"), name)?;
        Ok(fresh)
    }

    pub(crate) fn rename_owner(&self, from: &str, to: &str) -> Result<(), DocError> {
        let owner = self.style_owner(from);
        let builtin = self
            .raw_style_record(&owner, true)
            .is_some_and(|r| r.builtin);
        let mut keys = vec![
            format!("alias/{from}"),
            format!("alias/{to}"),
            format!("label/{owner}"),
        ];
        if builtin {
            keys.push(format!("base/{owner}"));
        }
        if self.legacy_builtin_context() {
            keys.push("legacy-builtins1".into());
        }
        self.check_identity_keys(&keys)?;
        if self
            .raw_style_record(&owner, true)
            .is_some_and(|r| r.builtin)
        {
            self.pin_builtin_owner(&owner)?;
        } else {
            self.author_style_identities()?;
        }
        let map = self.doc.get_map(ROOT);
        map.insert(&format!("alias/{from}"), owner.clone())?;
        map.insert(&format!("alias/{to}"), owner.clone())?;
        map.insert(&format!("label/{owner}"), to)?;
        Ok(())
    }

    pub(crate) fn identity_labels(&self) -> Vec<String> {
        self.identity_index().selectors.keys().cloned().collect()
    }

    pub(crate) fn reference_record(&self, reference: &str) -> Option<(String, StyleRecord)> {
        let (owner, builtin) = if let Some(name) = reference.strip_prefix(BUILTIN_PREFIX) {
            (name.to_owned(), true)
        } else if let Some(owner) = decode(reference) {
            let builtin = self.legacy_builtin_context()
                || self.identity_map().is_some_and(|map| {
                    get_str(&map, &format!("base/{owner}")).as_deref() == Some("builtin1")
                });
            (owner, builtin)
        } else {
            (self.style_owner(reference), true)
        };
        let mut record = self.raw_style_record(&owner, builtin)?;
        // Parent and v1 forwarding strings refer to their original owners.
        if let Some(parent) = &record.style.parent
            && !parent.starts_with(ID_PREFIX)
            && !parent.starts_with(BUILTIN_PREFIX)
        {
            record.style.parent = Some(
                if record.builtin && crate::builtin_style(parent).is_some() {
                    format!("{BUILTIN_PREFIX}{parent}")
                } else if self.has_style_identities()
                    || crate::BUILTIN_STYLES.contains(&parent.as_str())
                {
                    encode(parent)
                } else {
                    parent.clone()
                },
            );
        }
        if let StyleStatus::Renamed(to) = &record.status {
            record.status = StyleStatus::Renamed(encode(to));
        }
        Some((self.owner_label(&owner), record))
    }
}
