//! The sections envelope of a pasted fragment (sections §14, N7).
//!
//! [`split`] runs while a paste is prepared: it reads and validates the
//! envelope, decides which of its stories a paste refers to in place, copies or
//! skips, drops what the destination can't take, and hands back the fragment
//! with the copied stories' blocks flattened into it, so the one machinery that
//! stages blocks, ranges, annotations, lists, formatting and inline objects
//! serves a story as it serves the body. [`SectionsPaste`] is what the writer
//! needs of the rest. Everything that can be refused is refused here, before
//! anything is written.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use reprise_diag::Note;
use reprise_doc::fragment::{Fragment, FragmentError};
use reprise_doc::fragment_sections::{Binding, Boundary, Envelope, Record};
use reprise_doc::sections::{PastedStart, SectionId, SectionProperties};
use reprise_doc::{DocError, Document, Domain, NodeId};
use serde_json::Value;

use crate::paste::{Pasted, SECTION_CONTEXT, SECTION_DROPPED};
use crate::{EditError, Reason};

/// What a paste does with one story of the envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    /// The destination is the document the copy came from and the story is
    /// still live: bindings refer to it, and nothing is copied.
    InPlace,
    /// A new story is made from the envelope's blocks.
    Copy,
    /// Nothing the paste writes refers to it.
    Skip,
}

#[derive(Clone, Debug)]
struct StoryEntry {
    label: String,
    source: NodeId,
    plan: Plan,
}

/// What a paste writes of the envelope, once it is prepared.
#[derive(Clone, Debug)]
pub(crate) struct SectionsPaste {
    boundaries: Vec<Boundary>,
    context: Option<Record>,
    geometry: Option<SectionProperties>,
    stories: Vec<StoryEntry>,
    /// Fragment-local IDs of the roots of the stories this paste copies, and
    /// the entry each belongs to.
    roots: BTreeMap<NodeId, usize>,
    /// Every block of those stories, roots and descendants.
    blocks: BTreeSet<NodeId>,
    /// Records the copy could not carry (a dead block, one outside the body).
    dropped: usize,
}

/// A refusal of the envelope: a limit is a copy limit, anything else a
/// malformed fragment.
pub(crate) fn refuse(error: FragmentError) -> EditError {
    EditError {
        command: None,
        reason: match error {
            FragmentError::Limit(name) => Reason::CopyLimit(name),
            other => Reason::Fragment(other),
        },
    }
}

fn store(error: DocError) -> EditError {
    EditError {
        command: None,
        reason: Reason::from(error),
    }
}

fn invalid(why: &str) -> EditError {
    refuse(FragmentError::Invalid(why.into()))
}

/// What a paste makes of a fragment and its envelope.
pub(crate) struct Split<'f> {
    /// The fragment to paste: with the copied stories' blocks in it, and no
    /// envelope.
    pub fragment: Cow<'f, Fragment>,
    pub sections: Option<SectionsPaste>,
    /// What preparing it already has to report.
    pub notes: Vec<Note>,
}

/// Reads the envelope of `fragment`, if it has one, and settles what a paste at
/// `at` of it into `doc` does with it.
pub(crate) fn split<'f>(
    doc: &Document,
    fragment: &'f Fragment,
    at: Option<(NodeId, usize)>,
    target: &str,
) -> Result<Split<'f>, EditError> {
    let Some(raw) = fragment.sections.as_deref() else {
        return Ok(Split {
            fragment: Cow::Borrowed(fragment),
            sections: None,
            notes: Vec::new(),
        });
    };
    let envelope = Envelope::parse(raw).map_err(refuse)?;
    // The whole envelope counts, every story and every limit, before this
    // paste decides which stories it uses, discards them for a story
    // destination, or refers to them in place.
    envelope.validate_in(fragment).map_err(refuse)?;
    let mut notes = Vec::new();
    // A story has content, not a place: nothing of the envelope applies in one.
    if at.is_some_and(|(node, _)| matches!(doc.domain_of(node), Some(Domain::Story(_)))) {
        notes.push(Note::warning(
            SECTION_CONTEXT,
            "section boundaries, bindings and stories were not pasted into a story",
        ));
        let mut plain = fragment.clone();
        plain.sections = None;
        return Ok(Split {
            fragment: Cow::Owned(plain),
            sections: None,
            notes,
        });
    }
    let empty = at.is_none() && doc.top(Domain::Body).is_empty();
    let (mut context, mut geometry) = (envelope.context.clone(), envelope.geometry.clone());
    if !empty && (context.is_some() || geometry.is_some()) {
        notes.push(Note::warning(
            SECTION_CONTEXT,
            "the first section's settings and the geometry mode apply to an empty document only, and were not pasted",
        ));
        context = None;
        geometry = None;
    }
    // Which stories something that is written refers to.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut refer = |bindings: &BTreeMap<String, Binding>| {
        for binding in bindings.values() {
            if let Binding::Story { label } = binding {
                referenced.insert(label.clone());
            }
        }
    };
    for boundary in &envelope.boundaries {
        refer(&boundary.bindings);
    }
    if let Some(context) = &context {
        refer(&context.bindings);
    }
    let same = fragment.source == target;
    let live: BTreeSet<NodeId> = doc.stories().into_iter().collect();
    let stories: Vec<StoryEntry> = envelope
        .stories
        .iter()
        .map(|s| StoryEntry {
            label: s.label.clone(),
            source: s.source,
            plan: if same && live.contains(&s.source) {
                Plan::InPlace
            } else if empty || referenced.contains(&s.label) {
                // An empty destination takes the orphans of a complete copy too,
                // which stay orphans.
                Plan::Copy
            } else {
                Plan::Skip
            },
        })
        .collect();
    let keep: Vec<bool> = stories.iter().map(|s| s.plan == Plan::Copy).collect();
    // What is left of a validated envelope is within its limits too.
    let flat = fragment.flattened_for(&envelope, &keep);
    let roots = envelope
        .story_roots()
        .into_iter()
        .filter(|(_, index)| keep.get(*index).copied().unwrap_or(false))
        .collect();
    let blocks = envelope
        .stories
        .iter()
        .zip(&keep)
        .filter(|(_, keep)| **keep)
        .flat_map(|(s, _)| s.blocks.iter().map(|b| b.id))
        .collect();
    if envelope.dropped > 0 {
        notes.push(dropped_note(envelope.dropped));
    }
    let sections = SectionsPaste {
        boundaries: envelope.boundaries,
        context,
        geometry,
        stories,
        roots,
        blocks,
        dropped: 0,
    };
    Ok(Split {
        fragment: Cow::Owned(flat),
        sections: Some(sections),
        notes,
    })
}

