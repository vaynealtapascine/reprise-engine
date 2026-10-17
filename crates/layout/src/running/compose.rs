//! Composing a story into one running frame (sections §9, "Composition").
//!
//! A region is composed by the ordinary flow over `Domain::Story(id)` with the
//! one-frame source, so it stops at the frame end: the source is exhausted, and
//! it never makes a page. The flow runs on a scratch snapshot that holds only
//! that frame, so a page of 1,000 headers does no work proportional to the
//! document.
//!
//! The result is memoised by what it read: the story's blocks' inputs, the
//! frame's box and the field values it composed with. A story with no fields
//! is composed once for every page that shows it.

use super::{MAX_STORY_BLOCKS, MAX_STORY_TEXT, SlotId, role_of};
use crate::flow::Cursor;
use crate::inline::{FieldInstance, Reads};
use crate::pages::FrameSource;
use crate::regions::Plan;
use crate::template::{ResolvedFrame, ResolvedTemplate};
use crate::{BlockLayout, Diagnostic, Engine, LayoutSnapshot, PageLayout, TemplateSource};
use reprise_doc::{BlockKind, Document, Domain, FrameRole, NodeId};
use reprise_geom::{Length, Matrix};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

/// Compositions of one story kept at once.
const MAX_ENTRIES: usize = 16;

/// What composing a story into a frame produced.
#[derive(Clone, Debug)]
pub(crate) struct Composed {
    /// Blocks of the story, their lines in the scratch frame (index 0).
    pub blocks: Vec<BlockLayout>,
    /// The block-axis extent of what was placed.
    pub used: Length,
    /// The story has no blocks at all.
    pub empty: bool,
    /// Authored content was not shown: it did not fit, or was past the read cap.
    pub clipped: bool,
    /// Blocks the runner does not lay out (floats, notes, relation-placed).
    pub unsupported: Vec<NodeId>,
    /// What layout said about the content itself (style, shaping, ...).
    pub notes: Vec<Diagnostic>,
    /// The field values it read, and the page context it guessed with.
    pub reads: Reads,
}

/// Everything a story's blocks are composed from, read once per layout.
#[derive(Debug)]
pub(crate) struct Digest {
    nodes: Vec<NodeId>,
    inputs: Vec<(NodeId, Option<crate::incremental::Inputs>)>,
    /// Blocks past the read caps, which are not composed.
    omitted: bool,
}

/// A digest compared by identity first: pages of one layout share it.
#[derive(Clone, Debug)]
pub(crate) struct DigestRef(Rc<Digest>);
impl Eq for DigestRef {}
impl PartialEq for DigestRef {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
            || (self.0.nodes == other.0.nodes
                && self.0.omitted == other.0.omitted
                && self.0.inputs == other.0.inputs)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Key {
    digest: DigestRef,
    slot: SlotId,
    width: Length,
    depth: Length,
    page: (Length, Length),
}

#[derive(Clone)]
struct Entry {
    key: Key,
    page: usize,
    composed: Rc<Composed>,
}

#[derive(Default)]
struct Memo {
    entries: BTreeMap<NodeId, Vec<Entry>>,
    /// Valid within one job: the document cannot change under it.
    digests: BTreeMap<NodeId, Rc<Digest>>,
    compositions: usize,
    reused: usize,
}

/// The running memo: owned by a session (or by one plain layout), shared by
/// every cursor and region pass that composes regions.
#[derive(Default)]
pub(crate) struct Shared {
    memo: RefCell<Memo>,
    /// Story paragraphs are prepared (itemised, shaped) here once, keyed as the
    /// body's are, so a 1 MiB paragraph is shaped once, not per page.
    prepared: crate::incremental::Evaluation,
}

/// What `compose` needs to know about the frame and the page it is on.
pub(crate) struct Request<'a> {
    pub engine: &'a Engine,
    pub doc: &'a Document,
    pub plan: &'a Plan,
    pub story: NodeId,
    pub slot: SlotId,
    /// 0-based physical page.
    pub page: usize,
    /// The pages made so far, this one included.
    pub pages: usize,
    pub width: Length,
    pub depth: Length,
    pub page_size: (Length, Length),
    /// The label of `page`, which is not in the snapshot yet.
    pub label: &'a str,
}

