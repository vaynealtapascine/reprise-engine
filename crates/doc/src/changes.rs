//! What an import, undo or redo changed (27, 28, 29), collected from the
//! store's container events while the operation runs.
//!
//! The report may name more than changed, never less: a block whose deletion
//! flag or tree position changed brings its whole subtree, because the
//! visibility of every descendant may have changed with it.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use loro::event::Diff;
use loro::{ContainerID, Index, LoroTree, TextDelta, TreeID, TreeParentId};

use crate::{Document, NodeId, RangeId, RelationId};

/// What one operation changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeReport {
    /// Every block whose text, metadata or tree position changed, in ID order.
    /// Deleted blocks are included: a UI must learn that they disappeared.
    pub blocks: BTreeSet<NodeId>,
    /// Whether any content node was created, moved or deleted in the tree.
    pub structure: bool,
    /// Whether any named style, page template or page setup changed. Every
    /// block may have restyled or moved.
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
    /// For a text diff, the scalar ranges it touched in the new text.
    text: Vec<(usize, usize)>,
    /// For the break records map, the records it touched.
    keys: Vec<String>,
}

/// The scalar ranges of the new text a text diff inserted into or deleted
/// at (a deletion touches the point where it was).
fn touched(delta: &[TextDelta]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    for d in delta {
        match d {
            TextDelta::Retain { retain, .. } => at += retain,
            TextDelta::Insert { insert, .. } => {
                let n = insert.chars().count();
                out.push((at, at + n));
                at += n;
            }
            TextDelta::Delete { .. } => out.push((at, at)),
        }
    }
    out
}

fn root_name(id: &ContainerID) -> Option<String> {
    match id {
        ContainerID::Root { name, .. } => Some(name.to_string()),
        ContainerID::Normal { .. } => None,
    }
}

impl Document {
    /// Runs `f` and reports what it changed in the document, for example an
    /// undo or a merge.
    pub fn tracked<R>(&self, f: impl FnOnce() -> R) -> (R, ChangeReport) {
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
                let text = match &diff.diff {
                    Diff::Text(delta) => touched(delta),
                    _ => Vec::new(),
                };
                let mut keys: Vec<String> = diff
                    .path
                    .iter()
                    .skip(1)
                    .take(1)
                    .filter_map(|(_, index)| match index {
                        Index::Key(k) => Some(k.to_string()),
                        _ => None,
                    })
                    .collect();
                if root.as_deref() == Some(crate::flow::BREAKS)
                    && diff.path.len() <= 1
                    && let Diff::Map(m) = &diff.diff
                {
                    keys.extend(m.updated.keys().map(|k| k.to_string()));
                }
                sink.push(Raw {
                    root,
                    node,
                    tree,
                    text,
                    keys,
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
                        match self.flow(node) {
                            // Text in a flow: the paragraphs it touched.
                            Some(flow) if !event.text.is_empty() && !event.map => {
                                for &(a, b) in &event.text {
                                    report.blocks.extend(
                                        flow.paras
                                            .iter()
                                            .filter(|p| {
                                                p.region.start.1 <= b && a <= p.region.end.1
                                            })
                                            .map(|p| p.id),
                                    );
                                }
                            }
                            Some(flow) => {
                                report.blocks.insert(NodeId::tree(node));
                                report.blocks.extend(flow.paras.iter().map(|p| p.id));
                            }
                            None => {
                                report.blocks.insert(NodeId::tree(node));
                            }
                        }
                        if event.map {
                            subtrees.insert(node);
                        }
                    }
                }
                Some(crate::flow::BREAKS) => {
                    // A break appeared, went or changed: the paragraphs on
                    // both sides of it, and the paragraph list.
                    report.structure = true;
                    for key in &event.keys {
                        let Some(id) = NodeId::parse(key) else {
                            continue;
                        };
                        report.blocks.insert(id);
                        if let Some(flow) = self.flow(id.node)
                            && let Some(&(_, u)) = flow.marks.iter().find(|(m, _)| *m == id)
                        {
                            report.blocks.extend(
                                flow.paras
                                    .iter()
                                    .filter(|p| p.region.start.1 <= u + 1 && u <= p.region.end.1)
                                    .map(|p| p.id),
                            );
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
                Some("styles" | "page_templates" | "page_setup") => report.styles = true,
                _ => report.other = true,
            }
        }
        let tree = self.tree("content");
        for id in &report.ranges {
            let Ok(meta) = self.tree("ranges").get_meta(id.0) else {
                continue;
            };
            if meta.get("format1").is_none() {
                continue;
            }
            let Some(node) = crate::get_str(&meta, "node").and_then(|n| NodeId::parse(&n)) else {
                continue;
            };
            report.blocks.insert(node);
            if let Some(flow) = self.flow(node.node) {
                report.blocks.extend(flow.paras.iter().map(|p| p.id));
            }
        }
        for id in subtrees {
            report.blocks.insert(NodeId::tree(id));
            descendants(self, &tree, id, &mut report.blocks);
        }
        report
    }
}

/// Every node below `id`, live or not. Loro's tree is acyclic, and each node
/// is visited from its one parent, so this ends.
fn descendants(doc: &Document, tree: &LoroTree, id: TreeID, out: &mut BTreeSet<NodeId>) {
    let mut stack = vec![id];
    while let Some(at) = stack.pop() {
        if let Some(flow) = doc.flow(at) {
            out.extend(flow.paras.iter().map(|p| p.id));
        }
        for child in tree.children(TreeParentId::Node(at)).unwrap_or_default() {
            out.insert(NodeId::tree(child));
            stack.push(child);
        }
    }
}
