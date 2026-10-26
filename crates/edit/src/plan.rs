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
use reprise_doc::text::OBJECT;
use reprise_doc::{
    DocError, Document, NewBlock, NodeId, RangeState, Relation, RelationId, SchemaRegistry, Style,
    Target,
};

use crate::command::Command;
use crate::error::{EditError, Reason};

mod sections;
mod semantic;

/// The most commands one transaction may hold.
pub const MAX_COMMANDS: usize = 10_000;
/// The most text one `InsertText` or `InsertBlock` may carry, in bytes.
pub const MAX_INSERT_BYTES: usize = 16 << 20;

/// Maximum aggregate inserted block/text payload in one transaction.
pub const MAX_TRANSACTION_BYTES: usize = 16 << 20;
/// Maximum number of ancestors checked by one command. Excess is a typed
/// `TreeDepthLimit` error (and `edit.limit` diagnostic), never silent truncation.
pub const MAX_ANCESTORS: usize = 1024;

#[derive(Clone, Debug)]
pub(crate) enum Step {
    InsertStart {
        target: Slot,
        kind: reprise_doc::sections::BreakKind,
        properties: reprise_doc::sections::SectionProperties,
    },
    RemoveStart {
        block: NodeId,
    },
    SetSection {
        section: reprise_doc::sections::SectionId,
        patch: reprise_doc::sections::SectionProperties,
        clear: Vec<String>,
    },
    CreateStory {
        new: usize,
        binding: Option<(reprise_doc::sections::SectionId, String)>,
    },

    SetPageSetup(reprise_doc::page_setup::PageSetupPatch),
    SetAlignment {
        node: NodeId,
        alignment: reprise_doc::marks::Alignment,
    },
    SetTabs {
        node: NodeId,
        tabs: reprise_doc::marks::TabStops,
    },
    AlignLine {
        node: NodeId,
        at: usize,
        alignment: reprise_doc::marks::Alignment,
    },
    PinLine {
        node: NodeId,
        at: usize,
        edge: reprise_doc::marks::LineEdge,
        target: NodeId,
        target_at: usize,
        target_edge: reprise_doc::marks::AnchorEdge,
    },