impl Shared {
    /// A new job reads a possibly different document: forget its digests and
    /// start its counts.
    pub fn begin_job(&self) {
        let mut memo = self.memo.borrow_mut();
        memo.digests.clear();
        memo.compositions = 0;
        memo.reused = 0;
    }
    /// A document edit that raced a job can leave anything in the memo.
    pub fn clear(&self) {
        *self.memo.borrow_mut() = Memo::default();
        self.prepared.clear();
    }
    /// Compositions made and reused, for tests and `WorkCounters`.
    pub fn counts(&self) -> (usize, usize) {
        let m = self.memo.borrow();
        (m.compositions, m.reused)
    }

    fn digest(&self, doc: &Document, story: NodeId) -> Rc<Digest> {
        if let Some(d) = self.memo.borrow().digests.get(&story) {
            return d.clone();
        }
        let all = doc.top(Domain::Story(story));
        let mut nodes = Vec::new();
        let mut text = 0usize;
        let mut omitted = false;
        for node in all {
            let bytes = doc.block(node).map_or(0, |b| b.text.len());
            if nodes.len() >= MAX_STORY_BLOCKS || text.saturating_add(bytes) > MAX_STORY_TEXT {
                omitted = true;
                break;
            }
            text = text.saturating_add(bytes);
            nodes.push(node);
        }
        let inputs = nodes
            .iter()
            .map(|n| (*n, crate::incremental::Inputs::read(doc, *n)))
            .collect();
        let digest = Rc::new(Digest {
            nodes,
            inputs,
            omitted,
        });
        self.memo.borrow_mut().digests.insert(story, digest.clone());
        digest
    }

    /// The story composed for `request`, from the memo when what it read is
    /// unchanged.
    pub fn compose(&self, request: &Request<'_>) -> Rc<Composed> {
        let digest = self.digest(request.doc, request.story);
        let key = Key {
            digest: DigestRef(digest.clone()),
            slot: request.slot,
            width: request.width,
            depth: request.depth,
            page: request.page_size,
        };
        let hit = self
            .memo
            .borrow()
            .entries
            .get(&request.story)
            .and_then(|v| {
                v.iter()
                    .find(|e| e.key == key && still_valid(e, request))
                    .map(|e| e.composed.clone())
            });
        if let Some(composed) = hit {
            self.memo.borrow_mut().reused += 1;
            return composed;
        }
        let composed = Rc::new(self.run(request, &digest));
        let mut memo = self.memo.borrow_mut();
        memo.compositions += 1;
        let entries = memo.entries.entry(request.story).or_default();
        if entries.len() >= MAX_ENTRIES {
            entries.remove(0);
        }
        entries.push(Entry {
            key,
            page: request.page,
            composed: composed.clone(),
        });
        composed
    }

    fn run(&self, request: &Request<'_>, digest: &Digest) -> Composed {
        let engine = request.engine;
        let doc = request.doc;
        let role = role_of(request.slot);
        let frame = ResolvedFrame {
            name: role.clone(),
            role: FrameRole::Flow(role.clone()),
            x: Length::ZERO,
            y: Length::ZERO,
            width: request.width,
            depth: request.depth,
            transform: Matrix::IDENTITY,
            writing_mode: reprise_doc::WritingMode::HorizontalTb,
        };
        let template = ResolvedTemplate {
            name: role,
            source: TemplateSource::Builtin,
            width: request.page_size.0,
            height: request.page_size.1,
            frames: vec![frame],
            page_setup: None,
            thread: None,
        };
        let mut scratch = scratch(engine, &template);
        let trace = Rc::new(RefCell::new(Reads::default()));
        let instance = FieldInstance::Running {
            page: u32::try_from(request.page).unwrap_or(u32::MAX),
            slot: request.slot,
            repeat: 0,
        };
        let mut cursor = Cursor::new_in(
            engine,
            doc,
            &template,
            &mut scratch,
            request.plan,
            Domain::Story(request.story),
            FrameSource::OneFrame {
                index: 0,
                exhausted: false,
            },
        )
        .with_fields(crate::flow::FieldContext {
            instance,
            page: request.page,
            pages: request.pages,
            label: request.label.to_owned(),
            trace: Some(trace.clone()),
        });
        let owners = crate::regions::owners(engine, doc);
        let mut unsupported = Vec::new();
        let mut clipped = digest.omitted;
        for node in &digest.nodes {
            if is_unsupported(doc, *node, &owners) {
                unsupported.push(*node);
                continue;
            }
            if cursor.exhausted() {
                clipped = true;
                break;
            }
            cursor = cursor.step(
                engine,
                doc,
                &template,
                &mut scratch,
                request.plan,
                *node,
                Some(&self.prepared),
            );
        }
        let mut notes = Vec::new();
        for d in std::mem::take(&mut scratch.diagnostics) {
            let code = d.code.as_str();
            if code == crate::codes::TEXT_UNPLACED.as_str()
                || code == crate::codes::FRAME_OVERFLOW.as_str()
                || code == reprise_compose::codes::OVERFLOW.as_str()
            {
                clipped = true;
            } else if code != crate::codes::PAGE_LIMIT.as_str() {
                notes.push(d);
            }
        }
        let used = scratch
            .blocks
            .iter()
            .flat_map(|b| &b.lines)
            .map(|l| l.rect.origin.y + l.rect.height)
            .max()
            .unwrap_or(Length::ZERO)
            .max(Length::ZERO);
        let reads = std::mem::take(&mut *trace.borrow_mut());
        Composed {
            blocks: std::mem::take(&mut scratch.blocks),
            used,
            empty: digest.nodes.is_empty() && !digest.omitted,
            clipped,
            unsupported,
            notes,
            reads,
        }
    }
}

