//! The authored document (decisions 05, 06, 07, 08, 13, 14 and 15).
//!
//! Everything here is saved, undoable and collaborative, so it all lives in one
//! Loro document:
//!
//! - `content`: a tree of blocks (06).
//! - `ranges`: persistent ranges, each a pair of anchors with a policy (10, 12).
//! - `relations`: relations between nodes and targets (13, 14), stored as JSON.
//! - `styles`: named styles with inheritance (08).
//!
//! Blocks, ranges and relations are all nodes of Loro trees, so their IDs are
//! Loro operation IDs: opaque, never reused even after undo, and a deleted
//! item stays behind as a tombstone (07).
//!
//! Layout and display are derived from this and never stored here (05).

use std::fmt;
use std::ops::Range;

use loro::{Container, LoroDoc, LoroMap, LoroText, LoroTree, LoroValue, TreeID, ValueOrContainer};
use reprise_text::{Anchor, Empty, RangePolicy, Resolved, Text};
use serde::{Deserialize, Serialize};

mod changes;
pub mod codes;
pub mod context;
mod edit;
pub mod expr;
pub mod fragment;
pub mod function;
mod history;
#[cfg(any(test, feature = "hostile-peer"))]
pub mod hostile;
#[cfg(test)]
mod hostile_tests;
pub mod image;
pub mod invariants;
mod lifecycle;
mod page;
mod persist;
mod position;
mod ranges;
pub mod reading;
mod region_schema;
pub mod relation;
#[cfg(test)]
mod relation_tests;
mod resolve;
mod structure;
mod style;
pub mod sync;
#[cfg(test)]
mod sync_tests;
mod sync_text;
mod table;
mod table_grid;

pub use changes::ChangeReport;
pub use context::ResolutionContext;
pub use edit::{DEFAULT_UNDO_STEPS, NewBlock, UndoStack};
pub use expr::{ComputedLength, Dependency, Expr};
pub use function::FunctionRegistry;
pub use history::{
    DocumentAt, HistoryCache, MAX_SNAPSHOT_TEXT, SnapshotContent, SnapshotState, VersionError,
};
pub use page::{
    Basis, Dim, FrameRole, FrameTemplate, FrameTransform, MAIN_FLOW, Medium, PageTemplate,
    Rotation, Spiral, StoredTemplate, TemplateChoice, WritingMode,
};
pub use persist::{
    MAX_PERSIST_BYTES, MAX_PERSIST_EXPANDED_BYTES, MAX_PERSIST_OPS, PersistenceMode,
};
pub use position::Fallback;
pub use region_schema::{FloatSide, NotePlacement};
pub use relation::{
    LayoutQuery, Param, ParamKind, Relation, RelationSchema, SchemaError, SchemaId, SchemaRegistry,
    SnapshotOf, SnapshotRef, StructuralQuery, Target, TargetClass,
};
pub use reprise_text as text;
pub use resolve::{Binding, Cause, Found, Gone, Outcome, ResolvedRelation, ResolvedTarget};
pub use structure::{MAX_SUCCESSION_DEPTH, Succession};
pub use style::{
    Authored, Computed, ComputedStyle, LengthExpr, Property, Specified, StageExplanation, Style,
    StyleResolution, default_style,
};
pub use table::{CellInfo, Column, ColumnWidth, RowInfo, TableColumns, TableRole};
pub use table_grid::{
    CellInput, GridCell, GridIssue, GridIssueKind, GridRow, MAX_GRID_COLUMNS, MAX_GRID_ROWS,
    RowInput, TableGrid, resolve_grid,
};

