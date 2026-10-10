//! Relations (decisions 13, 14 and 15): what a relation targets, and the
//! registered schemas that say what each relation type means.
//!
//! A [`Relation`] is authored data: a schema name, an optional owner, targets
//! grouped by role, and typed parameters. A [`RelationSchema`] declares which
//! roles and parameters a type has, who owns it, and what deleting or copying
//! its targets does to it. Schemas live in a [`SchemaRegistry`], which is
//! engine configuration like the shaping adapter: the built-in types are
//! registered by [`SchemaRegistry::builtin`], and extensions register more.
//!
//! What a relation *does* to layout is not part of the schema. Layout looks
//! the schema up by [`SchemaId`] and applies its own behaviour for it; a
//! relation whose schema layout doesn't know is kept, reported and not
//! applied (34, 37).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{BlockKind, LengthExpr, NodeId, RangeId, RelationId, Revision};

/// A registered relation type's name, namespaced by its owner:
/// `reprise.follow` for built-ins, `<extension>.<name>` for extensions.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaId(Cow<'static, str>);

impl SchemaId {
    pub const fn new(id: &'static str) -> SchemaId {
        SchemaId(Cow::Borrowed(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for SchemaId {
    fn from(s: String) -> Self {
        SchemaId(Cow::Owned(s))
    }
}

impl fmt::Debug for SchemaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for SchemaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The classes of target from decision 13.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetClass {
    /// A node by ID.
    Node,
    /// A persistent range.
    Range,
    /// A query over the content tree, such as "the next stanza".
    Structural,
    /// A query over layout results, re-evaluated after every reflow.
    Layout,
    /// An explicit reference to an earlier layout snapshot.
    Snapshot,
}

/// What a relation points at (13).
///
/// Relations are stored as JSON, so the serialised form is part of the file
/// format (34): a variant's name and fields never change once released. A
/// relation with a variant this engine doesn't know fails to parse, and is
/// reported as `relation.unreadable` and kept, not dropped.
///
/// The enum is non-exhaustive so adding variants is not a breaking change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Target {
    Node(NodeId),
    Range(RangeId),
    /// A query over the content tree, answered by the document alone.
    Structural(StructuralQuery),
    /// A query over layout results, answered by layout after every reflow.
    Layout(LayoutQuery),
    /// Content as it was at a recorded document version.
    Snapshot(SnapshotRef),
}

impl Target {
    pub fn class(&self) -> TargetClass {
        match self {
            Target::Node(_) => TargetClass::Node,
            Target::Range(_) => TargetClass::Range,
            Target::Structural(_) => TargetClass::Structural,
            Target::Layout(_) => TargetClass::Layout,
            Target::Snapshot(_) => TargetClass::Snapshot,
        }
    }

    /// What this target reads, for incremental evaluation (27) and for
    /// "everything touching this node" queries (16). Sorted, without
    /// duplicates.
    pub fn dependencies(&self) -> Vec<Dependency> {
        let mut deps = match self {
            Target::Node(n) => vec![Dependency::Node(*n)],
            Target::Range(r) => vec![Dependency::Range(*r)],
            Target::Structural(q) => {
                let mut deps = vec![Dependency::Tree];
                deps.extend(q.anchor().map(Dependency::Node));
                deps
            }
            Target::Layout(q) => match q.anchor() {
                QueryAnchor::Range(r) => vec![Dependency::Range(r), Dependency::LayoutOfRange(r)],
                QueryAnchor::Node(n) => vec![Dependency::Node(n), Dependency::LayoutOfNode(n)],
            },
            Target::Snapshot(s) => vec![
                Dependency::History(s.version.clone()),
                match s.of {
                    SnapshotOf::Node(n) => Dependency::Node(n),
                    SnapshotOf::Range(r) => Dependency::Range(r),
                },
            ],
        };
        deps.sort();
        deps.dedup();
        deps
    }

    /// The IDs this target is made of, as copying sees them (35).
    fn copy_anchors(&self) -> CopyAnchors {
        let mut anchors = CopyAnchors::default();
        match self {
            Target::Node(n) => anchors.nodes.push(*n),
            Target::Range(r) => anchors.ranges.push(*r),
            Target::Structural(q) => match q.anchor() {
                Some(n) => anchors.nodes.push(n),
                // Relative to the document root, which is never copied.
                None => anchors.external = true,
            },
            Target::Layout(q) => match q.anchor() {
                QueryAnchor::Range(r) => anchors.ranges.push(r),
                QueryAnchor::Node(n) => anchors.nodes.push(n),
            },
            // A version of the source document's history. It can't be
            // remapped: the copy didn't exist then.
            Target::Snapshot(_) => anchors.external = true,
        }
        anchors
    }

    /// This target with copied IDs replaced by their copies. IDs not in the
    /// map are left alone, so a target pointing outside the copy still does.
    pub fn remapped(&self, map: &IdMap) -> Target {
        match self {
            Target::Node(n) => Target::Node(map.node(*n)),
            Target::Range(r) => Target::Range(map.range(*r)),
            Target::Structural(q) => Target::Structural(q.remapped(map)),
            Target::Layout(q) => Target::Layout(q.remapped(map)),
            Target::Snapshot(s) => Target::Snapshot(s.clone()),
        }
    }
}

/// Something a target reads (27). Evaluation caches key on these, and the
/// reverse-dependency queries of 16 and 39 are answered from them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Dependency {
    /// A node's existence, kind, text and succession links (15).
    Node(NodeId),
    /// A range's anchors and the text they sit in.
    Range(RangeId),
    /// The shape of the content tree: parents and the order of children.
    /// Deliberately coarse; narrower variants can be added.
    Tree,
    /// The layout of the block holding a range.
    LayoutOfRange(RangeId),
    /// The layout of a block.
    LayoutOfNode(NodeId),
    /// The document as it was at a version. Never changes while the version
    /// stays available.
    History(Revision),
}

/// Where a structural or layout query starts, for the schemas that need to
/// know what it is anchored on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryAnchor {
    Range(RangeId),
    Node(NodeId),
}

/// A query over the content tree (13), relative to a node.
///
/// The tree is a list of blocks today, but every query is defined over
/// parents and children, so stanzas holding lines and notes holding notes
/// need no new queries. `of: None` and `parent: None` mean the document root,
/// whose children are the top-level blocks.
///
/// Matches come in document order. Every query says what zero and several
/// matches mean:
///
/// -   **Zero:** the target is `Missing`, reported as `relation.no-match`. If
///     the anchor node is *deleted* that is a different thing (see
///     [`OnTargetDeleted`]) and is reported as `relation.missing-target`.
/// -   **Several:** only [`StructuralQuery::Children`] can match several
///     blocks. In a role that takes one target, several matches are
///     `Ambiguous`: no match is preferred (15).
///
/// `kind` narrows the candidates: "the next *annotation*" skips paragraphs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum StructuralQuery {
    /// The nearest following sibling of `from`. Zero matches after the last.
    NextSibling {
        from: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<BlockKind>,
    },
    /// The nearest preceding sibling of `from`. Zero matches before the first.
    PreviousSibling {
        from: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<BlockKind>,
    },
    /// The `index`th child of `of` (0-based), counting from the end when
    /// `from_end` is set. Zero matches when there are not enough children.
    NthChild {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        of: Option<NodeId>,
        index: u32,
        #[serde(default, skip_serializing_if = "is_false")]
        from_end: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<BlockKind>,
    },
    /// The first child of `of`.
    FirstChild {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        of: Option<NodeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<BlockKind>,
    },
    /// The last child of `of`.
    LastChild {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        of: Option<NodeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<BlockKind>,
    },
    /// The parent of `of`. Zero matches for a top-level block: the document
    /// root is not a node.
    Parent { of: NodeId },
    /// Every child of `of`. The one query that can match several blocks.
    Children {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        of: Option<NodeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<BlockKind>,
    },
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl StructuralQuery {
    /// The node the query is relative to, or `None` for the document root.
    pub fn anchor(&self) -> Option<NodeId> {
        match self {
            StructuralQuery::NextSibling { from, .. }
            | StructuralQuery::PreviousSibling { from, .. } => Some(*from),
            StructuralQuery::Parent { of } => Some(*of),
            StructuralQuery::NthChild { of, .. }
            | StructuralQuery::FirstChild { of, .. }
            | StructuralQuery::LastChild { of, .. }
            | StructuralQuery::Children { of, .. } => *of,
        }
    }

    /// The same query relative to another node.
    pub fn with_anchor(&self, anchor: NodeId) -> StructuralQuery {
        let mut q = self.clone();
        match &mut q {
            StructuralQuery::NextSibling { from, .. }
            | StructuralQuery::PreviousSibling { from, .. } => *from = anchor,
            StructuralQuery::Parent { of } => *of = anchor,
            StructuralQuery::NthChild { of, .. }
            | StructuralQuery::FirstChild { of, .. }
            | StructuralQuery::LastChild { of, .. }
            | StructuralQuery::Children { of, .. } => *of = Some(anchor),
        }
        q
    }

    /// Whether the answer is a list. Such a query yields a list even when it
    /// matches one block, so the shape of the answer depends only on the query.
    pub fn is_multi(&self) -> bool {
        matches!(self, StructuralQuery::Children { .. })
    }

    fn remapped(&self, map: &IdMap) -> StructuralQuery {
        match self.anchor() {
            Some(n) => self.with_anchor(map.node(n)),
            None => self.clone(),
        }
    }
}

/// A query over layout results (13). Each query says what zero and several
/// matches mean.
///
/// Queries anchored on a range use the line holding the *start* of the range.
/// All of them are zero matches when the anchor is missing, or when its block
/// wasn't laid out. A query the answer of which can have several lines is
/// `Ambiguous` in a role that takes one target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum LayoutQuery {
    /// The visual line containing the start of a range. Never several
    /// matches; zero when the range is missing or its block wasn't laid out.
    LineContaining { range: RangeId },
    /// The line before the one containing the start of a range, in the same
    /// block. Zero matches on the block's first line.
    PreviousLine { range: RangeId },
    /// The line after the one containing the start of a range, in the same
    /// block. Zero matches on the block's last line.
    NextLine { range: RangeId },
    /// The first line of a block.
    FirstLine { node: NodeId },
    /// The last line of a block.
    LastLine { node: NodeId },
    /// Every line holding part of a range, in order. The one layout query
    /// that can match several lines; it answers with a list even for one.
    LinesIn { range: RangeId },
    /// The frame holding the line that contains the start of a range.
    FrameContaining { range: RangeId },
    /// The page holding the line that contains the start of a range.
    PageContaining { range: RangeId },
}

