//! Fully validated native paste, staged before its single undoable commit.
use std::collections::BTreeMap;

use reprise_diag::{Code, Note};
use reprise_doc::fragment::{Fragment, FragmentError};
use reprise_doc::relation::{CopyAction, CopySet, IdMap, plan_copy};
use reprise_doc::{
    BlockKind, DocError, Document, NewBlock, NodeId, RelationId, SchemaRegistry, Style,
};

use crate::{Applied, EditError, Editor, Effect, Reason};

pub const STYLE_CLASH: Code = Code::new("clipboard.style-clash");
pub const RELATION_DROPPED: Code = Code::new("clipboard.relation-dropped");

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pasted {
    pub applied: Applied,
    pub ids: IdMap,
    /// Relation IDs are separate because the frozen IdMap maps nodes and ranges only.
    pub relations: BTreeMap<RelationId, RelationId>,
    pub notes: Vec<Note>,
}

#[derive(Debug)]
pub(crate) struct Prepared {
    fragment: Fragment,
    at: Option<(NodeId, usize)>,
    parent: Option<NodeId>,
    index: usize,
    styles: BTreeMap<String, Style>,
    names: BTreeMap<String, String>,
    prefix: String,
    suffix: String,
    host: Option<NewBlock>,
    notes: Vec<Note>,
}

fn invalid(error: FragmentError) -> EditError {
    EditError {
        command: None,
        reason: Reason::Fragment(error),
    }
}
fn store(error: DocError) -> EditError {
    EditError {
        command: None,
        reason: Reason::Store(error.to_string()),
    }
}

pub(crate) fn prepare(
    doc: &Document,
    schemas: &SchemaRegistry,
    fragment: &Fragment,
    at: Option<(NodeId, usize)>,
    target: &str,
) -> Result<Prepared, EditError> {
    fragment.validate().map_err(invalid)?;
    if target.is_empty() {
        return Err(invalid(FragmentError::Invalid(
            "empty target namespace".into(),
        )));
    }
    let mut prepared = Prepared {
        fragment: fragment.clone(),
        at,
        parent: None,
        index: doc.blocks().len(),
        styles: BTreeMap::new(),
        names: BTreeMap::new(),
        prefix: String::new(),
        suffix: String::new(),
        host: None,
        notes: Vec::new(),
    };
    if let Some((node, offset)) = at {
        let block = doc.block(node).map_err(|_| EditError {
            command: None,
            reason: Reason::NoSuchBlock(node),
        })?;
        let text = block.text.to_string();
        if !text.is_char_boundary(offset) {
            return Err(EditError {
                command: None,
                reason: Reason::BadOffset { node, offset },
            });
        }
        if !doc.children(Some(node)).is_empty() || doc.table_role(node).map_err(store)?.is_some() {
            return Err(EditError {
                command: None,
                reason: Reason::HasChildren(node),
            });
        }
        prepared.parent = doc.parent_of(node).flatten();
        let mut up = prepared.parent;
        for _ in 0..crate::MAX_ANCESTORS {
            let Some(p) = up else { break };
            up = doc.parent_of(p).flatten();
        }
        if up.is_some() {
            return Err(EditError {
                command: None,
                reason: Reason::TreeDepthLimit {
                    node,
                    max: crate::MAX_ANCESTORS,
                },
            });
        }
        prepared.index = doc
            .children(prepared.parent)
            .iter()
            .position(|&n| n == node)
            .ok_or(EditError {
                command: None,
                reason: Reason::NoSuchBlock(node),
            })?;
        prepared.prefix = text
            .get(..offset)
            .ok_or_else(|| invalid(FragmentError::Invalid("caret".into())))?
            .into();
        prepared.suffix = text
            .get(offset..)
            .ok_or_else(|| invalid(FragmentError::Invalid("caret".into())))?
            .into();
        prepared.host = Some(NewBlock {
            kind: block.kind,
            style: block.style.unwrap_or_default(),
            overrides: block.overrides,
            text: String::new(),
        });
    }
    // A clash anywhere in an inheritance chain renames the entire imported style graph.
    // Otherwise a non-clashing child could accidentally inherit the target's parent.
    let clash = fragment
        .styles
        .iter()
        .any(|(n, s)| doc.style(n).is_some_and(|t| t != *s));
    if clash {
        prepared.notes.push(Note::warning(
            STYLE_CLASH,
            "imported style graph renamed to preserve source inheritance",
        ));
    }
    for name in fragment.styles.keys() {
        let mut candidate = name.clone();
        if clash {
            let mut found = false;
            for serial in 1..=4096 {
                candidate = format!("{name} (paste {serial})");
                if doc.style(&candidate).is_none()
                    && !fragment.styles.contains_key(&candidate)
                    && !prepared.names.values().any(|n| n == &candidate)
                {
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(invalid(FragmentError::Limit("style names")));
            }
        }
        prepared.names.insert(name.clone(), candidate);
    }
    for (name, style) in &fragment.styles {
        let mut style = style.clone();
        if let Some(parent) = &mut style.parent
            && let Some(new) = prepared.names.get(parent)
        {
            *parent = new.clone();
        }
        let new = prepared
            .names
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.clone());
        if doc.style(&new).is_none() {
            prepared.styles.insert(new, style);
        }
    }
    let copied = CopySet {
        nodes: fragment.blocks.iter().map(|b| b.id).collect(),
        ranges: fragment.ranges.iter().map(|r| r.id).collect(),
    };
    let relations: Vec<_> = fragment
        .relations
        .iter()
        .map(|r| (r.id, Ok(r.relation.clone())))
        .collect();
    let actions: BTreeMap<_, _> = plan_copy(schemas, &relations, &copied)
        .into_iter()
        .map(|e| (e.relation, e.action))
        .collect();
    prepared.fragment.relations.retain(|r| {
        let retain = match actions.get(&r.id) {
            Some(CopyAction::Duplicate) => true,
            Some(CopyAction::KeepOutside) => fragment.source == target,
            _ => false,
        };
        if !retain {
            prepared.notes.push(Note::error(
                RELATION_DROPPED,
                format!(
                    "relation {} omitted by copy policy or external document boundary",
                    r.id
                ),
            ));
        }
        retain
    });
    for relation in &prepared.fragment.relations {
        schemas
            .validate(&relation.relation)
            .map_err(|e| EditError {
                command: None,
                reason: Reason::Schema(e),
            })?;
    }
    let total = fragment.blocks.iter().fold(
        prepared.prefix.len().saturating_add(prepared.suffix.len()),
        |n, b| n.saturating_add(b.text.len()),
    );
    if total > crate::MAX_TRANSACTION_BYTES {
        return Err(invalid(FragmentError::Limit("paste text bytes")));
    }
    Ok(prepared)
}