/// An entry is valid for a request when every field value it read is what the
/// request's plan says, and the page context it guessed with is the same.
fn still_valid(entry: &Entry, request: &Request<'_>) -> bool {
    let reads = &entry.composed.reads;
    if let Some(label) = &reads.label
        && label != request.label
    {
        return false;
    }
    if let Some((page, pages)) = reads.guessed
        && (page, pages) != (request.page, request.pages)
    {
        return false;
    }
    reads.fields.iter().all(|(read, was)| {
        // The entry may have been composed for another page: the same object
        // on this page is the same object of the same slot.
        let key = crate::inline::FieldKey {
            id: read.id,
            instance: match read.instance {
                FieldInstance::Running { slot, repeat, .. } => FieldInstance::Running {
                    page: u32::try_from(request.page).unwrap_or(u32::MAX),
                    slot,
                    repeat,
                },
                other => other,
            },
        };
        let _ = entry.page;
        request.plan.fields.get(&key) == was.as_ref()
    })
}

/// Floats, footnotes and relation-placed blocks are not laid out in a story.
fn is_unsupported(
    doc: &Document,
    node: NodeId,
    owners: &std::collections::BTreeSet<NodeId>,
) -> bool {
    match doc.block(node).map(|b| b.kind) {
        Ok(BlockKind::Annotation) => true,
        Ok(BlockKind::Image) => {
            owners.contains(&node)
                || doc.relations().iter().any(|(_, r)| {
                    r.as_ref().is_ok_and(|r| {
                        r.owner == Some(node) && r.schema == reprise_doc::relation::builtin::FOLLOW
                    })
                })
        }
        _ => false,
    }
}

/// A snapshot holding only the running frame, as page 0.
fn scratch(engine: &Engine, template: &ResolvedTemplate) -> LayoutSnapshot {
    LayoutSnapshot {
        font_settings: engine.fonts.settings().clone(),
        revision: reprise_doc::Revision(Vec::new()),
        adapter: engine.shaper.info(),
        composer: engine.composer.name().into(),
        medium: engine.medium,
        settings: engine.flow,
        template: crate::TemplateUsed {
            name: template.name.clone(),
            source: template.source,
        },
        pages: vec![PageLayout {
            width: template.width,
            height: template.height,
        }],
        frames: template.frames.iter().map(|f| f.layout(0)).collect(),
        blocks: Vec::new(),
        relations: Vec::new(),
        repeated_headers: Vec::new(),
        diagnostics: Vec::new(),
        sections: None,
    }
}

/// Moves a composed block set to frame `frame` of the real snapshot, for
/// field instance `instance`, and shifts it down by `by`.
pub(crate) fn place(
    blocks: &[BlockLayout],
    frame: usize,
    instance: FieldInstance,
    by: Length,
) -> Vec<BlockLayout> {
    blocks
        .iter()
        .cloned()
        .map(|mut block| {
            for line in &mut block.lines {
                line.frame = frame;
                line.rect.origin.y += by;
                line.baseline += by;
                for atom in &mut line.atoms {
                    atom.rect.origin.y += by;
                    atom.baseline += by;
                    if matches!(atom.instance, FieldInstance::Running { .. }) {
                        atom.instance = instance;
                    }
                }
            }
            block.sync_image();
            block
        })
        .collect()
}

/// Dedup key for reporting content diagnostics once per layout.
pub(crate) fn same(a: &Diagnostic, b: &Diagnostic) -> bool {
    a.code == b.code && a.subject == b.subject && a.message == b.message && a.bytes == b.bytes
}

#[allow(dead_code)]
fn _arc(_: Arc<()>) {}