impl LayoutQuery {
    /// What the query is anchored on.
    pub fn anchor(&self) -> QueryAnchor {
        match self {
            LayoutQuery::LineContaining { range }
            | LayoutQuery::PreviousLine { range }
            | LayoutQuery::NextLine { range }
            | LayoutQuery::LinesIn { range }
            | LayoutQuery::FrameContaining { range }
            | LayoutQuery::PageContaining { range } => QueryAnchor::Range(*range),
            LayoutQuery::FirstLine { node } | LayoutQuery::LastLine { node } => {
                QueryAnchor::Node(*node)
            }
        }
    }

    /// Whether the answer is a list of lines.
    pub fn is_multi(&self) -> bool {
        matches!(self, LayoutQuery::LinesIn { .. })
    }

    fn remapped(&self, map: &IdMap) -> LayoutQuery {
        let mut q = self.clone();
        match &mut q {
            LayoutQuery::LineContaining { range }
            | LayoutQuery::PreviousLine { range }
            | LayoutQuery::NextLine { range }
            | LayoutQuery::LinesIn { range }
            | LayoutQuery::FrameContaining { range }
            | LayoutQuery::PageContaining { range } => *range = map.range(*range),
            LayoutQuery::FirstLine { node } | LayoutQuery::LastLine { node } => {
                *node = map.node(*node)
            }
        }
        q
    }
}

