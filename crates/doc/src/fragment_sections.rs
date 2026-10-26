//! The sections envelope of a fragment (sections §14, N7).
//!
//! `Fragment.sections` is a version-1 JSON envelope that carries what a copy of
//! the body needs to rebuild its sections elsewhere: the section boundaries
//! inside the copy, `first`'s own keys, the raw geometry-mode record, and the
//! stories those records bind, each once, in one table. Lifetimes never travel:
//! a lifetime is an operation ID of the source, and means nothing anywhere
//! else. Paste mints a fresh one for every record it writes.
//!
//! Story blocks are ordinary fragment blocks that live in the envelope, so the
//! one machinery that validates and pastes blocks, ranges, annotations, lists,
//! formatting and inline objects serves bodies and stories alike. See
//! [`Fragment::flattened`].

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fragment::{
    CopyBlock, CopyScope, Fragment, FragmentBlock, FragmentError, FragmentRange,
};
use crate::sections::{
    BreakKind, SectionId, SectionProperties, StartRecord, binding_key, validate_properties,
};
use crate::{Document, Domain, NodeId, SchemaRegistry};

pub const ENVELOPE_VERSION: u32 = 1;
/// Section records one envelope carries.
pub const MAX_BOUNDARIES: usize = 16_384;
/// Property and binding keys of one record.
pub const MAX_RECORD_KEYS: usize = 64;
/// Bytes of one property value.
pub const MAX_VALUE_BYTES: usize = 2048;
/// Distinct stories one envelope carries, however they are referenced: bound
/// by a boundary, by `first`, or by nothing at all. It replaces the first
/// draft's 256 (the frames design needs room for thousands of chains).
pub const MAX_STORIES: usize = 16_384;

/// The names of the limits an envelope can exceed. A paste refuses over any of
/// them as a copy limit, before it writes anything.
pub const LIMITS: [&str; 4] = [
    "section boundaries",
    "section keys",
    "section value",
    "transferred stories",
];

/// Whether `limit` is one of the envelope's own.
pub fn is_envelope_limit(limit: &str) -> bool {
    LIMITS.contains(&limit)
}

/// What a copy covered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvelopeMode {
    /// A part of the body: only the boundaries strictly inside it.
    Partial,
    /// The whole body: also the leading boundary, inactive records, `first`'s
    /// context and the geometry-mode record.
    Complete,
}

/// A region binding, as the envelope holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Binding {
    /// Explicitly no region.
    None,
    /// A story of the envelope's table, by label.
    Story { label: String },
    /// A value that names no live story. It is kept as it was: it still
    /// renders empty, and is still reported.
    Raw { value: String },
}

/// `first`'s keys, or the keys of one boundary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    /// Raw keys of the source's current lifetime, unknown ones included, minus
    /// the region bindings.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub props: SectionProperties,
    /// The `region.<slot>.<variant>` keys.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, Binding>,
}

/// A section, page or column starting at a block of the fragment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
    /// The fragment-local block ID.
    pub block: NodeId,
    /// The source record's `active` flag. A partial envelope only ever holds
    /// `true`.
    pub active: bool,
    #[serde(rename = "break")]
    pub kind: BreakKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub props: SectionProperties,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, Binding>,
}

/// One transferred story. Its blocks are fragment blocks; its roots have no
/// parent in the envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Story {
    pub label: String,
    /// The story in the document it was copied from. A paste into that same
    /// document refers to it instead of copying it, while it is still live.
    pub source: NodeId,
    pub blocks: Vec<FragmentBlock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<FragmentRange>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub v: u32,
    pub mode: EnvelopeMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boundaries: Vec<Boundary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Record>,
    /// The raw `geometry1` record. Complete envelopes only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<SectionProperties>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stories: Vec<Story>,
    /// Records of the source that no block of the copy could carry (a dead
    /// block, one outside the body): inert there, so inert nowhere here.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dropped: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn invalid(why: impl Into<String>) -> FragmentError {
    FragmentError::Invalid(why.into())
}