    FormatText {
        node: NodeId,
        range: Range<usize>,
        style: reprise_doc::formatting::TextStyle,
    },
    InsertText {
        node: NodeId,
        at: usize,
        text: String,
    },
    DeleteText {
        node: NodeId,
        range: Range<usize>,
    },
    /// `new` indexes [`Plan::created`]; the split creates it.
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
    Delete {
        node: NodeId,
    },
    Move {
        node: NodeId,
        parent: Option<NodeId>,
        index: usize,
    },
    /// Moves a paragraph of a flow by copying it to `new` (a staged block
    /// in [`Plan::created`]) and deleting it.
    MoveCopy {
        node: NodeId,
        new: usize,
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
    SetRole {
        node: NodeId,
        role: reprise_doc::Role,
    },
    /// `new` indexes [`Plan::lists`].
    ActivateList {
        new: usize,
    },
    UpdateList {
        list: reprise_doc::ListId,
        definition: reprise_doc::ListDefinition,
    },
    DeleteList {
        list: crate::ListRef,
    },
    SetListItem {
        node: NodeId,
        list: crate::ListRef,
        level: u8,
    },
    RemoveListItem {
        node: NodeId,
    },
    SetListRestart {
        node: NodeId,
        value: Option<i64>,
    },
    DefineStyle {
        name: String,
        style: Style,
    },
    PatchStyle {
        name: String,
        patch: reprise_doc::StylePatch,
    },
    RenameStyle {
        from: String,
        to: String,
    },
    DeleteStyle {
        name: String,
    },
    SetStyleName {
        node: NodeId,
        name: String,
    },
    PatchOverrides {
        node: NodeId,
        patch: reprise_doc::StylePatch,
    },
    ClearAlignment {
        node: NodeId,
    },
    ClearLineAlignment {
        node: NodeId,
        at: usize,
    },
    AddAnnotation {
        node: NodeId,
        range: Range<usize>,
        data: reprise_doc::annotation::AnnotationData,
    },
    UpdateAnnotation {
        id: reprise_doc::RangeId,
        data: reprise_doc::annotation::AnnotationData,
    },
    RemoveAnnotation {
        id: reprise_doc::RangeId,
    },
    /// Stages the object and shows it. Staging commits what was pending in
    /// the step first, so the object joins the step's undo history.
    InsertInline {
        node: NodeId,
        at: usize,
        object: reprise_doc::InlineObject,
    },
    /// Finds the object at `at` when the step runs, which is where the
    /// earlier steps have left it.
    UpdateInline {
        node: NodeId,
        at: usize,
        patch: reprise_doc::InlinePatch,
    },
}

/// A block the transaction creates.
#[derive(Clone, Debug)]
pub(crate) enum Created {
    /// Staged before the steps run, then placed.
    Staged(Box<NewBlock>),
    /// Made by a split step.
    Split,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Plan {
    pub paste: Option<crate::paste::Prepared>,
    pub stories: Vec<StoryCreation>,
    /// Command-level exhaustive audit; steps additionally cover implicit writes.
    trial_command: bool,
    /// The blocks the transaction creates, in command order. Staged ones are
    /// staged first; steps name them by index.
    pub created: Vec<Created>,
    pub relations: Vec<Relation>,
    /// List definitions to stage; [`Step::ActivateList`] names them by index.
    pub lists: Vec<reprise_doc::ListDefinition>,
    pub steps: Vec<Step>,
    /// Command owning each write, including implicit structural effects.
    pub step_commands: Vec<usize>,
    /// A section, break, story or region command is in the batch.
    pub sections: bool,
    /// The commands' effects, with staged blocks as indices; the editor
    /// resolves them once those have IDs.
    pub effects: Vec<PlannedEffect>,
}

impl Plan {
    /// Explicit formatting counts are fully modelled. Structural copies can
    /// also write reset/copy actions; exercise those on the resulting state.
    pub(crate) fn needs_trial(&self, doc: &Document) -> Result<bool, DocError> {
        // Breaks, section properties, stories and region bindings write
        // lifetimes, staged copies and bindings against the state the earlier
        // steps of the batch made, so any of them may refuse late. Exercise
        // the whole batch on an isolated replica before anything is published.
        if self.sections || self.trial_command {
            return Ok(true);
        }
        if self.created.iter().any(|created| match created {
            Created::Staged(block) => {
                doc.style_record(&block.style).is_some_and(|r| r.builtin)
                    || block.overrides.parent.is_some()
                    || reprise_doc::PARAGRAPH_KEYS
                        .iter()
                        .any(|key| block.overrides.stored(key).is_some())
                    || block.overrides.alignment == Some(reprise_doc::marks::Alignment::Justify)
            }
            Created::Split => false,
        }) {
            return Ok(true);
        }
        if !self.lists.is_empty() || self.paste.is_some() {
            return Ok(true);
        }
        let explicit = self
            .steps
            .iter()
            .any(|s| matches!(s, Step::FormatText { .. }));
        for step in &self.steps {
            let node = match step {
                // Alias allocation and imported property validation can refuse.
                // Exercise preceding repairs and the resulting metadata state.
                Step::RenameStyle { .. }
                | Step::ActivateList { .. }
                | Step::UpdateList { .. }
                | Step::DeleteList { .. }
                | Step::SetListItem { .. }
                | Step::RemoveListItem { .. }
                | Step::SetListRestart { .. }
                | Step::InsertInline { .. }
                | Step::UpdateInline { .. }
                | Step::AddAnnotation { .. }
                | Step::UpdateAnnotation { .. }
                | Step::RemoveAnnotation { .. }
                // Section, break and story writes are always exercised
                // (`Plan::sections` already said so; the arm keeps the match
                // exhaustive).
                | Step::InsertStart { .. }
                | Step::RemoveStart { .. }
                | Step::SetSection { .. }
                | Step::CreateStory { .. } => return Ok(true),
                Step::SetAlignment {
                    alignment: reprise_doc::marks::Alignment::Justify,
                    ..
                } => return Ok(true),
                Step::FormatText { style, .. } if !style.clear.is_empty() => return Ok(true),
                Step::DefineStyle { .. } | Step::PatchStyle { .. } | Step::DeleteStyle { .. } => {
                    return Ok(true);
                }
                Step::SetStyleName { name, .. }
                    if doc.has_style_identities()
                        || doc.style_record(name).is_some_and(|r| r.builtin) =>
                {
                    return Ok(true);
                }
                Step::SetOverrides { style, .. }
                    if style.parent.is_some()
                        || reprise_doc::PARAGRAPH_KEYS
                            .iter()
                            .any(|k| style.stored(k).is_some())
                        || style.alignment == Some(reprise_doc::marks::Alignment::Justify) =>
                {
                    return Ok(true);
                }
                Step::PatchOverrides { patch, .. }
                    if patch.set.parent.is_some()
                        || reprise_doc::PARAGRAPH_KEYS
                            .iter()
                            .any(|k| patch.set.stored(k).is_some())
                        || patch.set.alignment == Some(reprise_doc::marks::Alignment::Justify) =>
                {
                    return Ok(true);
                }
                Step::Split { node, .. } | Step::MoveCopy { node, .. } => node,
                Step::Join { first, second } => {
                    if explicit
                        || doc.text_format_capacity(*first)?.0 != 0
                        || doc.text_format_capacity(*second)?.0 != 0
                    {
                        return Ok(true);
                    }
                    continue;
                }
                Step::SetPageSetup(_)
                | Step::SetAlignment { .. }
                | Step::SetTabs { .. }
                | Step::AlignLine { .. }
                | Step::PinLine { .. }
                | Step::FormatText { .. }
                | Step::InsertText { .. }
                | Step::DeleteText { .. }
                | Step::Place { .. }
                | Step::Delete { .. }
                | Step::Move { .. }
                | Step::SetOverrides { .. }
                | Step::AddRelation { .. }
                | Step::RemoveRelation { .. }
                | Step::SetRole { .. }
                | Step::SetStyleName { .. }
                | Step::PatchOverrides { .. }
                | Step::ClearAlignment { .. }
                | Step::ClearLineAlignment { .. } => continue,
            };
            if explicit || doc.text_format_capacity(*node)?.0 != 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[derive(Clone, Debug)]
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
    /// `node` moved as a copy, to `new`.
    Moved {
        node: NodeId,
        new: usize,
    },
}

/// A child in the model: a block that exists, or the nth block the
/// transaction creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Slot {
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
    payload: usize,
    page_setup: Option<(
        reprise_doc::page_setup::PageDimensions,
        reprise_doc::page_setup::PageSetupPatch,
    )>,
    /// Named styles the transaction made live (`true`) or not (`false`).
    styles: BTreeMap<String, bool>,
    /// List membership this transaction has changed: `None` for not an item.
    members: BTreeMap<NodeId, Option<(crate::ListRef, u8)>>,
    /// Lists this transaction has deleted.
    dead_lists: BTreeSet<crate::ListRef>,
    starts: BTreeMap<
        Slot,
        (
            reprise_doc::sections::BreakKind,
            reprise_doc::sections::SectionProperties,
        ),
    >,
    first: reprise_doc::sections::SectionProperties,
    /// The inline objects of each paragraph whose text the transaction has
    /// read, in text order, kept at their positions as the text changes.
    inlines: BTreeMap<NodeId, Vec<ModelInline>>,
}

/// An inline object as the transaction sees it: where its U+FFFC is now, and
/// its parameters as they will be.
#[derive(Clone, Debug)]
struct ModelInline {
    at: usize,
    object: reprise_doc::InlineObject,
}

/// Validates `commands` and turns them into a plan.
pub(crate) fn plan(
    doc: &Document,
    schemas: &SchemaRegistry,
    commands: &[Command],
) -> Result<Plan, EditError> {
    if commands.iter().any(|c| matches!(c, Command::Paste { .. })) {
        let [
            Command::Paste {
                fragment,
                at,
                target_namespace,
            },
        ] = commands
        else {
            return Err(EditError {
                command: None,
                reason: Reason::MixedPaste,
            });
        };
        return Ok(Plan {
            paste: Some(crate::paste::prepare(
                doc,
                schemas,
                fragment,
                *at,
                target_namespace,
            )?),
            ..Plan::default()
        });
    }
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
        payload: 0,
        page_setup: None,
        styles: BTreeMap::new(),
        members: BTreeMap::new(),
        dead_lists: BTreeSet::new(),
        starts: doc
            .effective_starts()
            .into_iter()
            .filter_map(|r| match (r.id, r.kind) {
                (reprise_doc::sections::SectionId::Block(id), Some(kind)) => {
                    Some((Slot::Real(id), (kind, r.own)))
                }
                _ => None,
            })
            .collect(),
        first: doc
            .start_records()
            .into_iter()
            .find(|r| r.id == reprise_doc::sections::SectionId::First)
            .map(|r| r.own)
            .unwrap_or_default(),
        inlines: BTreeMap::new(),
    };
    for (i, command) in commands.iter().enumerate() {
        model.plan.trial_command |= command_needs_trial(command);
        model.run(command).map_err(|r| EditError::at(i, r))?;
        model.plan.step_commands.resize(model.plan.steps.len(), i);
    }
    Ok(model.plan)
}

/// No wildcard: adding a command requires an explicit publication decision.
/// The model-only commands below validate their direct writes; structural
/// formatting transfers are selected separately by Plan::needs_trial.
fn command_needs_trial(command: &Command) -> bool {
    match command {
        Command::Paste { .. }
        | Command::SetPageSetup { .. }
        | Command::SwapPageOrientation { .. }
        | Command::SetAlignment { .. }
        | Command::SetTabStops { .. }
        | Command::AddAnchor { .. }
        | Command::ClearAlignment { .. }
        | Command::DefineStyle { .. }
        | Command::PatchStyle { .. }
        | Command::RenameStyle { .. }
        | Command::DeleteStyle { .. }
        | Command::SetStyleName { .. }
        | Command::SetStyleOverride { .. }
        | Command::PatchOverrides { .. }
        | Command::SetRole { .. }
        | Command::CreateList { .. }
        | Command::UpdateList { .. }
        | Command::DeleteList { .. }
        | Command::SetListItem { .. }
        | Command::RemoveListItem { .. }
        | Command::SetListLevel { .. }
        | Command::IndentListItem { .. }
        | Command::RestartList { .. }
        | Command::ContinueList { .. }
        | Command::AddAnnotation { .. }
        | Command::UpdateAnnotation { .. }
        | Command::RemoveAnnotation { .. }
        | Command::InsertInline { .. }
        | Command::UpdateInline { .. }
        | Command::InsertBreak { .. }
        | Command::RemoveBreak { .. }
        | Command::SetSection { .. }
        | Command::CreateStory { .. }
        | Command::SetRegion { .. }
        | Command::CreateRegion { .. }
        | Command::DeleteStory { .. } => true,
        Command::InsertText { .. }
        | Command::DeleteText { .. }
        | Command::InsertLineBreak { .. }
        | Command::InsertTab { .. }
        | Command::FormatText { .. }
        | Command::InsertBlock { .. }
        | Command::DeleteBlock { .. }
        | Command::MoveBlock { .. }
        | Command::SplitBlock { .. }
        | Command::JoinBlocks { .. }
        | Command::AddRelation { .. }
        | Command::RemoveRelation { .. }
        | Command::Enter { .. }
        | Command::BackspaceAtStart { .. } => false,
    }
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
    fn add_payload(&mut self, len: usize) -> Result<(), Reason> {
        check_text_size(len)?;
        self.payload = self.payload.saturating_add(len);
        if self.payload > MAX_TRANSACTION_BYTES {
            return Err(Reason::TransactionTooLarge {
                len: self.payload,
                max: MAX_TRANSACTION_BYTES,
            });
        }
        Ok(())
    }

    fn parent_of(&self, node: NodeId) -> Option<Option<NodeId>> {
        match self.parents.get(&node) {
            Some(&p) => Some(p),
            None => self.doc.parent_of(node),
        }
    }

    /// Alive now: alive in the document, and not under a block this
    /// transaction deleted.
    fn live(&self, node: NodeId) -> Result<bool, Reason> {
        if !self.doc.is_live(node) {
            return Ok(false);
        }
        let mut at = Some(node);
        for _ in 0..MAX_ANCESTORS {
            let Some(n) = at else { return Ok(true) };
            if self.killed.contains(&n) {
                return Ok(false);
            }
            match self.parent_of(n) {
                Some(p) => at = p,
                None => return Ok(false),
            }
        }
        if at.is_none() {
            return Ok(true);
        }
        Err(Reason::TreeDepthLimit {
            node,
            max: MAX_ANCESTORS,
        })
    }

    fn require(&self, node: NodeId) -> Result<(), Reason> {
        if self.live(node)? {
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
            self.load_inlines(node)?;
        }
        self.texts.get_mut(&node).ok_or(Reason::NoSuchBlock(node))
    }

    /// Records the objects of `node` as the document holds them. Called when
    /// the node's text is first read, so before the transaction changes it.
    fn load_inlines(&mut self, node: NodeId) -> Result<(), Reason> {
        if !self.doc.has_inline_objects() || !self.doc.in_flow(node) {
            return Ok(());
        }
        let list = self
            .doc
            .inline_objects(node)
            .map_err(Reason::from)?
            .into_iter()
            .map(|e| ModelInline {
                at: e.at,
                object: e.object,
            })
            .collect();
        self.inlines.insert(node, list);
        Ok(())
    }

    /// Moves the objects of `node` after its text changed: `removed` bytes at
    /// `at` became `inserted` bytes. Objects inside the removed bytes are gone
    /// (a deletion deactivates them); those after them move.
    fn shift_inlines(&mut self, node: NodeId, at: usize, removed: usize, inserted: usize) {
        let end = at.saturating_add(removed);
        if let Some(list) = self.inlines.get_mut(&node) {
            list.retain(|i| i.at < at || i.at >= end);
            for i in list.iter_mut().filter(|i| i.at >= end) {
                i.at = i.at.saturating_sub(removed).saturating_add(inserted);
            }
        }
    }

    fn kids(&mut self, parent: Option<NodeId>) -> &mut Vec<Slot> {
        let doc = self.doc;
        self.kids.entry(parent).or_insert_with(|| {
            doc.children_in(reprise_doc::Domain::Body, parent)
                .into_iter()
                .map(Slot::Real)
                .collect()
        })
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
            Some(p) if p.is_break() => {
                Err(Reason::Store("a break paragraph has no children".into()))
            }
            Some(p) => self.require(p),
            None => Ok(()),
        }
    }