/// An explicit reference to content as it was at a recorded document version
/// (13). The content is a node's whole text or a range's text.
///
/// The version is a [`Revision`]: a causal frontier of the operation history,
/// which a layout snapshot also records. The reference resolves against this
/// document's history, so it works only while that history is available:
///
/// -   the version is not in this document's history (it comes from a peer
///     whose edits were not merged, or from another document): the target is
///     `Missing`, `relation.snapshot-unavailable`;
/// -   the history was compacted away (07): the same;
/// -   the subject did not exist at that version: `Missing`,
///     `relation.no-match`;
/// -   otherwise the target is `Valid` and carries the text as it was, and
///     whether the subject still exists and is unchanged now. A subject that
///     has since been deleted does not make the target missing: the point of a
///     snapshot reference is that it doesn't change.
///
/// Deletion policies never apply to snapshot targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRef {
    pub version: Revision,
    pub of: SnapshotOf,
}

/// What a [`SnapshotRef`] refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotOf {
    /// A block's whole text.
    Node(NodeId),
    /// A persistent range, resolved at that version.
    Range(RangeId),
}

/// A typed parameter value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Param {
    Length(LengthExpr),
    Int(i64),
    Bool(bool),
    Text(String),
}

impl Param {
    pub fn kind(&self) -> ParamKind {
        match self {
            Param::Length(_) => ParamKind::Length,
            Param::Int(_) => ParamKind::Int,
            Param::Bool(_) => ParamKind::Bool,
            Param::Text(_) => ParamKind::Text,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ParamKind {
    Length,
    Int,
    Bool,
    Text,
}

/// One authored relation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub schema: SchemaId,
    /// The node that owns the relation, for schemas with [`Ownership::Owned`].
    pub owner: Option<NodeId>,
    /// Targets by role name. A role may take several targets, in order.
    pub targets: BTreeMap<String, Vec<Target>>,
    pub params: BTreeMap<String, Param>,
}

impl Relation {
    pub fn new(schema: SchemaId) -> Relation {
        Relation {
            schema,
            owner: None,
            targets: BTreeMap::new(),
            params: BTreeMap::new(),
        }
    }

    pub fn owned_by(mut self, owner: NodeId) -> Relation {
        self.owner = Some(owner);
        self
    }

    pub fn target(mut self, role: &str, target: Target) -> Relation {
        self.targets.entry(role.into()).or_default().push(target);
        self
    }

    pub fn param(mut self, name: &str, value: Param) -> Relation {
        self.params.insert(name.into(), value);
        self
    }

    /// The first target in `role`.
    pub fn first(&self, role: &str) -> Option<&Target> {
        self.targets.get(role).and_then(|t| t.first())
    }

    /// Everything the relation reads: its owner and all its targets (27).
    pub fn dependencies(&self) -> Vec<Dependency> {
        let mut deps: Vec<Dependency> = self.owner.map(Dependency::Node).into_iter().collect();
        deps.extend(
            self.targets
                .values()
                .flatten()
                .flat_map(Target::dependencies),
        );
        deps.sort();
        deps.dedup();
        deps
    }

    /// The relation with copied IDs replaced by their copies: the owner and
    /// every target. Parameters are values, and are copied as they are.
    pub fn remapped(&self, map: &IdMap) -> Relation {
        Relation {
            schema: self.schema.clone(),
            owner: self.owner.map(|o| map.node(o)),
            targets: self
                .targets
                .iter()
                .map(|(role, ts)| (role.clone(), ts.iter().map(|t| t.remapped(map)).collect()))
                .collect(),
            params: self.params.clone(),
        }
    }
}

/// Whether a relation belongs to a node (14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ownership {
    /// The relation belongs to its owner node, and is deleted and copied with it.
    Owned,
    /// The relation exists on its own, between its targets.
    Independent,
}

/// What happens to a relation when one of its targets is deleted (14, 15).
///
/// A policy is **never applied by mutating the document when a target is
/// deleted**. It is evaluated whenever the relation is resolved, from the
/// document's current state: the tombstones, and the succession links that
/// [`Document::supersede`](crate::Document::supersede) records. That is what
/// makes it correct under collaboration (29). A target deleted by another
/// peer and then merged is, to the resolver, exactly a target deleted
/// locally, because both are the same CRDT state. Nothing is rewritten, so
/// there is no operation for two peers to apply twice or in different orders.
/// Whether a restored node counts as the same target is a question of IDs:
/// Loro's undo restores a deleted block as a *new* node, so the kernel must
/// record it with [`Document::supersede`](crate::Document::supersede), after
/// which `Rebind` relations follow it.
///
/// "Deleted" means the target is gone: a node that is tombstoned, a range
/// whose block or text is gone, or a structural or layout query whose anchor
/// node is tombstoned. A query that merely matches nothing isn't deleted, and
/// a snapshot target never is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnTargetDeleted {
    /// Rebind automatically to the best surviving target, using tombstones and
    /// CRDT identity as evidence, and report the new state (15). The evidence
    /// is:
    ///
    /// -   **Ranges:** the characters' identity. A range that lost part of its
    ///     text is `Rebound` to what is left; one that lost all of it is
    ///     `Missing`. A range cannot follow a deleted block, because its
    ///     characters went with it.
    /// -   **Nodes**, and the anchors of structural and layout queries: the
    ///     node's succession links. When a block is replaced, split or merged,
    ///     [`Document::supersede`](crate::Document::supersede) records its
    ///     successors before the block is deleted. The nearest generation of
    ///     live successors wins. One is `Rebound`. Several in the same
    ///     generation are equally good, so the target is `Ambiguous` and lists
    ///     them. None, or too many generations, is `Missing`. A node deleted
    ///     with no successor has no evidence to rebind on and is `Missing`:
    ///     a neighbouring block is not the same thing.
    Rebind,
    /// Keep the relation, reported as missing, and never rebind it, even when
    /// a successor is recorded. It resolves again if the very same ID is
    /// live again. For schemas that opt out of rebinding.
    KeepMissing,
    /// The relation stops existing as soon as any one of its targets is
    /// deleted. It stays in the document until something deletes it, but
    /// resolution reports it as no longer in effect.
    /// [`Document::dead_relations`] lists the relations an editing command
    /// may now really delete.
    ///
    /// [`Document::dead_relations`]: crate::Document::dead_relations
    Delete,
}

