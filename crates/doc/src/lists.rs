//! Lists (06, 07, 29, 33): shared definitions, per-paragraph membership and
//! numbering derived at query time. See `docs/roles-and-lists.md`.
//!
//! -   A **definition** is a node of the `lists` tree, so its [`ListId`] is an
//!     opaque, never-reused ID (07). Its metadata is flat (`<level>.<property>`),
//!     so concurrent edits to different properties all survive.
//! -   **Membership** is three flat entries in the paragraph's own metadata
//!     (`list1`, `level1`, `restart1`), next to its role. A split copies the
//!     first two (`docs/flow.md`), a join keeps the first paragraph's.
//! -   **Numbering** is never stored (05): [`Document::list_numbering`] walks
//!     the document once per revision and caches the result.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use loro::{Frontiers, LoroMap, LoroValue, TreeID, ValueOrContainer};
use reprise_diag::Note;
use reprise_geom::Length;
use serde::{Deserialize, Serialize};

use crate::formatting::TextStyle;
use crate::lifecycle::STAGE_ORIGIN;
use crate::roles::codes;
use crate::{BlockKind, DocError, Document, ListId, NodeId, get_str};

/// The tree of list definitions.
pub(crate) const LISTS: &str = "lists";
/// Paragraph metadata: the list a paragraph belongs to (empty: none).
pub(crate) const LIST: &str = "list1";
/// Paragraph metadata: its level, 0 to 8.
pub(crate) const LEVEL: &str = "level1";
/// Paragraph metadata: restart numbering at this item with this value.
pub(crate) const RESTART: &str = "restart1";
const VERSION: &str = "v";
const DELETED: &str = "deleted";
const BASE: &str = "base";

/// The deepest level. Levels are 0 to `MAX_LEVEL`.
pub const MAX_LEVEL: u8 = 8;
/// How many levels a definition has.
pub const LEVELS: usize = MAX_LEVEL as usize + 1;
/// The longest marker template, in characters. Longer ones are cut.
pub const MAX_TEMPLATE: usize = 64;
/// How many `base` links inheritance follows.
pub const MAX_BASE_DEPTH: usize = 16;

/// How a level's counter is shown. Adding a format: see "Extension points"
/// in `docs/roles-and-lists.md`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NumberFormat {
    /// No counter: the template is a glyph.
    Bullet,
    Decimal,
    /// `01`, `02`, … `09`, `10` (the extension proof).
    DecimalLeadingZero,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
    /// No marker unless the template has literal text.
    None,
    /// A stored token this engine can't read; shown as decimal.
    Unknown(String),
}

impl NumberFormat {
    pub fn token(&self) -> &str {
        match self {
            NumberFormat::Bullet => "bullet",
            NumberFormat::Decimal => "decimal",
            NumberFormat::DecimalLeadingZero => "decimal-leading-zero",
            NumberFormat::LowerAlpha => "lower-alpha",
            NumberFormat::UpperAlpha => "upper-alpha",
            NumberFormat::LowerRoman => "lower-roman",
            NumberFormat::UpperRoman => "upper-roman",
            NumberFormat::None => "none",
            NumberFormat::Unknown(raw) => raw,
        }
    }

    pub fn parse(token: &str) -> NumberFormat {
        match token {
            "bullet" => NumberFormat::Bullet,
            "decimal" => NumberFormat::Decimal,
            "decimal-leading-zero" => NumberFormat::DecimalLeadingZero,
            "lower-alpha" => NumberFormat::LowerAlpha,
            "upper-alpha" => NumberFormat::UpperAlpha,
            "lower-roman" => NumberFormat::LowerRoman,
            "upper-roman" => NumberFormat::UpperRoman,
            "none" => NumberFormat::None,
            other => NumberFormat::Unknown(other.to_string()),
        }
    }

    /// Whether items show a sequence number (an `ol`, rather than a `ul`).
    pub fn is_ordered(&self) -> bool {
        !matches!(self, NumberFormat::Bullet | NumberFormat::None)
    }