    fn page_base(
        &mut self,
        medium: &reprise_doc::Medium,
    ) -> Result<
        (
            reprise_doc::page_setup::PageDimensions,
            reprise_doc::page_setup::PageSetupPatch,
        ),
        Reason,
    > {
        if let Some(setup) = self.page_setup {
            return Ok(setup);
        }
        let base = match self.doc.page_template() {
            reprise_doc::TemplateChoice::Template(t) => t,
            _ => reprise_doc::PageTemplate::builtin(),
        };
        let setup = reprise_doc::page_setup::PageDimensions::from_template(&base, medium)
            .map_err(Reason::PageSetup)?;
        let patch = self.doc.page_setup_patch().map_err(Reason::PageSetup)?;
        self.page_setup = Some((setup, patch));
        Ok((setup, patch))
    }

    fn page_dimensions(
        &mut self,
        medium: &reprise_doc::Medium,
    ) -> Result<reprise_doc::page_setup::PageDimensions, Reason> {
        let (base, patch) = self.page_base(medium)?;
        patch.apply(base).map_err(Reason::PageSetup)
    }

    fn set_page_setup(
        &mut self,
        patch: reprise_doc::page_setup::PageSetupPatch,
        medium: &reprise_doc::Medium,
    ) -> Result<(), Reason> {
        if patch.is_empty() {
            return Ok(());
        }
        let (base, previous) = self.page_base(medium)?;
        let candidate = previous.overlay(patch);
        candidate.apply(base).map_err(Reason::PageSetup)?;
        self.page_setup = Some((base, candidate));
        self.plan.steps.push(Step::SetPageSetup(patch));
        Ok(())
    }