/// What copying does to a relation (14, 35).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyPolicy {
    /// When the owner and every target are inside the copied fragment.
    pub inside: CopyInside,
    /// When some targets are outside the copied fragment.
    pub crossing: CopyCrossing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CopyInside {
    /// Copy the relation, remapped to the copied IDs.
    Duplicate,
    /// Leave it out of the copy.
    Drop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CopyCrossing {
    /// Copy it, still pointing at the original targets outside the fragment.
    KeepOutside,
    /// Leave it out of the copy.
    Drop,
}

/// What a copy takes: the nodes and ranges inside the copied fragment.
/// The clipboard decides which ranges are inside (for example, those over
/// copied text); planning only needs the answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CopySet {
    pub nodes: BTreeSet<NodeId>,
    pub ranges: BTreeSet<RangeId>,
}

impl CopySet {
    fn has_all(&self, anchors: &CopyAnchors) -> bool {
        !anchors.external
            && anchors.nodes.iter().all(|n| self.nodes.contains(n))
            && anchors.ranges.iter().all(|r| self.ranges.contains(r))
    }

    fn has_any(&self, anchors: &CopyAnchors) -> bool {
        anchors.nodes.iter().any(|n| self.nodes.contains(n))
            || anchors.ranges.iter().any(|r| self.ranges.contains(r))
    }
}

