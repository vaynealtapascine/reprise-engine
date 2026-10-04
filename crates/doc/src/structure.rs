//! The content tree as relations see it (13, 15): navigation, structural
//! queries and succession links.
//!
//! Everything here is a pure function of the document. Parents, siblings and
//! children are read from the live tree, including table/row/cell nesting.
//! A flagged ancestor hides its whole subtree from these queries.

use std::collections::{BTreeMap, BTreeSet};

use loro::{TreeID, TreeParentId};

use crate::relation::StructuralQuery;
use crate::{BlockKind, DocError, Document, NodeId, get_str};

/// Metadata keys of a node that start with this record a successor: the key
/// is the prefix followed by the successor's ID. One key per successor, so
/// that two peers recording different successors at the same time keep both
/// (a single map entry would keep only one of them).
const SUCCESSOR_PREFIX: &str = "succ/";

/// The same link, recorded on the successor instead, for when the old node
/// can no longer be written to. The key is the prefix and the old node's ID.
const PREDECESSOR_PREFIX: &str = "pred/";

/// How many generations of successors rebinding follows before giving up.
/// A chain this long is a mistake or an attack; giving up is reported with
/// `relation.rebind-limit` (37).
pub const MAX_SUCCESSION_DEPTH: usize = 32;

/// What succession found for a node that was deleted (15).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Succession {
    /// The live nodes of the nearest generation that has any, in document
    /// order. More than one means they are equally good.
    Live(Vec<NodeId>),
    /// No successor was recorded, or every line of succession ends in a
    /// deleted node.
    None,
    /// [`MAX_SUCCESSION_DEPTH`] generations without a live node.
    LimitReached,
}

impl Document {
    /// Whether a block is alive.
    pub fn is_live(&self, id: NodeId) -> bool {
        self.live(&self.tree("content"), id.0)
    }

    /// The children of `of`, or of the document root for `None`, in order.
    /// Empty when `of` isn't a live node.
    pub fn children(&self, of: Option<NodeId>) -> Vec<NodeId> {
        let tree = self.tree("content");
        let parent = match of {
            None => TreeParentId::Root,
            Some(n) if self.live(&tree, n.0) => TreeParentId::Node(n.0),
            Some(_) => return Vec::new(),
        };
        tree.children(parent)
            .unwrap_or_default()
            .into_iter()
            .filter(|&c| self.live(&tree, c))
            .map(NodeId)
            .collect()
    }

    /// The parent of a live node: `Some(None)` for a top-level block,
    /// `None` when `id` isn't a live node.
    pub fn parent_of(&self, id: NodeId) -> Option<Option<NodeId>> {
        let tree = self.tree("content");
        if !self.live(&tree, id.0) {
            return None;
        }
        match tree.parent(id.0)? {
            TreeParentId::Root => Some(None),
            TreeParentId::Node(p) if self.live(&tree, p) => Some(Some(NodeId(p))),
            _ => None,
        }
    }

    /// A node's kind. `None` when the node isn't live or its kind isn't one
    /// this engine knows (a newer engine may have written it).
    pub fn kind_of(&self, id: NodeId) -> Option<BlockKind> {
        let tree = self.tree("content");
        if !self.live(&tree, id.0) {
            return None;
        }
        let meta = tree.get_meta(id.0).ok()?;
        get_str(&meta, "kind").and_then(|k| BlockKind::parse(&k))
    }

    /// Every live node in document order: a node before its children, a
    /// node's children in order.
    pub fn document_order(&self) -> Vec<NodeId> {
        let tree = self.tree("content");
        let mut order = Vec::new();
        let mut stack: Vec<TreeID> = tree
            .children(TreeParentId::Root)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .collect();
        // Each node is pushed once, from its one parent, so this ends.
        while let Some(id) = stack.pop() {
            if !self.live(&tree, id) {
                continue;
            }
            order.push(NodeId(id));
            stack.extend(
                tree.children(TreeParentId::Node(id))
                    .unwrap_or_default()
                    .into_iter()
                    .rev(),
            );
        }
        order
    }

