//! Inline objects: U+FFFC characters in a flow host's text with a record in
//! the root `inline1` map (05, 07, 09, 11). See `docs/inline.md`.
//!
//! A U+FFFC is visible in its paragraph exactly when it has a readable,
//! active record; otherwise it is hidden like an inactive flow break.
//! Insertion is staged and deletion deactivates, so undo and redo keep an
//! object's identity and parameters.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;

use loro::cursor::Side;
use loro::{Container, ID, LoroMap, LoroValue, TreeID, ValueOrContainer};
use reprise_geom::Length;
use reprise_text::{Affinity, Anchor, OBJECT};
use serde::{Deserialize, Serialize};

use crate::lifecycle::STAGE_ORIGIN;
use crate::{DocError, Document, NodeId, get_str};

/// The root map of inline object records, keyed `host/char` like breaks.
pub(crate) const INLINE: &str = "inline1";
const VERSION: &str = "v";
const ACTIVE: &str = "active";
const KIND: &str = "kind";
const ALIGN: &str = "align";
/// Every parameter is its own entry, so concurrent edits of different
/// parameters both survive.
const PARAM: &str = "p:";

/// The built-in kinds. Others are kept verbatim and reported by layout.
pub const IMAGE: &str = "image";
pub const FIELD: &str = "field";

/// Bounds on authored objects. Records written by peers are read whatever
/// their size; layout reports what it can't use.
pub const MAX_INLINE_PARAMS: usize = 64;
pub const MAX_INLINE_NAME_BYTES: usize = 64;
pub const MAX_INLINE_VALUE_BYTES: usize = 16 * 1024;

/// An inline object's identity: its host and its character. Displayed like a
/// break paragraph's ID, `12@1/34@2`. Never reused: a copy gets a new one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InlineId {
    pub(crate) host: TreeID,
    pub(crate) mark: ID,
}

impl InlineId {
    pub fn parse(s: &str) -> Option<InlineId> {
        let id = NodeId::parse(s)?;
        Some(InlineId {
            host: id.node,
            mark: id.mark?,
        })
    }

    /// The flow host whose text holds the object.
    pub fn host(&self) -> NodeId {
        NodeId::tree(self.host)
    }

    fn key(&self) -> String {
        NodeId::at_break(self.host, self.mark).to_string()
    }
}

impl fmt::Display for InlineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.host, self.mark)
    }
}

impl fmt::Debug for InlineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Serialize for InlineId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for InlineId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        InlineId::parse(&s).ok_or_else(|| serde::de::Error::custom("bad InlineId"))
    }
}

/// Where an object sits relative to the line's baseline.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum InlineAlign {
    /// The box's bottom on the baseline. The default.
    #[default]
    Baseline,
    /// Centred half the surrounding face's x-height above the baseline.
    Middle,
    /// The box's top at the surrounding face's ascent.
    TextTop,
    /// The box's bottom at the surrounding face's descent.
    TextBottom,
}

impl InlineAlign {
    pub fn as_str(self) -> &'static str {
        match self {
            InlineAlign::Baseline => "baseline",
            InlineAlign::Middle => "middle",
            InlineAlign::TextTop => "text-top",
            InlineAlign::TextBottom => "text-bottom",
        }
    }

    pub fn parse(s: &str) -> Option<InlineAlign> {
        Some(match s {
            "baseline" => InlineAlign::Baseline,
            "middle" => InlineAlign::Middle,
            "text-top" => InlineAlign::TextTop,
            "text-bottom" => InlineAlign::TextBottom,
            _ => return None,
        })
    }
}

/// An inline object as stored: its kind, alignment and string parameters.
/// Every kind uses this form, so a kind this engine doesn't know survives
/// reading, copying and writing unchanged (34).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InlineObject {
    pub kind: String,
    #[serde(default)]
    pub align: InlineAlign,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

/// A length for an inline object's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineSize {
    Pt(Length),
    /// Thousandths of the surrounding font size.
    Em(i32),
    /// Thousandths of the frame's inline extent.
    Percent(i32),
}