/// Copied IDs and their copies. The clipboard makes it once it has created
/// the new IDs (07: copying creates new IDs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdMap {
    pub nodes: BTreeMap<NodeId, NodeId>,
    pub ranges: BTreeMap<RangeId, RangeId>,
}

impl IdMap {
    pub fn node(&self, id: NodeId) -> NodeId {
        self.nodes.get(&id).copied().unwrap_or(id)
    }

    pub fn range(&self, id: RangeId) -> RangeId {
        self.ranges.get(&id).copied().unwrap_or(id)
    }
}

/// The IDs a target is made of.
#[derive(Default)]
struct CopyAnchors {
    nodes: Vec<NodeId>,
    ranges: Vec<RangeId>,
    /// Refers to something that is never inside a copy: the document root, or
    /// a version of the source document's history.
    external: bool,
}

/// What to do with one relation when its fragment is copied (35).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "action", content = "why", rename_all = "kebab-case")]
pub enum CopyAction {
    /// Copy it with every ID remapped to its copy. The relation, and all it
    /// points at, is inside the fragment.
    Duplicate,
    /// Copy it. Remap what is inside the fragment (apply [`Relation::remapped`]
    /// with the new IDs) and leave what is outside pointing where it did.
    KeepOutside,
    /// Leave it out of the copy, and tell the author (35).
    Drop(DropReason),
}

