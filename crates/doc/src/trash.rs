//! Deletion as a move into a trash parent (07, 29).
//!
//! Loro's `UndoManager` undoes a deletion by creating a **new** node, so a
//! block, relation or range brought back by undo would have a new identity and
//! everything that refers to the old one (relations, ranges, carets,
//! collaborators' selections) would stay orphaned. Decision 07 wants undo,
//! collaboration and relations to *restore* an item. So nothing is ever
//! deleted from a tree: **deleting is moving the node under a trash root, and
//! undo moves it back.** The ID never changes, and neither does the node's
//! metadata, text and subtree.
//!
//! # The scheme
//!
//! -   A **trash root** is a root node whose metadata has `trash = true`. Each
//!     tree (`content`, `relations`, `ranges`) has one trash root per peer,
//!     created lazily by that peer the first time it deletes something. A root
//!     per peer means two peers never race to create *the* trash node, and
//!     every peer can find its own without a lookup in shared state.
//! -   A node is **live** when it is not tombstoned by Loro and its topmost
//!     ancestor is not a trash root. Everything that asks "is this alive" goes
//!     through [`Document::live`], so a trashed node behaves exactly like a
//!     deleted one: `blocks()`, `block()`, relation tombstone evidence, the
//!     `OnTargetDeleted` policies and `relation.owner-deleted` all see it as
//!     gone, and a deletion merged from another peer behaves like a local one.
//! -   A trashed node's subtree goes with it and comes back with it.
//! -   Trash roots are never moved, and nothing else ever becomes a trash root,
//!     so only the topmost ancestor needs checking.
//!
//! # Concurrency
//!
//! Loro orders concurrent moves of one node by operation ID and keeps the last.
//!
//! -   **Edits to a trashed block** (text, style, metadata, relations that
//!     mention it) merge as for any block and don't resurrect it: only a move
//!     out of the trash does.
//! -   **A restore concurrent with a delete** is two moves of the same node, so
//!     the later one in Loro's total order (Lamport timestamp, then peer ID)
//!     wins on every replica. Either result is a defined state; replicas never
//!     disagree. A restore that loses is not lost work: the node, its text and
//!     its ID are all still there.
//! -   **A move concurrent with a delete** is the same case. The block can come
//!     back if the move is later in that order. This is what plain Loro does
//!     too.
//!
//! # Compaction
//!
//! Trash is never emptied. Physically removing it is a later compaction policy
//! (07): drop trashed nodes that no relation, range, snapshot reference or
//! succession link still names, once every replica has seen the deletion.
//! Nothing here depends on how that is decided.
//!
//! # Staging
//!
//! A node that undo might remove (an inserted block, an added relation) would
//! come back under a new ID if it were created in place and the creation
//! undone. So the editing kernel **stages** such nodes: it creates them inside
//! the trash, outside the undo history, and a transaction's undoable step is
//! the move that puts them in place. Undo moves them back into the trash, redo
//! moves them out again, and the ID is the same throughout. See
//! [`Document::stage_block`].

use loro::{LoroMap, LoroText, LoroTree, LoroValue, TreeID, TreeParentId, ValueOrContainer};

use crate::edit::NewBlock;
use crate::{DocError, Document, NodeId, RelationId};

/// The metadata key that marks a trash root.
const TRASH_KEY: &str = "trash";

/// XORed into a peer ID to get the ID that creates that peer's trash roots.
const TRASH_PEER_BIT: u64 = 1 << 62;

/// The commit origin of changes the undo history must not record: creating a
/// trash root, and staging nodes inside it.
pub(crate) const STAGE_ORIGIN: &str = "reprise:stage";

/// How far up the tree liveness looks for a trash root. Loro trees can't
/// contain cycles, so the walk ends by itself; this only bounds the cost of a
/// hostile document that nests a million blocks, whose deepest nodes then
/// count as not live (37).
const MAX_DEPTH: usize = 1 << 16;

/// A tree of the document that has a trash.
#[derive(Clone, Copy)]
pub(crate) enum Store {
    Content,
    Relations,
}

impl Store {
    fn name(self) -> &'static str {
        match self {
            Store::Content => "content",
            Store::Relations => "relations",
        }
    }
}