impl InlineSize {
    pub fn parse(s: &str) -> Option<InlineSize> {
        let (unit, n) = s.split_once(':')?;
        let n: i32 = n.parse().ok()?;
        Some(match unit {
            "pt" => InlineSize::Pt(Length(n)),
            "em" => InlineSize::Em(n),
            "pct" => InlineSize::Percent(n),
            _ => return None,
        })
    }

    /// Never negative.
    pub fn resolve(self, em: Length, frame: Length) -> Length {
        match self {
            InlineSize::Pt(l) => l,
            InlineSize::Em(p) => em.mul_ratio(p, 1000),
            InlineSize::Percent(p) => frame.mul_ratio(p, 1000),
        }
        .max(Length::ZERO)
    }
}

impl fmt::Display for InlineSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InlineSize::Pt(l) => write!(f, "pt:{}", l.0),
            InlineSize::Em(p) => write!(f, "em:{p}"),
            InlineSize::Percent(p) => write!(f, "pct:{p}"),
        }
    }
}

/// The parameters of an `image` object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineImage {
    /// The image's SHA-256, as for image blocks.
    pub asset: String,
    pub width: Option<InlineSize>,
    pub height: Option<InlineSize>,
    pub alt: String,
}

fn is_asset(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_INLINE_NAME_BYTES
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
}

impl InlineObject {
    /// An image object. Sizes default to the image's own.
    pub fn image(asset: &str, alt: &str) -> InlineObject {
        InlineObject {
            kind: IMAGE.into(),
            ..Default::default()
        }
        .with("asset", asset)
        .with("alt", alt)
    }

    /// A field computed by the named field function.
    pub fn field(function: &str) -> InlineObject {
        InlineObject {
            kind: FIELD.into(),
            ..Default::default()
        }
        .with("field", function)
    }

    pub fn with(mut self, name: &str, value: impl Into<String>) -> InlineObject {
        self.params.insert(name.into(), value.into());
        self
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }

    /// The parameters of an `image` object, or why they can't be read.
    pub fn as_image(&self) -> Result<InlineImage, String> {
        if self.kind != IMAGE {
            return Err(format!("{} is not an image", self.kind));
        }
        let asset = self.param("asset").unwrap_or_default();
        if !is_asset(asset) {
            return Err("image asset is not a SHA-256".into());
        }
        let size = |name| match self.param(name) {
            None => Ok(None),
            Some(raw) => InlineSize::parse(raw)
                .map(Some)
                .ok_or_else(|| format!("unreadable image {name} {raw:?}")),
        };
        Ok(InlineImage {
            asset: asset.into(),
            width: size("width")?,
            height: size("height")?,
            alt: self.param("alt").unwrap_or_default().into(),
        })
    }

    /// Checks an object an author is about to store: bounded names and
    /// values, and readable parameters for the built-in kinds.
    pub fn validate(&self) -> Result<(), String> {
        if !is_name(&self.kind) {
            return Err(format!("bad inline object kind {:?}", self.kind));
        }
        if self.params.len() > MAX_INLINE_PARAMS {
            return Err("too many inline object parameters".into());
        }
        for (name, value) in &self.params {
            if !is_name(name) {
                return Err(format!("bad inline object parameter name {name:?}"));
            }
            if value.len() > MAX_INLINE_VALUE_BYTES {
                return Err(format!("inline object parameter {name} is too long"));
            }
        }
        match self.kind.as_str() {
            IMAGE => self.as_image().map(|_| ()),
            FIELD => match self.param("field") {
                Some(f) if is_name(f) => Ok(()),
                _ => Err("a field needs a field function name".into()),
            },
            _ => Ok(()),
        }
    }

    fn write(&self, map: &LoroMap) -> Result<(), DocError> {
        map.insert(KIND, self.kind.as_str())?;
        map.insert(ALIGN, self.align.as_str())?;
        for (name, value) in &self.params {
            map.insert(&format!("{PARAM}{name}"), value.as_str())?;
        }
        Ok(())
    }