pub(crate) fn write(
    doc: &Document,
    schemas: &SchemaRegistry,
    prepared: Prepared,
) -> Result<Pasted, EditError> {
    let Prepared {
        fragment,
        at,
        parent,
        index,
        styles,
        names,
        prefix,
        suffix,
        host,
        notes,
    } = prepared;
    let mut result = Pasted {
        notes,
        ..Pasted::default()
    };
    if fragment.blocks.is_empty() {
        if at.is_none() && doc.blocks().is_empty() {
            for (name, raw) in &fragment.templates {
                doc.store_raw_page_template(name, raw).map_err(store)?;
            }
            if let Some(choice) = &fragment.template_choice {
                doc.use_page_template(choice).map_err(store)?;
            }
        }
        return Ok(result);
    }
    let roots: Vec<_> = fragment
        .blocks
        .iter()
        .filter(|b| b.parent.is_none())
        .collect();
    let can_join = |b: &&reprise_doc::fragment::FragmentBlock| {
        host.as_ref()
            .is_some_and(|h| h.kind == BlockKind::Paragraph)
            && b.kind == BlockKind::Paragraph
            && b.table.is_none()
            && !fragment
                .blocks
                .iter()
                .any(|child| child.parent == Some(b.id))
    };
    let join_first = roots.first().is_some_and(can_join);
    let join_last = roots.last().is_some_and(can_join);
    let first = roots.first().map(|b| b.id);
    let last = roots.last().map(|b| b.id);
    let mut leading = None;
    let mut trailing = None;
    if !join_first && let Some(mut block) = host.clone() {
        block.text = prefix.clone();
        leading = Some(doc.stage_block(&block).map_err(store)?);
    }
    for block in &fragment.blocks {
        let mut text = block.text.clone();
        if join_first && Some(block.id) == first {
            text.insert_str(0, &prefix);
        }
        if join_last && Some(block.id) == last {
            text.push_str(&suffix);
        }
        let mut overrides = block.overrides.clone();
        if let Some(parent) = &mut overrides.parent
            && let Some(new) = names.get(parent)
        {
            *parent = new.clone();
        }
        let new_block = NewBlock {
            kind: block.kind,
            style: names
                .get(&block.style)
                .cloned()
                .unwrap_or_else(|| block.style.clone()),
            overrides,
            text,
        };
        // Independent documents may use the same peer. Skip any colliding source
        // label by retaining the candidate as an invisible staging tombstone.
        let mut new = doc.stage_block(&new_block).map_err(store)?;
        for _ in 0..=fragment.blocks.len() {
            if !fragment.blocks.iter().any(|b| b.id == new) {
                break;
            }
            new = doc.stage_block(&new_block).map_err(store)?;
        }
        if let Some(raw) = &block.table {
            doc.stage_fragment_table(new, raw).map_err(store)?;
        }
        result.ids.nodes.insert(block.id, new);
        result.applied.blocks.push(new);
    }
    if !join_last && let Some(mut block) = host {
        block.text = suffix;
        trailing = Some(doc.stage_block(&block).map_err(store)?);
    }
    for range in &fragment.ranges {
        let shift = if join_first && Some(range.node) == first {
            prefix.len()
        } else {
            0
        };
        let node = result
            .ids
            .nodes
            .get(&range.node)
            .copied()
            .ok_or_else(|| invalid(FragmentError::Invalid("range node".into())))?;
        let mut new = doc
            .stage_fragment_range(
                node,
                range.bytes.start.saturating_add(shift)..range.bytes.end.saturating_add(shift),
                range.policy,
            )
            .map_err(store)?;
        for _ in 0..=fragment.ranges.len() {
            if !fragment.ranges.iter().any(|r| r.id == new) {
                break;
            }
            new = doc
                .stage_fragment_range(
                    node,
                    range.bytes.start.saturating_add(shift)..range.bytes.end.saturating_add(shift),
                    range.policy,
                )
                .map_err(store)?;
        }
        result.ids.ranges.insert(range.id, new);
    }
    for relation in &fragment.relations {
        let mut new = doc
            .stage_relation(schemas, &relation.relation.remapped(&result.ids))
            .map_err(store)?;
        for _ in 0..=fragment.relations.len() {
            if !fragment.relations.iter().any(|r| r.id == new) {
                break;
            }
            new = doc
                .stage_relation(schemas, &relation.relation.remapped(&result.ids))
                .map_err(store)?;
        }
        result.relations.insert(relation.id, new);
        result.applied.relations.push(new);
    }
    // All staging is finished. No function below commits until the caller commits the step.
    for (name, style) in styles {
        doc.define_style(&name, &style).map_err(store)?;
    }
    if at.is_none() && doc.blocks().is_empty() {
        for (name, raw) in &fragment.templates {
            doc.store_raw_page_template(name, raw).map_err(store)?;
        }
        if let Some(choice) = &fragment.template_choice {
            doc.use_page_template(choice).map_err(store)?;
        }
    }
    let mut root_index = index;
    if let Some(new) = leading {
        doc.activate_block_at(new, parent, root_index)
            .map_err(store)?;
        root_index += 1;
    }
    let mut child_indices = BTreeMap::<NodeId, usize>::new();
    for block in &fragment.blocks {
        let new = result
            .ids
            .nodes
            .get(&block.id)
            .copied()
            .ok_or_else(|| invalid(FragmentError::Invalid("staged node".into())))?;
        if let Some(old_parent) = block.parent {
            let new_parent = result
                .ids
                .nodes
                .get(&old_parent)
                .copied()
                .ok_or_else(|| invalid(FragmentError::Invalid("staged parent".into())))?;
            let child_index = child_indices.entry(new_parent).or_default();
            doc.activate_block_at(new, Some(new_parent), *child_index)
                .map_err(store)?;
            *child_index += 1;
        } else {
            doc.activate_block_at(new, parent, root_index)
                .map_err(store)?;
            root_index += 1;
        }
    }
    if let Some(new) = trailing {
        doc.activate_block_at(new, parent, root_index)
            .map_err(store)?;
    }
    for new in result.ids.ranges.values() {
        doc.activate_fragment_range(*new).map_err(store)?;
    }
    for new in result.relations.values() {
        doc.restore_relation(*new).map_err(store)?;
    }
    if let Some((node, offset)) = at {
        // Position effects mirror a split and join, while retaining fresh pasted identities.
        let head = leading.or_else(|| first.and_then(|n| result.ids.nodes.get(&n).copied()));
        let tail = trailing.or_else(|| last.and_then(|n| result.ids.nodes.get(&n).copied()));
        if let (Some(head), Some(tail)) = (head, tail) {
            result.applied.effects.push(Effect::Split {
                node,
                at: offset,
                new: tail,
            });
            let tail_start = if join_last {
                roots.last().map_or(0, |b| b.text.len()).saturating_add(
                    if first == last && join_first {
                        prefix.len()
                    } else {
                        0
                    },
                )
            } else {
                0
            };
            result.applied.effects.push(Effect::Text {
                node: tail,
                at: 0,
                removed: 0,
                inserted: tail_start,
            });
            result.applied.effects.push(Effect::Join {
                first: head,
                second: node,
                at: 0,
            });
            doc.supersede(node, head).map_err(store)?;
        }
        doc.delete_block(node).map_err(store)?;
    }
    Ok(result)
}

impl Editor {
    /// Paste as one transaction and undo step, returning all identity remappings and losses.
    /// `at = None` appends whole roots; an empty target additionally receives page setup.
    pub fn paste(
        &mut self,
        fragment: &Fragment,
        at: Option<(NodeId, usize)>,
        target_namespace: &str,
    ) -> Result<Pasted, EditError> {
        let prepared = prepare(&self.doc, &self.schemas, fragment, at, target_namespace)?;
        let result = write(&self.doc, &self.schemas, prepared);
        self.doc.commit_step();
        result
    }
}