    fn run(&mut self, command: &Command) -> Result<(), Reason> {
        if matches!(
            command,
            Command::InsertBreak { .. }
                | Command::RemoveBreak { .. }
                | Command::SetSection { .. }
                | Command::CreateStory { .. }
                | Command::SetRegion { .. }
                | Command::CreateRegion { .. }
                | Command::DeleteStory { .. }
        ) {
            self.plan.sections = true;
        }
        match command {
            Command::InsertBreak { at, kind, patch } => self.insert_break(*at, *kind, patch),
            Command::RemoveBreak { block } => self.remove_break(*block),
            Command::SetSection {
                section,
                patch,
                clear,
            } => self.set_section(*section, patch, clear),
            Command::CreateStory { blocks } => self.create_story(blocks, None),
            Command::SetRegion {
                section,
                slot,
                variant,
                binding,
            } => self.set_region(*section, slot, variant, *binding),
            Command::CreateRegion {
                section,
                slot,
                variant,
                from,
            } => self.create_region(*section, slot, variant, *from),
            Command::DeleteStory { story } => {
                self.require_story(*story)?;
                self.killed.insert(*story);
                self.plan.steps.push(Step::Delete { node: *story });
                Ok(())
            }
            Command::SetPageSetup { patch, medium } => self.set_page_setup(*patch, medium),
            Command::SwapPageOrientation { medium } => {
                let setup = self.page_dimensions(medium)?;
                self.set_page_setup(
                    reprise_doc::page_setup::PageSetupPatch {
                        width: Some(setup.height),
                        height: Some(setup.width),
                        ..Default::default()
                    },
                    medium,
                )
            }
            Command::InsertLineBreak { node, at } => self.insert_text(*node, *at, "\n"),
            Command::InsertTab { node, at } => self.insert_text(*node, *at, "\t"),
            Command::SetTabStops { node, tabs } => {
                tabs.validate().map_err(Reason::from)?;
                self.add_payload(
                    serde_json::to_vec(tabs)
                        .map_err(|e| Reason::Store(e.to_string()))?
                        .len(),
                )?;
                let mut style = self.current_overrides(*node)?;
                style.tabs = Some(tabs.clone());
                style.unparsed_keywords.remove("tabs");
                self.overrides.insert(*node, style);
                self.plan.steps.push(Step::SetTabs {
                    node: *node,
                    tabs: tabs.clone(),
                });
                Ok(())
            }
            Command::SetAlignment {
                node,
                at: None,
                alignment,
            } => {
                let mut style = self.current_overrides(*node)?;
                style.alignment = Some(*alignment);
                style.unparsed_keywords.remove("alignment");
                self.overrides.insert(*node, style);
                self.plan.steps.push(Step::SetAlignment {
                    node: *node,
                    alignment: *alignment,
                });
                Ok(())
            }
            Command::SetAlignment {
                node,
                at: Some(at),
                alignment,
            } => {
                self.mark_point(*node, *at)?;
                self.mark_capacity()?;
                self.plan.steps.push(Step::AlignLine {
                    node: *node,
                    at: *at,
                    alignment: *alignment,
                });
                Ok(())
            }
            Command::AddAnchor {
                node,
                at,
                edge,
                target,
                target_at,
                target_edge,
            } => {
                self.mark_point(*node, *at)?;
                self.mark_point(*target, *target_at)?;
                self.mark_capacity()?;
                if matches!(
                    target_edge,
                    reprise_doc::marks::AnchorEdge::GapStart
                        | reprise_doc::marks::AnchorEdge::GapEnd
                ) && !self
                    .text(*target)?
                    .get(*target_at..)
                    .is_some_and(|s| s.starts_with('\t'))
                {
                    return Err(Reason::BadOffset {
                        node: *target,
                        offset: *target_at,
                    });
                }
                self.plan.steps.push(Step::PinLine {
                    node: *node,
                    at: *at,
                    edge: *edge,
                    target: *target,
                    target_at: *target_at,
                    target_edge: *target_edge,
                });
                Ok(())
            }
            Command::Paste { .. } => Err(Reason::MixedPaste),
            Command::InsertText { node, at, text } => self.insert_text(*node, *at, text),
            Command::DeleteText { node, range } => self.delete_text(*node, range),
            Command::FormatText { node, range, style } => self.format_text(*node, range, style),
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
                self.plan.steps.push(Step::Delete { node: *node });
                self.plan.effects.push(PlannedEffect::Deleted(*node));
                Ok(())
            }
            Command::MoveBlock {
                node,
                parent,
                index,
            } => self.move_block(*node, *parent, *index),
            Command::SetStyleOverride { node, style } => {
                if let Some(tabs) = &style.tabs {
                    tabs.validate().map_err(Reason::from)?;
                }
                self.set_overrides(*node, style.clone())
            }
            Command::AddRelation { relation } => self.add_relation(relation),
            Command::DefineStyle { name, style } => {
                self.new_style(name)?;
                if style.parent.as_deref() == Some(name.as_str()) {
                    return Err(Reason::Style(format!("{name} can't be its own parent")));
                }
                reprise_doc::StylePatch {
                    set: style.clone(),
                    ..Default::default()
                }
                .validate()
                .map_err(style_reason)?;
                self.add_payload(style_size(style))?;
                self.styles.insert(name.clone(), true);
                self.plan.steps.push(Step::DefineStyle {
                    name: name.clone(),
                    style: style.clone(),
                });
                Ok(())
            }
            Command::PatchStyle { name, patch } => {
                self.live_style(name)?;
                patch.validate().map_err(style_reason)?;
                if patch.set.parent.as_deref() == Some(name.as_str()) {
                    return Err(Reason::Style(format!("{name} can't be its own parent")));
                }
                self.add_payload(style_size(&patch.set))?;
                self.plan.steps.push(Step::PatchStyle {
                    name: name.clone(),
                    patch: patch.clone(),
                });
                Ok(())
            }
            Command::RenameStyle { from, to } => {
                if from.is_empty() || to.is_empty() {
                    return Err(Reason::Style("the document base can't be renamed".into()));
                }
                self.live_style(from)?;
                if from != to {
                    if self.is_live_style(to) {
                        return Err(Reason::Style(format!("a style named {to} exists")));
                    }
                    self.new_style(to)?;
                    self.styles.insert(from.clone(), false);
                    self.styles.insert(to.clone(), true);
                }
                self.plan.steps.push(Step::RenameStyle {
                    from: from.clone(),
                    to: to.clone(),
                });
                Ok(())
            }
            Command::DeleteStyle { name } => {
                if name.is_empty() {
                    return Err(Reason::Style("the document base can't be deleted".into()));
                }
                self.live_style(name)?;
                self.styles.insert(name.clone(), false);
                self.plan
                    .steps
                    .push(Step::DeleteStyle { name: name.clone() });
                Ok(())
            }
            Command::SetStyleName { node, name } => {
                self.require(*node)?;
                self.live_style(name)?;
                self.plan.steps.push(Step::SetStyleName {
                    node: *node,
                    name: name.clone(),
                });
                Ok(())
            }
            Command::PatchOverrides { node, patch } => {
                patch.validate().map_err(style_reason)?;
                self.add_payload(style_size(&patch.set))?;
                let mut style = self.current_overrides(*node)?;
                for key in &patch.clear {
                    style.unset(key);
                }
                for key in patch.supplied() {
                    if let Some(raw) = patch.set.stored(key) {
                        style.apply_stored(key, raw);
                    }
                }
                self.overrides.insert(*node, style);
                self.plan.steps.push(Step::PatchOverrides {
                    node: *node,
                    patch: patch.clone(),
                });
                Ok(())
            }
            Command::ClearAlignment { node, at: None } => {
                let mut style = self.current_overrides(*node)?;
                style.unset("alignment");
                self.overrides.insert(*node, style);
                self.plan.steps.push(Step::ClearAlignment { node: *node });
                Ok(())
            }
            Command::ClearAlignment { node, at: Some(at) } => {
                self.mark_point(*node, *at)?;
                self.plan.steps.push(Step::ClearLineAlignment {
                    node: *node,
                    at: *at,
                });
                Ok(())
            }
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
            Command::AddAnnotation { node, range, data } => self.add_annotation(*node, range, data),
            Command::UpdateAnnotation { id, data } => {
                self.annotation_unremoved(*id)?;
                self.annotation_target(data)?;
                self.doc
                    .check_annotation_update(*id, data)
                    .map_err(Reason::from)?;
                let payload = serde_json::to_vec(data).map_err(|e| Reason::Store(e.to_string()))?;
                self.add_payload(payload.len())?;
                self.plan.steps.push(Step::UpdateAnnotation {
                    id: *id,
                    data: data.clone(),
                });
                Ok(())
            }
            Command::RemoveAnnotation { id } => {
                self.annotation_unremoved(*id)?;
                self.doc
                    .check_annotation_removal(*id)
                    .map_err(Reason::from)?;
                self.plan.steps.push(Step::RemoveAnnotation { id: *id });
                Ok(())
            }
            Command::InsertInline { node, at, object } => self.insert_inline(*node, *at, object),
            Command::UpdateInline { node, at, patch } => self.update_inline(*node, *at, patch),
            other => self.semantic(other),
        }
    }

