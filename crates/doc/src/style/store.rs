//! Flat, merge-safe storage for named styles and per-block property patches
//! (`docs/paragraph-styles.md`, "Where values are stored").
//!
//! -   `styles1`: `<property>/<style name>` and `state/<style name>`.
//! -   `para1`: `<node>/<property>`, except `alignment` and `tabs`, which keep
//!     their `marks1` keys, so each property lives in exactly one flat root.
//!
//! Nothing here creates a container: concurrent first edits by two peers
//! touch different keys and both survive, as in `marks1` and `page-setup1`.
//! Clearing deletes the flat key and the key inside an existing nested map;
//! no sentinel is stored. Reads never create a root.

use std::collections::BTreeSet;

use loro::{Container, LoroMap, LoroValue, ValueOrContainer};

use super::{MAX_CHAIN, PARAGRAPH_KEYS, STORED_KEYS, builtin_style};
use crate::{DocError, Document, NodeId, Style, get_str};

pub(crate) const STYLES: &str = "styles1";
pub(crate) const PARA: &str = "para1";
/// The longest style name, in bytes.
pub const MAX_STYLE_NAME: usize = 256;
/// The most live named styles a document may hold through local edits, and
/// the most a listing returns.
pub const MAX_STYLES: usize = 1024;
/// The most flat keys a listing or feature scan reads before it stops.
const MAX_SCANNED_KEYS: usize = 1 << 20;

/// Whether a named style can be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StyleStatus {
    Live,
    /// Kept with its properties; whatever names it resolves through its
    /// parent, with `style.deleted`.
    Deleted,
    /// Forwards to the new name: blocks and child styles naming it follow.
    Renamed(String),
}

/// A named style as the document holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleRecord {
    /// The effective authored properties: the nested definition or the
    /// built-in one, with the flat patches on top.
    pub style: Style,
    pub status: StyleStatus,
    /// The base is the engine's built-in definition of this name.
    pub builtin: bool,
}

/// A property patch: set what `set` supplies, clear the keys in `clear`,
/// keep the rest. Keys are [`STORED_KEYS`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StylePatch {
    pub set: Style,
    pub clear: BTreeSet<String>,
}

impl StylePatch {
    /// The keys `set` supplies.
    pub fn supplied(&self) -> Vec<&'static str> {
        STORED_KEYS
            .into_iter()
            .filter(|k| self.set.stored(k).is_some())
            .collect()
    }

    /// Refuses unknown keys, a key both set and cleared, and values the
    /// engine would not store.
    pub fn validate(&self) -> Result<(), DocError> {
        for key in &self.clear {
            if !STORED_KEYS.contains(&key.as_str()) {
                return Err(DocError::Store(format!("unknown style property {key}")));
            }
        }
        for key in self.supplied() {
            if self.clear.contains(key) {
                return Err(DocError::Store(format!("{key} is both set and cleared")));
            }
        }
        validate_style(&self.set)
    }
}

/// Values a local edit may store. Remote peers may store anything; reading
/// keeps what it can't use and reports it.
pub(crate) fn validate_style(style: &Style) -> Result<(), DocError> {
    if let Some(tabs) = &style.tabs {
        tabs.validate()?;
    }
    if style.weight.is_some_and(|w| !(1..=1000).contains(&w)) {
        return Err(DocError::Store("weight must be in 1..1000".into()));
    }
    for n in [style.widows, style.orphans].into_iter().flatten() {
        if !(1..=super::MAX_LINE_KEEP).contains(&n) {
            return Err(DocError::Store(
                "widows and orphans must be in 1..32".into(),
            ));
        }
    }
    if !style.unparsed_keywords.is_empty() || style.unparsed_families.is_some() {
        return Err(DocError::Store(
            "unreadable values can't be authored".into(),
        ));
    }
    if let Some(parent) = &style.parent {
        check_name(parent)?;
    }
    Ok(())
}

/// A name a local edit may use: at most [`MAX_STYLE_NAME`] bytes, no control
/// characters. The empty name is the document base.
pub fn check_name(name: &str) -> Result<(), DocError> {
    if name.len() > MAX_STYLE_NAME || name.chars().any(char::is_control) {
        return Err(DocError::Store(format!(
            "style names are at most {MAX_STYLE_NAME} bytes, without control characters"
        )));
    }
    Ok(())
}

fn key(property: &str, name: &str) -> String {
    format!("{property}/{name}")
}

fn nested_map(map: &LoroMap, key: &str) -> Option<LoroMap> {
    match map.get(key)? {
        ValueOrContainer::Container(Container::Map(m)) => Some(m),
        _ => None,
    }
}