fn dropped_note(count: usize) -> Note {
    Note::info(
        SECTION_DROPPED,
        format!(
            "{count} section record{} had no block to sit on and {} not pasted",
            if count == 1 { "" } else { "s" },
            if count == 1 { "was" } else { "were" }
        ),
    )
}

impl SectionsPaste {
    /// Whether `block` is a block of a story this paste copies.
    pub fn owns(&self, block: NodeId) -> bool {
        self.blocks.contains(&block)
    }

    /// Makes the stories this paste copies, empty and not yet live: one new
    /// story for each, in the envelope's order (`None` for the others).
    pub fn stage(&self, doc: &Document) -> Result<Vec<Option<NodeId>>, EditError> {
        self.stories
            .iter()
            .map(|s| match s.plan {
                Plan::Copy => doc.stage_story(&[]).map(Some).map_err(store),
                _ => Ok(None),
            })
            .collect()
    }

    /// The new story a copied story root goes into.
    pub fn story_of(&self, root: NodeId, staged: &[Option<NodeId>]) -> Option<NodeId> {
        self.roots
            .get(&root)
            .and_then(|index| staged.get(*index).copied().flatten())
    }

    /// Writes the records: the stories become live, the boundaries sit on the
    /// pasted copies of their blocks with lifetimes of their own, `first` and
    /// the geometry mode are written when the destination takes them, and
    /// `result.stories` says which story each source story became. `joined` is
    /// the block, if any, that was joined into the paragraph at the caret and so
    /// has no place for a boundary.
    pub fn finish(
        &self,
        doc: &Document,
        staged: &[Option<NodeId>],
        joined: Option<NodeId>,
        result: &mut Pasted,
    ) -> Result<(), EditError> {
        let mut used: Vec<Option<NodeId>> = Vec::with_capacity(self.stories.len());
        for (entry, new) in self.stories.iter().zip(staged) {
            let story = match entry.plan {
                Plan::InPlace => Some(entry.source),
                Plan::Copy => *new,
                Plan::Skip => None,
            };
            if let Some(story) = story {
                result.stories.insert(entry.source, story);
            }
            used.push(story);
        }
        let by_label: BTreeMap<&str, Option<NodeId>> = self
            .stories
            .iter()
            .zip(&used)
            .map(|(e, u)| (e.label.as_str(), *u))
            .collect();
        let resolve = |bindings: &BTreeMap<String, Binding>,
                       into: &mut SectionProperties|
         -> Result<(), EditError> {
            for (key, binding) in bindings {
                let value = match binding {
                    Binding::None => String::new(),
                    Binding::Raw { value } => value.clone(),
                    Binding::Story { label } => by_label
                        .get(label.as_str())
                        .copied()
                        .flatten()
                        .ok_or_else(|| invalid("a binding names a story the paste did not make"))?
                        .to_string(),
                };
                into.insert(key.clone(), Value::String(value));
            }
            Ok(())
        };
        let mut dropped = self.dropped;
        let mut starts = Vec::with_capacity(self.boundaries.len());
        for boundary in &self.boundaries {
            if Some(boundary.block) == joined {
                dropped += 1;
                continue;
            }
            let block = result
                .ids
                .nodes
                .get(&boundary.block)
                .copied()
                .ok_or_else(|| invalid("a boundary names a block that was not pasted"))?;
            let mut properties = boundary.props.clone();
            if let Some(start) = &boundary.start {
                properties.insert("start".into(), Value::String(start.clone()));
            }
            resolve(&boundary.bindings, &mut properties)?;
            starts.push(PastedStart {
                block,
                kind: boundary.kind,
                active: boundary.active,
                properties,
            });
        }
        doc.write_pasted_starts(&starts).map_err(store)?;
        if let Some(context) = &self.context {
            let mut properties = context.props.clone();
            resolve(&context.bindings, &mut properties)?;
            doc.write_section_properties(SectionId::First, "0", &properties, &[])
                .map_err(store)?;
        }
        if let Some(geometry) = &self.geometry {
            doc.write_geometry_record(geometry).map_err(store)?;
        }
        if dropped > 0 {
            result.notes.push(dropped_note(dropped));
        }
        Ok(())
    }
}
