//! Transactions and the editor that applies them (29).

use reprise_doc::{Document, NodeId, RelationId, SchemaRegistry, UndoStack};

use crate::command::{Command, Effect};
use crate::error::{EditError, Reason};
use crate::plan::{self, Plan, PlannedEffect, Step};

/// Commands applied together: one atomic Loro commit and one undo step.
///
/// It is applied whole or not at all. Every command is validated, against the
/// document as the commands before it leave it, before anything is written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transaction {
    commands: Vec<Command>,
}

impl Transaction {
    pub fn new() -> Transaction {
        Transaction::default()
    }

    pub fn with(mut self, command: Command) -> Transaction {
        self.commands.push(command);
        self
    }

    pub fn push(&mut self, command: Command) {
        self.commands.push(command);
    }

    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl From<Command> for Transaction {
    fn from(command: Command) -> Transaction {
        Transaction::new().with(command)
    }
}

impl From<Vec<Command>> for Transaction {
    fn from(commands: Vec<Command>) -> Transaction {
        Transaction { commands }
    }
}

/// Which way a position moves when text is inserted exactly at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    /// Stays before the inserted text.
    Before,
    /// Moves after the inserted text, like a caret that is typing.
    After,
}

/// What a transaction did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// The blocks the transaction created (by `InsertBlock` and
    /// `SplitBlock`), in command order.
    pub blocks: Vec<NodeId>,
    /// The relations it created, in command order.
    pub relations: Vec<RelationId>,
    /// What its commands did to positions, in order.
    pub effects: Vec<Effect>,
}

impl Applied {
    /// Where a position (a block and a byte offset in it) is after the
    /// transaction, so that a caret or selection held by a UI follows the
    /// edit. `None` if its block was deleted.
    ///
    /// Positions in a deleted block's descendants aren't tracked here:
    /// check the document for those.
    pub fn map_position(&self, node: NodeId, offset: usize, bias: Bias) -> Option<(NodeId, usize)> {
        let mut at = (node, offset);
        for effect in &self.effects {
            match *effect {
                Effect::Text {
                    node: n,
                    at: start,
                    removed,
                    inserted,
                } if n == at.0 => {
                    let end = start.saturating_add(removed);
                    at.1 = if at.1 < start {
                        at.1
                    } else if at.1 > end {
                        (at.1 - removed).saturating_add(inserted)
                    } else if bias == Bias::After {
                        start.saturating_add(inserted)
                    } else {
                        start
                    };
                }
                Effect::Split {
                    node: n,
                    at: split,
                    new,
                } if n == at.0 => {
                    if at.1 > split || (at.1 == split && bias == Bias::After) {
                        at = (new, at.1 - split);
                    }
                }
                Effect::Join {
                    first,
                    second,
                    at: join,
                } if second == at.0 => {
                    at = (first, join.saturating_add(at.1));
                }
                Effect::Deleted(n) if n == at.0 => return None,
                _ => {}
            }
        }
        Some(at)
    }
}

/// A document, the relation schemas its transactions are validated against,
/// and this replica's undo history. Every edit a UI makes goes through here.
pub struct Editor {
    doc: Document,
    schemas: SchemaRegistry,
    undo: UndoStack,
}

