//! Explicit traversal of the body and independent authored stories.

use std::sync::Arc;

use loro::{TreeID, TreeParentId};

use crate::flow::{HostFlow, MAX_FLOW_DEPTH};
use crate::{Document, NodeId, get_str};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Domain {
    Body,
    Story(NodeId),
}

/// Bytes of a flow host's text decoded for one unit of work.
pub const DECODE_UNIT_BYTES: usize = 4096;

/// Where a bounded read of one parent's children has got to (see
/// [`Document::top_reader`]): the next physical child to look at, and the
/// flow host being expanded, if one is, as a stack of what is still to come.
/// Neither is gathered up front, so starting a read costs nothing and a read
/// costs what it reads.
#[derive(Clone, Debug, Default)]
pub struct ChildReader {
    parent: Option<TreeParentId>,
    /// Physical children when the reader was made, and how many were looked at.
    total: usize,
    at: usize,
    todo: Vec<Todo>,
    /// The first block of the physical child last looked at is paid for by
    /// the look. Spent by the first block that comes.
    free: bool,
}

/// What an expansion still has to do, innermost last.
#[derive(Clone, Debug)]
enum Todo {
    /// A live tree node, `depth` flows deep, not looked at yet.
    Node(TreeID, usize),
    /// Paragraph `.1` and the ones after it of a decoded host.
    Paras(Arc<HostFlow>, usize, usize),
}

impl ChildReader {
    /// Child IDs not looked at yet.
    pub fn remaining(&self) -> usize {
        self.total.saturating_sub(self.at)
    }

    /// Whether the reader has nothing left to read, a host half expanded
    /// included.
    pub fn is_done(&self) -> bool {
        self.at >= self.total && self.todo.is_empty()
    }
}

/// How much one [`Document::read_children`] call may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadLimits {
    /// Units of work: a physical child looked at costs one, and the first block
    /// it stands for with it. A flow host also costs its text in
    /// [`DECODE_UNIT_BYTES`] to decode, and one for each further paragraph or
    /// embedded node it stands for. So an ordinary paragraph is one unit, and
    /// a host of a thousand paragraphs a thousand. A decode dearer than `units`
    /// is the one thing that may cost more, and only for a [`fresh`] call: it
    /// runs alone (with the first block of the host) and overspends.
    ///
    /// [`fresh`]: ReadLimits::fresh
    pub units: usize,
    /// Most blocks a call appends.
    pub blocks: usize,
    /// Longest flow host text, in bytes, a call decodes. A longer host is not
    /// read: the call stops in front of it and says so.
    pub host_bytes: usize,
    /// Whether the call opens the piece of work `units` is the allowance of:
    /// none of it is spent yet. A caller that reads its allowance in several
    /// calls sets it on the first only. A call that is not fresh never decodes
    /// a host dearer than `units`: it stops in front of the host
    /// ([`ReadStep::deferred`]), possibly having done nothing, for a fresh
    /// call to decode it alone, so a decode never shares its piece of work with
    /// what came before it.
    pub fresh: bool,
}

impl ReadLimits {
    /// No bound at all: a read to the end.
    pub const fn unbounded() -> Self {
        Self {
            units: usize::MAX,
            blocks: usize::MAX,
            host_bytes: usize::MAX,
            fresh: true,
        }
    }
}

