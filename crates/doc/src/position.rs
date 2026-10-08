//! Where a caret goes when its block is gone (10, 12, 30): the deterministic
//! fallback for stable carets. See `docs/collaboration.md`, "Stable carets".
//!
//! Every answer is a pure function of the document, so every replica with
//! the same state chooses the same place.

use loro::{Container, TreeID, TreeParentId, ValueOrContainer};
use reprise_text::Text;

use crate::{Document, NodeId, Succession};

/// The nearest place a caret can go instead of a block that is not live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A live successor (15), in document order the first. The caret keeps
    /// its distance from the end of the old text.
    Successor(NodeId),
    /// The end of the nearest live caret block before the old one.
    EndOf(NodeId),
    /// The start of the nearest live caret block after it.
    StartOf(NodeId),
    /// The document has no live caret block.
    Nowhere,
}

impl Document {
    /// A live block that can hold a caret: its envelope reads, and it is not
    /// a table, row or cell container.
    pub fn is_caret_block(&self, id: NodeId) -> bool {
        self.block(id).is_ok() && matches!(self.table_role(id), Ok(None))
    }

    /// The text container of a node whether or not it is live, for resolving
    /// anchors left in deleted blocks. `None` if it has none. For a paragraph
    /// of a flow this is the whole shared text of its host, with offsets in
    /// that text; [`Document::locate`] gives paragraph offsets.
    pub fn text_of_any(&self, id: NodeId) -> Option<Text> {
        let meta = self.tree("content").get_meta(id.node).ok()?;
        match meta.get("text")? {
            ValueOrContainer::Container(Container::Text(t)) => Some(Text::from_loro(t)),
            _ => None,
        }
    }

    /// Where a caret in `id` goes when `id` is not live. For a live caret
    /// block this is `Successor(id)` itself.
    pub fn caret_fallback(&self, id: NodeId) -> Fallback {
        if self.is_caret_block(id) {
            return Fallback::Successor(id);
        }
        if let Succession::Live(live) = self.succession(id)
            && let Some(&first) = live.iter().find(|&&n| self.is_caret_block(n))
        {
            return Fallback::Successor(first);
        }
        let order = self.full_order();
        let Some(at) = order.iter().position(|&n| n == id) else {
            // Removed from the tree: no position to search from.
            return order
                .iter()
                .copied()
                .find(|&n| self.is_caret_block(n))
                .map_or(Fallback::Nowhere, Fallback::StartOf);
        };
        let before = order.get(..at).unwrap_or_default();
        if let Some(&n) = before.iter().rev().find(|&&n| self.is_caret_block(n)) {
            return Fallback::EndOf(n);
        }
        let after = order.get(at + 1..).unwrap_or_default();
        after
            .iter()
            .copied()
            .find(|&n| self.is_caret_block(n))
            .map_or(Fallback::Nowhere, Fallback::StartOf)
    }

    /// Every node in the content tree, live or not, in document order.
    /// Physically deleted nodes are not in the tree and are left out.
    /// A flow's break paragraphs, live or not, follow their host in text
    /// order.
    fn full_order(&self) -> Vec<NodeId> {
        let tree = self.tree("content");
        let mut order = Vec::new();
        let mut stack: Vec<TreeID> = tree.children(TreeParentId::Root).unwrap_or_default();
        stack.reverse();
        // Each node is pushed once, from its one parent, so this ends.
        while let Some(id) = stack.pop() {
            order.push(NodeId::tree(id));
            if let Some(flow) = self.flow(id) {
                order.extend(flow.marks.iter().map(|&(m, _)| m));
            }
            let mut kids = tree.children(TreeParentId::Node(id)).unwrap_or_default();
            kids.reverse();
            stack.extend(kids);
        }
        order
    }
}