    /// Reads a record leniently: whatever is there, so copies keep it.
    fn read(map: &LoroMap) -> InlineObject {
        let mut params = BTreeMap::new();
        map.for_each(|key, value| {
            if let Some(name) = key.strip_prefix(PARAM)
                && let ValueOrContainer::Value(LoroValue::String(v)) = value
            {
                params.insert(name.to_string(), v.to_string());
            }
        });
        InlineObject {
            kind: get_str(map, KIND).unwrap_or_default(),
            align: get_str(map, ALIGN)
                .and_then(|a| InlineAlign::parse(&a))
                .unwrap_or_default(),
            params,
        }
    }
}

/// A parameter change: `None` removes the parameter.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InlinePatch {
    #[serde(default)]
    pub align: Option<InlineAlign>,
    #[serde(default)]
    pub params: BTreeMap<String, Option<String>>,
}

impl InlinePatch {
    /// The object after this patch.
    pub fn apply(&self, object: &InlineObject) -> InlineObject {
        let mut out = object.clone();
        if let Some(align) = self.align {
            out.align = align;
        }
        for (name, value) in &self.params {
            match value {
                Some(v) => out.params.insert(name.clone(), v.clone()),
                None => out.params.remove(name),
            };
        }
        out
    }
}

/// One live object of a paragraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineEntry {
    pub id: InlineId,
    /// Byte offset of its U+FFFC in the paragraph's text.
    pub at: usize,
    pub object: InlineObject,
}

/// What a U+FFFC character in a host is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectState {
    /// A readable, active record: part of the paragraph's text.
    Visible,
    /// A readable record, inactive (staged, deleted or undone): hidden.
    Inactive,
    /// No readable record: hidden and reported.
    Orphan,
}

/// One U+FFFC of a host's text, in text order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ObjectMark {
    pub(crate) id: Option<ID>,
    /// Scalar index in the host text.
    pub(crate) u: usize,
    pub(crate) state: ObjectState,
}

/// Hostile or stale inline state, found at read time and never repaired.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineAudit {
    /// Hidden U+FFFC characters without a readable record, by the paragraph
    /// whose region holds them, with their count.
    pub orphan_markers: BTreeMap<NodeId, usize>,
    /// Record keys whose character is missing, deleted or not U+FFFC.
    pub orphan_records: Vec<String>,
}