/// Why a relation is not in the copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DropReason {
    /// Its schema says relations entirely inside the fragment are not copied.
    PolicyInside,
    /// Its schema says relations that cross the fragment's edge are not copied.
    PolicyCrossing,
    /// Its schema isn't registered, so nothing says how to copy it.
    UnknownSchema,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct CopyEntry {
    pub relation: RelationId,
    pub action: CopyAction,
}

/// Plans what a copy does with each relation (35): per relation, copy it with
/// remapped IDs, keep it pointing outside, or drop it. Pure: it reads the
/// relations and the schemas and nothing else, and doesn't create anything.
///
/// A relation is *in the fragment* when:
///
/// -   it is owned, and its owner is in `copied` (an owned relation goes with
///     its owner, and is never copied without it; 07), or
/// -   it is independent, and at least one of its targets is in `copied`.
///
/// Relations not in the fragment get no entry: they are not part of the copy,
/// so nothing is lost. For the rest, the schema's [`CopyPolicy`] decides:
///
/// -   *inside*: the owner (if any) and every target are in `copied`;
/// -   *crossing*: anything else. That includes targets that can't be
///     remapped: a structural query relative to the document root, and every
///     snapshot target, which names the source document's history, not a
///     node of the copy.
///
/// Relations whose schema isn't registered are dropped with
/// [`DropReason::UnknownSchema`]: nothing says how they copy, and it is
/// better to say so than to guess. Relations that can't be read are not
/// planned at all, because their owner and targets are unknown; they stay in
/// the source document (34).
///
/// Entries are in relation ID order.
pub fn plan_copy(
    schemas: &SchemaRegistry,
    relations: &[(RelationId, Result<Relation, String>)],
    copied: &CopySet,
) -> Vec<CopyEntry> {
    let mut plan = Vec::new();
    for (id, relation) in relations {
        let Ok(relation) = relation else { continue };
        let mut anchors = CopyAnchors::default();
        for target in relation.targets.values().flatten() {
            let a = target.copy_anchors();
            anchors.nodes.extend(a.nodes);
            anchors.ranges.extend(a.ranges);
            anchors.external |= a.external;
        }
        let owner_inside = relation.owner.map(|o| copied.nodes.contains(&o));
        let in_fragment = match owner_inside {
            Some(inside) => inside,
            None => copied.has_any(&anchors),
        };
        if !in_fragment {
            continue;
        }
        let all_inside = owner_inside.unwrap_or(true) && copied.has_all(&anchors);
        let action = match schemas.get(&relation.schema) {
            None => CopyAction::Drop(DropReason::UnknownSchema),
            Some(schema) => match (all_inside, schema.on_copy) {
                (
                    true,
                    CopyPolicy {
                        inside: CopyInside::Duplicate,
                        ..
                    },
                ) => CopyAction::Duplicate,
                (
                    true,
                    CopyPolicy {
                        inside: CopyInside::Drop,
                        ..
                    },
                ) => CopyAction::Drop(DropReason::PolicyInside),
                (
                    false,
                    CopyPolicy {
                        crossing: CopyCrossing::KeepOutside,
                        ..
                    },
                ) => CopyAction::KeepOutside,
                (
                    false,
                    CopyPolicy {
                        crossing: CopyCrossing::Drop,
                        ..
                    },
                ) => CopyAction::Drop(DropReason::PolicyCrossing),
            },
        };
        plan.push(CopyEntry {
            relation: *id,
            action,
        });
    }
    plan.sort_by_key(|e| e.relation);
    plan
}

