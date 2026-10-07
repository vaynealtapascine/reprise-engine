//! Structural edits and the undo history (07, 29): the document operations
//! the editing kernel (`reprise-edit`) and a later paste build on.
//!
//! These are plain operations on the document. None of them commits, so
//! several can be one atomic step: call [`Document::commit_step`] when the
//! step is complete. (The staging functions are the exception, see their
//! documentation.) Validation of whole transactions lives in the kernel; the
//! checks here only keep a single operation from corrupting the tree, and
//! report what they refuse as a [`DocError`], never a panic (37).
//!
//! # Identity under undo
//!
//! A block that undo might remove is **staged** first ([`Document::stage_block`]):
//! created with a deletion flag outside the undo history. The undoable step
//! places it and clears that flag. Undo and redo retain its ID. See `lifecycle.rs`.

use loro::UndoManager;

use crate::lifecycle::STAGE_ORIGIN;
use crate::{BlockKind, DocError, Document, NodeId, Style};

/// The commit origin of a finished editing step.
const STEP_ORIGIN: &str = "reprise:step";

/// How many steps an [`UndoStack`] keeps unless told otherwise.
pub const DEFAULT_UNDO_STEPS: usize = 1000;

/// A block to be created: everything the document stores about one.
#[derive(Clone, Debug, PartialEq)]
pub struct NewBlock {
    pub kind: BlockKind,
    /// The name of its style.
    pub style: String,
    /// Its direct overrides.
    pub overrides: Style,
    pub text: String,
}

impl NewBlock {
    pub fn new(kind: BlockKind, style: &str, text: &str) -> NewBlock {
        NewBlock {
            kind,
            style: style.to_owned(),
            overrides: Style::default(),
            text: text.to_owned(),
        }
    }
}

impl Document {
    /// Ends the current editing step: everything done since the last commit
    /// is one undo step and one Loro commit.
    pub fn commit_step(&self) {
        self.doc.set_next_commit_origin(STEP_ORIGIN);
        self.doc.commit();
        self.commit_lineage();
    }

    /// An undo history for this replica's edits (29). It records the edits
    /// this peer commits from now on, and never anyone else's, so undoing
    /// keeps collaborators' concurrent work.
    pub fn undo_stack(&self) -> UndoStack {
        self.commit();
        let mut inner = UndoManager::new(&self.doc);
        // Steps are decided by commits, not by the clock (purity).
        inner.set_merge_interval(0);
        inner.set_max_undo_steps(DEFAULT_UNDO_STEPS);
        // Staging is how identity survives undo, so it is never undone.
        inner.add_exclude_origin_prefix(STAGE_ORIGIN);
        UndoStack {
            inner,
            doc: self.doc.clone(),
        }
    }

    /// Creates a block at `index` among the live children of `parent` (the
    /// top level for `None`). It is staged first, so undoing and redoing the
    /// insertion keeps its ID.
    ///
    /// Staging commits what was pending, so call it before the changes of the
    /// step it belongs to, or use it alone.
    pub fn insert_block_at(
        &self,
        parent: Option<NodeId>,
        index: usize,
        block: &NewBlock,
    ) -> Result<NodeId, DocError> {
        self.check_insertion(parent, index)?;
        let id = self.stage_block(block)?;
        self.activate_block_at(id, parent, index)?;
        Ok(id)
    }

    fn check_insertion(&self, parent: Option<NodeId>, index: usize) -> Result<(), DocError> {
        if let Some(p) = parent
            && !self.is_live(p)
        {
            return Err(DocError::NoNode(p));
        }
        let len = self.children(parent).len();
        if index > len {
            return Err(DocError::BadIndex { index, len });
        }
        Ok(())
    }

    /// Moves a live block, with its subtree, to `index` among the live
    /// children of `parent`. The index counts the siblings without the block,
    /// so it is the block's index afterwards. A block can't move into itself
    /// or its own subtree.
    pub fn move_block(
        &self,
        id: NodeId,
        parent: Option<NodeId>,
        index: usize,
    ) -> Result<(), DocError> {
        if !self.is_live(id) {
            return Err(DocError::NoNode(id));
        }
        let mut up = parent;
        while let Some(p) = up {
            if p == id {
                return Err(DocError::Malformed(id, "can't move a block into itself"));
            }
            up = self.parent_of(p).flatten();
        }
        let tree = self.tree("content");
        self.place(&tree, id.0, parent, index)
    }