impl Envelope {
    /// Reads the envelope. An envelope of another version is refused with the
    /// version error, whole: this reader can't remap what it doesn't know.
    pub fn parse(raw: &str) -> Result<Envelope, FragmentError> {
        if raw.len() > crate::fragment::MAX_FRAGMENT_BYTES {
            return Err(FragmentError::Limit("payload bytes"));
        }
        let version = serde_json::from_str::<Value>(raw)
            .map_err(|e| invalid(format!("sections envelope: {e}")))?
            .get("v")
            .and_then(Value::as_u64);
        match version {
            Some(v) if v == u64::from(ENVELOPE_VERSION) => {}
            Some(v) => return Err(FragmentError::Version(u32::try_from(v).unwrap_or(u32::MAX))),
            None => return Err(invalid("sections envelope has no version")),
        }
        serde_json::from_str(raw).map_err(|e| invalid(format!("sections envelope: {e}")))
    }

    pub fn encode(&self) -> Result<String, FragmentError> {
        serde_json::to_string(self).map_err(|e| invalid(e.to_string()))
    }

    /// Checks everything about the envelope that doesn't need the blocks'
    /// content: counts, keys, labels and references. `fragment` supplies the
    /// body blocks the boundaries sit on.
    pub fn validate(&self, fragment: &Fragment) -> Result<(), FragmentError> {
        if self.stories.len() > MAX_STORIES {
            return Err(FragmentError::Limit("transferred stories"));
        }
        if self.boundaries.len() > MAX_BOUNDARIES {
            return Err(FragmentError::Limit("section boundaries"));
        }
        let mut labels = BTreeSet::new();
        let mut sources = BTreeSet::new();
        for story in &self.stories {
            if story.label.is_empty() || story.label.len() > 64 || !labels.insert(&*story.label) {
                return Err(invalid("story labels are unique and 1 to 64 bytes"));
            }
            if !sources.insert(story.source) {
                return Err(invalid("a story is carried once"));
            }
        }
        let roots: BTreeSet<_> = fragment
            .blocks
            .iter()
            .filter(|b| b.parent.is_none())
            .map(|b| b.id)
            .collect();
        let mut at = BTreeSet::new();
        for b in &self.boundaries {
            if !roots.contains(&b.block) || !at.insert(b.block) {
                return Err(invalid(
                    "a boundary sits on a top-level block of the copy, once",
                ));
            }
            if b.start.as_ref().is_some_and(|s| s.len() > MAX_VALUE_BYTES) {
                return Err(FragmentError::Limit("section value"));
            }
            check_keys(&b.props, &b.bindings, &labels)?;
            if self.mode == EnvelopeMode::Partial && !b.active {
                return Err(invalid("a partial envelope holds only active boundaries"));
            }
        }
        if let Some(context) = &self.context {
            check_keys(&context.props, &context.bindings, &labels)?;
        }
        if let Some(geometry) = &self.geometry {
            if self.mode == EnvelopeMode::Partial {
                return Err(invalid("geometry travels in a complete envelope"));
            }
            check_keys(geometry, &BTreeMap::new(), &labels)?;
        }
        Ok(())
    }

    /// The whole envelope against the fragment it travels in, before anything
    /// is planned or staged: the records ([`Envelope::validate`]), that every
    /// block, parent and range endpoint stays in the body or the one story
    /// that carries it, and the aggregate limits of the body and every story
    /// together, whether or not a paste will use each of them.
    pub fn validate_in(&self, fragment: &Fragment) -> Result<(), FragmentError> {
        self.validate(fragment)?;
        self.validate_ownership(fragment)?;
        fragment.flattened(self).validate()
    }

    /// Each component, the body and every story, is a tree of its own: a
    /// block's parent is an earlier block of the same component, IDs are
    /// unique across all of them, and a range, or the end of one, sits in the
    /// component that carries it. [`Fragment::flattened`] concatenates the
    /// components, and the flat validator accepts any earlier parent in that
    /// list, so a story block could otherwise be pasted into the body or into
    /// another story. A relation hangs on a block of the body: copies drop the
    /// ones a story owns.
    fn validate_ownership(&self, fragment: &Fragment) -> Result<(), FragmentError> {
        let mut seen = BTreeSet::new();
        let body = component_ids(&fragment.blocks, &mut seen)?;
        let in_component = |range: &FragmentRange, own: &BTreeSet<NodeId>| {
            own.contains(&range.node) && range.end.as_ref().is_none_or(|e| own.contains(&e.node))
        };
        if !fragment.ranges.iter().all(|r| in_component(r, &body)) {
            return Err(invalid("a range of the body ends in a story"));
        }
        if fragment
            .relations
            .iter()
            .any(|r| r.relation.owner.is_some_and(|o| !body.contains(&o)))
        {
            return Err(invalid("a relation is owned by a block outside the body"));
        }
        for story in &self.stories {
            let own = component_ids(&story.blocks, &mut seen)?;
            if !story.ranges.iter().all(|r| in_component(r, &own)) {
                return Err(invalid("a range of a story ends outside it"));
            }
        }
        Ok(())
    }