    /// `n` in this format, or `None` when the format can't show it (the
    /// caller falls back to decimal with `list.counter-range`). A bullet
    /// counter in a template shows as decimal; `none` shows nothing.
    pub fn render(&self, n: i64) -> Option<String> {
        match self {
            NumberFormat::Decimal | NumberFormat::Bullet | NumberFormat::Unknown(_) => {
                Some(n.to_string())
            }
            NumberFormat::DecimalLeadingZero => Some(if (-9..=9).contains(&n) {
                format!("{}0{}", if n < 0 { "-" } else { "" }, n.unsigned_abs())
            } else {
                n.to_string()
            }),
            NumberFormat::LowerAlpha => alpha(n).map(|s| s.to_ascii_lowercase()),
            NumberFormat::UpperAlpha => alpha(n),
            NumberFormat::LowerRoman => roman(n).map(|s| s.to_ascii_lowercase()),
            NumberFormat::UpperRoman => roman(n),
            NumberFormat::None => Some(String::new()),
        }
    }
}

/// Bijective base 26: A … Z, AA … Bounded: an `i64` has at most 14 letters.
fn alpha(n: i64) -> Option<String> {
    if n < 1 {
        return None;
    }
    let mut n = n as u64;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).ok()
}

/// Standard Roman numerals, 1 to 3,999.
fn roman(n: i64) -> Option<String> {
    if !(1..=3999).contains(&n) {
        return None;
    }
    const TABLE: [(i64, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut n = n;
    let mut out = String::new();
    for (value, numeral) in TABLE {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    Some(out)
}

impl fmt::Display for NumberFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.token())
    }
}

impl Serialize for NumberFormat {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.token())
    }
}

impl<'de> Deserialize<'de> for NumberFormat {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(NumberFormat::parse(&String::deserialize(d)?))
    }
}

/// One level of a definition, as authored. Unset properties are inherited
/// from the base definition, then default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LevelDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<NumberFormat>,
    /// The marker template: `%1` to `%9` are the counters of levels 1 to 9
    /// in their own formats, `%%` is `%`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
    /// The text's start indent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indent: Option<Length>,
    /// How far before the text the marker starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hanging: Option<Length>,
    /// Marker formatting on top of the item's first-run style.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<TextStyle>,
}

impl LevelDef {
    fn is_empty(&self) -> bool {
        *self == LevelDef::default()
    }
}

/// A list definition as authored: a base to inherit from, and levels 0 to 8.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ListDefinition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<ListId>,
    /// By level. Absent levels inherit or default.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub levels: BTreeMap<u8, LevelDef>,
}

impl ListDefinition {
    /// Bullets at every level: `•`, `◦`, `▪`, repeating.
    pub fn bulleted() -> ListDefinition {
        ListDefinition::uniform(|_| NumberFormat::Bullet)
    }

    /// `1.`, `a.`, `i.`, repeating by level.
    pub fn numbered() -> ListDefinition {
        ListDefinition::uniform(|n| match n % 3 {
            0 => NumberFormat::Decimal,
            1 => NumberFormat::LowerAlpha,
            _ => NumberFormat::LowerRoman,
        })
    }

    fn uniform(format: impl Fn(u8) -> NumberFormat) -> ListDefinition {
        ListDefinition {
            base: None,
            levels: (0..=MAX_LEVEL)
                .map(|n| {
                    (
                        n,
                        LevelDef {
                            format: Some(format(n)),
                            ..LevelDef::default()
                        },
                    )
                })
                .collect(),
        }
    }

    /// Refuses levels past 8, templates longer than [`MAX_TEMPLATE`],
    /// unknown formats, negative indents and invalid marker styles.
    pub fn validate(&self) -> Result<(), DocError> {
        let bad = |why: &str| Err(DocError::List(why.to_string()));
        for (&n, level) in &self.levels {
            if n > MAX_LEVEL {
                return bad("list levels are 0 to 8");
            }
            if level
                .text
                .as_ref()
                .is_some_and(|t| t.chars().count() > MAX_TEMPLATE || t.contains(reprise_text::BREAK))
            {
                return bad("a marker template is too long or contains a reserved character");
            }
            if matches!(level.format, Some(NumberFormat::Unknown(_))) {
                return bad("unknown numbering format");
            }
            if level.indent.is_some_and(|l| l < Length::ZERO)
                || level.hanging.is_some_and(|l| l < Length::ZERO)
            {
                return bad("list indents are not negative");
            }
            if let Some(style) = &level.marker {
                style.validate()?;
            }
        }
        Ok(())
    }
}

