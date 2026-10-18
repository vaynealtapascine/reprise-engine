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

/// Compositions of one story kept at once. A header that shows its page's
/// number is a composition per page, and a look-ahead makes them ahead of the
/// pages that use them, so this is how far ahead the memo keeps them.
const MAX_ENTRIES: usize = 64;

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

/// What one top-level block of a story is composed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    inputs: Option<crate::incremental::Inputs>,
    /// Floats, footnotes and relation-placed blocks are not laid out in a
    /// story. Whether an image is one depends on the relations that own it,
    /// which are no part of its inputs, so the verdict is part of the key.
    unsupported: bool,
    /// For a table: its declared role (columns) and every row, cell and cell
    /// block under it, in document order. A table is composed from all of it,
    /// and none of it is in the container block's own inputs.
    table: Option<Table>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Table {
    role: Option<reprise_doc::TableRole>,
    parts: Vec<Part>,
}

/// One node under a table: where it sits, what table role it declares (spans,
/// header flags, columns) and what its own content is.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Part {
    node: NodeId,
    parent: NodeId,
    role: Option<reprise_doc::TableRole>,
    /// The role record is there but unreadable.
    malformed: bool,
    inputs: Option<crate::incremental::Inputs>,
}

/// The table role of `node` as the digest keeps it.
fn role_of_node(doc: &Document, node: NodeId) -> (Option<reprise_doc::TableRole>, bool) {
    match doc.table_role(node) {
        Ok(role) => (role, false),
        Err(_) => (None, true),
    }
}

