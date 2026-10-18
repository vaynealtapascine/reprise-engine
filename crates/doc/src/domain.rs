//! Explicit traversal of the body and independent authored stories.

use loro::{TreeID, TreeParentId};

use crate::{Document, NodeId, get_str};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Domain {
    Body,
    Story(NodeId),
}

/// Where a bounded read of one parent's children has got to (see
/// [`Document::top_reader`]).
#[derive(Clone, Debug, Default)]
pub struct ChildReader {
    ids: std::sync::Arc<Vec<TreeID>>,
    at: usize,
}

impl ChildReader {
    /// Child IDs not looked at yet.
    pub fn remaining(&self) -> usize {
        self.ids.len().saturating_sub(self.at)
    }
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

    /// A resumable, bounded read of the live blocks at the top of `domain`
    /// (see [`Document::top`]). Only the raw list of child IDs is copied up
    /// front; a child is looked at when a read reaches it, so reading a prefix
    /// of a huge domain costs the prefix.
    pub fn top_reader(&self, domain: Domain) -> ChildReader {
        let parent = match domain {
            Domain::Body => TreeParentId::Root,
            Domain::Story(id)
                if !id.is_break() && self.is_live(id) && self.is_story_node(id.node) =>
            {
                TreeParentId::Node(id.node)
            }
            Domain::Story(_) => return ChildReader::default(),
        };
        self.reader(parent)
    }

    /// As [`Document::top_reader`], for the children of `node` (see
    /// [`Document::children_of`]).
    pub fn children_reader(&self, node: NodeId) -> ChildReader {
        if node.is_break() || !self.is_live(node) {
            return ChildReader::default();
        }
        self.reader(TreeParentId::Node(node.node))
    }

    fn reader(&self, parent: TreeParentId) -> ChildReader {
        ChildReader {
            ids: std::sync::Arc::new(self.tree("content").children(parent).unwrap_or_default()),
            at: 0,
        }
    }

    /// Looks at the next at most `inspect` child IDs of `reader` and appends
    /// the live blocks they stand for to `out` (a flow host stands for all its
    /// paragraphs). Returns true once the reader is exhausted. Every call looks
    /// at one ID at least, so a loop of reads ends.
    pub fn read_children(
        &self,
        reader: &mut ChildReader,
        inspect: usize,
        out: &mut Vec<NodeId>,
    ) -> bool {
        let tree = self.tree("content");
        let end = reader
            .at
            .saturating_add(inspect.max(1))
            .min(reader.ids.len());
        for &node in reader.ids.get(reader.at..end).unwrap_or_default() {
            if self.live(&tree, node) && !self.is_story_node(node) && !self.is_embedded(node) {
                self.expand(node, 0, out);
            }
        }
        reader.at = end;
        end >= reader.ids.len()
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