    /// The story labels carried by roots of the table, in order.
    pub fn story_roots(&self) -> BTreeMap<NodeId, usize> {
        let mut out = BTreeMap::new();
        for (index, story) in self.stories.iter().enumerate() {
            for block in story.blocks.iter().filter(|b| b.parent.is_none()) {
                out.insert(block.id, index);
            }
        }
        out
    }
}

/// The IDs of one component's blocks, if each parent is an earlier block of
/// that component and none repeats an ID already `seen` in any component.
fn component_ids(
    blocks: &[FragmentBlock],
    seen: &mut BTreeSet<NodeId>,
) -> Result<BTreeSet<NodeId>, FragmentError> {
    let mut own = BTreeSet::new();
    for block in blocks {
        if block.parent.is_some_and(|p| !own.contains(&p)) {
            return Err(invalid(
                "a block's parent is not an earlier block of its own body or story",
            ));
        }
        if !seen.insert(block.id) {
            return Err(invalid("duplicate node ID"));
        }
        own.insert(block.id);
    }
    Ok(own)
}

fn check_keys(
    props: &SectionProperties,
    bindings: &BTreeMap<String, Binding>,
    labels: &BTreeSet<&str>,
) -> Result<(), FragmentError> {
    if props.len().saturating_add(bindings.len()) > MAX_RECORD_KEYS {
        return Err(FragmentError::Limit("section keys"));
    }
    for (key, value) in props {
        if matches!(key.as_str(), "break" | "start")
            || (binding_key(key) && matches!(value, Value::String(_)))
        {
            return Err(invalid(format!("{key} is not a plain property")));
        }
        if matches!(value, Value::String(s) if s.len() > MAX_VALUE_BYTES) {
            return Err(FragmentError::Limit("section value"));
        }
    }
    validate_properties(props, &[]).map_err(|e| invalid(e.to_string()))?;
    for (key, binding) in bindings {
        if !binding_key(key) {
            return Err(invalid(format!("{key} is not a region binding")));
        }
        match binding {
            Binding::None => {}
            Binding::Story { label } => {
                if !labels.contains(label.as_str()) {
                    return Err(invalid(format!("binding names the unknown story {label}")));
                }
            }
            Binding::Raw { value } => {
                if value.len() > MAX_VALUE_BYTES {
                    return Err(FragmentError::Limit("section value"));
                }
            }
        }
    }
    Ok(())
}

impl Fragment {
    /// The envelope, if the fragment has one.
    pub fn envelope(&self) -> Result<Option<Envelope>, FragmentError> {
        self.sections.as_deref().map(Envelope::parse).transpose()
    }

    /// This fragment with the envelope's stories' blocks and ranges appended
    /// to its own, and no envelope. Story roots keep no parent; the envelope's
    /// [`Envelope::story_roots`] says which story each belongs to.
    pub fn flattened(&self, envelope: &Envelope) -> Fragment {
        self.flattened_for(envelope, &vec![true; envelope.stories.len()])
    }

    /// As [`Fragment::flattened`], with only the stories `keep` names. A story
    /// a paste refers to in place has nothing to paste.
    pub fn flattened_for(&self, envelope: &Envelope, keep: &[bool]) -> Fragment {
        let mut flat = self.clone();
        flat.sections = None;
        for (story, keep) in envelope.stories.iter().zip(keep) {
            if *keep {
                flat.blocks.extend(story.blocks.iter().cloned());
                flat.ranges.extend(story.ranges.iter().cloned());
            }
        }
        flat
    }
}

/// Stories, labelled in the order they are first referenced.
#[derive(Default)]
struct Labels {
    order: Vec<NodeId>,
    by_story: BTreeMap<NodeId, String>,
}

impl Labels {
    fn of(&mut self, story: NodeId) -> String {
        if let Some(label) = self.by_story.get(&story) {
            return label.clone();
        }
        let label = format!("s{}", self.order.len());
        self.order.push(story);
        self.by_story.insert(story, label.clone());
        label
    }
}

