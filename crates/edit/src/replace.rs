//! Replacing a stable selection with a fragment in one transaction (29, 35).
//!
//! Pasting over a selection used to be two transactions: insert, then delete.
//! A remote edit could land between them and the user had two undo steps. Here
//! the selection is resolved *inside* the transaction, the deletion and the
//! paste are written together, and one `commit_step` ends the undo step.
//!
//! The work is split in two so the property can be tested and so a bad
//! clipboard never erases text:
//!
//! 1.  [`Editor::stage_replacement`] validates everything that doesn't depend
//!     on where the text goes. It writes nothing.
//! 2.  [`Editor::replace_selection`] resolves the selection in the document as
//!     it is *now*, validates the deletion and a dry paste at the start of the
//!     range, and only then writes.
//!
//! The range is taken in document order (preorder of the content tree), which
//! needs no layout. Reading-order overrides are a layout notion and are not
//! consulted.

use std::collections::{BTreeMap, BTreeSet};

use reprise_diag::Note;
use reprise_doc::fragment::{Fragment, FragmentError};
use reprise_doc::{Document, NodeId};

use crate::command::Command;
use crate::editor::{Applied, Editor};
use crate::error::{EditError, Reason};
use crate::plan::{self, MAX_ANCESTORS, MAX_TRANSACTION_BYTES};
use crate::stable::StableSelection;

/// The most blocks a replacement orders. Past it the selection is refused.
const MAX_ORDERED_BLOCKS: usize = 200_000;

/// A validated fragment, ready to replace a selection.
#[derive(Clone, Debug)]
pub struct StagedReplacement {
    fragment: Fragment,
    target: String,
}

impl StagedReplacement {
    pub fn fragment(&self) -> &Fragment {
        &self.fragment
    }
}

/// What a replacement did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Replaced {
    /// The deletion's effects, then the paste's, in order, so one
    /// [`Applied::map_position`] follows a caret through both.
    pub applied: Applied,
    pub notes: Vec<Note>,
    /// Where a caret goes: the end of the inserted content, or, when the
    /// fragment had no blocks, the start of the replaced range.
    pub caret: Option<(NodeId, usize)>,
}

fn invalid(error: FragmentError) -> EditError {
    EditError {
        command: None,
        reason: Reason::Fragment(error),
    }
}

/// Live caret blocks in document order, with their rank.
fn order(doc: &Document) -> Result<(Vec<NodeId>, BTreeMap<NodeId, usize>), EditError> {
    let mut nodes = Vec::new();
    let mut rank = BTreeMap::new();
    let mut stack: Vec<NodeId> = doc.blocks();
    stack.reverse();
    let mut seen = BTreeSet::new();
    while let Some(node) = stack.pop() {
        if !seen.insert(node) {
            continue;
        }
        if seen.len() > MAX_ORDERED_BLOCKS {
            return Err(invalid(FragmentError::Limit("selection blocks")));
        }
        if doc.is_caret_block(node) {
            rank.insert(node, nodes.len());
            nodes.push(node);
        }
        stack.extend(doc.children(Some(node)).into_iter().rev());
    }
    Ok((nodes, rank))
}

fn ancestors(doc: &Document, node: NodeId) -> Result<BTreeSet<NodeId>, EditError> {
    let mut out = BTreeSet::new();
    let mut up = doc.parent_of(node).flatten();
    for _ in 0..MAX_ANCESTORS {
        let Some(p) = up else { return Ok(out) };
        out.insert(p);
        up = doc.parent_of(p).flatten();
    }
    Err(EditError {
        command: None,
        reason: Reason::TreeDepthLimit {
            node,
            max: MAX_ANCESTORS,
        },
    })
}

fn len_of(doc: &Document, node: NodeId) -> Result<usize, EditError> {
    doc.block(node)
        .map(|b| b.text.len())
        .map_err(|_| EditError {
            command: None,
            reason: Reason::NoSuchBlock(node),
        })
}