impl Document {
    /// Whether a root exists, without creating it. Presence includes an
    /// emptied root, whose operations stay in history.
    fn has_root(&self, name: &str) -> bool {
        self.doc.get_by_str_path(name).is_some()
    }

    fn flat_styles(&self) -> Option<LoroMap> {
        self.has_root(STYLES).then(|| self.doc.get_map(STYLES))
    }

    fn nested_style(&self, name: &str) -> Option<LoroMap> {
        nested_map(&self.doc.get_map("styles"), name)
    }

    /// The named style `name` as stored, without following a rename. `None`
    /// when the document never defined it and it is not built in.
    pub fn style_record(&self, name: &str) -> Option<StyleRecord> {
        let nested = self.nested_style(name);
        let flat = self.flat_styles();
        let state = flat.as_ref().and_then(|m| get_str(m, &key("state", name)));
        let status = match state.as_deref() {
            Some("deleted") => StyleStatus::Deleted,
            Some(s) => match s.strip_prefix("renamed:") {
                Some(to) => StyleStatus::Renamed(to.to_string()),
                None => StyleStatus::Live,
            },
            None => StyleStatus::Live,
        };
        let defined = state.as_deref() == Some("defined");
        let builtin = if nested.is_none() && !defined {
            builtin_style(name)
        } else {
            None
        };
        let mut patched = false;
        let mut style = match (&nested, &builtin) {
            (Some(m), _) => Style::read(m),
            (None, Some(b)) => b.clone(),
            (None, None) => Style::default(),
        };
        if let Some(m) = &flat {
            for property in STORED_KEYS {
                if let Some(raw) = get_str(m, &key(property, name)) {
                    style.apply_stored(property, raw);
                    patched = true;
                }
            }
        }
        if nested.is_none() && state.is_none() && builtin.is_none() && !patched {
            return None;
        }
        Some(StyleRecord {
            style,
            status,
            builtin: builtin.is_some(),
        })
    }

    /// The live definition `name` resolves to, following renames. `None` for
    /// an undefined or deleted style.
    pub(crate) fn live_style(&self, name: &str) -> Option<Style> {
        let mut name = name.to_string();
        for _ in 0..=MAX_CHAIN {
            let record = self.style_record(&name)?;
            match record.status {
                StyleStatus::Live => return Some(record.style),
                StyleStatus::Deleted => return None,
                StyleStatus::Renamed(to) => name = to,
            }
        }
        None
    }

    /// The name `name` resolves to after following renames (itself when it
    /// is not renamed, or the forwards loop or run too long).
    pub fn current_style_name(&self, name: &str) -> String {
        let mut current = name.to_string();
        for _ in 0..=MAX_CHAIN {
            match self.style_record(&current).map(|r| r.status) {
                Some(StyleStatus::Renamed(to)) if to != name => current = to,
                _ => return current,
            }
        }
        name.to_string()
    }

    /// What a copy carries for `name`: the live definition it resolves to
    /// (following renames), or for a deleted style just its parent, which is
    /// how it resolves, so pasted text keeps its appearance. A pasted
    /// document has no deleted style to report.
    pub(crate) fn style_for_copy(&self, name: &str) -> Option<Style> {
        let mut current = name.to_string();
        for _ in 0..=MAX_CHAIN {
            let record = self.style_record(&current)?;
            match record.status {
                StyleStatus::Live => return Some(record.style),
                StyleStatus::Deleted => {
                    return Some(Style {
                        parent: record.style.parent,
                        ..Style::default()
                    });
                }
                StyleStatus::Renamed(to) => current = to,
            }
        }
        None
    }

    /// The live named styles, built-ins included, sorted by name, at most
    /// [`MAX_STYLES`]. The flag says the listing was cut short.
    pub fn style_names(&self) -> (Vec<String>, bool) {
        let mut names: BTreeSet<String> = super::BUILTIN_STYLES
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut scanned = 0usize;
        let mut cut = false;
        for name in self.doc.get_map("styles").keys() {
            scanned += 1;
            if scanned > MAX_SCANNED_KEYS {
                cut = true;
                break;
            }
            names.insert(name.to_string());
        }
        if let Some(flat) = self.flat_styles() {
            for k in flat.keys() {
                scanned += 1;
                if scanned > MAX_SCANNED_KEYS {
                    cut = true;
                    break;
                }
                if let Some((property, name)) = k.split_once('/')
                    && (property == "state" || STORED_KEYS.contains(&property))
                {
                    names.insert(name.to_string());
                }
            }
        }
        let mut live = Vec::new();
        for name in names {
            if self
                .style_record(&name)
                .is_some_and(|r| r.status == StyleStatus::Live)
            {
                if live.len() >= MAX_STYLES {
                    cut = true;
                    break;
                }
                live.push(name);
            }
        }
        (live, cut)
    }