macro_rules! tree_ids {
    ($($(#[$m:meta])* $name:ident),*) => {$(
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(TreeID);

        impl $name {
            /// Parses the form produced by `Display`, such as `12@1`.
            pub fn parse(s: &str) -> Option<$name> {
                TreeID::try_from(s).ok().map($name)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                $name::parse(&s)
                    .ok_or_else(|| serde::de::Error::custom(concat!("bad ", stringify!($name))))
            }
        }
    )*};
}

tree_ids!(
    /// A block's identity in the content tree.
    NodeId,
    /// A persistent range's identity.
    RangeId,
    /// A relation's identity.
    RelationId
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockKind {
    /// Flows in the main text.
    Paragraph,
    /// Out of the flow; placed by a relation.
    Annotation,
    /// An unbreakable image box; its text container holds its alt text.
    Image,
}

impl BlockKind {
    fn as_str(self) -> &'static str {
        match self {
            BlockKind::Paragraph => "paragraph",
            BlockKind::Annotation => "annotation",
            BlockKind::Image => "image",
        }
    }

    fn parse(s: &str) -> Option<BlockKind> {
        match s {
            "paragraph" => Some(BlockKind::Paragraph),
            "annotation" => Some(BlockKind::Annotation),
            "image" => Some(BlockKind::Image),
            _ => None,
        }
    }
}

/// What a range resolves to now (15).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum RangeState {
    /// Both ends are on live characters.
    Valid { node: NodeId, bytes: Range<usize> },
    /// An end's character was deleted; the range kept its nearest position.
    Rebound { node: NodeId, bytes: Range<usize> },
    /// The range's content is gone, or so is its block.
    Missing { node: Option<NodeId> },
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
    Schema(#[from] SchemaError),
    #[error(transparent)]
    Text(#[from] reprise_text::TextError),
    #[error("the document store refused the change: {0}")]
    Store(String),
    #[error("index {index} is past the end of {len} children")]
    BadIndex { index: usize, len: usize },
}

impl From<loro::LoroError> for DocError {
    fn from(e: loro::LoroError) -> Self {
        DocError::Store(e.to_string())
    }
}

/// The document revision a result was computed from (28): the frontier of
/// the operation history, sorted.
///
/// Equal revisions mean equal documents. The derived ordering is only there
/// to keep revisions sortable; it is not causal order. Deciding whether one
/// revision is newer than another needs the document's history.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Revision(pub Vec<(u64, i32)>);

pub struct Document {
    doc: LoroDoc,
    /// A lazily built replica that sync checks out to read text lengths at
    /// past versions (see `sync`). Checking out `doc` itself would clear
    /// its undo history.
    shadow: std::sync::Mutex<Option<LoroDoc>>,
}

impl Document {
    fn wrap(doc: LoroDoc) -> Document {
        Document {
            doc,
            shadow: std::sync::Mutex::new(None),
        }
    }

    /// `peer` identifies this replica. Fixtures pin it so IDs are reproducible.
    pub fn new(peer: u64) -> Result<Document, DocError> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer)?;
        doc.get_tree("content").enable_fractional_index(0);
        Ok(Document::wrap(doc))
    }

    /// A second replica of this document for another peer.
    pub fn fork(&self, peer: u64) -> Result<Document, DocError> {
        self.commit();
        let doc = self.doc.fork();
        doc.set_peer_id(peer)?;
        Ok(Document::wrap(doc))
    }

    /// Merges another replica's edits into this one (29).
    pub fn merge(&self, other: &Document) -> Result<(), DocError> {
        self.commit();
        other.commit();
        // Only what this replica lacks: both documents are this process's own,
        // so the binary update encoding is trusted here (see `sync`).
        let updates = other
            .doc
            .export(loro::ExportMode::updates(&self.doc.oplog_vv()))
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

    fn tree(&self, name: &str) -> LoroTree {
        self.doc.get_tree(name)
    }

    /// Whether a node of `tree` is alive: not tombstoned, and not in the
    /// soft-deleted subtree (see [`lifecycle`]).
    fn live(&self, tree: &LoroTree, id: TreeID) -> bool {
        tree.contains(id)
            && !tree.is_node_deleted(&id).unwrap_or(true)
            && !self.deleted_in_tree(tree, id)
    }

    pub fn define_style(&self, name: &str, style: &Style) -> Result<(), DocError> {
        let map = self
            .doc
            .get_map("styles")
            .insert_container(name, LoroMap::new())?;
        style.write(&map)?;
        Ok(())
    }

    pub fn style(&self, name: &str) -> Option<Style> {
        match self.doc.get_map("styles").get(name)? {
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
        let tree = self.tree("content");
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
        self.children(None)
    }

    pub fn block(&self, id: NodeId) -> Result<Block, DocError> {
        let tree = self.tree("content");
        if !self.live(&tree, id.0) {
            return Err(DocError::NoNode(id));
        }
        let meta = tree.get_meta(id.0)?;
        let kind = get_str(&meta, "kind")
            .and_then(|k| BlockKind::parse(&k))
            .ok_or(DocError::Malformed(id, "kind"))?;
        let text = match meta.get("text") {
            Some(ValueOrContainer::Container(Container::Text(t))) => Text::from_loro(t),
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
        let meta = self.tree("content").get_meta(id.0)?;
        let map = meta.insert_container("overrides", LoroMap::new())?;
        style.write(&map)?;
        Ok(())
    }

    /// Flags a block as deleted in place, including its subtree (07). Undo
    /// restores the previous flag without changing identity or position.
    /// Relation deletion policies are evaluated from this tombstone at query
    /// time. Does not commit; deletion belongs to the caller's atomic step.
    pub fn delete_block(&self, id: NodeId) -> Result<(), DocError> {
        self.soft_delete_block(id)
    }

    /// Resolves a block's style in the default context: defaults, then its
    /// named style chain, then its direct overrides. Ems resolve against the
    /// font size at each step. Nothing geometric is known, so every frame,
    /// page and medium reference is unresolved (18); styles that use only `pt`
    /// and `em` resolve exactly as in any other context.
    pub fn computed_style(&self, id: NodeId) -> Result<ComputedStyle, DocError> {
        self.computed_style_in(id, &ResolutionContext::default())
    }

    /// Resolves a block's style against a context, with the built-in functions.
    pub fn computed_style_in(
        &self,
        id: NodeId,
        context: &ResolutionContext,
    ) -> Result<ComputedStyle, DocError> {
        self.computed_style_with(id, context, &FunctionRegistry::builtin())
    }

    /// Resolves a block's style against a context, with the engine's functions.
    pub fn computed_style_with(
        &self,
        id: NodeId,
        context: &ResolutionContext,
        functions: &FunctionRegistry,
    ) -> Result<ComputedStyle, DocError> {
        Ok(self.resolve_style(id, context, functions)?.style)
    }

    /// Like [`Document::computed_style_with`], with all four stages explained.
    pub fn resolve_style(
        &self,
        id: NodeId,
        context: &ResolutionContext,
        functions: &FunctionRegistry,
    ) -> Result<StyleResolution, DocError> {
        Ok(self
            .specified_style(id)?
            .compute(functions)
            .used(context, functions))
    }

    /// Stage 1 (08): the layers of a block's style, as authored.
    pub fn specified_style(&self, id: NodeId) -> Result<Specified, DocError> {
        Ok(style::specify(self, &self.block(id)?))
    }

    /// Creates a persistent range over part of a block's text.
    pub fn add_range(
        &self,
        node: NodeId,
        bytes: Range<usize>,
        policy: RangePolicy,
    ) -> Result<RangeId, DocError> {
        if bytes.start > bytes.end {
            return Err(reprise_text::TextError::BadRange(bytes).into());
        }
        let text = self.block(node)?.text;
        let start = text.anchor(bytes.start, policy.start)?;
        let end = text.anchor(bytes.end, policy.end)?;
        let tree = self.tree("ranges");
        let id = tree.create(None)?;
        let meta = tree.get_meta(id)?;
        meta.insert("node", node.to_string())?;
        meta.insert("start", start.encode())?;
        meta.insert("end", end.encode())?;
        ranges::write_policy(&meta, policy)?;
        Ok(RangeId(id))
    }

    /// Where a range is now. A range whose ends have crossed (possible after
    /// concurrent edits) is treated as empty at its start.
    pub fn resolve_range(&self, id: RangeId) -> RangeState {
        let tree = self.tree("ranges");
        if !self.live(&tree, id.0) {
            return RangeState::Missing { node: None };
        }
        let Ok(meta) = tree.get_meta(id.0) else {
            return RangeState::Missing { node: None };
        };
        let node = get_str(&meta, "node").and_then(|s| NodeId::parse(&s));
        let anchor = |key| match meta.get(key) {
            Some(ValueOrContainer::Value(LoroValue::Binary(b))) => Anchor::decode(&b),
            _ => None,
        };
        let keep_empty = match self.range_policy(id) {
            Ok(Some(policy)) => policy.empty == Empty::Keep,
            Ok(None) => get_str(&meta, "empty").as_deref() == Some("keep"),
            Err(_) => return RangeState::Missing { node },
        };
        let (Some(node), Some(start), Some(end)) = (node, anchor("start"), anchor("end")) else {
            return RangeState::Missing { node };
        };
        let Ok(block) = self.block(node) else {
            return RangeState::Missing { node: Some(node) };
        };
        let (Ok(s), Ok(e)) = (block.text.resolve(&start), block.text.resolve(&end)) else {
            return RangeState::Missing { node: Some(node) };
        };
        let bytes = s.offset()..e.offset().max(s.offset());
        if bytes.is_empty() && !keep_empty {
            return RangeState::Missing { node: Some(node) };
        }
        if matches!((s, e), (Resolved::Live(_), Resolved::Live(_))) {
            RangeState::Valid { node, bytes }
        } else {
            RangeState::Rebound { node, bytes }
        }
    }

    /// Adds a relation after checking it against its schema.
    pub fn add_relation(
        &self,
        schemas: &SchemaRegistry,
        relation: &Relation,
    ) -> Result<RelationId, DocError> {
        schemas.validate(relation)?;
        let tree = self.tree("relations");
        let id = tree.create(None)?;
        let json = serde_json::to_string(relation).expect("relations serialize");
        tree.get_meta(id)?.insert("json", json)?;
        Ok(RelationId(id))
    }

    /// Soft-deletes a relation by flag, retaining its ID through undo (07).
    pub fn delete_relation(&self, id: RelationId) -> Result<(), DocError> {
        self.soft_delete_relation(id)
    }

    /// Live relations in ID order. Each is `Err` with its stored text when it
    /// can't be read, for example because a newer engine wrote it; it is kept
    /// in the document either way (34).
    pub fn relations(&self) -> Vec<(RelationId, Result<Relation, String>)> {
        let tree = self.tree("relations");
        let mut ids: Vec<TreeID> = tree
            .nodes()
            .into_iter()
            .filter(|&id| self.live(&tree, id))
            .collect();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let json = tree
                    .get_meta(id)
                    .ok()
                    .and_then(|m| get_str(&m, "json"))
                    .unwrap_or_default();
                let parsed = serde_json::from_str(&json).map_err(|_| json);
                (RelationId(id), parsed)
            })
            .collect()
    }
}

fn get_bool(map: &LoroMap, key: &str) -> Option<bool> {
    match map.get(key)? {
        ValueOrContainer::Value(LoroValue::Bool(b)) => Some(b),
        _ => None,
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
    use crate::relation::builtin::FOLLOW;
    use reprise_geom::Length;

    fn follow(owner: NodeId, range: RangeId) -> Relation {
        Relation::new(FOLLOW).owned_by(owner).target(
            "line",
            Target::Layout(LayoutQuery::LineContaining { range }),
        )
    }

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
            doc.resolve_range(r),
            RangeState::Valid {
                node: p,
                bytes: 9..12
            }
        );
        text.delete(11..12).unwrap(); // the last character of "two"
        assert_eq!(
            doc.resolve_range(r),
            RangeState::Rebound {
                node: p,
                bytes: 9..11
            }
        );
        text.delete(9..11).unwrap();
        assert_eq!(doc.resolve_range(r), RangeState::Missing { node: Some(p) });
    }

    #[test]
    fn point_ranges_survive_being_emptied() {
        let doc = Document::new(1).unwrap();
        let p = doc
            .append_block(BlockKind::Paragraph, "body", "see note")
            .unwrap();
        let marker = doc.add_range(p, 3..3, RangePolicy::POINT).unwrap();
        assert_eq!(
            doc.resolve_range(marker),
            RangeState::Valid {
                node: p,
                bytes: 3..3
            }
        );
        let fixed = doc.add_range(p, 0..3, RangePolicy::FIXED).unwrap();
        doc.block(p).unwrap().text.delete(0..3).unwrap();
        assert_eq!(
            doc.resolve_range(fixed),
            RangeState::Missing { node: Some(p) }
        );
        assert_eq!(
            doc.resolve_range(marker),
            RangeState::Rebound {
                node: p,
                bytes: 0..0
            }
        );
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
        assert_eq!(doc.resolve_range(r), RangeState::Missing { node: Some(a) });
    }

    #[test]
    fn relations_round_trip_and_ids_are_never_reused() {
        let doc = Document::new(1).unwrap();
        let schemas = SchemaRegistry::builtin();
        let p = doc
            .append_block(BlockKind::Paragraph, "body", "text")
            .unwrap();
        let n = doc
            .append_block(BlockKind::Annotation, "note", "note")
            .unwrap();
        let r = doc.add_range(p, 0..4, RangePolicy::FIXED).unwrap();
        let rel = follow(n, r).param("offset", Param::Length(LengthExpr::Em(500)));
        let id = doc.add_relation(&schemas, &rel).unwrap();
        assert_eq!(doc.relations(), vec![(id, Ok(rel.clone()))]);

        doc.delete_relation(id).unwrap();
        assert!(doc.relations().is_empty());
        let again = doc.add_relation(&schemas, &rel).unwrap();
        assert_ne!(
            again, id,
            "a deleted relation's ID is never handed out again"
        );
        assert_eq!(RelationId::parse(&again.to_string()), Some(again));
    }

    #[test]
    fn relations_are_checked_against_their_schema() {
        let doc = Document::new(1).unwrap();
        let schemas = SchemaRegistry::builtin();
        let p = doc
            .append_block(BlockKind::Paragraph, "body", "text")
            .unwrap();
        let r = doc.add_range(p, 0..4, RangePolicy::FIXED).unwrap();
        let line = Target::Layout(LayoutQuery::LineContaining { range: r });
        let err = |rel: Relation| match doc.add_relation(&schemas, &rel) {
            Err(DocError::Schema(e)) => e,
            other => panic!("expected a schema error, got {other:?}"),
        };

        let unowned = Relation::new(FOLLOW).target("line", line.clone());
        assert!(matches!(err(unowned), SchemaError::MissingOwner { .. }));
        let two = follow(p, r).target("line", line.clone());
        assert!(matches!(
            err(two),
            SchemaError::Cardinality { count: 2, .. }
        ));
        let wrong_class = Relation::new(FOLLOW)
            .owned_by(p)
            .target("line", Target::Range(r));
        assert!(matches!(err(wrong_class), SchemaError::TargetClass { .. }));
        let stray_role = follow(p, r).target("other", line);
        assert!(matches!(err(stray_role), SchemaError::UnknownRole { .. }));
        let bad_param = follow(p, r).param("offset", Param::Bool(true));
        assert!(matches!(err(bad_param), SchemaError::ParamKind { .. }));
        let unknown = Relation::new(SchemaId::new("someone.else")).owned_by(p);
        assert!(matches!(err(unknown), SchemaError::Unknown(_)));
        assert!(doc.relations().is_empty(), "nothing invalid was stored");
    }

    #[test]
    fn unreadable_relations_are_kept_and_reported() {
        let doc = Document::new(1).unwrap();
        let tree = doc.tree("relations");
        let id = tree.create(None).unwrap();
        tree.get_meta(id)
            .unwrap()
            .insert("json", r#"{"schema":"future.kind","shape":"unknown"}"#)
            .unwrap();
        let relations = doc.relations();
        assert_eq!(relations.len(), 1);
        assert!(
            relations[0]
                .1
                .as_ref()
                .is_err_and(|raw| raw.contains("future.kind"))
        );
    }
}
