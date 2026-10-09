//! Fully validated native paste, staged before its single undoable commit.
use std::collections::{BTreeMap, BTreeSet};

use reprise_diag::{Code, Note};
use reprise_doc::fragment::{CopyBlock, Fragment, FragmentError, FragmentRange};
use reprise_doc::relation::{CopyAction, CopySet, IdMap, plan_copy};
use reprise_doc::{
    BlockKind, DocError, Document, NewBlock, NodeId, RelationId, SchemaRegistry, Style,
};

use crate::{Applied, EditError, Editor, Effect, Reason};

pub const STYLE_CLASH: Code = Code::new("clipboard.style-clash");
pub const RELATION_DROPPED: Code = Code::new("clipboard.relation-dropped");
pub const HOST_RANGE_DROPPED: Code = Code::new("clipboard.host-range-dropped");

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
    host_ranges: Vec<FragmentRange>,
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
        host_ranges: Vec::new(),
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
        prepared.host_ranges = doc
            .copy_fragment(target, &[CopyBlock { node, bytes: None }], schemas)
            .map_err(invalid)?
            .ranges;
    }
    // A clash anywhere in an inheritance chain renames the entire imported style graph.
    // Otherwise a non-clashing child could accidentally inherit the target's parent.
    let mut references: BTreeSet<_> = fragment.styles.keys().cloned().collect();
    for block in &fragment.blocks {
        references.insert(block.style.clone());
        if let Some(parent) = &block.overrides.parent {
            references.insert(parent.clone());
        }
    }
    for style in fragment.styles.values() {
        if let Some(parent) = &style.parent {
            references.insert(parent.clone());
        }
    }
    let clash = fragment
        .styles
        .iter()
        .any(|(n, s)| doc.style(n).is_some_and(|t| t != *s))
        || references
            .iter()
            .any(|n| !fragment.styles.contains_key(n) && doc.style(n).is_some());
    if clash {
        prepared.notes.push(Note::warning(
            STYLE_CLASH,
            "imported style graph renamed to preserve source inheritance",
        ));
    }
    for name in &references {
        let mut candidate = name.clone();
        if clash {
            let mut found = false;
            for serial in 1..=4096 {
                candidate = format!("{name} (paste {serial})");
                if doc.style(&candidate).is_none()
                    && !references.contains(&candidate)
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
    let formats: usize = fragment.blocks.iter().map(|b| b.formatting.len()).sum();
    if formats > 0 {
        let probe = at
            .map(|(n, _)| n)
            .or_else(|| fragment.blocks.first().map(|b| b.id))
            .ok_or_else(|| invalid(FragmentError::Invalid("formatting without blocks".into())))?;
        let (existing, order) = doc.text_format_capacity(probe).map_err(store)?;
        let existing = if at.is_some() { existing } else { 0 };
        if existing.saturating_add(formats) > reprise_doc::formatting::MAX_FORMATS_PER_HOST
            || order.saturating_add(formats as u64) > reprise_doc::formatting::MAX_FORMAT_ORDER
        {
            return Err(invalid(FragmentError::Limit("text formatting")));
        }
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
        host_ranges,
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
    if let Some((node, offset)) = at
        && doc.in_flow(node)
    {
        let plan = FlowPaste {
            node,
            offset,
            parent,
            join_first,
            join_last,
            first,
            last,
            single: roots.len() == 1,
        };
        return write_into_flow(doc, schemas, fragment, &names, styles, plan, result);
    }
    let mut leading = None;
    let mut trailing = None;
    if !join_first && let Some(mut block) = host.clone() {
        block.text = prefix.clone();
        leading = Some(doc.stage_block(&block).map_err(store)?);
        if let Some(new) = leading {
            result.applied.blocks.push(new);
        }
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
        if let Some(raw) = &block.image {
            doc.stage_fragment_image(new, raw).map_err(store)?;
        }
        result.ids.nodes.insert(block.id, new);
        result.applied.blocks.push(new);
    }
    if !join_last && let Some(mut block) = host {
        block.text = suffix;
        trailing = Some(doc.stage_block(&block).map_err(store)?);
        if let Some(new) = trailing {
            result.applied.blocks.push(new);
        }
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
            let move_end = |old: usize, affinity: reprise_doc::text::Affinity| {
                if old < offset
                    || (old == offset && affinity == reprise_doc::text::Affinity::Before)
                {
                    (head, old)
                } else {
                    (tail, tail_start.saturating_add(old.saturating_sub(offset)))
                }
            };
            for range in host_ranges {
                let (start_node, start) = move_end(range.bytes.start, range.policy.start);
                let (end_node, end) = move_end(range.bytes.end, range.policy.end);
                if start_node == end_node {
                    doc.reanchor_fragment_range(
                        range.id,
                        start_node,
                        start..end.max(start),
                        range.policy,
                    )
                    .map_err(store)?;
                } else {
                    result.notes.push(Note::error(HOST_RANGE_DROPPED, format!("range {} would span pasted blocks; the single-block range model cannot represent it", range.id)));
                }
            }
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
    let shifts = if join_first {
        first.map(|id| (id, prefix.len())).into_iter().collect()
    } else {
        BTreeMap::new()
    };
    write_formats(doc, &fragment, &result.ids, &shifts)?;
    Ok(result)
}

fn write_formats(
    doc: &Document,
    fragment: &Fragment,
    ids: &IdMap,
    shifts: &BTreeMap<NodeId, usize>,
) -> Result<(), EditError> {
    for block in &fragment.blocks {
        // Every fragment block was written above; a missing one is a bug,
        // not something document content may turn into a panic.
        let node = *ids.nodes.get(&block.id).ok_or_else(|| {
            invalid(FragmentError::Invalid(
                "formatted block was not written".into(),
            ))
        })?;
        let shift = shifts.get(&block.id).copied().unwrap_or(0);
        for run in &block.formatting {
            let mut style = run.style.clone();
            // A flattened source run replaces inherited inline overrides at
            // the paste position, then inherits unspecified paragraph styling.
            style.reset = true;
            doc.format_text(
                node,
                run.bytes.start + shift..run.bytes.end + shift,
                &style,
                reprise_doc::text::RangePolicy::EXPANDING,
            )
            .map_err(store)?;
        }
    }
    Ok(())
}

/// Where a paste into a paragraph of a flow goes.
struct FlowPaste {
    node: NodeId,
    offset: usize,
    parent: Option<NodeId>,
    /// Whether the first and last pasted roots are plain paragraphs, whose
    /// text joins the paragraph at the caret.
    join_first: bool,
    join_last: bool,
    first: Option<NodeId>,
    last: Option<NodeId>,
    single: bool,
}

/// The block a fragment block becomes, with imported style names.
fn new_block(
    block: &reprise_doc::fragment::FragmentBlock,
    names: &BTreeMap<String, String>,
) -> NewBlock {
    let mut overrides = block.overrides.clone();
    if let Some(parent) = &mut overrides.parent
        && let Some(new) = names.get(parent)
    {
        *parent = new.clone();
    }
    NewBlock {
        kind: block.kind,
        style: names
            .get(&block.style)
            .cloned()
            .unwrap_or_else(|| block.style.clone()),
        overrides,
        text: block.text.clone(),
    }
}

/// Pastes into a paragraph of a flow (see `docs/flow.md`) without copying
/// any of its text, so a collaborator's concurrent edits to it are kept:
/// the paragraph is split at the caret with a break, the first and last
/// pasted paragraphs' text joins the two halves, and the other pasted
/// blocks are staged and placed between them. A one-paragraph paste is a
/// text insertion.
fn write_into_flow(
    doc: &Document,
    schemas: &SchemaRegistry,
    fragment: Fragment,
    names: &BTreeMap<String, String>,
    styles: BTreeMap<String, Style>,
    plan: FlowPaste,
    mut result: Pasted,
) -> Result<Pasted, EditError> {
    let FlowPaste {
        node,
        offset,
        parent,
        join_first,
        join_last,
        first,
        last,
        single,
    } = plan;
    let inline = single && join_first;
    // The split comes first: it is staged, and everything after it may
    // name the new paragraph.
    let tail = if inline {
        None
    } else {
        let tail = doc.split_block(node, offset).map_err(store)?;
        result.applied.blocks.push(tail);
        result.applied.effects.push(Effect::Split {
            node,
            at: offset,
            new: tail,
        });
        Some(tail)
    };
    // Joined text goes into the halves now, so its ranges can be staged.
    let mut joined = BTreeMap::new();
    for block in &fragment.blocks {
        let target = if join_first && Some(block.id) == first {
            Some((node, offset))
        } else if join_last && Some(block.id) == last {
            tail.map(|t| (t, 0))
        } else {
            None
        };
        let Some((target, at)) = target else {
            continue;
        };
        if !block.text.is_empty() {
            doc.block(target)
                .map_err(store)?
                .text
                .insert(at, &block.text)
                .map_err(|e| store(e.into()))?;
            result.applied.effects.push(Effect::Text {
                node: target,
                at,
                removed: 0,
                inserted: block.text.len(),
            });
        }
        if Some(target) == tail {
            let pasted = new_block(block, names);
            doc.set_style_name(target, &pasted.style).map_err(store)?;
            doc.set_overrides(target, &pasted.overrides)
                .map_err(store)?;
        }
        result.ids.nodes.insert(block.id, target);
        joined.insert(block.id, at);
    }
    for block in &fragment.blocks {
        if joined.contains_key(&block.id) {
            continue;
        }
        let new_block = new_block(block, names);
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
        if let Some(raw) = &block.image {
            doc.stage_fragment_image(new, raw).map_err(store)?;
        }
        result.ids.nodes.insert(block.id, new);
        result.applied.blocks.push(new);
    }
    for range in &fragment.ranges {
        let node = result
            .ids
            .nodes
            .get(&range.node)
            .copied()
            .ok_or_else(|| invalid(FragmentError::Invalid("range node".into())))?;
        let shift = joined.get(&range.node).copied().unwrap_or(0);
        let bytes = range.bytes.start.saturating_add(shift)..range.bytes.end.saturating_add(shift);
        let mut new = doc
            .stage_fragment_range(node, bytes.clone(), range.policy)
            .map_err(store)?;
        for _ in 0..=fragment.ranges.len() {
            if !fragment.ranges.iter().any(|r| r.id == new) {
                break;
            }
            new = doc
                .stage_fragment_range(node, bytes.clone(), range.policy)
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
    for (name, style) in styles {
        doc.define_style(&name, &style).map_err(store)?;
    }
    // The staged roots go between the halves, in order.
    let mut root_index = match tail {
        Some(t) => doc
            .children(parent)
            .iter()
            .position(|&n| n == t)
            .ok_or_else(|| invalid(FragmentError::Invalid("split".into())))?,
        None => 0,
    };
    let mut child_indices = BTreeMap::<NodeId, usize>::new();
    for block in &fragment.blocks {
        if joined.contains_key(&block.id) {
            continue;
        }
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
    for new in result.ids.ranges.values() {
        doc.activate_fragment_range(*new).map_err(store)?;
    }
    for new in result.relations.values() {
        doc.restore_relation(*new).map_err(store)?;
    }
    write_formats(doc, &fragment, &result.ids, &joined)?;
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