    fn insert_inline(
        &mut self,
        node: NodeId,
        at: usize,
        object: &reprise_doc::InlineObject,
    ) -> Result<(), Reason> {
        object.validate().map_err(Reason::Inline)?;
        self.require(node)?;
        if !self.doc.in_flow(node) {
            return Err(Reason::Inline(
                "inline objects need a flow paragraph".into(),
            ));
        }
        self.add_payload(inline_size(object))?;
        let s = self.text(node)?;
        if !s.is_char_boundary(at) {
            return Err(Reason::BadOffset { node, offset: at });
        }
        s.insert(at, OBJECT);
        let width = OBJECT.len_utf8();
        self.shift_inlines(node, at, 0, width);
        let list = self.inlines.entry(node).or_default();
        let index = list.partition_point(|i| i.at < at);
        list.insert(
            index,
            ModelInline {
                at,
                object: object.clone(),
            },
        );
        self.plan.steps.push(Step::InsertInline {
            node,
            at,
            object: object.clone(),
        });
        self.plan.effects.push(PlannedEffect::Text {
            node,
            at,
            removed: 0,
            inserted: width,
        });
        Ok(())
    }

    fn update_inline(
        &mut self,
        node: NodeId,
        at: usize,
        patch: &reprise_doc::InlinePatch,
    ) -> Result<(), Reason> {
        let is_object = self
            .text(node)?
            .get(at..)
            .is_some_and(|s| s.starts_with(OBJECT));
        let no_object = || Reason::BadOffset { node, offset: at };
        if !is_object {
            return Err(no_object());
        }
        let next = {
            let entry = self
                .inlines
                .get(&node)
                .and_then(|list| list.iter().find(|i| i.at == at))
                .ok_or_else(no_object)?;
            patch.apply(&entry.object)
        };
        next.validate().map_err(Reason::Inline)?;
        self.add_payload(inline_size(&next))?;
        if let Some(entry) = self
            .inlines
            .get_mut(&node)
            .and_then(|list| list.iter_mut().find(|i| i.at == at))
        {
            entry.object = next;
        }
        self.plan.steps.push(Step::UpdateInline {
            node,
            at,
            patch: patch.clone(),
        });
        Ok(())
    }

