//! Post-merge invariant audit (07, 29, 37): states a merge can produce that
//! the editing kernel never would, found by reading, never repaired by
//! writing. See `docs/collaboration.md`, invariants I2 to I4.
//!
//! Every replica with the same CRDT state gets the same findings, in the
//! same order, so the audit needs no agreement between peers.

use loro::{LoroTree, TreeID, TreeParentId};
use reprise_diag::{Code, Note, Severity};

use crate::{DocError, Document, NodeId};

/// A live content node whose envelope can't be read (I2). Layout leaves it
/// out with `layout.malformed-block`, so this is an `Error`.
pub const MALFORMED_NODE: Code = Code::new("collab.malformed-node");
/// A node deleted physically from the store's tree rather than flagged (I3).
/// The kernel never does this, so the content is hidden without an author
/// having deleted it: a `Warning`.
pub const TREE_TOMBSTONE: Code = Code::new("collab.tree-tombstone");
/// Live content under a joined block (I4): a join requires the second block
/// to have no live children, so a concurrent insertion is hidden with it.
/// The output differs from what the inserting author asked: a `Warning`.
pub const HIDDEN_CONTENT: Code = Code::new("collab.hidden-content");

/// One audit finding.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub node: NodeId,
    pub note: Note,
}

impl Document {
    /// Checks invariants I2 to I4 over the whole content tree, in document
    /// order with physically deleted nodes last in ID order. O(nodes × depth).
    /// Nothing is written. The engine never runs this implicitly.
    pub fn audit(&self) -> Vec<Finding> {
        let tree = self.tree("content");
        let mut out = Vec::new();
        let mut stack: Vec<TreeID> = children(&tree, TreeParentId::Root);
        stack.reverse();
        // Each node is pushed once, from its one parent, so this ends.
        while let Some(id) = stack.pop() {
            let node = NodeId::tree(id);
            if self.live(&tree, id) {
                if let Err(DocError::Malformed(_, what)) = self.block(node) {
                    out.push(Finding {
                        node,
                        note: Note::new(
                            Severity::Error,
                            MALFORMED_NODE,
                            format!("node {node} has an unreadable {what}"),
                        ),
                    });
                }
            } else if self.is_soft_deleted(node) && !self.successors(node).is_empty() {
                for child in children(&tree, TreeParentId::Node(id)) {
                    if !self.is_soft_deleted(NodeId::tree(child)) {
                        out.push(Finding {
                            node: NodeId::tree(child),
                            note: Note::new(
                                Severity::Warning,
                                HIDDEN_CONTENT,
                                format!("{child} is hidden inside joined block {node}"),
                            ),
                        });
                    }
                }
            }
            let mut kids = children(&tree, TreeParentId::Node(id));
            kids.reverse();
            stack.extend(kids);
        }
        let mut gone: Vec<TreeID> = tree
            .get_nodes(true)
            .into_iter()
            .filter(|n| matches!(n.parent, TreeParentId::Deleted))
            .map(|n| n.id)
            .collect();
        gone.sort();
        for id in gone {
            let node = NodeId::tree(id);
            if !self.is_flagged_raw(&tree, id) {
                out.push(Finding {
                    node,
                    note: Note::new(
                        Severity::Warning,
                        TREE_TOMBSTONE,
                        format!("node {node} was removed from the tree, not deleted"),
                    ),
                });
            }
        }
        out
    }

    fn is_flagged_raw(&self, tree: &LoroTree, id: TreeID) -> bool {
        tree.get_meta(id)
            .ok()
            .and_then(|m| crate::get_bool(&m, "deleted"))
            .unwrap_or(false)
    }
}

fn children(tree: &LoroTree, parent: TreeParentId) -> Vec<TreeID> {
    tree.children(parent).unwrap_or_default()
}