/// What a region cell resolves to at some point of the body.
#[derive(Clone, Debug)]
enum Cell {
    Story(NodeId),
    None,
    Raw(String),
}

impl Document {
    /// Adds the sections envelope to a fragment just copied from this
    /// document's body. `whole` says the copy is the whole body, so the
    /// envelope is complete.
    pub(crate) fn attach_sections(
        &self,
        source: &str,
        fragment: &mut Fragment,
        schemas: &SchemaRegistry,
        scope: CopyScope,
        orphans: bool,
        selection: &[CopyBlock],
    ) -> Result<(), FragmentError> {
        let roots: Vec<NodeId> = fragment
            .blocks
            .iter()
            .filter(|b| b.parent.is_none())
            .map(|b| b.id)
            .collect();
        if roots
            .iter()
            .any(|r| self.domain_of(*r) != Some(Domain::Body))
        {
            // A copy inside a story has content, not a place: no sections.
            return Ok(());
        }
        if !self.has_sections() {
            return Ok(());
        }
        let body = self.top(Domain::Body);
        // The whole body, every block whole and in order, is a complete copy,
        // however it was asked for.
        let whole = selection.len() == body.len()
            && selection
                .iter()
                .zip(&body)
                .all(|(s, n)| s.bytes.is_none() && s.node == *n);
        let top: BTreeSet<NodeId> = body.into_iter().collect();
        let records: BTreeMap<SectionId, StartRecord> = self
            .start_records()
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        let live: BTreeSet<NodeId> = self.stories().into_iter().collect();
        let mut labels = Labels::default();
        let mut boundaries = Vec::new();
        let mut dropped = 0usize;
        let on_root: BTreeSet<NodeId> = roots.iter().copied().collect();
        for (index, &root) in roots.iter().enumerate() {
            if !top.contains(&root) {
                continue;
            }
            let Some(record) = records.get(&SectionId::Block(root)) else {
                continue;
            };
            let readable = record.life.is_some() && record.kind.is_some();
            let effective = record.active && readable;
            // A boundary at the start of a partial copy stays behind, as it
            // does when the text is deleted.
            if if whole {
                !readable
            } else {
                index == 0 || !effective
            } {
                continue;
            }
            boundaries.push(self.boundary_of(root, record, &live, &mut labels)?);
        }
        if whole {
            // A record whose block is not in the copy has nowhere to sit.
            dropped = records
                .iter()
                .filter(|(id, r)| {
                    matches!(id, SectionId::Block(b) if !on_root.contains(b)) && r.life.is_some()
                })
                .count()
                + records
                    .iter()
                    .filter(|(id, r)| {
                        matches!(id, SectionId::Block(b) if on_root.contains(b))
                            && r.life.is_some()
                            && r.kind.is_none()
                    })
                    .count();
        } else {
            self.materialise_inherited(&mut boundaries, &live, &mut labels)?;
        }
        let context = if whole {
            let first = records.get(&SectionId::First);
            if first.is_some_and(|r| r.unreadable) {
                return Err(invalid("a section record holds a container"));
            }
            first.filter(|r| !r.own.is_empty()).map(|r| {
                let (props, bindings) = split_bindings(&r.own, &live, &mut labels);
                Record { props, bindings }
            })
        } else {
            None
        };
        // The raw record, every key as it is. One the copy can't carry (a
        // container, a null, a number that is not an integer) is refused, as a
        // section record's is, and never dropped: not the key, not the record.
        let geometry = if whole {
            match self.geometry_mode_record() {
                Some(record) if !record.values().all(is_plain) => {
                    return Err(invalid(
                        "the geometry record holds a value a copy can't carry",
                    ));
                }
                record => record,
            }
        } else {
            None
        };
        if whole && orphans {
            // The orphans too, which stay orphans.
            for story in self.stories() {
                labels.of(story);
            }
        }
        // Counted before any story is copied: past the limit, nothing is.
        if labels.order.len() > MAX_STORIES {
            return Err(FragmentError::Limit("transferred stories"));
        }
        if boundaries.len() > MAX_BOUNDARIES {
            return Err(FragmentError::Limit("section boundaries"));
        }
        let mut stories = self.copy_stories(source, &labels, schemas, scope, fragment)?;
        self.follow_body_targets(&mut stories, fragment, selection);
        if boundaries.is_empty()
            && context.is_none()
            && geometry.is_none()
            && stories.is_empty()
            && dropped == 0
        {
            return Ok(());
        }
        let envelope = Envelope {
            v: ENVELOPE_VERSION,
            mode: if whole {
                EnvelopeMode::Complete
            } else {
                EnvelopeMode::Partial
            },
            boundaries,
            context,
            geometry,
            stories,
            dropped,
        };
        fragment.sections = Some(envelope.encode()?);
        // The story blocks count against the fragment's own block, range and
        // size limits as if they were pasted blocks: they are.
        envelope.validate_in(fragment)
    }