impl Editor {
    /// Takes over `doc`. Only edits made from now on can be undone.
    pub fn new(doc: Document, schemas: SchemaRegistry) -> Editor {
        let undo = doc.undo_stack();
        Editor { doc, schemas, undo }
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn schemas(&self) -> &SchemaRegistry {
        &self.schemas
    }

    pub fn into_document(self) -> Document {
        self.doc
    }

    /// Merges another replica's edits. They are never undone by this
    /// editor's [`Editor::undo`].
    pub fn merge(&mut self, other: &Document) -> Result<(), EditError> {
        self.doc.merge(other).map_err(store)
    }

    pub fn apply_command(&mut self, command: Command) -> Result<Applied, EditError> {
        self.apply(&Transaction::from(command))
    }

    /// Validates and applies a transaction. On `Err` the document is exactly
    /// as it was.
    pub fn apply(&mut self, transaction: &Transaction) -> Result<Applied, EditError> {
        let plan = plan::plan(&self.doc, &self.schemas, transaction.commands())?;
        if plan.steps.is_empty() {
            return Ok(Applied::default());
        }
        let applied = self.write(plan);
        // Whatever happened, what was written is one step.
        self.doc.commit_step();
        applied
    }

    /// Writes a validated plan. Validation made every check, and the document
    /// applies the steps the model applied, so a failure here would be a bug
    /// in this crate or the store; it is reported, never panicked on (37).
    fn write(&self, plan: Plan) -> Result<Applied, EditError> {
        // Staging commits, so all of it comes before the step's own changes.
        let blocks = plan
            .blocks
            .iter()
            .map(|b| self.doc.stage_block(b))
            .collect::<Result<Vec<_>, _>>()
            .map_err(store)?;
        let relations = plan
            .relations
            .iter()
            .map(|r| self.doc.stage_relation(&self.schemas, r))
            .collect::<Result<Vec<_>, _>>()
            .map_err(store)?;
        for step in &plan.steps {
            self.step(step, &blocks, &relations).map_err(store)?;
        }
        let new = |i: usize| blocks.get(i).copied();
        let effects = plan
            .effects
            .iter()
            .filter_map(|e| {
                Some(match *e {
                    PlannedEffect::Text {
                        node,
                        at,
                        removed,
                        inserted,
                    } => Effect::Text {
                        node,
                        at,
                        removed,
                        inserted,
                    },
                    PlannedEffect::Split { node, at, new: i } => Effect::Split {
                        node,
                        at,
                        new: new(i)?,
                    },
                    PlannedEffect::Join { first, second, at } => Effect::Join { first, second, at },
                    PlannedEffect::Deleted(n) => Effect::Deleted(n),
                })
            })
            .collect();
        Ok(Applied {
            blocks,
            relations,
            effects,
        })
    }

    fn step(
        &self,
        step: &Step,
        blocks: &[NodeId],
        relations: &[RelationId],
    ) -> Result<(), reprise_doc::DocError> {
        let missing = || reprise_doc::DocError::Store("a staged item is missing".into());
        match step {
            Step::InsertText { node, at, text } => {
                self.doc.block(*node)?.text.insert(*at, text)?;
            }
            Step::DeleteText { node, range } => {
                self.doc.block(*node)?.text.delete(range.clone())?;
            }
            Step::Split { node, at, new } => {
                let new = blocks.get(*new).copied().ok_or_else(missing)?;
                self.doc.split_block_into(*node, *at, new)?;
            }
            Step::Join { first, second } => self.doc.join_blocks(*first, *second)?,
            Step::Place { new, parent, index } => {
                let new = blocks.get(*new).copied().ok_or_else(missing)?;
                self.doc.activate_block_at(new, *parent, *index)?;
            }
            Step::Trash { node } => self.doc.delete_block(*node)?,
            Step::Move {
                node,
                parent,
                index,
            } => self.doc.move_block(*node, *parent, *index)?,
            Step::SetOverrides { node, style } => self.doc.set_overrides(*node, style)?,
            Step::AddRelation { new } => {
                let id = relations.get(*new).copied().ok_or_else(missing)?;
                self.doc.restore_relation(id)?;
            }
            Step::RemoveRelation { id } => self.doc.delete_relation(*id)?,
        }
        Ok(())
    }

    /// Undoes this replica's last transaction. Collaborators' edits, merged
    /// meanwhile, stay. `false` when there is nothing to undo.
    pub fn undo(&mut self) -> Result<bool, EditError> {
        self.undo.undo().map_err(store)
    }

    pub fn redo(&mut self) -> Result<bool, EditError> {
        self.undo.redo().map_err(store)
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// How many transactions can be undone.
    pub fn undo_count(&self) -> usize {
        self.undo.undo_count()
    }

    /// How many undone transactions can be redone.
    pub fn redo_count(&self) -> usize {
        self.undo.redo_count()
    }
}

fn store(e: reprise_doc::DocError) -> EditError {
    EditError {
        command: None,
        reason: Reason::Store(e.to_string()),
    }
}
