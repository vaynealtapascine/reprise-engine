//! The authored document (decisions 05, 06, 07, 08, 13, 14 and 15).
//!
//! Everything here is saved, undoable and collaborative, so it all lives in one
//! Loro document:
//!
//! - `content`: a tree of blocks. Node IDs are Loro tree IDs: opaque, never
//!   reused, and deleted nodes are kept as tombstones (07).
//! - `ranges`: named ranges, each a pair of anchors with a policy (10, 12).
//! - `relations`: typed relations between blocks and targets (13, 14).
//! - `styles`: named styles with inheritance (08).
//!
//! Layout and display are derived from this and never stored here (05).

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;

use loro::{Container, LoroDoc, LoroMap, LoroText, LoroTree, LoroValue, TreeID, ValueOrContainer};
use reprise_geom::Length;
use reprise_text::{Anchor, RangePolicy, Resolved, Text};
use serde::{Deserialize, Serialize};

pub use reprise_text as text;

/// A block's identity in the content tree.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(TreeID);

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl NodeId {
    pub fn parse(s: &str) -> Option<NodeId> {
        TreeID::try_from(s).ok().map(NodeId)
    }
}

impl Serialize for NodeId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for NodeId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        NodeId::parse(&s).ok_or_else(|| serde::de::Error::custom("bad node id"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockKind {
    /// Flows in the main text.
    Paragraph,
    /// Out of the flow; placed by a relation.
    Annotation,
}

impl BlockKind {
    fn as_str(self) -> &'static str {
        match self {
            BlockKind::Paragraph => "paragraph",
            BlockKind::Annotation => "annotation",
        }
    }

    fn parse(s: &str) -> Option<BlockKind> {
        match s {
            "paragraph" => Some(BlockKind::Paragraph),
            "annotation" => Some(BlockKind::Annotation),
            _ => None,
        }
    }
}

/// A length as authored: a literal or relative to the font size (17).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LengthExpr {
    Pt(Length),
    /// Thousandths of an em.
    Em(i32),
}

impl LengthExpr {
    fn resolve(self, em: Length) -> Length {
        match self {
            LengthExpr::Pt(l) => l,
            LengthExpr::Em(permille) => em.mul_ratio(permille, 1000),
        }
    }

    fn to_loro(self) -> LoroValue {
        match self {
            LengthExpr::Pt(l) => format!("pt:{}", l.0).into(),
            LengthExpr::Em(p) => format!("em:{p}").into(),
        }
    }

    fn from_loro(v: &str) -> Option<LengthExpr> {
        let (unit, n) = v.split_once(':')?;
        let n: i32 = n.parse().ok()?;
        match unit {
            "pt" => Some(LengthExpr::Pt(Length(n))),
            "em" => Some(LengthExpr::Em(n)),
            _ => None,
        }
    }
}

/// Style properties as authored, in a named style or as direct overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub parent: Option<String>,
    pub family: Option<String>,
    pub size: Option<LengthExpr>,
    pub line_height: Option<LengthExpr>,
}

impl Style {
    fn write(&self, map: &LoroMap) -> loro::LoroResult<()> {
        if let Some(p) = &self.parent {
            map.insert("parent", p.as_str())?;
        }
        if let Some(f) = &self.family {
            map.insert("family", f.as_str())?;
        }
        if let Some(s) = self.size {
            map.insert("size", s.to_loro())?;
        }
        if let Some(l) = self.line_height {
            map.insert("line-height", l.to_loro())?;
        }
        Ok(())
    }

    fn read(map: &LoroMap) -> Style {
        Style {
            parent: get_str(map, "parent"),
            family: get_str(map, "family"),
            size: get_str(map, "size").and_then(|v| LengthExpr::from_loro(&v)),
            line_height: get_str(map, "line-height").and_then(|v| LengthExpr::from_loro(&v)),
        }
    }
}

/// Used values after inheritance and overrides (08), with where each came from (39).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputedStyle {
    pub family: String,
    pub size: Length,
    pub line_height: Length,
    pub explain: BTreeMap<String, String>,
}

