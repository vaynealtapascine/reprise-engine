//! Roles and lists in the transaction model (see `docs/roles-and-lists.md`).
//!
//! Membership the transaction changed is tracked in `Model::members`, so a
//! command sees what the commands before it did: Enter after an indent sees
//! the new level, and an item added to a list deleted earlier is refused.

use reprise_doc::BlockKind;
use reprise_doc::lists::MAX_LEVEL;

use super::{Model, Step};
use crate::command::{Command, ListRef};
use crate::error::Reason;
use reprise_doc::NodeId;

impl Model<'_> {
    pub(super) fn semantic(&mut self, command: &Command) -> Result<(), Reason> {
        match command {
            Command::SetRole { node, role } => {
                self.require(*node)?;
                if !matches!(
                    self.doc.kind_of(*node),
                    Some(BlockKind::Paragraph | BlockKind::Annotation)
                ) {
                    return Err(Reason::NotText(*node));
                }
                if !role.is_valid() {
                    return Err(Reason::BadRole);
                }
                self.plan.steps.push(Step::SetRole {
                    node: *node,
                    role: *role,
                });
                Ok(())
            }
            Command::CreateList { definition } => {
                definition
                    .validate()
                    .map_err(|e| Reason::InvalidList(e.to_string()))?;
                self.check_base(definition)?;
                let new = self.plan.lists.len();
                self.add_payload(
                    serde_json::to_vec(definition)
                        .map_err(|e| Reason::Store(e.to_string()))?
                        .len(),
                )?;
                self.plan.lists.push(definition.clone());
                self.plan.steps.push(Step::ActivateList { new });
                Ok(())
            }
            Command::UpdateList { list, definition } => {
                self.list_live(ListRef::Existing(*list))?;
                definition
                    .validate()
                    .map_err(|e| Reason::InvalidList(e.to_string()))?;
                self.check_base(definition)?;
                self.add_payload(
                    serde_json::to_vec(definition)
                        .map_err(|e| Reason::Store(e.to_string()))?
                        .len(),
                )?;
                self.plan.steps.push(Step::UpdateList {
                    list: *list,
                    definition: definition.clone(),
                });
                Ok(())
            }
            Command::DeleteList { list } => {
                self.list_live(*list)?;
                self.dead_lists.insert(*list);
                self.plan.steps.push(Step::DeleteList { list: *list });
                Ok(())
            }
            Command::SetListItem { node, list, level } => self.set_item(*node, *list, *level),
            Command::RemoveListItem { node } => {
                self.require(*node)?;
                self.remove_item(*node);
                Ok(())
            }
            Command::SetListLevel { node, level } => {
                let (list, _) = self.item(*node)?.ok_or(Reason::NotAListItem(*node))?;
                self.set_item(*node, list, *level)
            }
            Command::IndentListItem { node, by } => {
                let Some((list, level)) = self.item(*node)? else {
                    return Ok(());
                };
                let next = (i16::from(level) + i16::from(*by)).clamp(0, i16::from(MAX_LEVEL));
                if next as u8 != level {
                    self.set_item(*node, list, next as u8)?;
                }
                Ok(())
            }
            Command::RestartList { node, value } => {
                self.item(*node)?.ok_or(Reason::NotAListItem(*node))?;
                self.plan.steps.push(Step::SetListRestart {
                    node: *node,
                    value: *value,
                });
                Ok(())
            }
            Command::ContinueList { node } => self.continue_list(*node),
            Command::Enter { node, at } => {
                if !self.text(*node)?.is_char_boundary(*at) {
                    return Err(Reason::BadOffset {
                        node: *node,
                        offset: *at,
                    });
                }
                match self.item(*node)? {
                    Some((list, level)) if self.text(*node)?.is_empty() => {
                        if level > 0 {
                            self.set_item(*node, list, level - 1)
                        } else {
                            self.remove_item(*node);
                            Ok(())
                        }
                    }
                    _ => self.split(*node, *at),
                }
            }
            Command::BackspaceAtStart { node } => {
                if self.item(*node)?.is_some() {
                    self.remove_item(*node);
                    return Ok(());
                }
                let (parent, index) = self.slot_of(*node)?;
                let previous = index
                    .checked_sub(1)
                    .and_then(|i| self.kids(parent).get(i).copied());
                match previous {
                    Some(super::Slot::Real(prev))
                        if self.doc.kind_of(prev) == self.doc.kind_of(*node)
                            && self.doc.table_role(prev).ok().flatten().is_none() =>
                    {
                        self.join(prev, *node)
                    }
                    // Nothing joinable before it: Backspace does nothing.
                    _ => Ok(()),
                }
            }
            _ => Err(Reason::Store("unhandled command".into())),
        }
    }

    fn check_base(&self, definition: &reprise_doc::ListDefinition) -> Result<(), Reason> {
        match definition.base {
            Some(base) => self.list_live(ListRef::Existing(base)),
            None => Ok(()),
        }
    }

    fn list_live(&self, list: ListRef) -> Result<(), Reason> {
        let live = !self.dead_lists.contains(&list)
            && match list {
                ListRef::Existing(id) => self.doc.is_list_live(id),
                ListRef::New(i) => i < self.plan.lists.len(),
            };
        if live {
            Ok(())
        } else {
            Err(Reason::NoSuchList(list))
        }
    }

    /// A block's list and level now, if it is an item of a live list.
    fn item(&mut self, node: NodeId) -> Result<Option<(ListRef, u8)>, Reason> {
        self.require(node)?;
        let current = match self.members.get(&node) {
            Some(m) => *m,
            None => self
                .doc
                .list_item(node)
                .map(|i| (ListRef::Existing(i.list), i.level)),
        };
        Ok(current.filter(|(list, _)| self.list_live(*list).is_ok()))
    }

    fn set_item(&mut self, node: NodeId, list: ListRef, level: u8) -> Result<(), Reason> {
        self.require(node)?;
        if level > MAX_LEVEL {
            return Err(Reason::BadLevel(level));
        }
        if !matches!(
            self.doc.kind_of(node),
            Some(BlockKind::Paragraph | BlockKind::Annotation)
        ) || self.doc.table_role(node).ok().flatten().is_some()
        {
            return Err(Reason::NotText(node));
        }
        self.list_live(list)?;
        self.members.insert(node, Some((list, level)));
        self.plan
            .steps
            .push(Step::SetListItem { node, list, level });
        Ok(())
    }

    fn remove_item(&mut self, node: NodeId) {
        self.members.insert(node, None);
        self.plan.steps.push(Step::RemoveListItem { node });
    }

    fn continue_list(&mut self, node: NodeId) -> Result<(), Reason> {
        let (own, _) = self.item(node)?.ok_or(Reason::NotAListItem(node))?;
        // An earlier command in this transaction may have set a restart.
        self.plan
            .steps
            .push(Step::SetListRestart { node, value: None });
        let order = self.doc.order(reprise_doc::Domain::Body);
        let Some(at) = order.iter().position(|&n| n == node) else {
            return Ok(());
        };
        let mut previous = None;
        for &n in order.iter().take(at).rev() {
            if self.live(n)?
                && let Some((list, _)) = self.item(n)?
            {
                previous = Some(list);
                break;
            }
        }
        let Some(target) = previous.filter(|&l| l != own) else {
            return Ok(());
        };
        self.list_live(target)?;
        for &n in order.iter().skip(at) {
            if self.live(n)?
                && let Some((list, level)) = self.item(n)?
                && list == own
            {
                self.set_item(n, target, level)?;
            }
        }
        Ok(())
    }
}
