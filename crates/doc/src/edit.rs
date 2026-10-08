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

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use loro::{UndoItemMeta, UndoManager, UndoOrRedo};

use crate::lifecycle::STAGE_ORIGIN;
use crate::{BlockKind, DocError, Document, NodeId, Style};

/// The commit origin of a finished editing step.
const STEP_ORIGIN: &str = "reprise:step";

/// How many steps an [`UndoStack`] keeps unless told otherwise.
pub const DEFAULT_UNDO_STEPS: usize = 1000;

/// Underlying CRDT items retained. An oversized editing step is forgotten
/// whole rather than becoming partially undoable.
const MAX_UNDO_ITEMS: usize = 16_384;

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

/// Which editing step this replica's commits belong to. A step is usually
/// one commit, but a step that stages breaks or embeds (see `docs/flow.md`)
/// is several: staging commits what came before it, and its own commit is
/// left out of the undo history. [`UndoStack`] undoes whole steps.
#[derive(Debug, Default)]
pub(crate) struct StepLog {
    serial: AtomicI64,
}

impl StepLog {
    fn current(&self) -> i64 {
        self.serial.load(Ordering::Relaxed)
    }

    pub(crate) fn end(&self) {
        self.serial.fetch_add(1, Ordering::Relaxed);
    }
}

impl Document {
    /// Ends the current editing step: everything done since the last step
    /// ended is one undo step.
    pub fn commit_step(&self) {
        self.doc.set_next_commit_origin(STEP_ORIGIN);
        self.doc.commit();
        self.commit_lineage();
        self.steps.end();
    }

    /// Commits what is pending as part of the current step, without ending
    /// it. Staging calls this before its own commit.
    pub(crate) fn commit_part(&self) {
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
        inner.set_max_undo_steps(MAX_UNDO_ITEMS);
        // Staging is how identity survives undo, so it is never undone.
        inner.add_exclude_origin_prefix(STAGE_ORIGIN);
        let log = Arc::new(Mutex::new(Log {
            limit: DEFAULT_UNDO_STEPS,
            ..Log::default()
        }));
        let steps = self.steps.clone();
        let pushed = log.clone();
        inner.set_on_push(Some(Box::new(move |kind, _, _| {
            let mut log = pushed.lock().unwrap_or_else(|e| e.into_inner());
            if !log.processing && kind == UndoOrRedo::Undo {
                log.pushed(steps.current());
            }
            UndoItemMeta::new()
        })));
        UndoStack {
            inner,
            doc: self.doc.clone(),
            log,
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
        self.check_movable(id)?;
        let placement = self.placement(parent, index, Some(id))?;
        self.check_not_inside(id, placement)?;
        self.put(id, placement)
    }

    /// A paragraph of a flow other than a lone head moves by copying, which
    /// the editing kernel does; only whole content-tree nodes move here.
    pub(crate) fn check_movable(&self, id: NodeId) -> Result<(), DocError> {
        if id.is_break() || self.flow(id.node).is_some_and(|f| f.has_more_than_head()) {
            return Err(DocError::Malformed(
                id,
                "a paragraph of a flow moves by copying",
            ));
        }
        Ok(())
    }

    /// Refuses a placement whose tree parent is `id` or inside it.
    pub(crate) fn check_not_inside(
        &self,
        id: NodeId,
        placement: crate::flow::Placement,
    ) -> Result<(), DocError> {
        let tree = self.tree("content");
        let mut up = match placement {
            crate::flow::Placement::Tree {
                parent: loro::TreeParentId::Node(p),
                ..
            } => Some(p),
            crate::flow::Placement::Embed { host, .. } => Some(host),
            _ => None,
        };
        // Loro's tree is acyclic, so this climbs to the root and ends.
        while let Some(p) = up {
            if p == id.node {
                return Err(DocError::Malformed(id, "can't move a block into itself"));
            }
            up = match tree.parent(p) {
                Some(loro::TreeParentId::Node(n)) => Some(n),
                _ => None,
            };
        }
        Ok(())
    }

    /// Splits a block at byte `at`: the text from `at` on becomes a new
    /// block just after it, with the same kind, style and overrides.
    /// Returns the new block. The original keeps its ID and the text before
    /// `at`.
    ///
    /// A paragraph of a flow (see `docs/flow.md`) is split with a break: no
    /// text moves, so collaborators' concurrent edits stay where they were
    /// made. A split inside text inherited before its marker after head
    /// deletion uses the copying path instead (see `split_copies_text`).
    /// The break is staged, which commits what was pending as part of
    /// the current step. Other blocks are split by copying, see
    /// [`Document::split_block_into`].
    pub fn split_block(&self, id: NodeId, at: usize) -> Result<NodeId, DocError> {
        if !self.split_copies_text(id, at)? {
            return self.split_flow(id, at);
        }
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

    /// Joins `second` onto the end of `first`. When `second` is the
    /// paragraph after `first` in one flow, its break is dropped and no text
    /// moves. Otherwise its text is copied onto `first`, and it is deleted. `second` is recorded as superseded by `first`, so relations
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
        // Adjacent paragraphs of one flow: drop the break. No text moves.
        if self.flow_predecessor(second) == Some(first) {
            self.set_record_active(second, false)?;
            return self.supersede(second, first);
        }
        let at = a.text.len();
        a.text.insert(at, &b.text.to_string())?;
        self.record_transfer(second, 0, first, at, b.text.len())?;
        self.supersede(second, first)?;
        self.delete_block(second)
    }
}

/// The steps an [`UndoStack`] can undo and redo: each step's serial and how
/// many Loro undo items it is, newest last.
#[derive(Debug, Default)]
struct Log {
    undo: Vec<(i64, usize)>,
    redo: Vec<(i64, usize)>,
    /// Set while undoing or redoing, whose own items are counted here.
    processing: bool,
    limit: usize,
}

impl Log {
    fn pushed(&mut self, serial: i64) {
        match self.undo.last_mut() {
            Some((s, n)) if *s == serial => *n += 1,
            _ => self.undo.push((serial, 1)),
        }
        self.redo.clear();
        if self.undo.len() > self.limit {
            let excess = self.undo.len() - self.limit;
            self.undo.drain(..excess);
        }
    }
}

/// This replica's undo and redo history (29).
///
/// It undoes only what this peer committed, by applying inverse operations on
/// top of the current state. Everything collaborators did meanwhile stays, and
/// text edits are transformed around it. Because deletion changes a flag,
/// undoing a deleted block reveals the **same node** again, so its
/// relations, ranges and anchors work again.
///
/// One undo is one editing step (see [`Document::commit_step`]), even when
/// the step is several commits because it staged breaks or embeds.
pub struct UndoStack {
    inner: UndoManager,
    doc: loro::LoroDoc,
    log: Arc<Mutex<Log>>,
}

impl UndoStack {
    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Loro evicts individual items, whereas our public history consists of
    /// complete editing steps. Remove any group whose oldest item was evicted.
    fn prune(&self) {
        let available = self.inner.undo_count();
        let mut log = self.log();
        let mut items: usize = log.undo.iter().map(|(_, n)| n).sum();
        let mut drop = 0;
        while items > available && drop < log.undo.len() {
            items -= log.undo[drop].1;
            drop += 1;
        }
        log.undo.drain(..drop);
    }