    fn boundary_of(
        &self,
        block: NodeId,
        record: &StartRecord,
        live: &BTreeSet<NodeId>,
        labels: &mut Labels,
    ) -> Result<Boundary, FragmentError> {
        if record.unreadable {
            return Err(invalid("a section record holds a container"));
        }
        let Some(kind) = record.kind else {
            return Err(invalid("a section record has no readable break"));
        };
        let mut own = record.own.clone();
        own.remove("break");
        let start = match own.remove("start") {
            Some(Value::String(s)) => Some(s),
            Some(other) => {
                // Not a token: kept as an ordinary property.
                own.insert("start".into(), other);
                None
            }
            None => None,
        };
        // `start` is read from the record, so a non-string stays in `props`:
        // `check_keys` refuses it there, which refuses the copy rather than
        // lose it.
        let (props, bindings) = split_bindings(&own, live, labels);
        Ok(Boundary {
            block,
            active: record.active,
            kind,
            start,
            props,
            bindings,
        })
    }

    /// At the first section boundary of a partial copy, every region cell it
    /// leaves to "same as previous" is written as what it is here, so the
    /// paste never picks up headers of its destination (sections §14).
    fn materialise_inherited(
        &self,
        boundaries: &mut [Boundary],
        live: &BTreeSet<NodeId>,
        labels: &mut Labels,
    ) -> Result<(), FragmentError> {
        let Some(first) = boundaries.iter_mut().find(|b| b.kind == BreakKind::Section) else {
            return Ok(());
        };
        let mut cells: BTreeMap<String, Cell> = BTreeMap::new();
        for view in self.section_table() {
            if view.id == SectionId::Block(first.block) {
                break;
            }
            for (key, value) in &view.own {
                if !binding_key(key) {
                    continue;
                }
                let cell = match value {
                    Value::String(s) if s.is_empty() => Cell::None,
                    Value::String(s) => match NodeId::parse(s) {
                        Some(story) if live.contains(&story) => Cell::Story(story),
                        _ => Cell::Raw(s.clone()),
                    },
                    other => Cell::Raw(other.to_string()),
                };
                cells.insert(key.clone(), cell);
            }
        }
        for slot in ["header", "footer", "background"] {
            for variant in ["default", "first", "even"] {
                let key = format!("region.{slot}.{variant}");
                if first.bindings.contains_key(&key) || first.props.contains_key(&key) {
                    continue;
                }
                let binding = match cells.get(&key) {
                    Some(Cell::Story(story)) => Binding::Story {
                        label: labels.of(*story),
                    },
                    Some(Cell::Raw(value)) => Binding::Raw {
                        value: value.clone(),
                    },
                    Some(Cell::None) | None => Binding::None,
                };
                first.bindings.insert(key, binding);
            }
        }
        Ok(())
    }