    /// Whether `name` is a live style at this point of the transaction.
    fn is_live_style(&self, name: &str) -> bool {
        match self.styles.get(name) {
            Some(live) => *live,
            None => self
                .doc
                .style_record(name)
                .is_some_and(|r| r.status == reprise_doc::StyleStatus::Live),
        }
    }
    /// The empty name (the document base) is always accepted.
    fn live_style(&self, name: &str) -> Result<(), Reason> {
        if name.is_empty() || self.is_live_style(name) {
            Ok(())
        } else {
            Err(Reason::Style(format!("no live style {name}")))
        }
    }
    /// A name that may be (re)defined: valid, and within the style limit
    /// when it is not live yet.
    fn new_style(&mut self, name: &str) -> Result<(), Reason> {
        reprise_doc::check_style_name(name).map_err(style_reason)?;
        if self.is_live_style(name) {
            return Ok(());
        }
        let mut live: BTreeSet<String> = self.doc.style_names().0.into_iter().collect();
        for (n, alive) in &self.styles {
            if *alive {
                live.insert(n.clone());
            } else {
                live.remove(n);
            }
        }
        if live.len() >= reprise_doc::MAX_STYLES {
            return Err(Reason::StyleLimit {
                max: reprise_doc::MAX_STYLES,
            });
        }
        Ok(())
    }

    /// An annotation this transaction has not already removed.
    fn annotation_unremoved(&self, id: reprise_doc::RangeId) -> Result<(), Reason> {
        if self
            .plan
            .steps
            .iter()
            .any(|s| matches!(s, Step::RemoveAnnotation { id: r } if *r == id))
        {
            return Err(reprise_doc::annotation::AnnotationError::Missing(id).into());
        }
        Ok(())
    }