    /// Defines `name` as exactly `style`: supplied properties are written,
    /// every other one is cleared, and a built-in base no longer applies.
    /// Does not commit.
    pub fn define_named_style(&self, name: &str, style: &Style) -> Result<(), DocError> {
        check_name(name)?;
        validate_style(style)?;
        let flat = self.doc.get_map(STYLES);
        let nested = self.nested_style(name);
        for property in STORED_KEYS {
            let k = key(property, name);
            match style.stored(property) {
                Some(value) => {
                    flat.insert(&k, value)?;
                }
                None => {
                    if flat.get(&k).is_some() {
                        flat.delete(&k)?;
                    }
                }
            }
            if let Some(m) = &nested
                && m.get(property).is_some()
            {
                m.delete(property)?;
            }
        }
        flat.insert(&key("state", name), "defined")?;
        Ok(())
    }

    /// Patches `name`: supplied properties are written, cleared ones removed
    /// (revealing the parent's), the rest kept. Does not commit.
    pub fn patch_named_style(&self, name: &str, patch: &StylePatch) -> Result<(), DocError> {
        check_name(name)?;
        patch.validate()?;
        let flat = self.doc.get_map(STYLES);
        for property in patch.supplied() {
            if let Some(value) = patch.set.stored(property) {
                flat.insert(&key(property, name), value)?;
            }
        }
        let nested = self.nested_style(name);
        for property in &patch.clear {
            let k = key(property, name);
            if flat.get(&k).is_some() {
                flat.delete(&k)?;
            }
            if let Some(m) = &nested
                && m.get(property).is_some()
            {
                m.delete(property)?;
            }
        }
        Ok(())
    }

    /// Renames `from` to `to`: `to` is defined with `from`'s effective
    /// properties, and `from` forwards to it. Does not commit.
    pub fn rename_named_style(&self, from: &str, to: &str) -> Result<(), DocError> {
        check_name(to)?;
        let record = self
            .style_record(from)
            .filter(|r| r.status == StyleStatus::Live)
            .ok_or_else(|| DocError::Store(format!("no live style {from}")))?;
        if from == to {
            return Ok(());
        }
        self.define_named_style(to, &record.style)?;
        self.doc
            .get_map(STYLES)
            .insert(&key("state", from), format!("renamed:{to}"))?;
        Ok(())
    }

    /// Deletes `name`, keeping its properties: whatever names it resolves
    /// through its parent. Does not commit.
    pub fn delete_named_style(&self, name: &str) -> Result<(), DocError> {
        self.doc
            .get_map(STYLES)
            .insert(&key("state", name), "deleted")?;
        Ok(())
    }

    /// After a nested (legacy) definition replaces `name`, the flat record
    /// must not mask it. Touches nothing in a document without `styles1`.
    pub(crate) fn reset_flat_style(&self, name: &str) -> Result<(), DocError> {
        let Some(flat) = self.flat_styles() else {
            return Ok(());
        };
        for property in STORED_KEYS.into_iter().chain(["state"]) {
            let k = key(property, name);
            if flat.get(&k).is_some() {
                flat.delete(&k)?;
            }
        }
        Ok(())
    }

    /// Patches a block's direct overrides property by property. Does not
    /// commit.
    pub fn patch_overrides(&self, node: NodeId, patch: &StylePatch) -> Result<(), DocError> {
        self.block(node)?;
        patch.validate()?;
        for property in patch.supplied() {
            let Some(value) = patch.set.stored(property) else {
                continue;
            };
            let root = if matches!(property, "alignment" | "tabs") {
                crate::marks::PATCHES
            } else {
                PARA
            };
            self.doc
                .get_map(root)
                .insert(&format!("{node}/{property}"), value)?;
        }
        if patch.clear.is_empty() {
            return Ok(());
        }
        let nested = self
            .meta_of(node)
            .ok()
            .and_then(|meta| nested_map(&meta, "overrides"));
        for property in &patch.clear {
            let k = format!("{node}/{property}");
            for root in [crate::marks::PATCHES, PARA] {
                if self.has_root(root) {
                    let map = self.doc.get_map(root);
                    if map.get(&k).is_some() {
                        map.delete(&k)?;
                    }
                }
            }
            if let Some(m) = &nested
                && m.get(property).is_some()
            {
                m.delete(property)?;
            }
        }
        Ok(())
    }