/// A level with every property decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLevel {
    pub format: NumberFormat,
    pub text: String,
    pub start: i64,
    pub indent: Length,
    pub hanging: Length,
    pub marker: Option<TextStyle>,
}

impl ResolvedLevel {
    fn default_for(n: u8, format: Option<&NumberFormat>) -> ResolvedLevel {
        let format = format.cloned().unwrap_or(NumberFormat::Decimal);
        ResolvedLevel {
            text: default_text(n, &format),
            format,
            start: 1,
            indent: Length::from_pt(36).mul_ratio(i32::from(n) + 1, 1),
            hanging: Length::from_pt(18),
            marker: None,
        }
    }
}

fn default_text(n: u8, format: &NumberFormat) -> String {
    match format {
        NumberFormat::Bullet => ["•", "◦", "▪"][usize::from(n) % 3].to_string(),
        NumberFormat::None => String::new(),
        _ => format!("%{}.", n + 1),
    }
}

/// A paragraph's stored list membership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Membership {
    /// `None` when the stored ID can't be read.
    pub list: Option<ListId>,
    /// The stored level, unclamped.
    pub level: i64,
    pub restart: Option<i64>,
}

/// A numbered list item, derived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    pub list: ListId,
    /// 0 to 8.
    pub level: u8,
    /// The counter at this item's level.
    pub value: i64,
    /// Whether the item restarts numbering.
    pub restart: bool,
    /// The rendered marker, such as `3.` or `•`.
    pub marker: String,
    pub format: NumberFormat,
    pub indent: Length,
    pub hanging: Length,
    pub marker_style: Option<TextStyle>,
}

/// Every item's numbering, for one document revision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListNumbering {
    pub items: BTreeMap<NodeId, ListItem>,
    /// Items in document order.
    pub sequence: Vec<NodeId>,
    /// What went wrong reading each paragraph's membership, by paragraph.
    pub notes: BTreeMap<NodeId, Vec<Note>>,
}

#[derive(Default)]
pub(crate) struct ListCache {
    frontiers: Option<Frontiers>,
    numbering: Option<Arc<ListNumbering>>,
}

fn get_i64(map: &LoroMap, key: &str) -> Result<Option<i64>, ()> {
    match map.get(key) {
        None | Some(ValueOrContainer::Value(LoroValue::Null)) => Ok(None),
        Some(ValueOrContainer::Value(LoroValue::I64(n))) => Ok(Some(n)),
        Some(_) => Err(()),
    }
}

fn get_string(map: &LoroMap, key: &str) -> Result<Option<String>, ()> {
    match map.get(key) {
        None | Some(ValueOrContainer::Value(LoroValue::Null)) => Ok(None),
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Ok(Some(s.to_string())),
        Some(_) => Err(()),
    }
}

fn key(level: u8, property: &str) -> String {
    format!("{level}.{property}")
}

/// Reads a definition's metadata. `Err` when it is not version 1. Properties
/// with values of the wrong type are left unset and reported.
fn read_definition(meta: &LoroMap) -> Result<(ListDefinition, Vec<Note>), ()> {
    if get_i64(meta, VERSION) != Ok(Some(1)) {
        return Err(());
    }
    let mut notes = Vec::new();
    let mut unreadable = |what: String| {
        notes.push(Note::warning(
            codes::LIST_DEFINITION_UNREADABLE,
            format!("list property {what} can't be read; inherited or default used"),
        ))
    };
    let base = match get_string(meta, BASE) {
        Ok(None) => None,
        Ok(Some(raw)) => match ListId::parse(&raw) {
            Some(id) => Some(id),
            None => {
                unreadable(BASE.into());
                None
            }
        },
        Err(()) => {
            unreadable(BASE.into());
            None
        }
    };
    let mut levels = BTreeMap::new();
    for n in 0..=MAX_LEVEL {
        let mut level = LevelDef::default();
        match get_string(meta, &key(n, "format")) {
            Ok(v) => level.format = v.map(|t| NumberFormat::parse(&t)),
            Err(()) => unreadable(key(n, "format")),
        }
        match get_string(meta, &key(n, "text")) {
            Ok(v) => level.text = v,
            Err(()) => unreadable(key(n, "text")),
        }
        match get_i64(meta, &key(n, "start")) {
            Ok(v) => level.start = v,
            Err(()) => unreadable(key(n, "start")),
        }
        for (property, slot) in [("indent", &mut level.indent), ("hanging", &mut level.hanging)] {
            match get_i64(meta, &key(n, property)) {
                Ok(v) => {
                    *slot = v.map(|v| Length(v.clamp(0, i64::from(i32::MAX)) as i32));
                }
                Err(()) => unreadable(key(n, property)),
            }
        }
        match get_string(meta, &key(n, "marker")) {
            Ok(None) => {}
            Ok(Some(json)) => match serde_json::from_str::<TextStyle>(&json) {
                Ok(style) if style.validate().is_ok() => level.marker = Some(style),
                _ => unreadable(key(n, "marker")),
            },
            Err(()) => unreadable(key(n, "marker")),
        }
        if !level.is_empty() {
            levels.insert(n, level);
        }
    }
    Ok((ListDefinition { base, levels }, notes))
}