/// Engine defaults, the bottom of every style chain.
pub fn default_style() -> Style {
    Style {
        parent: None,
        family: Some("Source Serif Pro".into()),
        size: Some(LengthExpr::Pt(Length::from_pt(10))),
        line_height: Some(LengthExpr::Em(1200)),
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct RangeId(pub String);

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct RelationId(pub String);

/// What a range resolves to now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum RangeState {
    /// Both ends are on live characters.
    Valid { node: NodeId, bytes: Range<usize> },
    /// An end's character was deleted; the range kept its nearest position (15).
    Rebound { node: NodeId, bytes: Range<usize> },
    /// The range's content is gone.
    Missing { node: Option<NodeId> },
}

/// What a relation points at (13). The spike implements one layout query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "kebab-case")]
pub enum Target {
    /// The visual line that contains the start of a range.
    LineContaining { range: RangeId },
}

/// Relation types (14). These become registered schemas; the spike has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelationKind {
    /// The source block sits beside its target line and moves with it.
    Follow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub kind: RelationKind,
    pub source: NodeId,
    pub target: Target,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub id: NodeId,
    pub kind: BlockKind,
    pub style: Option<String>,
    pub overrides: Style,
    pub text: Text,
}

#[derive(Debug, thiserror::Error)]
pub enum DocError {
    #[error("no live node {0}")]
    NoNode(NodeId),
    #[error("node {0} is malformed: {1}")]
    Malformed(NodeId, &'static str),
    #[error("could not export the document: {0}")]
    Export(String),
    #[error(transparent)]
    Text(#[from] reprise_text::TextError),
    #[error(transparent)]
    Loro(#[from] loro::LoroError),
}

/// The document revision a result was computed from (28).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Revision(pub Vec<(u64, i32)>);

pub struct Document {
    doc: LoroDoc,
}

impl Document {
    /// `peer` identifies this replica. Fixtures pin it so IDs are reproducible.
    pub fn new(peer: u64) -> Result<Document, DocError> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer)?;
        doc.get_tree("content").enable_fractional_index(0);
        Ok(Document { doc })
    }

    pub fn loro(&self) -> &LoroDoc {
        &self.doc
    }

    /// A second replica of this document for another peer.
    pub fn fork(&self, peer: u64) -> Result<Document, DocError> {
        self.commit();
        let doc = self.doc.fork();
        doc.set_peer_id(peer)?;
        Ok(Document { doc })
    }

    /// Merges another replica's edits into this one (29).
    pub fn merge(&self, other: &Document) -> Result<(), DocError> {
        self.commit();
        other.commit();
        let updates = other
            .doc
            .export(loro::ExportMode::all_updates())
            .map_err(|e| DocError::Export(e.to_string()))?;
        self.doc.import(&updates)?;
        Ok(())
    }

    pub fn commit(&self) {
        self.doc.commit();
    }

    pub fn revision(&self) -> Revision {
        let mut ids: Vec<_> = self
            .doc
            .state_frontiers()
            .iter()
            .map(|id| (id.peer, id.counter))
            .collect();
        ids.sort();
        Revision(ids)
    }

    fn tree(&self) -> LoroTree {
        self.doc.get_tree("content")
    }

    fn map(&self, name: &str) -> LoroMap {
        self.doc.get_map(name)
    }

    fn new_id(&self, map: &LoroMap, prefix: &str) -> String {
        format!("{prefix}{}@{}", map.len(), self.doc.peer_id())
    }

    pub fn define_style(&self, name: &str, style: &Style) -> Result<(), DocError> {
        let map = self.map("styles").insert_container(name, LoroMap::new())?;
        style.write(&map)?;
        Ok(())
    }

    pub fn style(&self, name: &str) -> Option<Style> {
        match self.map("styles").get(name)? {
            ValueOrContainer::Container(Container::Map(m)) => Some(Style::read(&m)),
            _ => None,
        }
    }

    /// Appends a top-level block.
    pub fn append_block(
        &self,
        kind: BlockKind,
        style: &str,
        text: &str,
    ) -> Result<NodeId, DocError> {
        let tree = self.tree();
        let id = tree.create(None)?;
        let meta = tree.get_meta(id)?;
        meta.insert("kind", kind.as_str())?;
        meta.insert("style", style)?;
        let t = meta.insert_container("text", LoroText::new())?;
        t.insert_utf8(0, text)?;
        Ok(NodeId(id))
    }

    /// Top-level blocks in document order.
    pub fn blocks(&self) -> Vec<NodeId> {
        self.tree().roots().into_iter().map(NodeId).collect()
    }

    pub fn block(&self, id: NodeId) -> Result<Block, DocError> {
        let tree = self.tree();
        if !tree.contains(id.0) || tree.is_node_deleted(&id.0).unwrap_or(true) {
            return Err(DocError::NoNode(id));
        }
        let meta = tree.get_meta(id.0)?;
        let kind = get_str(&meta, "kind")
            .and_then(|k| BlockKind::parse(&k))
            .ok_or(DocError::Malformed(id, "kind"))?;
        let text = match meta.get("text") {
            Some(ValueOrContainer::Container(Container::Text(t))) => Text::new(t),
            _ => return Err(DocError::Malformed(id, "text")),
        };
        let overrides = match meta.get("overrides") {
            Some(ValueOrContainer::Container(Container::Map(m))) => Style::read(&m),
            _ => Style::default(),
        };
        Ok(Block {
            id,
            kind,
            style: get_str(&meta, "style"),
            overrides,
            text,
        })
    }

    pub fn set_overrides(&self, id: NodeId, style: &Style) -> Result<(), DocError> {
        let meta = self.tree().get_meta(id.0)?;
        let map = meta.insert_container("overrides", LoroMap::new())?;
        style.write(&map)?;
        Ok(())
    }

    pub fn delete_block(&self, id: NodeId) -> Result<(), DocError> {
        Ok(self.tree().delete(id.0)?)
    }

    /// Resolves a block's style: defaults, then its named style chain, then
    /// its direct overrides. Ems resolve against the font size at each step.
    pub fn computed_style(&self, id: NodeId) -> Result<ComputedStyle, DocError> {
        let block = self.block(id)?;
        let mut layers = vec![("default".to_string(), default_style())];
        let mut chain = Vec::new();
        let mut next = block.style.clone();
        while let Some(name) = next {
            if chain.iter().any(|(n, _)| n == &name) || chain.len() > 32 {
                break; // A cycle or a runaway chain; stop at what we have.
            }
            let Some(style) = self.style(&name) else {
                break;
            };
            next = style.parent.clone();
            chain.push((name, style));
        }
        layers.extend(
            chain
                .into_iter()
                .rev()
                .map(|(n, s)| (format!("style {n}"), s)),
        );
        layers.push(("direct".into(), block.overrides));

        let mut explain = BTreeMap::new();
        let mut family = String::new();
        let mut size = Length::ZERO;
        let mut line_height = None;
        for (layer, style) in &layers {
            if let Some(f) = &style.family {
                family = f.clone();
                explain.insert("family".into(), layer.clone());
            }
            if let Some(s) = style.size {
                size = s.resolve(size); // An em size is relative to the inherited size.
                explain.insert("size".into(), layer.clone());
            }
            if let Some(l) = style.line_height {
                line_height = Some(l);
                explain.insert("line-height".into(), layer.clone());
            }
        }
        // Line height resolves against the final font size, so an inherited
        // 1.2em follows a size change further down the chain.
        let line_height = line_height.map_or(size, |l| l.resolve(size));
        Ok(ComputedStyle {
            family,
            size,
            line_height,
            explain,
        })
    }

    /// Creates a named range over part of a block's text.
    pub fn add_range(
        &self,
        node: NodeId,
        bytes: Range<usize>,
        policy: RangePolicy,
    ) -> Result<RangeId, DocError> {
        let text = self.block(node)?.text;
        let start = text.anchor(bytes.start, policy.start)?;
        let end = text.anchor(bytes.end, policy.end)?;
        let ranges = self.map("ranges");
        let id = self.new_id(&ranges, "r");
        let map = ranges.insert_container(&id, LoroMap::new())?;
        map.insert("node", node.to_string())?;
        map.insert("start", start.encode())?;
        map.insert("end", end.encode())?;
        Ok(RangeId(id))
    }

    pub fn resolve_range(&self, id: &RangeId) -> RangeState {
        let Some(ValueOrContainer::Container(Container::Map(map))) = self.map("ranges").get(&id.0)
        else {
            return RangeState::Missing { node: None };
        };
        let node = get_str(&map, "node").and_then(|s| NodeId::parse(&s));
        let anchor = |key| match map.get(key) {
            Some(ValueOrContainer::Value(LoroValue::Binary(b))) => Anchor::decode(&b),
            _ => None,
        };
        let (Some(node), Some(start), Some(end)) = (node, anchor("start"), anchor("end")) else {
            return RangeState::Missing { node };
        };
        if self.block(node).is_err() {
            return RangeState::Missing { node: Some(node) };
        }
        let resolve = |a: &Anchor| reprise_text::resolve(&self.doc, a).ok();
        match (resolve(&start), resolve(&end)) {
            (Some(s), Some(e)) if s.offset() < e.offset() => {
                let bytes = s.offset()..e.offset();
                if matches!((s, e), (Resolved::Live(_), Resolved::Live(_))) {
                    RangeState::Valid { node, bytes }
                } else {
                    RangeState::Rebound { node, bytes }
                }
            }
            _ => RangeState::Missing { node: Some(node) },
        }
    }

    pub fn add_relation(&self, relation: &Relation) -> Result<RelationId, DocError> {
        let relations = self.map("relations");
        let id = self.new_id(&relations, "rel");
        let json = serde_json::to_string(relation).expect("relations serialize");
        relations.insert(&id, json)?;
        Ok(RelationId(id))
    }

    /// Relations in ID order. Unreadable entries are skipped, not fatal.
    pub fn relations(&self) -> Vec<(RelationId, Relation)> {
        let map = self.map("relations");
        let mut out: Vec<_> = map
            .keys()
            .filter_map(|k| {
                let json = get_str(&map, &k)?;
                Some((RelationId(k.to_string()), serde_json::from_str(&json).ok()?))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

fn get_str(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key)? {
        ValueOrContainer::Value(LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_inherit_and_override() {
        let doc = Document::new(1).unwrap();
        doc.define_style(
            "body",
            &Style {
                size: Some(LengthExpr::Pt(Length::from_pt(12))),
                ..Default::default()
            },
        )
        .unwrap();
        doc.define_style(
            "note",
            &Style {
                parent: Some("body".into()),
                size: Some(LengthExpr::Em(750)),
                ..Default::default()
            },
        )
        .unwrap();
        let note = doc
            .append_block(BlockKind::Annotation, "note", "n")
            .unwrap();
        let s = doc.computed_style(note).unwrap();
        assert_eq!(s.size, Length::from_pt(9));
        assert_eq!(s.line_height, Length::from_pt(9).mul_ratio(1200, 1000));
        assert_eq!(s.explain["size"], "style note");
        assert_eq!(s.explain["family"], "default");

        doc.set_overrides(
            note,
            &Style {
                size: Some(LengthExpr::Pt(Length::from_pt(8))),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.computed_style(note).unwrap().size, Length::from_pt(8));
    }

    #[test]
    fn ranges_track_edits_and_report_state() {
        let doc = Document::new(1).unwrap();
        let p = doc
            .append_block(BlockKind::Paragraph, "body", "one two three")
            .unwrap();
        let r = doc.add_range(p, 4..7, RangePolicy::FIXED).unwrap();
        let text = doc.block(p).unwrap().text;
        text.insert(0, "zero ").unwrap();
        assert_eq!(
            doc.resolve_range(&r),
            RangeState::Valid {
                node: p,
                bytes: 9..12
            }
        );
        text.delete(11..12).unwrap(); // the last character of "two"
        assert_eq!(
            doc.resolve_range(&r),
            RangeState::Rebound {
                node: p,
                bytes: 9..11
            }
        );
        text.delete(9..11).unwrap();
        assert_eq!(doc.resolve_range(&r), RangeState::Missing { node: Some(p) });
    }

    #[test]
    fn deleted_blocks_are_missing_not_reused() {
        let doc = Document::new(1).unwrap();
        let a = doc.append_block(BlockKind::Paragraph, "body", "a").unwrap();
        let r = doc.add_range(a, 0..1, RangePolicy::FIXED).unwrap();
        doc.delete_block(a).unwrap();
        let b = doc.append_block(BlockKind::Paragraph, "body", "b").unwrap();
        assert_ne!(a, b);
        assert!(doc.block(a).is_err());
        assert_eq!(doc.resolve_range(&r), RangeState::Missing { node: Some(a) });
    }

    #[test]
    fn relations_round_trip() {
        let doc = Document::new(1).unwrap();
        let p = doc
            .append_block(BlockKind::Paragraph, "body", "text")
            .unwrap();
        let n = doc
            .append_block(BlockKind::Annotation, "note", "note")
            .unwrap();
        let r = doc.add_range(p, 0..4, RangePolicy::FIXED).unwrap();
        let rel = Relation {
            kind: RelationKind::Follow,
            source: n,
            target: Target::LineContaining { range: r },
        };
        let id = doc.add_relation(&rel).unwrap();
        assert_eq!(doc.relations(), vec![(id, rel)]);
    }
}