/// What one [`Document::read_children`] call did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadStep {
    /// The reader has nothing left.
    pub done: bool,
    /// The host in front of the reader is longer than
    /// [`ReadLimits::host_bytes`]. It was not read, and a call again meets it
    /// again.
    pub refused: bool,
    /// Physical child IDs looked at.
    pub looked: usize,
    /// Blocks appended.
    pub blocks: usize,
    /// The call stopped in front of a host whose decoding costs more than the
    /// units it had left. A [`fresh`](ReadLimits::fresh) call decodes it,
    /// alone, and overspends.
    pub deferred: bool,
    /// Bytes of host text decoded.
    pub decoded: usize,
    /// Units of work done, as [`ReadLimits::units`] counts them.
    pub units: usize,
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
    /// (see [`Document::top`]). Nothing is gathered up front: the reader keeps
    /// the number of physical children and its place among them, and a child
    /// is found, by index, and looked at when a read reaches it, so reading a
    /// prefix of a huge domain costs the prefix.
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
            parent: Some(parent),
            total: self.tree("content").children_num(parent).unwrap_or(0),
            at: 0,
            todo: Vec::new(),
            free: false,
        }
    }

    /// Reads on in `reader`, within `limits`, appending to `out` the live
    /// blocks [`Document::top`] or [`Document::children_of`] would list, in
    /// the same order: a flow host stands for its paragraphs and the nodes
    /// embedded in them, which are expanded in turn. A host is decoded when
    /// the reader reaches it and left again once its last paragraph is out, so
    /// a call does a bounded amount of work however many paragraphs, or how
    /// much text, the host has (up to `limits.host_bytes`), and the next call
    /// carries on in the same place.
    ///
    /// A call with `limits.fresh` set does at least one thing, so a loop of
    /// fresh reads ends. One that is not fresh may do nothing: it stops in
    /// front of a host dearer than its allowance and says so
    /// ([`ReadStep::deferred`]), having read nothing if the host is the next
    /// thing in the reader, and says it again however often it is repeated. A
    /// caller must handle `deferred`, and open a fresh piece of work (a fresh
    /// call) to get past the host.
    pub fn read_children(
        &self,
        reader: &mut ChildReader,
        limits: ReadLimits,
        out: &mut Vec<NodeId>,
    ) -> ReadStep {
        let mut read = ReadStep::default();
        let want = limits.units.max(1);
        let room = limits.blocks.max(1);
        let tree = self.tree("content");
        let mut handler = None;
        loop {
            // The look at a child paid for its first block: the call is not
            // over before that block is out.
            let owed = reader.free && !reader.todo.is_empty();
            if read.blocks >= room || (read.units >= want && !owed) {
                return read;
            }
            let Some(todo) = reader.todo.pop() else {
                // Whatever the child stood for is out; a credit it did not
                // use (a host with nothing visible) is void.
                reader.free = false;
                if read.units >= want {
                    return read;
                }
                let Some(parent) = reader.parent.filter(|_| reader.at < reader.total) else {
                    read.done = true;
                    return read;
                };
                let children = handler.get_or_insert_with(|| self.doc.inner().get_tree("content"));
                let child = children.get_child_at(&parent, reader.at);
                reader.at += 1;
                read.looked += 1;
                read.units += 1;
                if let Some(node) = child
                    && self.live(&tree, node)
                    && !self.is_story_node(node)
                    && !self.is_embedded(node)
                {
                    reader.todo.push(Todo::Node(node, 0));
                    reader.free = true;
                }
                continue;
            };
            match todo {
                Todo::Paras(flow, i, depth) => {
                    let Some(para) = flow.paras.get(i) else {
                        continue;
                    };
                    if i + 1 < flow.paras.len() {
                        reader.todo.push(Todo::Paras(flow.clone(), i + 1, depth));
                    }
                    if i > 0 || flow.head_live {
                        out.push(para.id);
                        read.blocks += 1;
                        read.units += usize::from(!std::mem::take(&mut reader.free));
                    }
                    if depth < MAX_FLOW_DEPTH {
                        for &(embedded, _, _) in para.embeds.iter().rev() {
                            reader.todo.push(Todo::Node(embedded, depth + 1));
                        }
                    }
                }
                Todo::Node(node, depth) => {
                    if !self.live(&tree, node) || self.is_story_node(node) {
                        continue;
                    }
                    let Some(bytes) = self.host_text(node).map(|t| t.len_utf8()) else {
                        // Not a flow host: the block itself. At the top the ID
                        // looked at has paid for it; one embedded in a flow
                        // is a block of the host's.
                        out.push(NodeId::tree(node));
                        read.blocks += 1;
                        read.units += usize::from(!std::mem::take(&mut reader.free));
                        continue;
                    };
                    let cost = bytes / DECODE_UNIT_BYTES;
                    if bytes > limits.host_bytes {
                        reader.todo.push(Todo::Node(node, depth));
                        read.refused = true;
                        return read;
                    }
                    if (read.units > 0 || !limits.fresh) && read.units.saturating_add(cost) > want {
                        // Dearer than what is left of this call, or of the
                        // piece of work it is in: a call that opens one, with
                        // all of its units, decodes it.
                        reader.todo.push(Todo::Node(node, depth));
                        read.deferred = true;
                        return read;
                    }
                    match self.flow(node) {
                        Some(flow) => {
                            read.decoded += bytes;
                            read.units += cost;
                            reader.todo.push(Todo::Paras(flow, 0, depth));
                        }
                        None => {
                            out.push(NodeId::tree(node));
                            read.blocks += 1;
                            read.units += usize::from(!std::mem::take(&mut reader.free));
                        }
                    }
                }
            }
        }
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