fn is_trash_root(tree: &LoroTree, id: TreeID) -> bool {
    tree.get_meta(id)
        .ok()
        .and_then(|m| m.get(TRASH_KEY))
        .is_some_and(|v| matches!(v, ValueOrContainer::Value(LoroValue::Bool(true))))
}

impl Document {
    /// Whether `id` is in a trash: itself a trash root, or under one.
    pub(crate) fn in_trash(&self, tree: &LoroTree, id: TreeID) -> bool {
        let mut at = id;
        for _ in 0..MAX_DEPTH {
            match tree.parent(at) {
                Some(TreeParentId::Node(parent)) => at = parent,
                Some(TreeParentId::Root) => return is_trash_root(tree, at),
                // Tombstoned by Loro, or unknown: not alive either way.
                _ => return true,
            }
        }
        true
    }

    /// This peer's trash root in `store`, created (outside the undo history)
    /// if this peer has none yet.
    ///
    /// The root is created under a peer ID derived from this peer's
    /// ([`TRASH_PEER_BIT`]), not under the peer's own. That costs this peer no
    /// operation counters, so deleting a block consumes exactly the one
    /// counter that a plain Loro deletion did, and every block ID created
    /// after a deletion is what it always was. IDs appear in saved documents
    /// and snapshots, so creating the trash must not renumber the document.
    /// If the derived ID is already in use (another replica's own peer ID
    /// happens to be it), the root is created under this peer's own ID
    /// instead, and nothing else changes.
    pub(crate) fn trash_root(&self, store: Store) -> Result<TreeID, DocError> {
        let tree = self.tree(store.name());
        let peer = self.doc.peer_id();
        let alt = peer ^ TRASH_PEER_BIT;
        let found = tree.roots().into_iter().find(|&r| {
            (r.peer == peer || r.peer == alt)
                && is_trash_root(&tree, r)
                && !tree.is_node_deleted(&r).unwrap_or(true)
        });
        if let Some(root) = found {
            return Ok(root);
        }
        // Changes made so far are one step, and this is not part of it.
        self.commit();
        let derived = self.doc.oplog_vv().get(&alt).is_none() && self.doc.set_peer_id(alt).is_ok();
        self.doc.set_next_commit_origin(STAGE_ORIGIN);
        let created = tree.create(None).and_then(|root| {
            tree.get_meta(root)?.insert(TRASH_KEY, true)?;
            Ok(root)
        });
        self.doc.commit();
        if derived {
            self.doc.set_peer_id(peer)?;
        }
        Ok(created?)
    }

    /// Moves a node of `store` into this peer's trash.
    pub(crate) fn trash_node(&self, store: Store, id: TreeID) -> Result<(), DocError> {
        let tree = self.tree(store.name());
        let trash = self.trash_root(store)?;
        tree.mov(id, trash)?;
        Ok(())
    }

    /// Moves a block, with its subtree, into the trash. It is gone for every
    /// question the document answers, and `OnTargetDeleted` policies apply
    /// to relations that name it, exactly as for a deleted block.
    /// [`Document::restore_block`] or an undo brings it back **with the same
    /// ID**. See the module documentation for what happens concurrently.
    pub(crate) fn trash_block(&self, id: NodeId) -> Result<(), DocError> {
        if !self.is_live(id) {
            return Err(DocError::NoNode(id));
        }
        self.trash_node(Store::Content, id.0)
    }

    /// Moves a block out of the trash to `index` among the live children of
    /// `parent` (the top level for `None`). Errors with `NoNode` if the block
    /// is already live, or isn't in a trash at all.
    pub fn restore_block(
        &self,
        id: NodeId,
        parent: Option<NodeId>,
        index: usize,
    ) -> Result<(), DocError> {
        let tree = self.tree("content");
        if !tree.contains(id.0)
            || tree.is_node_deleted(&id.0).unwrap_or(true)
            || !self.in_trash(&tree, id.0)
            || is_trash_root(&tree, id.0)
        {
            return Err(DocError::NoNode(id));
        }
        self.place(&tree, id.0, parent, index)
    }

    /// Whether `id` is a block in the trash: deleted, and restorable.
    pub fn is_trashed(&self, id: NodeId) -> bool {
        let tree = self.tree("content");
        tree.contains(id.0)
            && !tree.is_node_deleted(&id.0).unwrap_or(true)
            && self.in_trash(&tree, id.0)
            && !is_trash_root(&tree, id.0)
    }