/// The commands that delete `(start, end)`, in three fallback strengths: whole
/// blocks removed and the ends joined, whole blocks removed, and finally only
/// text removed. A later one is used when the kernel refuses an earlier one
/// (for example a table in the middle, or two blocks of different kinds).
fn deletions(
    doc: &Document,
    nodes: &[NodeId],
    rank: &BTreeMap<NodeId, usize>,
    start: (NodeId, usize),
    end: (NodeId, usize),
) -> Result<Vec<Vec<Command>>, EditError> {
    let (sn, so) = start;
    let (en, eo) = end;
    if sn == en {
        return Ok(vec![if so < eo {
            vec![Command::DeleteText {
                node: sn,
                range: so..eo,
            }]
        } else {
            Vec::new()
        }]);
    }
    let (Some(&a), Some(&b)) = (rank.get(&sn), rank.get(&en)) else {
        return Err(EditError {
            command: None,
            reason: Reason::SelectionGone,
        });
    };
    let mut ends = ancestors(doc, sn)?;
    ends.extend(ancestors(doc, en)?);
    let mut head = Vec::new();
    let slen = len_of(doc, sn)?;
    if so < slen {
        head.push(Command::DeleteText {
            node: sn,
            range: so..slen,
        });
    }
    if eo > 0 {
        head.push(Command::DeleteText {
            node: en,
            range: 0..eo,
        });
    }
    let middle = nodes.get(a.saturating_add(1)..b).unwrap_or_default();
    let mut removed = BTreeSet::new();
    let mut blocks = Vec::new();
    let mut texts = Vec::new();
    for &m in middle {
        let under_removed = ancestors(doc, m)?.iter().any(|p| removed.contains(p));
        if under_removed {
            continue;
        }
        let len = len_of(doc, m)?;
        if len > 0 {
            texts.push(Command::DeleteText {
                node: m,
                range: 0..len,
            });
        }
        // An ancestor of an end must survive with its subtree, so only its
        // text goes.
        if ends.contains(&m) {
            blocks.push(Command::DeleteText {
                node: m,
                range: 0..len,
            });
        } else {
            removed.insert(m);
            blocks.push(Command::DeleteBlock { node: m });
        }
    }
    // Empty-range text deletions are refused by the kernel: drop them.
    blocks.retain(|c| !matches!(c, Command::DeleteText { range, .. } if range.is_empty()));
    let join = Command::JoinBlocks {
        first: sn,
        second: en,
    };
    let mut a1 = head.clone();
    a1.extend(blocks.clone());
    a1.push(join);
    let mut a2 = head.clone();
    a2.extend(blocks);
    let mut a3 = head;
    a3.extend(texts);
    Ok(vec![a1, a2, a3])
}

impl Editor {
    /// Validates `fragment` for use by [`Editor::replace_selection`]. Writes
    /// nothing, and a refusal here means the selection is untouched.
    pub fn stage_replacement(
        &self,
        fragment: &Fragment,
        target_namespace: &str,
    ) -> Result<StagedReplacement, EditError> {
        // Everything that doesn't depend on the caret: validation, style
        // renaming, relation policy, size.
        crate::paste::prepare(&self.doc, &self.schemas, fragment, None, target_namespace)?;
        Ok(StagedReplacement {
            fragment: fragment.clone(),
            target: target_namespace.into(),
        })
    }

    /// Replaces `selection`, resolved in the document as it is now, with the
    /// staged fragment: one transaction and one undo step. An error before any
    /// write leaves the document untouched; `Store` after one means the kernel
    /// refused what it had validated (a bug), and what was written is one step.
    pub fn replace_selection(
        &mut self,
        selection: &StableSelection,
        staged: &StagedReplacement,
    ) -> Result<Replaced, EditError> {
        let (resolved, _) = selection.resolve(&self.doc).ok_or(EditError {
            command: None,
            reason: Reason::SelectionGone,
        })?;
        let (nodes, rank) = order(&self.doc)?;
        let key = |c: &crate::Caret| {
            rank.get(&c.node)
                .map(|&r| (r, c.offset, c.node))
                .ok_or(EditError {
                    command: None,
                    reason: Reason::SelectionGone,
                })
        };
        let (a, f) = (key(&resolved.anchor)?, key(&resolved.focus)?);
        let (start, end) = if a <= f {
            (resolved.anchor, resolved.focus)
        } else {
            (resolved.focus, resolved.anchor)
        };
        let (start, end) = ((start.node, start.offset), (end.node, end.offset));
        // Bound the text kept around the paste before anything is written.
        let around = len_of(&self.doc, start.0)?.saturating_add(len_of(&self.doc, end.0)?);
        let inserted: usize = staged.fragment.blocks.iter().map(|b| b.text.len()).sum();
        if around.saturating_add(inserted) > MAX_TRANSACTION_BYTES {
            return Err(invalid(FragmentError::Limit("paste text bytes")));
        }
        // A dry paste at the start of the range checks the caret-dependent
        // refusals (children, depth, format capacity) while nothing is written.
        crate::paste::prepare(
            &self.doc,
            &self.schemas,
            &staged.fragment,
            Some(start),
            &staged.target,
        )?;
        let mut chosen = None;
        let mut first_error = None;
        for commands in deletions(&self.doc, &nodes, &rank, start, end)? {
            match plan::plan(&self.doc, &self.schemas, &commands) {
                Ok(p) => {
                    chosen = Some(p);
                    break;
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        let deletion = match (chosen, first_error) {
            (Some(p), _) => p,
            (None, Some(e)) => return Err(e),
            (None, None) => plan::Plan::default(),
        };
        // Nothing is refused from here on, by validation.
        let mut result = Replaced::default();
        let written = (|| {
            if !deletion.steps.is_empty() {
                result.applied = self.write(deletion)?;
            }
            let prepared = crate::paste::prepare(
                &self.doc,
                &self.schemas,
                &staged.fragment,
                Some(start),
                &staged.target,
            )?;
            let pasted = crate::paste::write(&self.doc, &self.schemas, prepared)?;
            result.applied.blocks.extend(pasted.applied.blocks);
            result.applied.relations.extend(pasted.applied.relations);
            result.applied.effects.extend(pasted.applied.effects);
            result.notes = pasted.notes;
            result.caret = pasted.end.or(Some(start));
            Ok(())
        })();
        // Whatever happened, what was written is one step.
        self.doc.commit_step();
        written.map(|()| result)
    }
}