/// A role a relation fills targets into.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSpec {
    pub name: Cow<'static, str>,
    pub accepts: Cow<'static, [TargetClass]>,
    pub min: u32,
    /// `None` for no upper limit.
    pub max: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamSpec {
    pub name: Cow<'static, str>,
    pub kind: ParamKind,
    pub required: bool,
}

/// A registered relation type (14).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationSchema {
    pub id: SchemaId,
    /// Bumped when the schema's meaning changes; documents record it (34).
    pub version: u32,
    pub ownership: Ownership,
    pub roles: Vec<RoleSpec>,
    pub params: Vec<ParamSpec>,
    pub on_target_deleted: OnTargetDeleted,
    pub on_copy: CopyPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    #[error("no relation schema {0} is registered")]
    Unknown(SchemaId),
    #[error("relation schema {0} is already registered")]
    Duplicate(SchemaId),
    #[error("{schema} relations must have an owner")]
    MissingOwner { schema: SchemaId },
    #[error("{schema} relations are independent and can't have an owner")]
    UnexpectedOwner { schema: SchemaId },
    #[error("{schema} has no role {role:?}")]
    UnknownRole { schema: SchemaId, role: String },
    #[error("role {role:?} of {schema} takes {min}..{max:?} targets, not {count}")]
    Cardinality {
        schema: SchemaId,
        role: String,
        min: u32,
        max: Option<u32>,
        count: usize,
    },
    #[error("role {role:?} of {schema} doesn't accept {class:?} targets")]
    TargetClass {
        schema: SchemaId,
        role: String,
        class: TargetClass,
    },
    #[error("{schema} has no parameter {name:?}")]
    UnknownParam { schema: SchemaId, name: String },
    #[error("parameter {name:?} of {schema} is required")]
    MissingParam { schema: SchemaId, name: String },
    #[error("parameter {name:?} of {schema} must be {expected:?}")]
    ParamKind {
        schema: SchemaId,
        name: String,
        expected: ParamKind,
    },
}

/// The relation schemas an engine knows.
#[derive(Clone, Debug, Default)]
pub struct SchemaRegistry {
    schemas: BTreeMap<SchemaId, RelationSchema>,
}

impl SchemaRegistry {
    /// A registry with every built-in schema.
    pub fn builtin() -> SchemaRegistry {
        let mut registry = SchemaRegistry::default();
        for schema in builtin::all() {
            registry
                .register(schema)
                .expect("built-in schema IDs are distinct");
        }
        registry
    }

    pub fn register(&mut self, schema: RelationSchema) -> Result<(), SchemaError> {
        if self.schemas.contains_key(&schema.id) {
            return Err(SchemaError::Duplicate(schema.id));
        }
        self.schemas.insert(schema.id.clone(), schema);
        Ok(())
    }

    pub fn get(&self, id: &SchemaId) -> Option<&RelationSchema> {
        self.schemas.get(id)
    }

    /// Schemas in ID order.
    pub fn iter(&self) -> impl Iterator<Item = &RelationSchema> {
        self.schemas.values()
    }