    fn add_annotation(
        &mut self,
        node: NodeId,
        range: &Range<usize>,
        data: &reprise_doc::annotation::AnnotationData,
    ) -> Result<(), Reason> {
        let text = self.text(node)?;
        if range.is_empty() {
            return Err(reprise_doc::annotation::AnnotationError::Empty.into());
        }
        if text.get(range.clone()).is_none() {
            return Err(Reason::BadRange {
                node,
                range: range.clone(),
            });
        }
        let payload = serde_json::to_vec(data).map_err(|e| Reason::Store(e.to_string()))?;
        self.add_payload(payload.len())?;
        let pending = self
            .plan
            .steps
            .iter()
            .filter(|s| matches!(s, Step::AddAnnotation { .. }))
            .count();
        self.annotation_target(data)?;
        // The document checks the payload, its targets and capacity; the
        // span was checked against the text as this transaction leaves it.
        self.doc
            .check_annotation_data(data, pending)
            .map_err(Reason::from)?;
        self.plan.steps.push(Step::AddAnnotation {
            node,
            range: range.clone(),
            data: data.clone(),
        });
        Ok(())
    }

    /// Overlay structural and annotation deletions on the document's checks.
    /// Text-anchor changes (including split/join and empty-range policies) are
    /// exercised by the shared isolated preflight before publication.
    fn annotation_target(
        &self,
        data: &reprise_doc::annotation::AnnotationData,
    ) -> Result<(), Reason> {
        use reprise_doc::annotation::{AnnotationData, AnnotationError, LinkTarget};
        let live = match data {
            AnnotationData::Link(LinkTarget::Node { node } | LinkTarget::Heading { node }) => {
                self.live(*node)?
            }
            AnnotationData::Link(LinkTarget::Range { range }) => {
                self.annotation_unremoved(*range)?;
                match self.doc.resolve_range(*range) {
                    RangeState::Valid { node, .. } | RangeState::Rebound { node, .. } => {
                        self.live(node)?
                    }
                    RangeState::Missing { .. } => false,
                }
            }
            _ => true,
        };
        if !live {
            return Err(AnnotationError::Payload(
                "link target is not live in the transaction".into(),
            )
            .into());
        }
        Ok(())
    }

    fn current_overrides(&self, node: NodeId) -> Result<Style, Reason> {
        self.require(node)?;
        Ok(self
            .overrides
            .get(&node)
            .cloned()
            .unwrap_or(self.doc.block(node).map_err(Reason::from)?.overrides))
    }
    fn set_overrides(&mut self, node: NodeId, style: Style) -> Result<(), Reason> {
        self.require(node)?;
        self.overrides.insert(node, style.clone());
        self.plan.steps.push(Step::SetOverrides { node, style });
        Ok(())
    }
    fn mark_point(&mut self, node: NodeId, at: usize) -> Result<(), Reason> {
        if !self.text(node)?.is_char_boundary(at) {
            return Err(Reason::BadOffset { node, offset: at });
        }
        Ok(())
    }
    fn mark_capacity(&self) -> Result<(), Reason> {
        let pending = self
            .plan
            .steps
            .iter()
            .filter(|s| matches!(s, Step::AlignLine { .. } | Step::PinLine { .. }))
            .count()
            .saturating_add(
                self.plan
                    .relations
                    .iter()
                    .filter(|r| r.schema == reprise_doc::marks::ALIGNMENT)
                    .count(),
            );
        if self.doc.alignment_count().saturating_add(pending)
            >= reprise_doc::marks::MAX_ALIGNMENT_RELATIONS
        {
            return Err(Reason::MarksLimit);
        }
        Ok(())
    }