    /// The blocks a structural query matches, in document order. The query's
    /// anchor is assumed to be alive; a deleted anchor matches nothing, and
    /// telling that apart from a query that matches nothing is up to the
    /// caller (see `resolve_target`).
    pub fn evaluate(&self, query: &StructuralQuery) -> Vec<NodeId> {
        let of_kind = |kind: &Option<BlockKind>| {
            let kind = *kind;
            move |doc: &Document, id: NodeId| kind.is_none_or(|k| doc.kind_of(id) == Some(k))
        };
        match query {
            StructuralQuery::NextSibling { from, kind }
            | StructuralQuery::PreviousSibling { from, kind } => {
                let Some(parent) = self.parent_of(*from) else {
                    return Vec::new();
                };
                let siblings = self.children(parent);
                let Some(at) = siblings.iter().position(|s| s == from) else {
                    return Vec::new();
                };
                let matches = of_kind(kind);
                let found = if matches!(query, StructuralQuery::NextSibling { .. }) {
                    siblings[at + 1..].iter().find(|&&s| matches(self, s))
                } else {
                    siblings[..at].iter().rev().find(|&&s| matches(self, s))
                };
                found.copied().into_iter().collect()
            }
            StructuralQuery::NthChild {
                of,
                index,
                from_end,
                kind,
            } => {
                let matches = of_kind(kind);
                let children: Vec<NodeId> = self
                    .children(*of)
                    .into_iter()
                    .filter(|&c| matches(self, c))
                    .collect();
                let index = *index as usize;
                let at = if *from_end {
                    children.len().checked_sub(index.saturating_add(1))
                } else {
                    Some(index)
                };
                at.and_then(|i| children.get(i))
                    .copied()
                    .into_iter()
                    .collect()
            }
            StructuralQuery::FirstChild { of, kind } => {
                let matches = of_kind(kind);
                self.children(*of)
                    .into_iter()
                    .find(|&c| matches(self, c))
                    .into_iter()
                    .collect()
            }
            StructuralQuery::LastChild { of, kind } => {
                let matches = of_kind(kind);
                self.children(*of)
                    .into_iter()
                    .rev()
                    .find(|&c| matches(self, c))
                    .into_iter()
                    .collect()
            }
            StructuralQuery::Parent { of } => match self.parent_of(*of) {
                Some(Some(parent)) => vec![parent],
                _ => Vec::new(),
            },
            StructuralQuery::Children { of, kind } => {
                let matches = of_kind(kind);
                self.children(*of)
                    .into_iter()
                    .filter(|&c| matches(self, c))
                    .collect()
            }
        }
    }

    /// Records that `new` takes the place of `old`: the evidence relations
    /// need to rebind from `old` to `new` once `old` is deleted (15).
    ///
    /// `new` must be alive; `old` may be alive or already deleted. Call it:
    ///
    /// -   when a block is **replaced, split or merged**: once for each
    ///     successor. Halves of a split block are equally good successors, so
    ///     relations on it become `Ambiguous`.
    /// -   when a legacy **physical tree-delete undo** creates a replacement
    ///     node with a new ID. The kernel's flag-based undo restores the original
    ///     node and needs no succession link.
    ///
    /// The link is one metadata key, on `old` while it is alive and on `new`
    /// once `old` is deleted, so it
    /// merges like any other edit and two peers' successors are both kept.
    pub fn supersede(&self, old: NodeId, new: NodeId) -> Result<(), DocError> {
        if old == new {
            return Err(DocError::Malformed(old, "a node can't succeed itself"));
        }
        let tree = self.tree("content");
        if !self.live(&tree, new.0) {
            return Err(DocError::NoNode(new));
        }
        if self.live(&tree, old.0) {
            tree.get_meta(old.0)?
                .insert(&format!("{SUCCESSOR_PREFIX}{new}"), true)?;
        } else if tree.contains(old.0) {
            tree.get_meta(new.0)?
                .insert(&format!("{PREDECESSOR_PREFIX}{old}"), true)?;
        } else {
            return Err(DocError::NoNode(old));
        }
        Ok(())
    }

    /// Every recorded succession link: node to its successors, whichever
    /// side recorded it. Reads every node's metadata, including deleted
    /// nodes', which outlive them.
    fn successor_index(&self) -> BTreeMap<NodeId, BTreeSet<NodeId>> {
        let tree = self.tree("content");
        let mut index: BTreeMap<NodeId, BTreeSet<NodeId>> = BTreeMap::new();
        for id in tree.nodes() {
            let Ok(meta) = tree.get_meta(id) else {
                continue;
            };
            for key in meta.keys() {
                if let Some(new) = key.strip_prefix(SUCCESSOR_PREFIX).and_then(NodeId::parse) {
                    index.entry(NodeId(id)).or_default().insert(new);
                } else if let Some(old) =
                    key.strip_prefix(PREDECESSOR_PREFIX).and_then(NodeId::parse)
                {
                    index.entry(old).or_default().insert(NodeId(id));
                }
            }
        }
        index
    }

    /// The successors recorded for a node, live or not, in ID order.
    pub fn successors(&self, id: NodeId) -> Vec<NodeId> {
        self.successor_index()
            .remove(&id)
            .map(|s| s.into_iter().collect())
            .unwrap_or_default()
    }

    /// What replaces a deleted node (15). Searches generation by generation:
    /// the nearest generation with a live node wins, so a successor beats its
    /// own successors. Terminates: every node is visited once, and the
    /// search stops after [`MAX_SUCCESSION_DEPTH`] generations.
    pub fn succession(&self, from: NodeId) -> Succession {
        let index = self.successor_index();
        let mut seen: BTreeSet<NodeId> = BTreeSet::from([from]);
        let mut generation = vec![from];
        for _ in 0..MAX_SUCCESSION_DEPTH {
            let mut next = Vec::new();
            for node in &generation {
                for s in index.get(node).into_iter().flatten() {
                    if seen.insert(*s) {
                        next.push(*s);
                    }
                }
            }
            if next.is_empty() {
                return Succession::None;
            }
            let live: BTreeSet<NodeId> =
                next.iter().copied().filter(|&n| self.is_live(n)).collect();
            if !live.is_empty() {
                // Document order, so that candidates read the way the
                // document does. Only the order of a short list is at stake.
                let order = self.document_order();
                let mut live: Vec<NodeId> = live.into_iter().collect();
                live.sort_by_key(|n| order.iter().position(|o| o == n));
                return Succession::Live(live);
            }
            generation = next;
        }
        Succession::LimitReached
    }
}