fn get_i64(map: &LoroMap, key: &str) -> Option<i64> {
    match map.get(key)? {
        ValueOrContainer::Value(LoroValue::I64(n)) => Some(n),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || s.len() > 4096 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

impl Document {
    /// The record map, created if the document has none: for writes. The
    /// handle is kept, so later reads don't look the root up again.
    pub(crate) fn inline_map(&self) -> LoroMap {
        let map = self.doc.get_map(INLINE);
        *self.inline_root.lock().unwrap_or_else(|e| e.into_inner()) = Some(map.clone());
        map
    }

    /// The record map, if the document has one. A read must not create the
    /// root (the package would change), and must not call `LoroDoc::get_value`:
    /// that computes every root's value to tell whether it is empty, so each
    /// call costs the whole content tree and recurses to its depth (a 1,024-
    /// deep tree overflowed the stack). Looking the root up by name touches
    /// only it, but still reads the whole record map to tell whether it is
    /// empty, so a root found is remembered: a record read per object would
    /// otherwise make 50,000 objects quadratic. Roots are never removed.
    fn read_inline_map(&self) -> Option<LoroMap> {
        let mut known = self.inline_root.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(map) = known.as_ref() {
            return Some(map.clone());
        }
        let map = self.map_root(INLINE)?;
        *known = Some(map.clone());
        Some(map)
    }

    /// A readable record (version 1), active or not.
    pub(crate) fn inline_record(&self, host: TreeID, mark: ID) -> Option<LoroMap> {
        let key = NodeId::at_break(host, mark).to_string();
        match self.read_inline_map()?.get(&key)? {
            ValueOrContainer::Container(Container::Map(m)) if get_i64(&m, VERSION) == Some(1) => {
                Some(m)
            }
            _ => None,
        }
    }

    /// What a U+FFFC with character ID `mark` in `host` is.
    pub(crate) fn object_state(&self, host: TreeID, mark: ID) -> ObjectState {
        match self.inline_record(host, mark) {
            None => ObjectState::Orphan,
            Some(rec) => match rec.get(ACTIVE) {
                Some(ValueOrContainer::Value(LoroValue::Bool(true))) => ObjectState::Visible,
                _ => ObjectState::Inactive,
            },
        }
    }

    /// Whether any inline object record exists, live or not. Packages that
    /// have one need an engine that understands them.
    pub fn has_inline_objects(&self) -> bool {
        self.read_inline_map().is_some_and(|map| !map.is_empty())
    }

    /// The live objects of a paragraph, in text order.
    pub fn inline_objects(&self, node: NodeId) -> Result<Vec<InlineEntry>, DocError> {
        let block = self.block(node)?;
        let Some(flow) = self.flow(node.node) else {
            return Ok(Vec::new());
        };
        if flow.objects.is_empty() {
            return Ok(Vec::new());
        }
        let segs = flow
            .para(node)
            .map(|p| p.segs.clone())
            .ok_or(DocError::NoNode(node))?;
        let inside = |u: usize| segs.iter().any(|s| s.chars.contains(&u));
        let marks = flow
            .objects
            .iter()
            .filter(|m| m.state == ObjectState::Visible && inside(m.u));
        let text = block.text.to_string();
        let offsets = text
            .char_indices()
            .filter(|&(_, c)| c == OBJECT)
            .map(|(i, _)| i);
        Ok(marks
            .zip(offsets)
            .filter_map(|(m, at)| {
                let mark = m.id?;
                let rec = self.inline_record(node.node, mark)?;
                Some(InlineEntry {
                    id: InlineId {
                        host: node.node,
                        mark,
                    },
                    at,
                    object: InlineObject::read(&rec),
                })
            })
            .collect())
    }

    /// The object whose U+FFFC is at byte `at` of a paragraph.
    pub fn inline_at(&self, node: NodeId, at: usize) -> Option<InlineId> {
        self.inline_objects(node)
            .ok()?
            .into_iter()
            .find(|e| e.at == at)
            .map(|e| e.id)
    }

    /// A live object's paragraph and byte offset.
    pub fn inline_location(&self, id: InlineId) -> Option<(NodeId, usize)> {
        let flow = self.flow(id.host)?;
        let mark = flow
            .objects
            .iter()
            .find(|m| m.id == Some(id.mark) && m.state == ObjectState::Visible)?;
        let para = flow
            .live()
            .find(|p| p.segs.iter().any(|s| s.chars.contains(&mark.u)))?;
        let block = self.block(para.id).ok()?;
        let at = block.text.from_shared(mark.u).ok()??;
        Some((para.id, at))
    }

    /// A record's object, live or not.
    pub fn inline_object(&self, id: InlineId) -> Result<InlineObject, DocError> {
        self.inline_record(id.host, id.mark)
            .map(|r| InlineObject::read(&r))
            .ok_or(DocError::Malformed(id.host(), "no inline object record"))
    }

    /// Stages an object at byte `at` of a flow paragraph: its U+FFFC and an
    /// inactive record, in a commit outside the undo history (which commits
    /// what was pending as part of the step). Invisible until
    /// [`Document::activate_inline`].
    pub fn stage_inline(
        &self,
        node: NodeId,
        at: usize,
        object: &InlineObject,
    ) -> Result<InlineId, DocError> {
        object
            .validate()
            .map_err(|_| DocError::Malformed(node, "invalid inline object"))?;
        let block = self.block(node)?;
        if !self.in_flow(node) {
            return Err(DocError::Malformed(
                node,
                "inline objects need a flow paragraph",
            ));
        }
        let shared = block.text.to_shared(at)?;
        let text = block.text.loro().clone();
        self.commit_part();
        self.doc.set_next_commit_origin(STAGE_ORIGIN);
        let staged = (|| {
            text.insert(shared, &OBJECT.to_string())?;
            let mark = text
                .get_cursor(shared, Side::Left)
                .and_then(|c| c.id)
                .ok_or_else(|| DocError::Store("the object has no identity".into()))?;
            let id = InlineId {
                host: node.node,
                mark,
            };
            self.write_inline_record(id, object, false)?;
            Ok(id)
        })();
        self.doc.commit();
        staged
    }

    fn write_inline_record(
        &self,
        id: InlineId,
        object: &InlineObject,
        active: bool,
    ) -> Result<(), DocError> {
        let rec = self
            .inline_map()
            .insert_container(&id.key(), LoroMap::new())?;
        rec.insert(VERSION, 1)?;
        rec.insert(ACTIVE, active)?;
        object.write(&rec)
    }

    /// Shows a staged or deleted object. Does not commit.
    pub fn activate_inline(&self, id: InlineId) -> Result<(), DocError> {
        let rec = self
            .inline_record(id.host, id.mark)
            .ok_or(DocError::Malformed(id.host(), "no inline object record"))?;
        rec.insert(ACTIVE, true)?;
        Ok(())
    }

    /// Inserts an object at byte `at` of a flow paragraph, keeping its ID
    /// through undo and redo. Staging commits what was pending first.
    pub fn insert_inline(
        &self,
        node: NodeId,
        at: usize,
        object: &InlineObject,
    ) -> Result<InlineId, DocError> {
        let id = self.stage_inline(node, at, object)?;
        self.activate_inline(id)?;
        Ok(id)
    }

    /// Changes a live object's parameters. Each parameter is its own entry,
    /// so concurrent changes to different parameters merge. Does not commit.
    pub fn update_inline(&self, id: InlineId, patch: &InlinePatch) -> Result<(), DocError> {
        if self.inline_location(id).is_none() {
            return Err(DocError::Malformed(id.host(), "no live inline object"));
        }
        let current = self.inline_object(id)?;
        patch
            .apply(&current)
            .validate()
            .map_err(|_| DocError::Malformed(id.host(), "invalid inline object"))?;
        let rec = self
            .inline_record(id.host, id.mark)
            .ok_or(DocError::Malformed(id.host(), "no inline object record"))?;
        if let Some(align) = patch.align {
            rec.insert(ALIGN, align.as_str())?;
        }
        for (name, value) in &patch.params {
            let key = format!("{PARAM}{name}");
            match value {
                Some(v) => rec.insert(&key, v.as_str())?,
                None => rec.delete(&key)?,
            }
        }
        Ok(())
    }

    /// Deactivates the objects at these scalar indexes of `host`'s text and
    /// returns those it deactivated, which a deletion must keep. Does not
    /// commit.
    pub(crate) fn retire_objects(
        &self,
        host: TreeID,
        scalars: &[usize],
    ) -> Result<Vec<usize>, DocError> {
        let Some(text) = self.host_text(host) else {
            return Ok(Vec::new());
        };
        let mut kept = Vec::new();
        for &u in scalars {
            let Some(mark) = text.get_cursor(u, Side::Left).and_then(|c| c.id) else {
                continue;
            };
            if self.object_state(host, mark) == ObjectState::Visible
                && let Some(rec) = self.inline_record(host, mark)
            {
                rec.insert(ACTIVE, false)?;
                kept.push(u);
            }
        }
        Ok(kept)
    }

    /// The objects of a paragraph whose U+FFFC is in `bytes`, in order.
    pub fn inline_slice(
        &self,
        node: NodeId,
        bytes: Range<usize>,
    ) -> Result<Vec<InlineObject>, DocError> {
        Ok(self
            .inline_objects(node)?
            .into_iter()
            .filter(|e| bytes.contains(&e.at))
            .map(|e| e.object)
            .collect())
    }

    /// Gives the U+FFFC characters of a staged block's text new, active
    /// records for `objects`, in order, outside the undo history (the block
    /// itself is still invisible). Characters beyond `objects` stay hidden.
    pub fn stage_inline_copies(
        &self,
        staged: NodeId,
        objects: &[InlineObject],
    ) -> Result<(), DocError> {
        if objects.is_empty() {
            return Ok(());
        }
        let Some(text) = self.host_text(staged.node) else {
            return Ok(());
        };
        let positions: Vec<usize> = text
            .to_string()
            .chars()
            .enumerate()
            .filter(|&(_, c)| c == OBJECT)
            .map(|(u, _)| u)
            .collect();
        self.commit_part();
        self.doc.set_next_commit_origin(STAGE_ORIGIN);
        let written = (|| {
            for (u, object) in positions.into_iter().zip(objects) {
                let Some(mark) = text.get_cursor(u, Side::Left).and_then(|c| c.id) else {
                    continue;
                };
                self.write_inline_record(
                    InlineId {
                        host: staged.node,
                        mark,
                    },
                    object,
                    true,
                )?;
            }
            Ok(())
        })();
        self.doc.commit();
        written
    }

    /// Inserts copied text into a live paragraph, recreating the objects of
    /// its U+FFFC characters (in order) with new identity. Their markers are
    /// staged first, so undo and redo keep them. Outside a flow the objects
    /// are dropped and only the text goes in.
    pub fn insert_with_inline(
        &self,
        node: NodeId,
        at: usize,
        text: &str,
        objects: &[InlineObject],
    ) -> Result<(), DocError> {
        let block = self.block(node)?;
        if !text.contains(OBJECT) {
            block.text.insert(at, text)?;
            return Ok(());
        }
        if !self.in_flow(node) {
            let plain: String = text.chars().filter(|&c| c != OBJECT).collect();
            block.text.insert(at, &plain)?;
            return Ok(());
        }
        let pieces: Vec<&str> = text.split(OBJECT).collect();
        let count = pieces.len().saturating_sub(1);
        let shared = block.text.to_shared(at)?;
        let raw = block.text.loro().clone();
        self.commit_part();
        self.doc.set_next_commit_origin(STAGE_ORIGIN);
        let staged = (|| {
            raw.insert(shared, &OBJECT.to_string().repeat(count))?;
            let mut ids = Vec::with_capacity(count);
            for k in 0..count {
                let mark = raw
                    .get_cursor(shared + k, Side::Left)
                    .and_then(|c| c.id)
                    .ok_or_else(|| DocError::Store("the object has no identity".into()))?;
                let id = InlineId {
                    host: node.node,
                    mark,
                };
                let object = objects.get(k).cloned().unwrap_or_default();
                // A copy without a readable object keeps a hidden marker.
                if objects.get(k).is_some() {
                    self.write_inline_record(id, &object, false)?;
                }
                ids.push(id);
            }
            Ok::<_, DocError>(ids)
        })();
        self.doc.commit();
        let ids = staged?;
        // The markers sit at `shared..shared + count`; put each piece of
        // text before the marker that follows it.
        let mut pos = shared;
        for piece in &pieces {
            if !piece.is_empty() {
                raw.insert(pos, piece)?;
            }
            pos += piece.chars().count() + 1;
        }
        for (k, id) in ids.into_iter().enumerate() {
            if objects.get(k).is_some() {
                self.activate_inline(id)?;
            }
        }
        Ok(())
    }

    /// A persistent position for a `page-of` field's `target` parameter.
    pub fn field_target(&self, node: NodeId, at: usize) -> Result<String, DocError> {
        let block = self.block(node)?;
        let anchor = block.text.anchor(at, Affinity::After)?;
        Ok(format!("{node} {}", hex(&anchor.encode())))
    }

    /// Where a `target` parameter points now: its paragraph (following
    /// splits and joins) and byte offset. `None` when unreadable or gone.
    ///
    /// Text that a join, split or move across hosts copied keeps its lineage;
    /// when the targeted character is gone this follows it to the copy, the same
    /// bounded and ordered walk that carries a caret (`transfer_anchors`), so a
    /// cross-reference survives its target's paragraph being merged elsewhere.
    pub fn resolve_field_target(&self, raw: &str) -> Option<(NodeId, usize)> {
        use std::collections::{BTreeSet, VecDeque};

        use crate::text::Resolved;
        /// The most lineage edges one resolution visits.
        const MAX_HOPS: usize = 256;
        let (node, anchor) = raw.split_once(' ')?;
        let node = NodeId::parse(node)?;
        let anchor = Anchor::decode(&unhex(anchor)?)?;
        let mut tombstone = None;
        match self.locate(node, &anchor) {
            Some((at, Resolved::Live(offset))) => return Some((at, offset)),
            Some((at, Resolved::Tombstoned(offset))) => tombstone = Some((at, offset)),
            None => {}
        }
        let mut seen = BTreeSet::from([(node, anchor.encode())]);
        let mut queue = VecDeque::from([(node, anchor)]);
        let mut visited = 0usize;
        while let Some((from, mut current)) = queue.pop_front() {
            visited += 1;
            if visited > MAX_HOPS {
                break;
            }
            // Undo can recreate the copied span under new characters.
            if let Some(text) = self.text_of_any(from)
                && matches!(text.resolve_shared(&current), Ok(Resolved::Tombstoned(_)))
                && let Some(restored) = self.restored_transfer_anchor(from, &current)
            {
                current = restored;
            }
            if let Some((at, Resolved::Live(offset))) = self.locate(from, &current) {
                return Some((at, offset));
            }
            for next in self.transfer_anchors(from, &current) {
                if queue.len() < MAX_HOPS && seen.insert((next.0, next.1.encode())) {
                    queue.push_back(next);
                }
            }
        }
        tombstone
    }

    /// Reanchor portable fragment page-of targets after destination text exists.
    /// Each map entry supplies the destination node and copied text's byte shift.
    /// Targets outside the copy keep their persistent identity.
    pub fn remap_inline_targets(
        &self,
        nodes: &BTreeMap<NodeId, (NodeId, usize)>,
    ) -> Result<(), DocError> {
        let destinations: std::collections::BTreeSet<_> =
            nodes.values().map(|&(node, _)| node).collect();
        for node in destinations {
            for entry in self.inline_objects(node)? {
                if entry.object.kind != FIELD || entry.object.param("field") != Some("page-of") {
                    continue;
                }
                let Some(raw) = entry
                    .object
                    .param("target")
                    .and_then(|s| s.strip_prefix("copy:"))
                else {
                    continue;
                };
                let Some((source, at)) = raw.split_once(' ') else {
                    continue;
                };
                let Some(source) = NodeId::parse(source) else {
                    continue;
                };
                let Ok(at) = at.parse::<usize>() else {
                    continue;
                };
                let Some(&(target, shift)) = nodes.get(&source) else {
                    continue;
                };
                if let Ok(value) = self.field_target(target, at.saturating_add(shift)) {
                    let patch = InlinePatch {
                        align: None,
                        params: BTreeMap::from([("target".into(), Some(value))]),
                    };
                    self.update_inline(entry.id, &patch)?;
                }
            }
        }
        Ok(())
    }

    /// Orphaned markers and records, for diagnostics. Read-only.
    pub fn inline_audit(&self) -> InlineAudit {
        let mut audit = InlineAudit::default();
        let map = self.read_inline_map();
        let mut hosts: BTreeMap<TreeID, std::collections::BTreeSet<ID>> = BTreeMap::new();
        let tree = self.tree("content");
        for host in tree.nodes() {
            if let Some(flow) = self.flow(host)
                && !flow.objects.is_empty()
            {
                for m in flow
                    .objects
                    .iter()
                    .filter(|m| m.state == ObjectState::Orphan)
                {
                    let para = flow
                        .paras
                        .iter()
                        .find(|p| p.region.start.1 <= m.u && m.u <= p.region.end.1)
                        .map_or(NodeId::tree(host), |p| p.id);
                    *audit.orphan_markers.entry(para).or_default() += 1;
                }
                hosts.insert(host, flow.objects.iter().filter_map(|m| m.id).collect());
            }
        }
        let mut keys: Vec<String> = map
            .into_iter()
            .flat_map(|map| map.keys().map(|k| k.to_string()).collect::<Vec<_>>())
            .collect();
        keys.sort();
        for key in keys {
            let found = InlineId::parse(&key).is_some_and(|id| {
                hosts
                    .get(&id.host)
                    .is_some_and(|marks| marks.contains(&id.mark))
            });
            if !found {
                audit.orphan_records.push(key);
            }
        }
        audit
    }
}