/// A table's rows (depth 0), their cells (1) and the cells' blocks (2), the
/// levels the table layout reads. Anything at a level that does not declare
/// the role expected there is kept, and not entered.
fn table_parts(doc: &Document, table: NodeId) -> Vec<Part> {
    use reprise_doc::TableRole;
    fn level(doc: &Document, parent: NodeId, depth: usize, out: &mut Vec<Part>) {
        for node in doc.children_of(parent) {
            let (role, malformed) = role_of_node(doc, node);
            let enter = matches!(
                (depth, &role),
                (0, Some(TableRole::Row(_))) | (1, Some(TableRole::Cell(_)))
            );
            out.push(Part {
                node,
                parent,
                role,
                malformed,
                inputs: crate::incremental::Inputs::read(doc, node),
            });
            if enter {
                level(doc, node, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    level(doc, table, 0, &mut out);
    out
}

/// Everything a story's blocks are composed from, read once per layout.
#[derive(Debug)]
pub(crate) struct Digest {
    nodes: Vec<NodeId>,
    items: Vec<Item>,
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
                && self.0.items == other.0.items)
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
    classes: Option<Rc<Classes>>,
    compositions: usize,
    reused: usize,
    /// Compositions a stepped flow had to make itself, because the look-ahead
    /// had not made them. Zero when the look-ahead is complete.
    inline: usize,
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

/// How the document's relations classify blocks: read once per job, when a story
/// first holds an image.
#[derive(Debug, Default)]
pub(crate) struct Classes {
    /// Owners of floats and notes.
    owners: std::collections::BTreeSet<NodeId>,
    /// Blocks placed by following a line.
    followers: std::collections::BTreeSet<NodeId>,
}

impl Classes {
    fn of(engine: &Engine, doc: &Document) -> Self {
        Self {
            owners: crate::regions::owners(engine, doc),
            followers: doc
                .relations()
                .into_iter()
                .filter_map(|(_, r)| {
                    let r = r.ok()?;
                    (r.schema == reprise_doc::relation::builtin::FOLLOW)
                        .then_some(r.owner)
                        .flatten()
                })
                .collect(),
        }
    }
}

/// A [`Request`] that owns its data: what a look-ahead is asked to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Want {
    pub story: NodeId,
    pub slot: SlotId,
    pub page: usize,
    pub pages: usize,
    pub width: Length,
    pub depth: Length,
    pub page_size: (Length, Length),
    pub label: String,
}

impl Request<'_> {
    pub fn want(&self) -> Want {
        Want {
            story: self.story,
            slot: self.slot,
            page: self.page,
            pages: self.pages,
            width: self.width,
            depth: self.depth,
            page_size: self.page_size,
            label: self.label.to_owned(),
        }
    }
}

/// What a memo-only lookup is missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Need {
    /// The story's blocks have not been read yet.
    Digest(NodeId),
    /// The story is not composed for this request.
    Story(Want),
}

/// Blocks of a story read per look-ahead unit when its digest is made.
pub(crate) const DIGEST_CHUNK: usize = 64;

/// A story digest being made, a bounded number of blocks at a time.
#[derive(Clone, Debug)]
pub(crate) struct DigestBuild {
    story: NodeId,
    all: Vec<NodeId>,
    at: usize,
    nodes: Vec<NodeId>,
    items: Vec<Item>,
    text: usize,
    omitted: bool,
    done: bool,
}

impl DigestBuild {
    pub fn new(doc: &Document, story: NodeId) -> Self {
        Self {
            story,
            all: doc.top(Domain::Story(story)),
            at: 0,
            nodes: Vec::new(),
            items: Vec::new(),
            text: 0,
            omitted: false,
            done: false,
        }
    }
    pub fn story(&self) -> NodeId {
        self.story
    }
    pub fn is_done(&self) -> bool {
        self.done
    }
    /// Reads at most `blocks` more blocks; returns how many it read.
    pub fn step(
        &mut self,
        shared: &Shared,
        engine: &Engine,
        doc: &Document,
        blocks: usize,
    ) -> usize {
        let mut read = 0;
        while read < blocks && !self.done {
            let Some(&node) = self.all.get(self.at) else {
                self.done = true;
                break;
            };
            let bytes = doc.block(node).map_or(0, |b| b.text.len());
            if self.nodes.len() >= MAX_STORY_BLOCKS
                || self.text.saturating_add(bytes) > MAX_STORY_TEXT
            {
                self.omitted = true;
                self.done = true;
                break;
            }
            self.text = self.text.saturating_add(bytes);
            self.nodes.push(node);
            let unsupported = match doc.block(node).map(|b| b.kind) {
                Ok(BlockKind::Annotation) => true,
                Ok(BlockKind::Image) => {
                    let classes = shared.classes(engine, doc);
                    classes.owners.contains(&node) || classes.followers.contains(&node)
                }
                _ => false,
            };
            let table = match doc.table_role(node) {
                Ok(Some(role @ reprise_doc::TableRole::Table(_))) => Some(Table {
                    role: Some(role),
                    parts: table_parts(doc, node),
                }),
                _ => None,
            };
            self.items.push(Item {
                inputs: crate::incremental::Inputs::read(doc, node),
                unsupported,
                table,
            });
            self.at = self.at.saturating_add(1);
            read += 1;
        }
        if self.at >= self.all.len() {
            self.done = true;
        }
        read
    }
    fn finish(self) -> Digest {
        Digest {
            nodes: self.nodes,
            items: self.items,
            omitted: self.omitted,
        }
    }
}

impl Shared {
    /// A new job reads a possibly different document: forget its digests and
    /// start its counts.
    pub fn begin_job(&self) {
        let mut memo = self.memo.borrow_mut();
        memo.digests.clear();
        memo.classes = None;
        memo.compositions = 0;
        memo.reused = 0;
        memo.inline = 0;
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

    fn digest(&self, engine: &Engine, doc: &Document, story: NodeId) -> Rc<Digest> {
        if let Some(d) = self.ready_digest(story) {
            return d;
        }
        let mut build = DigestBuild::new(doc, story);
        build.step(self, engine, doc, usize::MAX);
        self.set_digest(build)
    }

    /// The relations' classification of blocks, read once per job.
    fn classes(&self, engine: &Engine, doc: &Document) -> Rc<Classes> {
        if let Some(c) = self.memo.borrow().classes.clone() {
            return c;
        }
        let classes = Rc::new(Classes::of(engine, doc));
        self.memo.borrow_mut().classes = Some(classes.clone());
        classes
    }

    /// The story's digest, when this job has made it.
    pub fn ready_digest(&self, story: NodeId) -> Option<Rc<Digest>> {
        self.memo.borrow().digests.get(&story).cloned()
    }

    /// Records a finished digest.
    pub fn set_digest(&self, build: DigestBuild) -> Rc<Digest> {
        let story = build.story();
        let digest = Rc::new(build.finish());
        self.memo.borrow_mut().digests.insert(story, digest.clone());
        digest
    }

    fn find(&self, key: &Key, request: &Request<'_>) -> Option<Rc<Composed>> {
        self.memo
            .borrow()
            .entries
            .get(&request.story)
            .and_then(|v| {
                v.iter()
                    .find(|e| e.key == *key && still_valid(e, request))
                    .map(|e| e.composed.clone())
            })
    }

    fn store(&self, key: Key, story: NodeId, page: usize, composed: Rc<Composed>) {
        let mut memo = self.memo.borrow_mut();
        memo.compositions += 1;
        let entries = memo.entries.entry(story).or_default();
        if entries.len() >= MAX_ENTRIES {
            entries.remove(0);
        }
        entries.push(Entry {
            key,
            page,
            composed,
        });
    }

    /// The memo alone: the composition for `request` if it is there, else what
    /// is missing. `count` says whether a hit is a reuse (a probe is not).
    pub fn lookup(&self, request: &Request<'_>, count: bool) -> Result<Rc<Composed>, Need> {
        let Some(digest) = self.ready_digest(request.story) else {
            return Err(Need::Digest(request.story));
        };
        let key = Key::of(&digest, &request.want());
        match self.find(&key, request) {
            Some(composed) => {
                if count {
                    self.memo.borrow_mut().reused += 1;
                }
                Ok(composed)
            }
            None => Err(Need::Story(request.want())),
        }
    }

    /// Stores a composition a look-ahead made, whole.
    pub fn install(&self, run: &StoryRun, composed: Composed) {
        self.store(
            run.key.clone(),
            run.want.story,
            run.want.page,
            Rc::new(composed),
        );
    }

    /// What preparing the stories' paragraphs did since the last call.
    pub fn take_work(&self) -> crate::incremental::StepWork {
        self.prepared.take_work()
    }

    /// The evaluation story paragraphs are prepared in.
    pub fn evaluation(&self) -> &crate::incremental::Evaluation {
        &self.prepared
    }

    /// Compositions a stepped flow made itself.
    pub fn inline_count(&self) -> usize {
        self.memo.borrow().inline
    }

    /// The story composed for `request`, from the memo when what it read is
    /// unchanged, else made whole, here. `stepped` says the caller is a
    /// stepped flow, for which making a composition here means its look-ahead
    /// was incomplete.
    pub fn compose_counted(&self, request: &Request<'_>, stepped: bool) -> Rc<Composed> {
        let digest = self.digest(request.engine, request.doc, request.story);
        let key = Key::of(&digest, &request.want());
        if let Some(composed) = self.find(&key, request) {
            self.memo.borrow_mut().reused += 1;
            return composed;
        }
        if stepped {
            self.memo.borrow_mut().inline += 1;
        }
        let composed = Rc::new(self.run(request, &digest));
        self.store(key, request.story, request.page, composed.clone());
        composed
    }

    /// Runs the stepped composition to the end, unit after unit.
    fn run(&self, request: &Request<'_>, digest: &Rc<Digest>) -> Composed {
        let mut run = StoryRun::new(
            request.engine,
            request.doc,
            request.plan,
            request.want(),
            digest.clone(),
        );
        loop {
            let (_, done) = run.step(
                request.engine,
                request.doc,
                request.plan,
                &self.prepared,
                usize::MAX,
            );
            if let Some(composed) = done {
                return composed;
            }
        }
    }
}

impl Key {
    fn of(digest: &Rc<Digest>, want: &Want) -> Self {
        Key {
            digest: DigestRef(digest.clone()),
            slot: want.slot,
            width: want.width,
            depth: want.depth,
            page: want.page_size,
        }
    }
}

/// One story being composed into a running frame, a unit at a time: the same
/// flow the body uses over the story's blocks, with the one-frame source.
/// Running it to the end gives exactly what composing the story whole gives,
/// which is how [`Shared::compose`] makes its compositions.
#[derive(Clone)]
pub(crate) struct StoryRun {
    key: Key,
    want: Want,
    digest: Rc<Digest>,
    template: ResolvedTemplate,
    scratch: LayoutSnapshot,
    cursor: Option<Cursor>,
    trace: crate::inline::Trace,
    next: usize,
    unsupported: Vec<NodeId>,
    clipped: bool,
    /// Story blocks started, for the work counters.
    pub started: usize,
}

impl StoryRun {
    pub fn new(
        engine: &Engine,
        doc: &Document,
        plan: &Plan,
        want: Want,
        digest: Rc<Digest>,
    ) -> Self {
        let role = role_of(want.slot);
        let frame = ResolvedFrame {
            name: role.clone(),
            role: FrameRole::Flow(role.clone()),
            x: Length::ZERO,
            y: Length::ZERO,
            width: want.width,
            depth: want.depth,
            transform: Matrix::IDENTITY,
            writing_mode: reprise_doc::WritingMode::HorizontalTb,
        };
        let template = ResolvedTemplate {
            name: role,
            source: TemplateSource::Builtin,
            width: want.page_size.0,
            height: want.page_size.1,
            frames: vec![frame],
            page_setup: None,
            thread: None,
        };
        let mut scratch = scratch(engine, &template);
        let trace = Rc::new(RefCell::new(Reads::default()));
        let instance = FieldInstance::Running {
            page: u32::try_from(want.page).unwrap_or(u32::MAX),
            slot: want.slot,
            repeat: 0,
        };
        let cursor = Cursor::new_in(
            engine,
            doc,
            &template,
            &mut scratch,
            plan,
            Domain::Story(want.story),
            FrameSource::OneFrame {
                index: 0,
                exhausted: false,
            },
        )
        .with_fields(crate::flow::FieldContext {
            instance,
            page: want.page,
            pages: want.pages,
            label: want.label.clone(),
            trace: Some(trace.clone()),
        });
        let clipped = digest.omitted;
        Self {
            key: Key::of(&digest, &want),
            want,
            digest,
            template,
            scratch,
            cursor: Some(cursor),
            trace,
            next: 0,
            unsupported: Vec::new(),
            clipped,
            started: 0,
        }
    }

    /// One unit of the composition. `allowance` is how many units the caller
    /// may still charge. Returns the units charged and, when the story is
    /// composed, the result.
    pub fn step(
        &mut self,
        engine: &Engine,
        doc: &Document,
        plan: &Plan,
        prepared: &crate::incremental::Evaluation,
        allowance: usize,
    ) -> (usize, Option<Composed>) {
        let Some(cursor) = self.cursor.take() else {
            return (1, Some(self.finish()));
        };
        let mut start = None;
        if !cursor.busy() {
            while let Some(&node) = self.digest.nodes.get(self.next) {
                let unsupported = self
                    .digest
                    .items
                    .get(self.next)
                    .is_some_and(|i| i.unsupported);
                self.next = self.next.saturating_add(1);
                if unsupported {
                    self.unsupported.push(node);
                    continue;
                }
                if cursor.exhausted() {
                    self.clipped = true;
                    self.next = self.digest.nodes.len();
                    break;
                }
                start = Some(node);
                break;
            }
            if start.is_none() {
                return (1, Some(self.finish()));
            }
        }
        self.started = self.started.saturating_add(usize::from(start.is_some()));
        let (cursor, used) = cursor.ordinary_unit(
            engine,
            doc,
            &self.template,
            &mut self.scratch,
            plan,
            Some(prepared),
            start,
            &[],
            allowance,
        );
        self.cursor = Some(cursor);
        (used.max(1), None)
    }

    fn finish(&mut self) -> Composed {
        let mut clipped = self.clipped;
        let mut notes = Vec::new();
        for d in std::mem::take(&mut self.scratch.diagnostics) {
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
        let used = self
            .scratch
            .blocks
            .iter()
            .flat_map(|b| &b.lines)
            .map(|l| l.rect.origin.y + l.rect.height)
            .max()
            .unwrap_or(Length::ZERO)
            .max(Length::ZERO);
        let reads = std::mem::take(&mut *self.trace.borrow_mut());
        Composed {
            blocks: std::mem::take(&mut self.scratch.blocks),
            used,
            empty: self.digest.nodes.is_empty() && !self.digest.omitted,
            clipped,
            unsupported: std::mem::take(&mut self.unsupported),
            notes,
            reads,
        }
    }
}

// An entry is valid for a request when every field value it read is what the
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
