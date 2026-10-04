//! Soft deletion (07, 29): an authored flag on the original node.
//!
//! Deletion never moves or physically deletes a node. Its position, metadata,
//! text and identity survive; undo restores the flag's previous value. A block
//! inherits deletion from every ancestor. Restoring a parent reveals its
//! descendants except those independently flagged or physically deleted.
//!
//! Concurrent delete/restore writes use Loro map last-writer-wins (Lamport
//! timestamp, then peer ID). Text edits never change the flag and cannot revive
//! a deleted block. Concurrent moves change position, not deletion state.
//! Physical tree tombstones remain deleted. Purging soft tombstones is a later
//! compaction policy, requiring agreement from replicas and reference retention.
//!
//! New blocks/relations are staged with `deleted = true` outside undo history.
//! Their activation is undoable, so undo/redo preserves inserted IDs too. All
//! operations use the real peer; there are no reserved or synthetic identities.

use loro::{LoroMap, LoroText, LoroTree, LoroValue, TreeID, TreeParentId, ValueOrContainer};

use crate::{DocError, Document, NewBlock, NodeId, RelationId};

const DELETED: &str = "deleted";
pub(crate) const STAGE_ORIGIN: &str = "reprise:stage";

fn flagged(tree: &LoroTree, id: TreeID) -> bool {
    tree.get_meta(id)
        .ok()
        .and_then(|m| m.get(DELETED))
        .is_some_and(|v| matches!(v, ValueOrContainer::Value(LoroValue::Bool(true))))
}

impl Document {
    pub(crate) fn deleted_in_tree(&self, tree: &LoroTree, id: TreeID) -> bool {
        let mut at = id;
        // Loro guarantees an acyclic tree: every iteration ascends one parent,
        // and the maximum number of iterations is the stored tree's height.
        loop {
            if flagged(tree, at) {
                return true;
            }
            match tree.parent(at) {
                Some(TreeParentId::Node(parent)) => at = parent,
                Some(TreeParentId::Root) => return false,
                _ => return true,
            }
        }
    }

    pub(crate) fn soft_delete_block(&self, id: NodeId) -> Result<(), DocError> {
        if !self.is_live(id) {
            return Err(DocError::NoNode(id));
        }
        self.tree("content").get_meta(id.0)?.insert(DELETED, true)?;
        Ok(())
    }

    /// Whether this node itself carries a soft tombstone. Descendants of a
    /// flagged parent are not live but do not acquire their own flag.
    pub fn is_soft_deleted(&self, id: NodeId) -> bool {
        let tree = self.tree("content");
        tree.contains(id.0) && !tree.is_node_deleted(&id.0).unwrap_or(true) && flagged(&tree, id.0)
    }

    /// Clears this block's flag in place. Its ancestors must be live. Does not
    /// commit, reposition, or restore a physical Loro tree tombstone.
    pub fn restore_block(&self, id: NodeId) -> Result<(), DocError> {
        let tree = self.tree("content");
        if !self.is_soft_deleted(id) {
            return Err(DocError::NoNode(id));
        }
        if let Some(TreeParentId::Node(parent)) = tree.parent(id.0)
            && !self.live(&tree, parent)
        {
            return Err(DocError::NoNode(NodeId(parent)));
        }
        tree.get_meta(id.0)?.insert(DELETED, false)?;
        Ok(())
    }

    /// Places a staged block at a live-child index and activates it. Neither
    /// part commits; the move and flag clear belong to the caller's step.
    pub fn activate_block_at(
        &self,
        id: NodeId,
        parent: Option<NodeId>,
        index: usize,
    ) -> Result<(), DocError> {
        if !self.is_soft_deleted(id) {
            return Err(DocError::NoNode(id));
        }
        let tree = self.tree("content");
        self.place(&tree, id.0, parent, index)?;
        self.restore_block(id)
    }

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
        let siblings: Vec<TreeID> = tree
            .children(loro_parent)
            .unwrap_or_default()
            .into_iter()
            .filter(|&s| s != id && self.live(tree, s))
            .collect();
        match (siblings.get(index), siblings.last()) {
            (Some(&next), _) => tree.mov_before(id, next)?,
            (None, Some(&last)) if index == siblings.len() => tree.mov_after(id, last)?,
            (None, None) if index == 0 => tree.mov(id, loro_parent)?,
            _ => {
                return Err(DocError::BadIndex {
                    index,
                    len: siblings.len(),
                });
            }
        }
        Ok(())
    }

    /// Creates an invisible flagged block outside undo history. Commits pending
    /// changes first; call before the transaction's own undoable operations.
    pub fn stage_block(&self, block: &NewBlock) -> Result<NodeId, DocError> {
        self.commit();
        self.doc.set_next_commit_origin(STAGE_ORIGIN);
        let tree = self.tree("content");
        let id = tree.create(None)?;
        let meta = tree.get_meta(id)?;
        meta.insert(DELETED, true)?;
        meta.insert("kind", block.kind.as_str())?;
        meta.insert("style", block.style.as_str())?;
        meta.insert_container("text", LoroText::new())?
            .insert_utf8(0, &block.text)?;
        if block.overrides != crate::Style::default() {
            block
                .overrides
                .write(&meta.insert_container("overrides", LoroMap::new())?)?;
        }
        self.doc.commit();
        Ok(NodeId(id))
    }

    pub(crate) fn soft_delete_relation(&self, id: RelationId) -> Result<(), DocError> {
        let tree = self.tree("relations");
        if !self.live(&tree, id.0) {
            return Err(DocError::Store(format!("no live relation {id}")));
        }
        tree.get_meta(id.0)?.insert(DELETED, true)?;
        Ok(())
    }

    /// Validates and stages a flagged relation outside undo history.
    pub fn stage_relation(
        &self,
        schemas: &crate::SchemaRegistry,
        relation: &crate::Relation,
    ) -> Result<RelationId, DocError> {
        schemas.validate(relation)?;
        let json = serde_json::to_string(relation).map_err(|e| DocError::Store(e.to_string()))?;
        self.commit();
        self.doc.set_next_commit_origin(STAGE_ORIGIN);
        let tree = self.tree("relations");
        let id = tree.create(None)?;
        let meta = tree.get_meta(id)?;
        meta.insert(DELETED, true)?;
        meta.insert("json", json)?;
        self.doc.commit();
        Ok(RelationId(id))
    }

    /// Clears a staged/deleted relation's flag without changing its ID.
    pub fn restore_relation(&self, id: RelationId) -> Result<(), DocError> {
        let tree = self.tree("relations");
        if !tree.contains(id.0)
            || tree.is_node_deleted(&id.0).unwrap_or(true)
            || !flagged(&tree, id.0)
        {
            return Err(DocError::Store(format!("no soft-deleted relation {id}")));
        }
        tree.get_meta(id.0)?.insert(DELETED, false)?;
        Ok(())
    }
}
