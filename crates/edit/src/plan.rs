//! Validating a transaction before anything is written (29).
//!
//! [`plan`] runs the commands against a small model of the parts of the
//! document they touch: the text of the blocks they edit, the children of the
//! parents they change, the blocks they delete. Every check a command needs is
//! made against the model's state *at that point in the transaction*, so a
//! command sees the effect of those before it. If any command is refused, the
//! whole transaction is, and the document was never touched.
//!
//! What comes out is a [`Plan`]: the blocks and relations to stage, and the
//! low-level steps to apply in order. The model and the real document apply
//! the same steps in the same order, so a plan that validated also applies.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use reprise_doc::relation::QueryAnchor;
use reprise_doc::{
    DocError, Document, NewBlock, NodeId, RangeState, Relation, RelationId, SchemaRegistry, Style,
    Target,
};

use crate::command::Command;
use crate::error::{EditError, Reason};

/// The most commands one transaction may hold.
pub const MAX_COMMANDS: usize = 10_000;
/// The most text one `InsertText` or `InsertBlock` may carry, in bytes.
pub const MAX_INSERT_BYTES: usize = 16 << 20;

/// How far up the tree the model looks for a deleted ancestor: the same
/// bound the document uses for liveness.
const MAX_DEPTH: usize = 1 << 16;

#[derive(Debug)]
pub(crate) enum Step {
    InsertText {
        node: NodeId,
        at: usize,
        text: String,
    },
    DeleteText {
        node: NodeId,
        range: Range<usize>,
    },
    /// `new` indexes [`Plan::blocks`].
    Split {
        node: NodeId,
        at: usize,
        new: usize,
    },
    Join {
        first: NodeId,
        second: NodeId,
    },
    Place {
        new: usize,
        parent: Option<NodeId>,
        index: usize,
    },
    Trash {
        node: NodeId,
    },
    Move {
        node: NodeId,
        parent: Option<NodeId>,
        index: usize,
    },
    SetOverrides {
        node: NodeId,
        style: Style,
    },
    /// `new` indexes [`Plan::relations`].
    AddRelation {
        new: usize,
    },
    RemoveRelation {
        id: RelationId,
    },
}

#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// Blocks to stage first, in order.
    pub blocks: Vec<NewBlock>,
    pub relations: Vec<Relation>,
    pub steps: Vec<Step>,
    /// The commands' effects, with staged blocks as indices; the editor
    /// resolves them once those have IDs.
    pub effects: Vec<PlannedEffect>,
}

#[derive(Debug)]
pub(crate) enum PlannedEffect {
    Text {
        node: NodeId,
        at: usize,
        removed: usize,
        inserted: usize,
    },
    Split {
        node: NodeId,
        at: usize,
        new: usize,
    },
    Join {
        first: NodeId,
        second: NodeId,
        at: usize,
    },
    Deleted(NodeId),
}

/// A child in the model: a block that exists, or the nth block the
/// transaction creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Real(NodeId),
    New(usize),
}

struct Model<'a> {
    doc: &'a Document,
    schemas: &'a SchemaRegistry,
    plan: Plan,
    texts: BTreeMap<NodeId, String>,
    overrides: BTreeMap<NodeId, Style>,
    /// Blocks deleted by the transaction so far (their subtrees go too).
    killed: BTreeSet<NodeId>,
    /// Parents of blocks the transaction has moved.
    parents: BTreeMap<NodeId, Option<NodeId>>,
    /// The children of parents the transaction has changed.
    kids: BTreeMap<Option<NodeId>, Vec<Slot>>,
    live_relations: Option<BTreeSet<RelationId>>,
}

/// Validates `commands` and turns them into a plan.
pub(crate) fn plan(
    doc: &Document,
    schemas: &SchemaRegistry,
    commands: &[Command],
) -> Result<Plan, EditError> {
    if commands.len() > MAX_COMMANDS {
        return Err(EditError {
            command: None,
            reason: Reason::TooManyCommands {
                count: commands.len(),
                max: MAX_COMMANDS,
            },
        });
    }
    let mut model = Model {
        doc,
        schemas,
        plan: Plan::default(),
        texts: BTreeMap::new(),
        overrides: BTreeMap::new(),
        killed: BTreeSet::new(),
        parents: BTreeMap::new(),
        kids: BTreeMap::new(),
        live_relations: None,
    };
    for (i, command) in commands.iter().enumerate() {
        model.run(command).map_err(|r| EditError::at(i, r))?;
    }
    Ok(model.plan)
}

fn check_text_size(len: usize) -> Result<(), Reason> {
    if len > MAX_INSERT_BYTES {
        return Err(Reason::TextTooLong {
            len,
            max: MAX_INSERT_BYTES,
        });
    }
    Ok(())
}