    /// One story as an envelope entry: its blocks and ranges as the ordinary
    /// copy makes them. Styles, lists and notes join the fragment's own.
    /// A `page-of` field in a story that is aimed at text of the body copy
    /// follows the copy, as one in the body does: it is written as a target
    /// inside the fragment, for the paste to remap. (The copy of the story is
    /// made apart from the body's, so it could not see those blocks.)
    fn follow_body_targets(
        &self,
        stories: &mut [Story],
        fragment: &Fragment,
        selection: &[CopyBlock],
    ) {
        // Where each copied body block starts in its source, and ends.
        let mut cuts: BTreeMap<NodeId, std::ops::Range<usize>> = BTreeMap::new();
        for block in &fragment.blocks {
            let start = match block.parent {
                None => selection
                    .iter()
                    .find(|s| s.node == block.id)
                    .and_then(|s| s.bytes.as_ref())
                    .map_or(0, |bytes| bytes.start),
                Some(_) => 0,
            };
            cuts.insert(block.id, start..start.saturating_add(block.text.len()));
        }
        if cuts.is_empty() {
            return;
        }
        for object in stories
            .iter_mut()
            .flat_map(|s| s.blocks.iter_mut())
            .flat_map(|b| b.inline.iter_mut())
        {
            if object.kind != crate::inline::FIELD || object.param("field") != Some("page-of") {
                continue;
            }
            let Some((node, at)) = object
                .param("target")
                .and_then(|raw| self.resolve_field_target(raw))
            else {
                continue;
            };
            let Some(cut) = cuts.get(&node) else {
                continue;
            };
            // As for a body field: the end of a partial cut is the next
            // character, which stays behind.
            if cut.start <= at
                && (at < cut.end
                    || (at == cut.end && self.block(node).is_ok_and(|b| b.text.len() == cut.end)))
            {
                object.params.insert(
                    "target".into(),
                    format!("copy:{node} {}", at.saturating_sub(cut.start)),
                );
            }
        }
    }

    fn copy_stories(
        &self,
        source: &str,
        labels: &Labels,
        schemas: &SchemaRegistry,
        scope: CopyScope,
        into: &mut Fragment,
    ) -> Result<Vec<Story>, FragmentError> {
        // One copy of every story's blocks, so what a copy reads of the whole
        // document (the ranges, the relations) is read once, not once a story:
        // the table can hold thousands. The result is then split by root.
        let mut selection = Vec::new();
        let mut story_of_root: BTreeMap<NodeId, usize> = BTreeMap::new();
        for (index, &story) in labels.order.iter().enumerate() {
            for node in self.top(Domain::Story(story)) {
                story_of_root.insert(node, index);
                selection.push(CopyBlock { node, bytes: None });
            }
        }
        let mut part = self.copy_blocks(source, &selection, schemas, scope)?;
        let mut stories: Vec<Story> = labels
            .order
            .iter()
            .map(|&story| Story {
                label: labels.by_story[&story].clone(),
                source: story,
                blocks: Vec::new(),
                ranges: Vec::new(),
            })
            .collect();
        let mut owner: BTreeMap<NodeId, usize> = BTreeMap::new();
        let mut current = 0;
        for block in std::mem::take(&mut part.blocks) {
            if block.parent.is_none() {
                current = *story_of_root
                    .get(&block.id)
                    .ok_or_else(|| invalid("a copied story root is not a story's"))?;
            }
            owner.insert(block.id, current);
            if let Some(story) = stories.get_mut(current) {
                story.blocks.push(block);
            }
        }
        for range in std::mem::take(&mut part.ranges) {
            // A range lies in one domain, so the story of its first block is
            // its story.
            if let Some(story) = owner.get(&range.node).and_then(|&i| stories.get_mut(i)) {
                story.ranges.push(range);
            }
        }
        for (name, style) in std::mem::take(&mut part.styles) {
            into.styles.entry(name).or_insert(style);
        }
        for (id, def) in std::mem::take(&mut part.lists) {
            into.lists.entry(id).or_insert(def);
        }
        into.notes.append(&mut part.notes);
        if !part.relations.is_empty() {
            into.notes.push(reprise_diag::Note::error(
                reprise_diag::Code::new("clipboard.relation-dropped"),
                "relations owned by or anchored in a story are not copied",
            ));
        }
        Ok(stories)
    }
}

fn is_plain(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Bool(_)) || value.as_i64().is_some()
}

/// Splits a record's raw keys into plain properties and region bindings.
fn split_bindings(
    own: &SectionProperties,
    live: &BTreeSet<NodeId>,
    labels: &mut Labels,
) -> (SectionProperties, BTreeMap<String, Binding>) {
    let mut props = SectionProperties::new();
    let mut bindings = BTreeMap::new();
    for (key, value) in own {
        match value {
            Value::String(s) if binding_key(key) => {
                let binding = if s.is_empty() {
                    Binding::None
                } else {
                    match NodeId::parse(s) {
                        Some(story) if live.contains(&story) => Binding::Story {
                            label: labels.of(story),
                        },
                        _ => Binding::Raw { value: s.clone() },
                    }
                };
                bindings.insert(key.clone(), binding);
            }
            other => {
                props.insert(key.clone(), other.clone());
            }
        }
    }
    (props, bindings)
}