    fn insert_text(&mut self, node: NodeId, at: usize, text: &str) -> Result<(), Reason> {
        if text.contains(reprise_doc::text::BREAK) {
            return Err(Reason::Store("U+FDD0 is not text".into()));
        }
        // Refused by the store too, but only after earlier steps wrote: check
        // here, so that the whole transaction is refused.
        if text.contains(OBJECT) {
            return Err(Reason::Inline(
                "U+FFFC is an inline object, not text".into(),
            ));
        }
        self.add_payload(text.len())?;
        let s = self.text(node)?;
        if !s.is_char_boundary(at) {
            return Err(Reason::BadOffset { node, offset: at });
        }
        if text.is_empty() {
            // Valid, and nothing: not a step, so not an undo step either.
            return Ok(());
        }
        s.insert_str(at, text);
        self.shift_inlines(node, at, 0, text.len());
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

    fn format_text(
        &mut self,
        node: NodeId,
        range: &Range<usize>,
        style: &reprise_doc::formatting::TextStyle,
    ) -> Result<(), Reason> {
        use reprise_doc::formatting::{MAX_FORMAT_ORDER, MAX_FORMATS_PER_HOST};
        let text = self.text(node)?;
        if range.is_empty() || text.get(range.clone()).is_none() {
            return Err(Reason::BadRange {
                node,
                range: range.clone(),
            });
        }
        style.validate().map_err(Reason::from)?;
        let payload = serde_json::to_vec(style).map_err(|e| Reason::Store(e.to_string()))?;
        self.add_payload(payload.len())?;
        let (count, order) = self.doc.text_format_capacity(node).map_err(Reason::from)?;
        let pending: Vec<_> = self
            .plan
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::FormatText { node, .. } => Some(*node),
                _ => None,
            })
            .collect();
        if count + pending.iter().filter(|n| n.host() == node.host()).count()
            >= MAX_FORMATS_PER_HOST
            || order.saturating_add(pending.len() as u64) >= MAX_FORMAT_ORDER
        {
            return Err(Reason::Store("text formatting limit".into()));
        }
        self.plan.steps.push(Step::FormatText {
            node,
            range: range.clone(),
            style: style.clone(),
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
        self.shift_inlines(node, range.start, range.len(), 0);
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
        // Earlier text commands may shift the inherited prefix. Count the
        // tail conservatively for break paragraphs whose source head is gone.
        let copy_may_be_needed =
            !self.doc.in_flow(node) || (node.is_break() && !self.doc.is_live(node.host()));
        let s = self.text(node)?;
        if !s.is_char_boundary(at) {
            return Err(Reason::BadOffset { node, offset: at });
        }
        let tail = s.split_off(at);
        if let Some(list) = self.inlines.get_mut(&node) {
            // The tail's objects go with the tail.
            list.retain(|i| i.at < at);
        }
        if copy_may_be_needed {
            // Only a block outside a flow is split by copying its tail.
            self.add_payload(tail.len())?;
        }
        let new = self.plan.created.len();
        self.plan.created.push(Created::Split);
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
        if self.domain(first)? != self.domain(second)? {
            return Err(Reason::CrossDomain { first, second });
        }
        if self.doc.kind_of(first) != self.doc.kind_of(second) {
            return Err(Reason::KindMismatch { first, second });
        }
        let (parent, at) = self.slot_of(second)?;
        if !self.kids(Some(second)).is_empty() {
            return Err(Reason::HasChildren(second));
        }
        let tail = self.text(second)?.clone();
        self.add_payload(tail.len())?;
        let head = self.text(first)?;
        let join_at = head.len();
        head.push_str(&tail);
        if let Some(moved) = self.inlines.remove(&second) {
            // The second paragraph's objects follow its text into the first.
            let list = self.inlines.entry(first).or_default();
            list.extend(moved.into_iter().map(|i| ModelInline {
                at: i.at.saturating_add(join_at),
                object: i.object,
            }));
        }
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
        self.add_payload(block.text.len())?;
        self.check_parent(parent)?;
        if block.text.contains(reprise_doc::text::BREAK) {
            return Err(Reason::Store("U+FDD0 is not text".into()));
        }
        let new = self.plan.created.len();
        let kids = self.kids(parent);
        if index > kids.len() {
            return Err(Reason::BadIndex {
                index,
                len: kids.len(),
            });
        }
        kids.insert(index, Slot::New(new));
        self.plan
            .created
            .push(Created::Staged(Box::new(block.clone())));
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
        for _ in 0..MAX_ANCESTORS {
            match up {
                Some(p) if p == node => return Err(Reason::Cycle(node)),
                Some(p) => up = self.parent_of(p).flatten(),
                None => break,
            }
        }
        if up.is_some() {
            return Err(Reason::TreeDepthLimit {
                node,
                max: MAX_ANCESTORS,
            });
        }
        let (old_parent, at) = self.slot_of(node)?;
        self.kids(old_parent).remove(at);
        let len = self.kids(parent).len();
        if index > len {
            // The transaction is refused whole, but keep the model honest.
            self.kids(old_parent).insert(at, Slot::Real(node));
            return Err(Reason::BadIndex { index, len });
        }
        let split_in_transaction = self.doc.in_flow(node)
            && self.plan.steps.iter().any(|step| {
                matches!(step, Step::Split { node: split, .. } if split.host() == node.host())
            });
        if self.doc.moves_by_copy(node) || split_in_transaction {
            // A paragraph of a flow: its text moves as a copy, and the
            // paragraph is deleted (see `docs/flow.md`).
            let text = self.text(node)?.clone();
            self.add_payload(text.len())?;
            let block = self
                .doc
                .block(node)
                .map_err(|_| Reason::NoSuchBlock(node))?;
            let overrides = self
                .overrides
                .get(&node)
                .cloned()
                .unwrap_or(block.overrides);
            let new = self.plan.created.len();
            self.plan.created.push(Created::Staged(Box::new(NewBlock {
                kind: block.kind,
                style: self.doc.style_reference(node).unwrap_or_default(),
                overrides,
                text,
            })));
            self.kids(parent).insert(index, Slot::New(new));
            self.killed.insert(node);
            self.plan.steps.push(Step::MoveCopy {
                node,
                new,
                parent,
                index,
            });
            self.plan.effects.push(PlannedEffect::Moved { node, new });
            return Ok(());
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
        if relation.schema == reprise_doc::marks::ALIGNMENT {
            self.mark_capacity()?;
        }
        if let Some(owner) = relation.owner {
            self.require(owner).map_err(|r| dead_block(r, owner))?;
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
        let node = |n: NodeId| self.require(n).map_err(|r| dead_block(r, n));
        let range = |r| match self.doc.resolve_range(r) {
            RangeState::Missing { node: None } => Err(Reason::DeadRange(r)),
            RangeState::Valid { node: n, .. }
            | RangeState::Rebound { node: n, .. }
            | RangeState::Missing { node: Some(n) } => node(n),
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
        match e {
            DocError::UncommittedEdit => Reason::UncommittedEdit,
            DocError::EditPreflightDepth(max) => Reason::PreflightDepth { max },
            e => Reason::Store(e.to_string()),
        }
    }
}

fn style_reason(e: DocError) -> Reason {
    Reason::Style(e.to_string())
}

/// Bytes a style edit adds to the transaction payload.
fn style_size(style: &Style) -> usize {
    reprise_doc::STORED_KEYS
        .iter()
        .filter_map(|k| style.stored(k))
        .map(|v| v.len().saturating_add(16))
        .sum()
}

/// Bytes an inline object adds to the transaction payload.
fn inline_size(object: &reprise_doc::InlineObject) -> usize {
    object.params.iter().fold(object.kind.len(), |n, (k, v)| {
        n.saturating_add(k.len())
            .saturating_add(v.len())
            .saturating_add(16)
    })
}

fn dead_block(reason: Reason, node: NodeId) -> Reason {
    match reason {
        Reason::NoSuchBlock(_) => Reason::DeadBlock(node),
        other => other,
    }
}

#[derive(Clone, Debug)]
pub(crate) enum StoryCreation {
    Blocks(Vec<NewBlock>),
    Copy(Box<crate::paste::Prepared>),
}