impl Model<'_> {
    fn parent_of(&self, node: NodeId) -> Option<Option<NodeId>> {
        match self.parents.get(&node) {
            Some(&p) => Some(p),
            None => self.doc.parent_of(node),
        }
    }

    /// Alive now: alive in the document, and not under a block this
    /// transaction deleted.
    fn live(&self, node: NodeId) -> bool {
        if !self.doc.is_live(node) {
            return false;
        }
        let mut at = Some(node);
        for _ in 0..MAX_DEPTH {
            let Some(n) = at else { return true };
            if self.killed.contains(&n) {
                return false;
            }
            match self.parent_of(n) {
                Some(p) => at = p,
                None => return false,
            }
        }
        false
    }

    fn require(&self, node: NodeId) -> Result<(), Reason> {
        if self.live(node) {
            Ok(())
        } else {
            Err(Reason::NoSuchBlock(node))
        }
    }

    fn text(&mut self, node: NodeId) -> Result<&mut String, Reason> {
        self.require(node)?;
        if !self.texts.contains_key(&node) {
            let text = self
                .doc
                .block(node)
                .map_err(|_| Reason::NoSuchBlock(node))?
                .text
                .to_string();
            self.texts.insert(node, text);
        }
        self.texts.get_mut(&node).ok_or(Reason::NoSuchBlock(node))
    }

    fn kids(&mut self, parent: Option<NodeId>) -> &mut Vec<Slot> {
        let doc = self.doc;
        self.kids
            .entry(parent)
            .or_insert_with(|| doc.children(parent).into_iter().map(Slot::Real).collect())
    }

    /// Where `node` sits among its parent's children.
    fn slot_of(&mut self, node: NodeId) -> Result<(Option<NodeId>, usize), Reason> {
        let parent = self.parent_of(node).ok_or(Reason::NoSuchBlock(node))?;
        let at = self
            .kids(parent)
            .iter()
            .position(|s| *s == Slot::Real(node))
            .ok_or(Reason::NoSuchBlock(node))?;
        Ok((parent, at))
    }

    fn check_parent(&self, parent: Option<NodeId>) -> Result<(), Reason> {
        match parent {
            Some(p) => self.require(p),
            None => Ok(()),
        }
    }

    fn run(&mut self, command: &Command) -> Result<(), Reason> {
        match command {
            Command::InsertText { node, at, text } => self.insert_text(*node, *at, text),
            Command::DeleteText { node, range } => self.delete_text(*node, range),
            Command::SplitBlock { node, at } => self.split(*node, *at),
            Command::JoinBlocks { first, second } => self.join(*first, *second),
            Command::InsertBlock {
                parent,
                index,
                block,
            } => self.insert_block(*parent, *index, block),
            Command::DeleteBlock { node } => {
                self.require(*node)?;
                let (parent, at) = self.slot_of(*node)?;
                self.kids(parent).remove(at);
                self.killed.insert(*node);
                self.plan.steps.push(Step::Trash { node: *node });
                self.plan.effects.push(PlannedEffect::Deleted(*node));
                Ok(())
            }
            Command::MoveBlock {
                node,
                parent,
                index,
            } => self.move_block(*node, *parent, *index),
            Command::SetStyleOverride { node, style } => {
                self.require(*node)?;
                self.overrides.insert(*node, style.clone());
                self.plan.steps.push(Step::SetOverrides {
                    node: *node,
                    style: style.clone(),
                });
                Ok(())
            }
            Command::AddRelation { relation } => self.add_relation(relation),
            Command::RemoveRelation { id } => {
                let doc = self.doc;
                let live = self
                    .live_relations
                    .get_or_insert_with(|| doc.relations().into_iter().map(|(id, _)| id).collect());
                if !live.remove(id) {
                    return Err(Reason::NoSuchRelation(*id));
                }
                self.plan.steps.push(Step::RemoveRelation { id: *id });
                Ok(())
            }
        }
    }

    fn insert_text(&mut self, node: NodeId, at: usize, text: &str) -> Result<(), Reason> {
        check_text_size(text.len())?;
        let s = self.text(node)?;
        if !s.is_char_boundary(at) {
            return Err(Reason::BadOffset { node, offset: at });
        }
        if text.is_empty() {
            // Valid, and nothing: not a step, so not an undo step either.
            return Ok(());
        }
        s.insert_str(at, text);
        self.plan.steps.push(Step::InsertText {
            node,
            at,
            text: text.to_owned(),
        });
        self.plan.effects.push(PlannedEffect::Text {
            node,
            at,
            removed: 0,
            inserted: text.len(),
        });
        Ok(())
    }

    fn delete_text(&mut self, node: NodeId, range: &Range<usize>) -> Result<(), Reason> {
        let s = self.text(node)?;
        let ok = range.start <= range.end
            && range.end <= s.len()
            && s.is_char_boundary(range.start)
            && s.is_char_boundary(range.end);
        if !ok {
            return Err(Reason::BadRange {
                node,
                range: range.clone(),
            });
        }
        if range.is_empty() {
            return Ok(());
        }
        s.replace_range(range.clone(), "");
        self.plan.steps.push(Step::DeleteText {
            node,
            range: range.clone(),
        });
        self.plan.effects.push(PlannedEffect::Text {
            node,
            at: range.start,
            removed: range.len(),
            inserted: 0,
        });
        Ok(())
    }

    fn split(&mut self, node: NodeId, at: usize) -> Result<(), Reason> {
        let s = self.text(node)?;
        if !s.is_char_boundary(at) {
            return Err(Reason::BadOffset { node, offset: at });
        }
        let tail = s.split_off(at);
        let block = self
            .doc
            .block(node)
            .map_err(|_| Reason::NoSuchBlock(node))?;
        let overrides = self
            .overrides
            .get(&node)
            .cloned()
            .unwrap_or(block.overrides);
        let new = self.plan.blocks.len();
        self.plan.blocks.push(NewBlock {
            kind: block.kind,
            style: block.style.unwrap_or_default(),
            overrides,
            text: tail,
        });
        let (parent, index) = self.slot_of(node)?;
        self.kids(parent).insert(index + 1, Slot::New(new));
        self.plan.steps.push(Step::Split { node, at, new });
        self.plan
            .effects
            .push(PlannedEffect::Split { node, at, new });
        Ok(())
    }

    fn join(&mut self, first: NodeId, second: NodeId) -> Result<(), Reason> {
        if first == second {
            return Err(Reason::SameBlock(first));
        }
        self.require(first)?;
        self.require(second)?;
        if self.doc.kind_of(first) != self.doc.kind_of(second) {
            return Err(Reason::KindMismatch { first, second });
        }
        let (parent, at) = self.slot_of(second)?;
        if !self.kids(Some(second)).is_empty() {
            return Err(Reason::HasChildren(second));
        }
        let tail = self.text(second)?.clone();
        let head = self.text(first)?;
        let join_at = head.len();
        head.push_str(&tail);
        self.kids(parent).remove(at);
        self.killed.insert(second);
        self.plan.steps.push(Step::Join { first, second });
        self.plan.effects.push(PlannedEffect::Join {
            first,
            second,
            at: join_at,
        });
        Ok(())
    }

    fn insert_block(
        &mut self,
        parent: Option<NodeId>,
        index: usize,
        block: &NewBlock,
    ) -> Result<(), Reason> {
        check_text_size(block.text.len())?;
        self.check_parent(parent)?;
        let new = self.plan.blocks.len();
        let kids = self.kids(parent);
        if index > kids.len() {
            return Err(Reason::BadIndex {
                index,
                len: kids.len(),
            });
        }
        kids.insert(index, Slot::New(new));
        self.plan.blocks.push(block.clone());
        self.plan.steps.push(Step::Place { new, parent, index });
        Ok(())
    }

    fn move_block(
        &mut self,
        node: NodeId,
        parent: Option<NodeId>,
        index: usize,
    ) -> Result<(), Reason> {
        self.require(node)?;
        self.check_parent(parent)?;
        let mut up = parent;
        for _ in 0..MAX_DEPTH {
            match up {
                Some(p) if p == node => return Err(Reason::Cycle(node)),
                Some(p) => up = self.parent_of(p).flatten(),
                None => break,
            }
        }
        let (old_parent, at) = self.slot_of(node)?;
        self.kids(old_parent).remove(at);
        let len = self.kids(parent).len();
        if index > len {
            // The transaction is refused whole, but keep the model honest.
            self.kids(old_parent).insert(at, Slot::Real(node));
            return Err(Reason::BadIndex { index, len });
        }
        self.kids(parent).insert(index, Slot::Real(node));
        self.parents.insert(node, parent);
        self.plan.steps.push(Step::Move {
            node,
            parent,
            index,
        });
        Ok(())
    }

    fn add_relation(&mut self, relation: &Relation) -> Result<(), Reason> {
        self.schemas.validate(relation).map_err(Reason::Schema)?;
        if let Some(owner) = relation.owner {
            self.require(owner).map_err(|_| Reason::DeadBlock(owner))?;
        }
        for target in relation.targets.values().flatten() {
            self.check_target(target)?;
        }
        let new = self.plan.relations.len();
        self.plan.relations.push(relation.clone());
        self.plan.steps.push(Step::AddRelation { new });
        Ok(())
    }

    /// A relation may name what is deleted *later* (that is what the deletion
    /// policies are for), but not what is deleted or missing when it is made:
    /// that is a mistake of the caller's.
    fn check_target(&self, target: &Target) -> Result<(), Reason> {
        let node = |n: NodeId| self.require(n).map_err(|_| Reason::DeadBlock(n));
        let range = |r| match self.doc.resolve_range(r) {
            RangeState::Missing { node: None } => Err(Reason::DeadRange(r)),
            RangeState::Missing { node: Some(n) } if !self.live(n) => Err(Reason::DeadBlock(n)),
            _ => Ok(()),
        };
        match target {
            Target::Node(n) => node(*n),
            Target::Range(r) => range(*r),
            Target::Structural(q) => q.anchor().map_or(Ok(()), node),
            Target::Layout(q) => match q.anchor() {
                QueryAnchor::Range(r) => range(r),
                QueryAnchor::Node(n) => node(n),
            },
            // A snapshot names a past version: what it names may be gone.
            // Target is non-exhaustive; a kind added later is not checked.
            _ => Ok(()),
        }
    }
}

impl From<DocError> for Reason {
    fn from(e: DocError) -> Reason {
        Reason::Store(e.to_string())
    }
}