/// Writes the properties of `def` that differ from `old`, so that concurrent
/// edits to other properties survive. Unset properties are removed.
fn write_definition(
    meta: &LoroMap,
    def: &ListDefinition,
    old: &ListDefinition,
) -> Result<(), DocError> {
    if def.base != old.base {
        match def.base {
            Some(base) => meta.insert(BASE, base.to_string())?,
            None => meta.delete(BASE)?,
        }
    }
    let empty = LevelDef::default();
    for n in 0..=MAX_LEVEL {
        let new = def.levels.get(&n).unwrap_or(&empty);
        let was = old.levels.get(&n).unwrap_or(&empty);
        let string = |value: Option<String>, was: Option<String>, property: &str| {
            if value == was {
                return Ok(());
            }
            match value {
                Some(v) => meta.insert(&key(n, property), v),
                None => meta.delete(&key(n, property)),
            }
        };
        let int = |value: Option<i64>, was: Option<i64>, property: &str| {
            if value == was {
                return Ok(());
            }
            match value {
                Some(v) => meta.insert(&key(n, property), v),
                None => meta.delete(&key(n, property)),
            }
        };
        string(
            new.format.as_ref().map(|f| f.token().to_string()),
            was.format.as_ref().map(|f| f.token().to_string()),
            "format",
        )?;
        string(new.text.clone(), was.text.clone(), "text")?;
        int(new.start, was.start, "start")?;
        int(
            new.indent.map(|l| i64::from(l.0)),
            was.indent.map(|l| i64::from(l.0)),
            "indent",
        )?;
        int(
            new.hanging.map(|l| i64::from(l.0)),
            was.hanging.map(|l| i64::from(l.0)),
            "hanging",
        )?;
        let json = |s: &Option<TextStyle>| s.as_ref().and_then(|s| serde_json::to_string(s).ok());
        string(json(&new.marker), json(&was.marker), "marker")?;
    }
    Ok(())
}

impl Document {
    fn lists_tree(&self) -> loro::LoroTree {
        self.tree(LISTS)
    }

    fn list_flagged(&self, id: ListId) -> bool {
        let tree = self.lists_tree();
        tree.get_meta(id.0)
            .ok()
            .is_none_or(|m| !matches!(m.get(DELETED), Some(ValueOrContainer::Value(LoroValue::Bool(false)))))
    }

    /// Whether a definition exists, is not deleted and is readable.
    pub fn is_list_live(&self, id: ListId) -> bool {
        let tree = self.lists_tree();
        tree.contains(id.0)
            && !tree.is_node_deleted(&id.0).unwrap_or(true)
            && !self.list_flagged(id)
            && tree
                .get_meta(id.0)
                .is_ok_and(|m| get_i64(&m, VERSION) == Ok(Some(1)))
    }

    /// A live definition as authored.
    pub fn list_definition(&self, id: ListId) -> Result<ListDefinition, DocError> {
        if !self.is_list_live(id) {
            return Err(DocError::List(format!("no live list {id}")));
        }
        let meta = self.lists_tree().get_meta(id.0)?;
        read_definition(&meta)
            .map(|(def, _)| def)
            .map_err(|()| DocError::List(format!("list {id} is unreadable")))
    }

