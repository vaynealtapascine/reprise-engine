//! Explicit traversal of the body and independent authored stories.

use loro::{TreeID, TreeParentId};

use crate::{Document, NodeId, get_str};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Domain {
    Body,
    Story(NodeId),
}

impl Document {
    /// Isolated validation replica with identical operation-ID allocation.
    ///
    /// A plain fork commits what is pending, which would publish a native
    /// caller's raw writes on behalf of an edit that may then be refused. This
    /// is the preflight fork: it refuses pending writes and over-deep trees
    /// first (`DocError::UncommittedEdit`, `EditPreflightDepth`).
    pub fn validation_fork(&self) -> Result<Document, crate::DocError> {
        self.fork_for_edit()
    }

    pub(crate) fn is_story_node(&self, id: TreeID) -> bool {
        self.tree("content")
            .get_meta(id)
            .ok()
            .and_then(|m| get_str(&m, "kind"))
            .as_deref()
            == Some("story")
    }

    /// Live root-level stories, in authored tree order. Stories have no slot.
    pub fn stories(&self) -> Vec<NodeId> {
        let tree = self.tree("content");
        tree.children(TreeParentId::Root)
            .unwrap_or_default()
            .into_iter()
            .filter(|&id| self.is_story_node(id) && self.live(&tree, id))
            .map(NodeId::tree)
            .collect()
    }

    /// The live blocks at the domain's top level, expanding flow hosts.
    pub fn top(&self, domain: Domain) -> Vec<NodeId> {
        let parent = match domain {
            Domain::Body => TreeParentId::Root,
            Domain::Story(id)
                if !id.is_break() && self.is_live(id) && self.is_story_node(id.node) =>
            {
                TreeParentId::Node(id.node)
            }
            Domain::Story(_) => return Vec::new(),
        };
        self.content_children(parent)
    }

    /// Children of a node, or the explicitly chosen domain root.
    pub fn children_in(&self, domain: Domain, of: Option<NodeId>) -> Vec<NodeId> {
        match of {
            Some(node) => self.children_of(node),
            None => self.top(domain),
        }
    }

    /// Live children within a domain; a story itself is a domain root.
    pub fn children_of(&self, node: NodeId) -> Vec<NodeId> {
        if node.is_break() || !self.is_live(node) {
            return Vec::new();
        }
        self.content_children(TreeParentId::Node(node.node))
    }

    fn content_children(&self, parent: TreeParentId) -> Vec<NodeId> {
        let tree = self.tree("content");
        let mut out = Vec::new();
        for node in tree.children(parent).unwrap_or_default() {
            if self.live(&tree, node) && !self.is_story_node(node) && !self.is_embedded(node) {
                self.expand(node, 0, &mut out);
            }
        }
        out
    }

    /// Domain order: containers precede their children; flows expand in text order.
    pub fn order(&self, domain: Domain) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = self.top(domain);
        stack.reverse();
        while let Some(node) = stack.pop() {
            out.push(node);
            let mut children = self.children_of(node);
            children.reverse();
            stack.extend(children);
        }
        out
    }

    /// Every live authored block: body, then each root story in tree order.
    pub fn authored_order(&self) -> Vec<NodeId> {
        let mut out = self.order(Domain::Body);
        for story in self.stories() {
            out.extend(self.order(Domain::Story(story)));
        }
        out
    }

    /// A live node's domain. Nested stories and their subtrees have none.
    pub fn domain_of(&self, node: NodeId) -> Option<Domain> {
        if !self.is_live(node) {
            return None;
        }
        let tree = self.tree("content");
        let mut at = node.node;
        loop {
            match tree.parent(at)? {
                TreeParentId::Root => {
                    return Some(if self.is_story_node(at) {
                        Domain::Story(NodeId::tree(at))
                    } else {
                        Domain::Body
                    });
                }
                TreeParentId::Node(parent) => at = parent,
                _ => return None,
            }
        }
    }
}
