//! What an import, undo or redo changed (27, 28, 29), collected from the
//! store's container events while the operation runs.
//!
//! The report may name more than changed, never less: a block whose deletion
//! flag or tree position changed brings its whole subtree, because the
//! visibility of every descendant may have changed with it.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use loro::event::Diff;
use loro::{ContainerID, Index, LoroTree, TreeID, TreeParentId};

use crate::{Document, NodeId, RangeId, RelationId};

/// What one operation changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeReport {
    /// Every block whose text, metadata or tree position changed, in ID order.
    /// Deleted blocks are included: a UI must learn that they disappeared.
    pub blocks: BTreeSet<NodeId>,
    /// Whether any content node was created, moved or deleted in the tree.
    pub structure: bool,
    /// Whether any named style changed. Every block may have restyled.
    pub styles: bool,
    /// Relations whose records changed.
    pub relations: BTreeSet<RelationId>,
    /// Ranges whose records changed.
    pub ranges: BTreeSet<RangeId>,
    /// Whether anything outside the engine's known containers changed.
    pub other: bool,
}

impl ChangeReport {
    pub fn is_empty(&self) -> bool {
        *self == ChangeReport::default()
    }

    /// Adds everything `other` reports.
    pub fn extend(&mut self, other: ChangeReport) {
        self.blocks.extend(other.blocks);
        self.structure |= other.structure;
        self.styles |= other.styles;
        self.relations.extend(other.relations);
        self.ranges.extend(other.ranges);
        self.other |= other.other;
    }
}

/// One container event, reduced to owned data the callback may keep.
#[derive(Debug)]
struct Raw {
    /// The root container the event is under.
    root: Option<String>,
    /// The tree node the event's container belongs to, if any.
    node: Option<TreeID>,
    /// Tree nodes created, moved or deleted, when the event is a tree diff.
    tree: Vec<TreeID>,
    /// Whether the event is a map diff (node metadata: flags, kind, records).
    map: bool,
}

fn root_name(id: &ContainerID) -> Option<String> {
    match id {
        ContainerID::Root { name, .. } => Some(name.to_string()),
        ContainerID::Normal { .. } => None,
    }
}

impl Document {
    /// Runs `f` and reports what it changed in the document.
    pub(crate) fn tracked<R>(&self, f: impl FnOnce() -> R) -> (R, ChangeReport) {
        let raw: Arc<Mutex<Vec<Raw>>> = Arc::default();
        let sink = Arc::clone(&raw);
        let subscription = self.doc.subscribe_root(Arc::new(move |event| {
            let Ok(mut sink) = sink.lock() else {
                return;
            };
            for diff in event.events {
                let root = diff
                    .path
                    .first()
                    .and_then(|(id, _)| root_name(id))
                    .or_else(|| root_name(diff.target));
                let node = diff.path.iter().find_map(|(_, index)| match index {
                    Index::Node(id) => Some(*id),
                    _ => None,
                });
                let tree = match &diff.diff {
                    Diff::Tree(tree) => tree.diff.iter().map(|item| item.target).collect(),
                    _ => Vec::new(),
                };
                let map = matches!(diff.diff, Diff::Map(_));
                sink.push(Raw {
                    root,
                    node,
                    tree,
                    map,
                });
            }
        }));
        let result = f();
        drop(subscription);
        let raw = match Arc::try_unwrap(raw) {
            Ok(m) => m.into_inner().unwrap_or_else(|e| e.into_inner()),
            Err(shared) => shared
                .lock()
                .map(|mut v| std::mem::take(&mut *v))
                .unwrap_or_default(),
        };
        (result, self.report(raw))
    }

    fn report(&self, raw: Vec<Raw>) -> ChangeReport {
        let mut report = ChangeReport::default();
        let mut subtrees = BTreeSet::new();
        for event in raw {
            match event.root.as_deref() {
                Some("content") => {
                    if !event.tree.is_empty() {
                        report.structure = true;
                        subtrees.extend(event.tree.iter().copied());
                    }
                    if let Some(node) = event.node {
                        report.blocks.insert(NodeId(node));
                        if event.map {
                            subtrees.insert(node);
                        }
                    }
                }
                Some("relations") => {
                    report
                        .relations
                        .extend(event.tree.iter().copied().map(RelationId));
                    report.relations.extend(event.node.map(RelationId));
                }
                Some("ranges") => {
                    report
                        .ranges
                        .extend(event.tree.iter().copied().map(RangeId));
                    report.ranges.extend(event.node.map(RangeId));
                }
                Some("styles") => report.styles = true,
                _ => report.other = true,
            }
        }
        let tree = self.tree("content");
        for id in subtrees {
            report.blocks.insert(NodeId(id));
            descendants(&tree, id, &mut report.blocks);
        }
        report
    }
}

/// Every node below `id`, live or not. Loro's tree is acyclic, and each node
/// is visited from its one parent, so this ends.
fn descendants(tree: &LoroTree, id: TreeID, out: &mut BTreeSet<NodeId>) {
    let mut stack = vec![id];
    while let Some(at) = stack.pop() {
        for child in tree.children(TreeParentId::Node(at)).unwrap_or_default() {
            out.insert(NodeId(child));
            stack.push(child);
        }
    }
}