    /// Undoes the last step. `false` when there was nothing to undo.
    pub fn undo(&mut self) -> Result<bool, DocError> {
        self.prune();
        let Some((serial, items)) = self.log().undo.pop() else {
            return Ok(false);
        };
        let done = self.replay(items, true)?;
        if done > 0 {
            self.log().redo.push((serial, done));
            Document::wrap(self.doc.clone()).capture_restored_lineage();
        }
        Ok(done > 0)
    }

    /// Redoes the last undone step. `false` when there was nothing to redo.
    pub fn redo(&mut self) -> Result<bool, DocError> {
        let Some((serial, items)) = self.log().redo.pop() else {
            return Ok(false);
        };
        let done = self.replay(items, false)?;
        if done > 0 {
            self.log().undo.push((serial, done));
            Document::wrap(self.doc.clone()).capture_restored_lineage();
        }
        Ok(done > 0)
    }

    /// Undoes or redoes `items` Loro items: the parts of one step.
    fn replay(&mut self, items: usize, undo: bool) -> Result<usize, DocError> {
        self.log().processing = true;
        let mut done = 0;
        let mut result = Ok(());
        for _ in 0..items {
            let step = if undo {
                self.inner.undo()
            } else {
                self.inner.redo()
            };
            match step {
                Ok(true) => done += 1,
                Ok(false) => break,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.log().processing = false;
        result?;
        Ok(done)
    }

    pub fn can_undo(&self) -> bool {
        self.prune();
        !self.log().undo.is_empty() && self.inner.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        !self.log().redo.is_empty() && self.inner.can_redo()
    }

    pub fn undo_count(&self) -> usize {
        self.prune();
        self.log().undo.len()
    }

    pub fn redo_count(&self) -> usize {
        self.log().redo.len()
    }

    /// Forgets all steps.
    pub fn clear(&self) {
        self.inner.clear();
        let mut log = self.log();
        log.undo.clear();
        log.redo.clear();
    }

    /// Keeps at most `steps` undo steps.
    pub fn set_limit(&mut self, steps: usize) {
        let mut log = self.log();
        log.limit = steps;
        if log.undo.len() > steps {
            let excess = log.undo.len() - steps;
            log.undo.drain(..excess);
        }
    }
}