    /// Checks a relation against its schema: ownership, roles, target classes,
    /// cardinality and parameters.
    pub fn validate(&self, relation: &Relation) -> Result<(), SchemaError> {
        let id = &relation.schema;
        let schema = self
            .get(id)
            .ok_or_else(|| SchemaError::Unknown(id.clone()))?;
        match (schema.ownership, relation.owner) {
            (Ownership::Owned, None) => {
                return Err(SchemaError::MissingOwner { schema: id.clone() });
            }
            (Ownership::Independent, Some(_)) => {
                return Err(SchemaError::UnexpectedOwner { schema: id.clone() });
            }
            _ => {}
        }
        if let Some(role) = relation
            .targets
            .keys()
            .find(|r| !schema.roles.iter().any(|s| s.name == r.as_str()))
        {
            return Err(SchemaError::UnknownRole {
                schema: id.clone(),
                role: role.clone(),
            });
        }
        for spec in &schema.roles {
            let targets = relation
                .targets
                .get(spec.name.as_ref())
                .map_or(&[][..], Vec::as_slice);
            let count = targets.len();
            if count < spec.min as usize || spec.max.is_some_and(|m| count > m as usize) {
                return Err(SchemaError::Cardinality {
                    schema: id.clone(),
                    role: spec.name.to_string(),
                    min: spec.min,
                    max: spec.max,
                    count,
                });
            }
            if let Some(t) = targets.iter().find(|t| !spec.accepts.contains(&t.class())) {
                return Err(SchemaError::TargetClass {
                    schema: id.clone(),
                    role: spec.name.to_string(),
                    class: t.class(),
                });
            }
        }
        for (name, value) in &relation.params {
            let spec = schema
                .params
                .iter()
                .find(|p| p.name == name.as_str())
                .ok_or_else(|| SchemaError::UnknownParam {
                    schema: id.clone(),
                    name: name.clone(),
                })?;
            if value.kind() != spec.kind {
                return Err(SchemaError::ParamKind {
                    schema: id.clone(),
                    name: name.clone(),
                    expected: spec.kind,
                });
            }
        }
        if let Some(missing) = schema
            .params
            .iter()
            .find(|p| p.required && !relation.params.contains_key(p.name.as_ref()))
        {
            return Err(SchemaError::MissingParam {
                schema: id.clone(),
                name: missing.name.to_string(),
            });
        }
        Ok(())
    }
}

/// The built-in relation schemas (14). Alignment, spacing, order, grouping,
/// breaks and references are added with the relations workstream.
pub mod builtin {
    use std::borrow::Cow;

    use super::*;

    /// `reprise.follow`: the owner block sits beside its target line, level
    /// with the line's top, and moves with it when the text reflows.
    ///
    /// -   Owned by the block it places.
    /// -   Role `line`: exactly one layout query.
    /// -   Parameter `offset` (length, optional): moves the block down from the
    ///     line's top, or up when negative.
    pub const FOLLOW: SchemaId = SchemaId::new("reprise.follow");

    pub fn follow() -> RelationSchema {
        RelationSchema {
            id: FOLLOW,
            version: 1,
            ownership: Ownership::Owned,
            roles: vec![RoleSpec {
                name: Cow::Borrowed("line"),
                accepts: Cow::Borrowed(&[TargetClass::Layout]),
                min: 1,
                max: Some(1),
            }],
            params: vec![ParamSpec {
                name: Cow::Borrowed("offset"),
                kind: ParamKind::Length,
                required: false,
            }],
            on_target_deleted: OnTargetDeleted::Rebind,
            on_copy: CopyPolicy {
                inside: CopyInside::Duplicate,
                crossing: CopyCrossing::KeepOutside,
            },
        }
    }

    /// `reprise.reference`: the owner node refers to something else. Layout
    /// resolves the target and reports how it stands, and does nothing more:
    /// what a reference looks like is for the schemas and styles built on it.
    ///
    /// -   Owned by the node that holds the reference.
    /// -   Role `to`: exactly one target, of any class. A query that matches
    ///     several candidates is `Ambiguous` here.
    /// -   Rebinds when its target is deleted (15), and is copied wherever its
    ///     owner is copied.
    pub const REFERENCE: SchemaId = SchemaId::new("reprise.reference");

    pub fn reference() -> RelationSchema {
        RelationSchema {
            id: REFERENCE,
            version: 1,
            ownership: Ownership::Owned,
            roles: vec![RoleSpec {
                name: Cow::Borrowed("to"),
                accepts: Cow::Borrowed(&[
                    TargetClass::Node,
                    TargetClass::Range,
                    TargetClass::Structural,
                    TargetClass::Layout,
                    TargetClass::Snapshot,
                ]),
                min: 1,
                max: Some(1),
            }],
            params: Vec::new(),
            on_target_deleted: OnTargetDeleted::Rebind,
            on_copy: CopyPolicy {
                inside: CopyInside::Duplicate,
                crossing: CopyCrossing::KeepOutside,
            },
        }
    }

    pub use crate::region_schema::{FLOAT, NOTE, float, note};

    pub fn all() -> Vec<RelationSchema> {
        vec![
            follow(),
            crate::marks::schema(),
            reference(),
            crate::reading::schema(),
            float(),
            note(),
        ]
    }
}