    /// Overlays `para1` patches on a block's overrides.
    pub(crate) fn read_para_patches(&self, node: NodeId, style: &mut Style) {
        if !self.has_root(PARA) {
            return;
        }
        let map = self.doc.get_map(PARA);
        for property in STORED_KEYS {
            if matches!(property, "alignment" | "tabs") {
                continue;
            }
            let k = format!("{node}/{property}");
            if map.get(&k).is_none() {
                continue;
            }
            let raw = get_str(&map, &k).unwrap_or_else(|| "unreadable patch value".into());
            style.apply_stored(property, raw);
        }
    }

    /// A replacement of the overrides drops the observed patches.
    pub(crate) fn clear_para_patches(&self, node: NodeId) -> Result<(), DocError> {
        if !self.has_root(PARA) {
            return Ok(());
        }
        let map = self.doc.get_map(PARA);
        for property in STORED_KEYS {
            let k = format!("{node}/{property}");
            if map.get(&k).is_some() {
                map.delete(&k)?;
            }
        }
        Ok(())
    }

    /// Removes the line defaults (`reprise.alignment` relations with an
    /// `alignment` parameter) on the authored line containing `at`, so the
    /// line follows the paragraph. Returns how many were removed. Does not
    /// commit.
    pub fn clear_line_alignment(&self, node: NodeId, at: usize) -> Result<usize, DocError> {
        use crate::marks::{ALIGNMENT, authored_start};
        use crate::relation::Target;
        let text = self.block(node)?.text;
        text.slice(at..at)?;
        let raw = text.to_string();
        let line = authored_start(&raw, at);
        let mut removed = 0;
        for (id, relation) in self.relations() {
            let Ok(r) = relation else { continue };
            if r.schema != ALIGNMENT || !r.params.contains_key("alignment") {
                continue;
            }
            let Some(Target::Range(range)) = r.first("line") else {
                continue;
            };
            let Some(extent) = self.range_extent(*range) else {
                continue;
            };
            if extent.start.0 == node && authored_start(&raw, extent.start.1) == line {
                self.delete_relation(id)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Conservative detection for the paragraph-style package bit: the flat
    /// roots (even emptied), any stored paragraph key or `justify`, in named
    /// styles, overrides or `marks1`, and formatting actions that clear.
    pub fn has_paragraph_styles(&self) -> bool {
        if self.has_root(STYLES) || self.has_root(PARA) {
            return true;
        }
        let uses = |map: &LoroMap| {
            PARAGRAPH_KEYS.iter().any(|k| map.get(k).is_some())
                || get_str(map, "alignment").as_deref() == Some("justify")
        };
        let LoroValue::Map(roots) = self.doc.get_value() else {
            return true;
        };
        let mut visited = 0usize;
        if roots.contains_key("styles") {
            let styles = self.doc.get_map("styles");
            for name in styles.keys() {
                visited += 1;
                if visited > MAX_SCANNED_KEYS {
                    return true;
                }
                match nested_map(&styles, &name) {
                    Some(m) if uses(&m) => return true,
                    Some(_) => {}
                    None => return true,
                }
            }
        }
        if roots.contains_key(crate::marks::PATCHES) {
            let marks = self.doc.get_map(crate::marks::PATCHES);
            for k in marks.keys() {
                visited += 1;
                if visited > MAX_SCANNED_KEYS {
                    return true;
                }
                if k.ends_with("/alignment") && get_str(&marks, &k).as_deref() == Some("justify") {
                    return true;
                }
            }
        }
        if roots.contains_key(crate::flow::BREAKS)
            && let LoroValue::Map(records) = self.doc.get_map(crate::flow::BREAKS).get_value()
        {
            for value in records.values() {
                visited += 1;
                if visited > MAX_SCANNED_KEYS {
                    return true;
                }
                let LoroValue::Container(id) = value else {
                    return true;
                };
                let Some(Container::Map(m)) = self.doc.get_container(id.clone()) else {
                    return true;
                };
                if nested_map(&m, "overrides").is_some_and(|o| uses(&o)) {
                    return true;
                }
            }
        }
        if roots.contains_key("content") {
            let tree = self.doc.get_tree("content");
            for id in tree.nodes() {
                visited += 1;
                if visited > MAX_SCANNED_KEYS {
                    return true;
                }
                let Ok(meta) = tree.get_meta(id) else {
                    return true;
                };
                if nested_map(&meta, "overrides").is_some_and(|o| uses(&o)) {
                    return true;
                }
            }
        }
        self.has_clearing_formats()
    }
}

#[cfg(test)]
mod tests;