    /// Splits a block at byte `at`: the text from `at` on moves to a new
    /// block just after it, with the same kind, style and overrides.
    /// Returns the new block. The original keeps its ID and the text before
    /// `at`. See [`Document::split_block_into`] for what happens to ranges.
    pub fn split_block(&self, id: NodeId, at: usize) -> Result<NodeId, DocError> {
        let block = self.block(id)?;
        let tail = block.text.slice(at..block.text.len())?;
        let new = self.stage_block(&NewBlock {
            kind: block.kind,
            style: block.style.unwrap_or_default(),
            overrides: block.overrides,
            text: tail,
        })?;
        self.split_block_into(id, at, new)?;
        Ok(new)
    }

    /// The undoable half of a split: `new`, staged with the tail of `id`'s
    /// text, takes its place after `id`, and the tail leaves `id`.
    ///
    /// The tail is recreated in `new`; compact authored character lineage
    /// lets stable carets follow it on every replica. Ranges still keep their
    /// authored block and report rebound/missing (12).
    pub fn split_block_into(&self, id: NodeId, at: usize, new: NodeId) -> Result<(), DocError> {
        let block = self.block(id)?;
        if !self.is_soft_deleted(new) {
            return Err(DocError::NoNode(new));
        }
        let end = block.text.len();
        if at > end {
            return Err(reprise_text::TextError::BadOffset(at).into());
        }
        let parent = self.parent_of(id).flatten();
        let index = self
            .children(parent)
            .iter()
            .position(|&c| c == id)
            .ok_or(DocError::NoNode(id))?;
        self.record_transfer(id, at, new, 0, end - at)?;
        block.text.delete(at..end)?;
        self.activate_block_at(new, parent, index + 1)
    }

    /// Joins `second` onto the end of `first`: its text is appended, and it
    /// is deleted. `second` is recorded as superseded by `first`, so relations
    /// that targeted it follow the text (15). The blocks must have the same
    /// kind, and `second` must have no children.
    pub fn join_blocks(&self, first: NodeId, second: NodeId) -> Result<(), DocError> {
        if first == second {
            return Err(DocError::Malformed(first, "can't join a block to itself"));
        }
        let (a, b) = (self.block(first)?, self.block(second)?);
        if a.kind != b.kind {
            return Err(DocError::Malformed(
                second,
                "different kinds can't be joined",
            ));
        }
        if !self.children(Some(second)).is_empty() {
            return Err(DocError::Malformed(
                second,
                "a block with children can't be joined",
            ));
        }
        let at = a.text.len();
        a.text.insert(at, &b.text.to_string())?;
        self.record_transfer(second, 0, first, at, b.text.len())?;
        self.supersede(second, first)?;
        self.soft_delete_block(second)
    }
}

/// This replica's undo and redo history (29).
///
/// It undoes only what this peer committed, by applying inverse operations on
/// top of the current state. Everything collaborators did meanwhile stays, and
/// text edits are transformed around it. Because deletion changes a flag,
/// undoing a deleted block reveals the **same node** again, so its
/// relations, ranges and anchors work again.
pub struct UndoStack {
    inner: UndoManager,
    doc: loro::LoroDoc,
}

impl UndoStack {
    /// Undoes the last step. `false` when there was nothing to undo.
    pub fn undo(&mut self) -> Result<bool, DocError> {
        let changed = self.inner.undo()?;
        if changed {
            Document::wrap(self.doc.clone()).capture_restored_lineage();
        }
        Ok(changed)
    }

    /// Redoes the last undone step. `false` when there was nothing to redo.
    pub fn redo(&mut self) -> Result<bool, DocError> {
        let changed = self.inner.redo()?;
        if changed {
            Document::wrap(self.doc.clone()).capture_restored_lineage();
        }
        Ok(changed)
    }

    pub fn can_undo(&self) -> bool {
        self.inner.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.inner.can_redo()
    }

    pub fn undo_count(&self) -> usize {
        self.inner.undo_count()
    }

    pub fn redo_count(&self) -> usize {
        self.inner.redo_count()
    }

    /// Forgets all steps.
    pub fn clear(&self) {
        self.inner.clear();
    }

    /// Keeps at most `steps` undo steps.
    pub fn set_limit(&mut self, steps: usize) {
        self.inner.set_max_undo_steps(steps);
    }
}