    /// Every live definition in ID order. Unreadable ones are `Err` and kept
    /// in the document (34).
    pub fn lists(&self) -> Vec<(ListId, Result<ListDefinition, String>)> {
        let tree = self.lists_tree();
        let mut ids: Vec<TreeID> = tree
            .nodes()
            .into_iter()
            .filter(|&id| {
                tree.contains(id)
                    && !tree.is_node_deleted(&id).unwrap_or(true)
                    && !self.list_flagged(ListId(id))
            })
            .collect();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let read = tree
                    .get_meta(id)
                    .map_err(|e| e.to_string())
                    .and_then(|m| {
                        read_definition(&m)
                            .map(|(d, _)| d)
                            .map_err(|()| "unreadable list definition".to_string())
                    });
                (ListId(id), read)
            })
            .collect()
    }

    /// Creates a deleted definition outside the undo history, so undo and
    /// redo of its creation keep its ID (07). Commits what was pending first,
    /// as part of the current step. [`Document::activate_list`] makes it live.
    pub fn stage_list(&self, def: &ListDefinition) -> Result<ListId, DocError> {
        def.validate()?;
        self.commit_part();
        self.doc.set_next_commit_origin(STAGE_ORIGIN);
        let staged = (|| {
            let tree = self.lists_tree();
            let id = tree.create(None)?;
            let meta = tree.get_meta(id)?;
            meta.insert(VERSION, 1)?;
            meta.insert(DELETED, true)?;
            write_definition(&meta, def, &ListDefinition::default())?;
            Ok(ListId(id))
        })();
        self.doc.commit();
        staged
    }

    /// Makes a staged or deleted definition live. Does not commit.
    pub fn activate_list(&self, id: ListId) -> Result<(), DocError> {
        let tree = self.lists_tree();
        if !tree.contains(id.0) || tree.is_node_deleted(&id.0).unwrap_or(true) {
            return Err(DocError::List(format!("no list {id}")));
        }
        tree.get_meta(id.0)?.insert(DELETED, false)?;
        Ok(())
    }

    /// Stages and activates a definition: for imports and tests that don't
    /// need the creation to be its own undo step.
    pub fn create_list(&self, def: &ListDefinition) -> Result<ListId, DocError> {
        let id = self.stage_list(def)?;
        self.activate_list(id)?;
        Ok(id)
    }

    /// Writes the properties of a live definition that changed. Does not commit.
    pub fn update_list(&self, id: ListId, def: &ListDefinition) -> Result<(), DocError> {
        def.validate()?;
        let old = self.list_definition(id)?;
        write_definition(&self.lists_tree().get_meta(id.0)?, def, &old)
    }

    /// Soft-deletes a definition (07). Its items become plain paragraphs with
    /// `list.definition-missing`; undo brings it back. Does not commit.
    pub fn delete_list(&self, id: ListId) -> Result<(), DocError> {
        if !self.is_list_live(id) {
            return Err(DocError::List(format!("no live list {id}")));
        }
        self.lists_tree().get_meta(id.0)?.insert(DELETED, true)?;
        Ok(())
    }

    /// A block's stored membership, if it names a list.
    pub fn list_membership(&self, id: NodeId) -> Result<Option<Membership>, DocError> {
        self.block(id)?;
        Ok(read_membership(&self.meta_of(id)?))
    }

    fn check_item(&self, id: NodeId) -> Result<(), DocError> {
        let block = self.block(id)?;
        if !matches!(block.kind, BlockKind::Paragraph | BlockKind::Annotation) {
            return Err(DocError::Malformed(id, "only text blocks can be list items"));
        }
        Ok(())
    }

    /// Makes a block an item of `list` at `level`, keeping any restart. Does
    /// not commit.
    pub fn set_list_item(&self, id: NodeId, list: ListId, level: u8) -> Result<(), DocError> {
        if level > MAX_LEVEL {
            return Err(DocError::List("list levels are 0 to 8".into()));
        }
        self.check_item(id)?;
        if !self.is_list_live(list) {
            return Err(DocError::List(format!("no live list {list}")));
        }
        let meta = self.meta_of(id)?;
        if get_string(&meta, LIST).ok().flatten() != Some(list.to_string()) {
            meta.insert(LIST, list.to_string())?;
        }
        if get_i64(&meta, LEVEL).ok().flatten() != Some(i64::from(level)) {
            meta.insert(LEVEL, i64::from(level))?;
        }
        Ok(())
    }

    /// Sets an item's level. Does not commit.
    pub fn set_list_level(&self, id: NodeId, level: u8) -> Result<(), DocError> {
        if level > MAX_LEVEL {
            return Err(DocError::List("list levels are 0 to 8".into()));
        }
        self.check_item(id)?;
        self.meta_of(id)?.insert(LEVEL, i64::from(level))?;
        Ok(())
    }

    /// Takes a block out of its list: it is a plain paragraph again (its role
    /// stays). Does not commit.
    pub fn remove_list_item(&self, id: NodeId) -> Result<(), DocError> {
        self.block(id)?;
        let meta = self.meta_of(id)?;
        meta.insert(LIST, "")?;
        if meta.get(RESTART).is_some() {
            meta.insert(RESTART, LoroValue::Null)?;
        }
        Ok(())
    }

    /// Restarts numbering at an item with `value`, or (`None`) continues it.
    /// Does not commit.
    pub fn set_list_restart(&self, id: NodeId, value: Option<i64>) -> Result<(), DocError> {
        self.check_item(id)?;
        let meta = self.meta_of(id)?;
        match value {
            Some(v) => meta.insert(RESTART, v)?,
            None if meta.get(RESTART).is_some() => meta.insert(RESTART, LoroValue::Null)?,
            None => {}
        }
        Ok(())
    }

    /// A list item's derived numbering, if `id` is one.
    pub fn list_item(&self, id: NodeId) -> Option<ListItem> {
        self.list_numbering().items.get(&id).cloned()
    }

    /// Every item's numbering, derived from the whole document. Cached per
    /// revision; read fresh while a transaction is open.
    pub fn list_numbering(&self) -> Arc<ListNumbering> {
        if self.doc.get_pending_txn_len() > 0 {
            return Arc::new(self.number_lists());
        }
        let frontiers = self.doc.state_frontiers();
        {
            let cache = self.list_cache.lock().unwrap_or_else(|e| e.into_inner());
            if cache.frontiers.as_ref() == Some(&frontiers)
                && let Some(hit) = &cache.numbering
            {
                return hit.clone();
            }
        }
        let value = Arc::new(self.number_lists());
        let mut cache = self.list_cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.frontiers = Some(frontiers);
        cache.numbering = Some(value.clone());
        value
    }

    /// The levels of a list with every property decided, or why not.
    fn resolve_list(&self, id: ListId) -> Result<(Vec<ResolvedLevel>, Vec<Note>), Note> {
        let missing = |what: String| Note::warning(codes::LIST_DEFINITION_MISSING, what);
        if !self.is_list_live(id) {
            return Err(missing(format!(
                "list {id} is missing, deleted or unreadable; the item is a plain paragraph"
            )));
        }
        let tree = self.lists_tree();
        let mut notes = Vec::new();
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut at = Some(id);
        while let Some(list) = at.take() {
            if !seen.insert(list) || chain.len() >= MAX_BASE_DEPTH {
                notes.push(Note::warning(
                    codes::LIST_DEFINITION_CYCLE,
                    format!("the base chain of list {id} loops or is too long; inheritance stops"),
                ));
                break;
            }
            if list != id && !self.is_list_live(list) {
                notes.push(missing(format!(
                    "base list {list} is missing, deleted or unreadable; inheritance stops"
                )));
                break;
            }
            let Ok(meta) = tree.get_meta(list.0) else {
                break;
            };
            let Ok((def, read_notes)) = read_definition(&meta) else {
                break;
            };
            notes.extend(read_notes);
            at = def.base;
            chain.push(def);
        }
        let levels = (0..=MAX_LEVEL)
            .map(|n| {
                let pick = |f: &dyn Fn(&LevelDef) -> bool| {
                    chain.iter().find_map(|d| d.levels.get(&n).filter(|l| f(l)))
                };
                let format = pick(&|l| l.format.is_some()).and_then(|l| l.format.clone());
                let mut level = ResolvedLevel::default_for(n, format.as_ref());
                if let Some(t) = pick(&|l| l.text.is_some()).and_then(|l| l.text.clone()) {
                    level.text = t;
                }
                if let Some(v) = pick(&|l| l.start.is_some()).and_then(|l| l.start) {
                    level.start = v;
                }
                if let Some(v) = pick(&|l| l.indent.is_some()).and_then(|l| l.indent) {
                    level.indent = v;
                }
                if let Some(v) = pick(&|l| l.hanging.is_some()).and_then(|l| l.hanging) {
                    level.hanging = v;
                }
                level.marker = pick(&|l| l.marker.is_some()).and_then(|l| l.marker.clone());
                level
            })
            .collect();
        Ok((levels, notes))
    }

    fn number_lists(&self) -> ListNumbering {
        let mut out = ListNumbering::default();
        let mut resolved: BTreeMap<ListId, Result<(Vec<ResolvedLevel>, Vec<Note>), Note>> =
            BTreeMap::new();
        let mut counters: BTreeMap<ListId, [Option<i64>; LEVELS]> = BTreeMap::new();
        for node in self.document_order() {
            let Ok(meta) = self.meta_of(node) else {
                continue;
            };
            let Some(membership) = read_membership(&meta) else {
                continue;
            };
            let mut notes = Vec::new();
            let kind = get_str(&meta, "kind").and_then(|k| BlockKind::parse(&k));
            if !matches!(kind, Some(BlockKind::Paragraph | BlockKind::Annotation)) {
                continue;
            }
            let Some(list) = membership.list else {
                out.notes.entry(node).or_default().push(Note::warning(
                    codes::LIST_DEFINITION_MISSING,
                    "the item's list ID can't be read; it is a plain paragraph",
                ));
                continue;
            };
            let levels = match resolved
                .entry(list)
                .or_insert_with(|| self.resolve_list(list))
            {
                Ok((levels, list_notes)) => {
                    notes.extend(list_notes.iter().cloned());
                    levels
                }
                Err(note) => {
                    out.notes.entry(node).or_default().push(note.clone());
                    continue;
                }
            };
            let level = membership.level.clamp(0, i64::from(MAX_LEVEL)) as u8;
            if i64::from(level) != membership.level {
                notes.push(Note::warning(
                    codes::LIST_LEVEL_CLAMPED,
                    format!("list level {} is outside 0 to 8; clamped", membership.level),
                ));
            }
            let index = usize::from(level);
            let Some(spec) = levels.get(index) else {
                continue;
            };
            let values = counters.entry(list).or_insert([None; LEVELS]);
            let value = membership.restart.unwrap_or_else(|| {
                values[index].map_or(spec.start, |v| v.saturating_add(1))
            });
            values[index] = Some(value);
            for deeper in values.iter_mut().skip(index + 1) {
                *deeper = None;
            }
            let marker = render_marker(&spec.text, values, levels, &mut notes);
            out.sequence.push(node);
            out.items.insert(
                node,
                ListItem {
                    list,
                    level,
                    value,
                    restart: membership.restart.is_some(),
                    marker,
                    format: spec.format.clone(),
                    indent: spec.indent,
                    hanging: spec.hanging,
                    marker_style: spec.marker.clone(),
                },
            );
            if !notes.is_empty() {
                out.notes.entry(node).or_default().extend(notes);
            }
        }
        out
    }
}