    /// Moves `id` to `index` among the live children of `parent`. The target
    /// parent must be live. `id` may already be a child of `parent` (a
    /// reorder), and the index then counts the siblings without `id`.
    pub(crate) fn place(
        &self,
        tree: &LoroTree,
        id: TreeID,
        parent: Option<NodeId>,
        index: usize,
    ) -> Result<(), DocError> {
        let loro_parent = match parent {
            None => TreeParentId::Root,
            Some(p) if self.live(tree, p.0) => TreeParentId::Node(p.0),
            Some(p) => return Err(DocError::NoNode(p)),
        };
        // The tree's own indices count its trash roots among the top-level
        // blocks, so position relative to the live siblings instead.
        let live: Vec<TreeID> = tree
            .children(loro_parent)
            .unwrap_or_default()
            .into_iter()
            .filter(|&s| s != id && self.live(tree, s))
            .collect();
        match (live.get(index), live.last()) {
            (Some(&next), _) => tree.mov_before(id, next)?,
            (None, Some(&last)) if index == live.len() => tree.mov_after(id, last)?,
            (None, None) if index == 0 => tree.mov(id, loro_parent)?,
            _ => {
                return Err(DocError::BadIndex {
                    index,
                    len: live.len(),
                });
            }
        }
        Ok(())
    }

    /// Creates a block **in the trash**, outside the undo history, ready to be
    /// placed with [`Document::restore_block`]. Used so that a transaction
    /// can insert it and undo and redo move the same node (see the module
    /// documentation). Commits what was pending first, so call it before a
    /// transaction's own changes, not in the middle of them.
    pub fn stage_block(&self, block: &NewBlock) -> Result<NodeId, DocError> {
        let tree = self.tree("content");
        let trash = self.trash_root(Store::Content)?;
        self.commit();
        self.doc.set_next_commit_origin(STAGE_ORIGIN);
        let id = tree.create(trash)?;
        let meta = tree.get_meta(id)?;
        meta.insert("kind", block.kind.as_str())?;
        meta.insert("style", block.style.as_str())?;
        let t = meta.insert_container("text", LoroText::new())?;
        t.insert_utf8(0, &block.text)?;
        if block.overrides != crate::Style::default() {
            let map = meta.insert_container("overrides", LoroMap::new())?;
            block.overrides.write(&map)?;
        }
        self.doc.commit();
        Ok(NodeId(id))
    }

    /// Moves a relation into the trash.
    pub(crate) fn trash_relation(&self, id: RelationId) -> Result<(), DocError> {
        let tree = self.tree("relations");
        if !self.live(&tree, id.0) {
            return Err(DocError::Store(format!("no live relation {id}")));
        }
        self.trash_node(Store::Relations, id.0)
    }

    /// Creates a relation in the trash, outside the undo history, ready to be
    /// placed with [`Document::restore_relation`]. Validates like
    /// [`Document::add_relation`].
    pub fn stage_relation(
        &self,
        schemas: &crate::SchemaRegistry,
        relation: &crate::Relation,
    ) -> Result<RelationId, DocError> {
        schemas.validate(relation)?;
        let tree = self.tree("relations");
        let trash = self.trash_root(Store::Relations)?;
        self.commit();
        self.doc.set_next_commit_origin(STAGE_ORIGIN);
        let id = tree.create(trash)?;
        let json = serde_json::to_string(relation).map_err(|e| DocError::Store(e.to_string()))?;
        tree.get_meta(id)?.insert("json", json)?;
        self.doc.commit();
        Ok(RelationId(id))
    }

    /// Moves a staged or deleted relation out of the trash, in force again
    /// with its old ID.
    pub fn restore_relation(&self, id: RelationId) -> Result<(), DocError> {
        let tree = self.tree("relations");
        if !tree.contains(id.0)
            || tree.is_node_deleted(&id.0).unwrap_or(true)
            || !self.in_trash(&tree, id.0)
            || is_trash_root(&tree, id.0)
        {
            return Err(DocError::Store(format!("no trashed relation {id}")));
        }
        tree.mov(id.0, TreeParentId::Root)?;
        Ok(())
    }
}