fn read_membership(meta: &LoroMap) -> Option<Membership> {
    let raw = match meta.get(LIST)? {
        ValueOrContainer::Value(LoroValue::String(s)) => s.to_string(),
        ValueOrContainer::Value(LoroValue::Null) => return None,
        _ => {
            return Some(Membership {
                list: None,
                level: 0,
                restart: None,
            });
        }
    };
    if raw.is_empty() {
        return None;
    }
    Some(Membership {
        list: ListId::parse(&raw),
        level: match meta.get(LEVEL) {
            Some(ValueOrContainer::Value(LoroValue::I64(n))) => n,
            _ => 0,
        },
        restart: get_i64(meta, RESTART).ok().flatten(),
    })
}

/// Fills a template: `%1` to `%9` with the counters of levels 1 to 9 in their
/// own formats (an unset ancestor shows its start), `%%` with `%`.
fn render_marker(
    template: &str,
    values: &[Option<i64>; LEVELS],
    levels: &[ResolvedLevel],
    notes: &mut Vec<Note>,
) -> String {
    let mut chars: Vec<char> = template.chars().collect();
    if chars.len() > MAX_TEMPLATE {
        chars.truncate(MAX_TEMPLATE);
        notes.push(Note::warning(
            codes::LIST_LIMIT,
            format!("a marker template is longer than {MAX_TEMPLATE} characters; cut"),
        ));
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        i += 1;
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.get(i) {
            Some('%') => {
                out.push('%');
                i += 1;
            }
            Some(&d @ '1'..='9') => {
                i += 1;
                let k = d as usize - '1' as usize;
                let (Some(level), Some(value)) = (levels.get(k), values.get(k)) else {
                    continue;
                };
                let n = value.unwrap_or(level.start);
                if let NumberFormat::Unknown(raw) = &level.format {
                    notes.push(Note::warning(
                        codes::LIST_FORMAT_UNKNOWN,
                        format!("numbering format {raw:?} is not known; shown as decimal"),
                    ));
                }
                match level.format.render(n) {
                    Some(s) => out.push_str(&s),
                    None => {
                        notes.push(Note::warning(
                            codes::LIST_COUNTER_RANGE,
                            format!("{} can't show {n}; shown as decimal", level.format),
                        ));
                        out.push_str(&n.to_string());
                    }
                }
            }
            _ => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_render_and_fall_back() {
        assert_eq!(roman(1).as_deref(), Some("I"));
        assert_eq!(roman(3999).as_deref(), Some("MMMCMXCIX"));
        assert_eq!(roman(4000), None);
        assert_eq!(roman(0), None);
        assert_eq!(roman(-5), None);
        assert_eq!(alpha(1).as_deref(), Some("A"));
        assert_eq!(alpha(26).as_deref(), Some("Z"));
        assert_eq!(alpha(27).as_deref(), Some("AA"));
        assert_eq!(alpha(702).as_deref(), Some("ZZ"));
        assert_eq!(alpha(703).as_deref(), Some("AAA"));
        assert_eq!(alpha(0), None);
        assert!(alpha(i64::MAX).is_some_and(|s| s.len() <= 14));
        let lz = NumberFormat::DecimalLeadingZero;
        assert_eq!(lz.render(3).as_deref(), Some("03"));
        assert_eq!(lz.render(-3).as_deref(), Some("-03"));
        assert_eq!(lz.render(12).as_deref(), Some("12"));
        assert_eq!(
            NumberFormat::Decimal.render(i64::MIN).as_deref(),
            Some("-9223372036854775808")
        );
        for f in [
            "bullet",
            "decimal",
            "decimal-leading-zero",
            "lower-alpha",
            "upper-alpha",
            "lower-roman",
            "upper-roman",
            "none",
        ] {
            assert_eq!(NumberFormat::parse(f).token(), f);
            assert!(!matches!(NumberFormat::parse(f), NumberFormat::Unknown(_)));
        }
        assert_eq!(
            NumberFormat::parse("klingon"),
            NumberFormat::Unknown("klingon".into())
        );
    }

    #[test]
    fn templates_fill_placeholders_and_are_bounded() {
        let levels: Vec<ResolvedLevel> = (0..=MAX_LEVEL)
            .map(|n| ResolvedLevel::default_for(n, Some(&NumberFormat::LowerRoman)))
            .collect();
        let mut values = [None; LEVELS];
        values[0] = Some(4);
        values[1] = Some(2);
        let mut notes = Vec::new();
        assert_eq!(
            render_marker("%1.%2.%3 %% %x%", &values, &levels, &mut notes),
            "iv.ii.i % %x%"
        );
        assert!(notes.is_empty());
        values[0] = Some(5000);
        let long = "%1".repeat(100);
        let s = render_marker(&long, &values, &levels, &mut notes);
        assert_eq!(s, "5000".repeat(32));
        let codes: Vec<_> = notes.iter().map(|n| n.code).collect();
        assert!(codes.contains(&codes::LIST_LIMIT));
        assert!(codes.contains(&codes::LIST_COUNTER_RANGE));
    }
}
