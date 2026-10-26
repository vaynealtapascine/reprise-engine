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
use crate::flow::{Cursor, Link};
use crate::inline::{FieldInstance, Reads};
use crate::ownership::{Class, Classes};
use crate::pages::FrameSource;
use crate::regions::Plan;
use crate::template::{ResolvedFrame, ResolvedTemplate};
use crate::{BlockLayout, Diagnostic, Engine, LayoutSnapshot, PageLayout, TemplateSource};
use reprise_doc::{BlockKind, ChildReader, Document, Domain, FrameRole, NodeId, ReadLimits};
use reprise_geom::{Length, Matrix};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

/// Compositions of one story kept at once. A header that shows its page's
/// number is a composition per page, and a look-ahead makes them ahead of the
/// pages that use them, so this is how far ahead the memo keeps them.
pub(crate) const MAX_ENTRIES: usize = 64;

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
    /// What a keep-with-next chain makes of the block (see `flow::Link`): the
    /// look-ahead reads the relations that own an image, and a table's role,
    /// so the verdict is part of the key.
    link: Link,
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

/// Everything a story's blocks are composed from, read once per layout.
#[derive(Debug)]
pub(crate) struct Digest {
    nodes: Vec<NodeId>,
    items: Vec<Item>,
    /// What the relations make of top-level annotations and images, and of
    /// annotations in table cells (a block no relation names is not in it:
    /// it is `Class::Free`). It is all the composition
    /// learns of ownership (`ownership.rs`): the flow of a story is answered
    /// from here, never from the document, so the memo keyed by the digest is
    /// keyed by every answer it was composed with.
    classes: BTreeMap<NodeId, Class>,
    /// Blocks past the read caps, which are not composed.
    omitted: bool,
}

impl Digest {
    /// What the relations make of `node`, a block the story holds.
    pub(crate) fn class_of(&self, node: NodeId) -> Class {
        self.classes.get(&node).copied().unwrap_or(Class::Free)
    }
    /// The blocks the story is composed from, in order: the only ones the
    /// composition may read, lookahead included.
    pub(crate) fn nodes(&self) -> &[NodeId] {
        &self.nodes
    }
    /// What the keep look-ahead makes of the `at`th of [`Digest::nodes`].
    pub(crate) fn link_at(&self, at: usize) -> Option<Link> {
        self.items.get(at).map(|item| item.link)
    }
    /// Whether two digests hold the same inputs. Reads all of both.
    fn same_as(&self, other: &Digest) -> bool {
        self.nodes == other.nodes
            && self.omitted == other.omitted
            && self.items == other.items
            && self.classes == other.classes
    }
}

/// A digest compared by identity first: pages of one layout share it. Entries
/// of an earlier job hold that job's digest; [`Shared::find`] swaps in the
/// current job's the first time the two are found equal, so each cached
/// composition costs one comparison of contents per job.
#[derive(Clone, Debug)]
pub(crate) struct DigestRef(Rc<Digest>);
impl Eq for DigestRef {}
impl PartialEq for DigestRef {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0) || self.0.same_as(&other.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Key {
    slot: SlotId,
    width: Length,
    depth: Length,
    page: (Length, Length),
    /// Last: the fields above are cheap to compare, this one is not.
    digest: DigestRef,
}

impl Key {
    /// Everything but the digest.
    fn same_request(&self, other: &Key) -> bool {
        self.slot == other.slot
            && self.width == other.width
            && self.depth == other.depth
            && self.page == other.page
    }
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
    /// Compositions a region allocation needs and asked the look-ahead for.
    /// The allocation needs every page's at once, which can be more than
    /// [`MAX_ENTRIES`], so these are kept, uncapped, until the allocation that
    /// needed them has consumed them ([`Shared::release_held`]).
    held: BTreeMap<NodeId, Vec<Entry>>,
    /// Valid within one job: the document cannot change under it.
    digests: BTreeMap<NodeId, Rc<Digest>>,
    classes: Option<Rc<Classes>>,
    /// Digests compared by contents this job: the pointer fast path is not one.
    compares: usize,
    compositions: usize,
    reused: usize,
    /// Compositions a stepped flow had to make itself, because the look-ahead
    /// had not made them. Zero when the look-ahead is complete.
    inline: usize,
    /// Names the job these digests and held compositions belong to: a new one
    /// each time they are dropped ([`Shared::begin_job`], [`Shared::clear`]).
    /// A suspended job keeps the one it was made under, and can resume only
    /// while the memo still has it.
    epoch: Rc<()>,
    /// A job found that something it had made, and depends on, was gone.
    lost: bool,
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

/// A [`Request`] that owns its data: what a look-ahead is asked to make.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Need {
    /// The story's blocks have not been read yet.
    Digest(NodeId),
    /// The story is not composed for this request.
    Story(Want),
}

/// Units of reading per look-ahead unit when a story's digest is made: child
/// IDs looked at, and paragraphs and embedded nodes of flow hosts, one each,
/// and the decoding of a host, one and a share of its text (see
/// [`ReadLimits::units`]).
pub(crate) const DIGEST_CHUNK: usize = 64;

/// The longest flow host text, in bytes, a digest decodes. A story reads at
/// most [`MAX_STORY_TEXT`] bytes of paragraphs and [`MAX_STORY_BLOCKS`] blocks,
/// and each paragraph of a host costs the host three bytes of break marker, so
/// a host longer than this is cut by the read limits inside itself whatever
/// else the story holds. It is not decoded at all, and the story is cut in
/// front of it, like a table that does not fit: a host is decoded to the end
/// in one go, so what it costs is bounded by this, not by the author.
///
/// The cutoff is a property of how the text is stored (how many paragraphs
/// share a physical host), not of the content: a host over it is left out
/// whole, even if its first paragraphs would have fitted the story, and the
/// story is reported clipped. Nothing built on the digest should treat it as a
/// semantic boundary.
pub(crate) const MAX_HOST_BYTES: usize = MAX_STORY_TEXT + 3 * MAX_STORY_BLOCKS;

/// The most look-ahead units one step of a digest charges: the decode of a host
/// of [`MAX_HOST_BYTES`] (the reading units [`ReadLimits::units`] counts for
/// it), in the [`DIGEST_CHUNK`] a unit holds. A digest step charges one unit,
/// except the one that decodes such a host: it is alone in its step and charges
/// all of its cost there.
pub(crate) const MAX_DECODE_STEP_UNITS: usize =
    (MAX_HOST_BYTES / reprise_doc::DECODE_UNIT_BYTES).div_ceil(DIGEST_CHUNK);
const _: () = assert!(MAX_DECODE_STEP_UNITS == 5);

/// What a stretch of reading a digest did, for the work counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DigestWork {
    /// Story blocks read: a block taken, or a node of a table read.
    pub blocks: usize,
    /// Child IDs looked at.
    pub looked: usize,
    /// Bytes of flow host text decoded.
    pub decoded: usize,
    /// Units of reading done.
    pub units: usize,
    /// The step stopped in front of a host that takes one of its own.
    pub deferred: bool,
}

impl DigestWork {
    fn add(&mut self, other: DigestWork) {
        self.blocks = self.blocks.saturating_add(other.blocks);
        self.looked = self.looked.saturating_add(other.looked);
        self.decoded = self.decoded.saturating_add(other.decoded);
        self.units = self.units.saturating_add(other.units);
    }
}

/// A story digest being made, a bounded amount of work at a time.
///
/// The story is read through a [`ChildReader`], so a story of a million blocks
/// costs what the read budget costs: the digest stops at the 4,096th block or
/// the 1 MiB of text. A table is part of the same budget, every row, cell and
/// cell block of it counting as a block and every cell's text as text: it is
/// read whole or, if it would take the story past a limit, not at all (the
/// story is cut before it, like any block past a limit). So a table's cells are
/// charged before any of them is prepared for composition.
#[derive(Clone, Debug)]
pub(crate) struct DigestBuild {
    story: NodeId,
    top: ChildReader,
    top_done: bool,
    nodes: Vec<NodeId>,
    items: Vec<Item>,
    /// As [`Digest::classes`].
    classes: BTreeMap<NodeId, Class>,
    /// Blocks and bytes of text taken so far, tables' parts included.
    blocks: usize,
    text: usize,
    /// The table being read.
    walk: Option<Walk>,
    omitted: bool,
    done: bool,
}

/// A table being read, level by level: its rows, then each row's cells, then
/// each cell's blocks.
#[derive(Clone, Debug)]
struct Walk {
    node: NodeId,
    item: Item,
    levels: Vec<Level>,
    /// What the table has taken, the container included.
    blocks: usize,
    text: usize,
    /// What the relations make of the annotations in its cells so far. They
    /// join [`Digest::classes`] when the table is taken whole: a table the
    /// story is cut in front of is not composed, and keys nothing.
    classes: Vec<(NodeId, Class)>,
}

#[derive(Clone, Debug)]
struct Level {
    parent: NodeId,
    /// 0: the table's rows, 1: a row's cells, 2: a cell's blocks.
    depth: usize,
    reader: ChildReader,
}

impl DigestBuild {
    pub fn new(doc: &Document, story: NodeId) -> Self {
        Self {
            story,
            top: doc.top_reader(Domain::Story(story)),
            top_done: false,
            nodes: Vec::new(),
            items: Vec::new(),
            classes: BTreeMap::new(),
            blocks: 0,
            text: 0,
            walk: None,
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

    /// Reads on for up to `units` units of work (see [`DIGEST_CHUNK`]). The
    /// story's top level is read one block at a time and each is taken as it
    /// comes, so a step takes at most the units it was given, with one
    /// exception: a host whose decoding costs more than a step's units is
    /// decoded by a step of its own, as one atomic unit. That step does the
    /// decode (at most [`MAX_HOST_BYTES`] of text), takes the first block it
    /// stands for, and nothing else; it reports what it cost, in
    /// `DigestWork::units`, and no step after it pays anything back.
    pub fn step(
        &mut self,
        shared: &Shared,
        engine: &Engine,
        doc: &Document,
        units: usize,
    ) -> DigestWork {
        let mut work = DigestWork::default();
        let room = units.max(1);
        let mut spent = 0usize;
        let mut out = Vec::new();
        while spent < room && !self.done {
            let left = room.saturating_sub(spent);
            if self.walk.is_some() {
                let done = self.walk_step(shared, engine, doc, left, spent == 0);
                spent = spent.saturating_add(done.units);
                work.add(done);
                if done.deferred {
                    break;
                }
                continue;
            }
            if self.top_done {
                self.done = true;
                break;
            }
            let read = doc.read_children(
                &mut self.top,
                ReadLimits {
                    units: left,
                    blocks: 1,
                    host_bytes: MAX_HOST_BYTES,
                    fresh: spent == 0,
                },
                &mut out,
            );
            spent = spent.saturating_add(read.units);
            work.looked = work.looked.saturating_add(read.looked);
            work.decoded = work.decoded.saturating_add(read.decoded);
            work.units = work.units.saturating_add(read.units);
            for node in out.drain(..) {
                work.blocks = work
                    .blocks
                    .saturating_add(self.take(shared, engine, doc, node));
            }
            if read.refused {
                self.cut();
                break;
            }
            self.top_done = read.done;
            if read.deferred {
                // A host dearer than what is left of this step is decoded by
                // a step of its own, which opens with it: `fresh`.
                break;
            }
        }
        if self.top_done && self.walk.is_none() {
            self.done = true;
        }
        work
    }

    /// Takes the next top-level block, or cuts the story before it. Returns the
    /// blocks read now (a table's container is read now, its parts as they
    /// come).
    fn take(&mut self, shared: &Shared, engine: &Engine, doc: &Document, node: NodeId) -> usize {
        let bytes = doc.block(node).map_or(0, |b| b.text.len());
        if self.blocks >= MAX_STORY_BLOCKS || self.text.saturating_add(bytes) > MAX_STORY_TEXT {
            self.cut();
            return 0;
        }
        let kind = doc.block(node).map(|b| b.kind);
        let class = Self::read_class(shared, engine, doc, node);
        if class != Class::Free {
            self.classes.insert(node, class);
        }
        let unsupported = match kind {
            Ok(BlockKind::Annotation) => true,
            Ok(BlockKind::Image) => class != Class::Free,
            _ => false,
        };
        let inputs = crate::incremental::Inputs::read(doc, node);
        let link = Link::of(doc, node, || class == Class::Owner);
        if let Ok(Some(role @ reprise_doc::TableRole::Table(_))) = doc.table_role(node) {
            self.walk = Some(Walk {
                node,
                item: Item {
                    inputs,
                    unsupported,
                    link,
                    table: Some(Table {
                        role: Some(role),
                        parts: Vec::new(),
                    }),
                },
                levels: vec![Level {
                    parent: node,
                    depth: 0,
                    reader: doc.children_reader(node),
                }],
                blocks: 1,
                text: bytes,
                classes: Vec::new(),
            });
            return 1;
        }
        self.nodes.push(node);
        self.items.push(Item {
            inputs,
            unsupported,
            link,
            table: None,
        });
        self.blocks = self.blocks.saturating_add(1);
        self.text = self.text.saturating_add(bytes);
        1
    }

    /// What the relations make of `node`, if the flow of the story will ask:
    /// an annotation or an image. The caller records it in
    /// [`Digest::classes`], which is the only place a story's composition
    /// learns it.
    fn read_class(shared: &Shared, engine: &Engine, doc: &Document, node: NodeId) -> Class {
        if matches!(
            doc.kind_of(node),
            Some(BlockKind::Annotation | BlockKind::Image)
        ) {
            shared.classes(engine, doc).class(node)
        } else {
            Class::Free
        }
    }

    /// Reads on in the table being read, for up to `room` units; `fresh` when
    /// the step has spent none yet.
    fn walk_step(
        &mut self,
        shared: &Shared,
        engine: &Engine,
        doc: &Document,
        room: usize,
        fresh: bool,
    ) -> DigestWork {
        use reprise_doc::TableRole;
        let mut work = DigestWork::default();
        let Some(mut walk) = self.walk.take() else {
            return work;
        };
        let Some(level) = walk.levels.last_mut() else {
            // Every level is read: the table is taken whole.
            self.nodes.push(walk.node);
            self.items.push(walk.item);
            self.classes.extend(walk.classes);
            self.blocks = self.blocks.saturating_add(walk.blocks);
            self.text = self.text.saturating_add(walk.text);
            work.units = 1;
            return work;
        };
        let (parent, depth) = (level.parent, level.depth);
        let mut out = Vec::new();
        let read = doc.read_children(
            &mut level.reader,
            ReadLimits {
                units: room,
                blocks: room,
                host_bytes: MAX_HOST_BYTES,
                fresh,
            },
            &mut out,
        );
        work.looked = read.looked;
        work.decoded = read.decoded;
        work.units = read.units;
        work.deferred = read.deferred;
        if read.refused {
            // A cell holds a host too long to read: the table does not fit.
            self.cut();
            return work;
        }
        if read.done {
            walk.levels.pop();
        }
        let mut deeper = Vec::new();
        for node in out {
            let bytes = doc.block(node).map_or(0, |b| b.text.len());
            if self.blocks.saturating_add(walk.blocks) >= MAX_STORY_BLOCKS
                || self.text.saturating_add(walk.text).saturating_add(bytes) > MAX_STORY_TEXT
            {
                // The table does not fit what is left of the story: the story
                // is cut before it, and none of its cells is ever prepared.
                self.cut();
                return work;
            }
            walk.blocks = walk.blocks.saturating_add(1);
            walk.text = walk.text.saturating_add(bytes);
            work.blocks = work.blocks.saturating_add(1);
            let (role, malformed) = role_of_node(doc, node);
            let enter = matches!(
                (depth, &role),
                (0, Some(TableRole::Row(_))) | (1, Some(TableRole::Cell(_)))
            );
            if enter {
                deeper.push(Level {
                    parent: node,
                    depth: depth + 1,
                    reader: doc.children_reader(node),
                });
            }
            // The one thing composing a cell reads of the relations: whether
            // a float or note owns its annotation (`Flow::table_rows`).
            if doc.kind_of(node) == Some(BlockKind::Annotation) {
                let class = Self::read_class(shared, engine, doc, node);
                if class != Class::Free {
                    walk.classes.push((node, class));
                }
            }
            if let Some(table) = walk.item.table.as_mut() {
                table.parts.push(Part {
                    node,
                    parent,
                    role,
                    malformed,
                    inputs: crate::incremental::Inputs::read(doc, node),
                });
            }
        }
        // The first row's cells before the second row.
        walk.levels.extend(deeper.into_iter().rev());
        self.walk = Some(walk);
        work
    }

    /// The story is cut here: what is past a read limit is not composed.
    fn cut(&mut self) {
        self.omitted = true;
        self.done = true;
        self.walk = None;
    }

    fn finish(self) -> Digest {
        Digest {
            nodes: self.nodes,
            items: self.items,
            classes: self.classes,
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
        memo.held.clear();
        memo.classes = None;
        memo.compares = 0;
        memo.compositions = 0;
        memo.reused = 0;
        memo.inline = 0;
        memo.epoch = Rc::new(());
        memo.lost = false;
    }
    /// The job the memo's digests and held compositions belong to.
    pub fn epoch(&self) -> Rc<()> {
        self.memo.borrow().epoch.clone()
    }
    /// Whether a suspended job made under `epoch` can go on: nothing it holds
    /// has been dropped since.
    pub fn holds(&self, epoch: &Rc<()>) -> bool {
        Rc::ptr_eq(&self.memo.borrow().epoch, epoch)
    }
    /// Something a job depends on is not in the memo although the job made it:
    /// the job can only be abandoned (see `LayoutJob::step`), never made good
    /// by composing inline.
    pub fn lose(&self) {
        self.memo.borrow_mut().lost = true;
    }
    /// Whether the job has found its memo state gone.
    pub fn lost(&self) -> bool {
        self.memo.borrow().lost
    }
    /// A document edit that raced a job can leave anything in the memo.
    pub fn clear(&self) {
        *self.memo.borrow_mut() = Memo::default();
        self.prepared.clear();
    }
    /// Digests compared by contents since the job began, for `WorkCounters`.
    pub fn digest_compares(&self) -> usize {
        self.memo.borrow().compares
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

    /// The relations' classification of blocks, read once per job. The digests
    /// of the job copy out of it what their stories hold ([`Digest::classes`]).
    fn classes(&self, engine: &Engine, doc: &Document) -> Rc<Classes> {
        if let Some(c) = self.memo.borrow().classes.clone() {
            return c;
        }
        let classes = Rc::new(Classes::read(engine, doc));
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

    /// The cached composition for `key`, if what it read still holds.
    ///
    /// The cheap parts of a key are compared before its digest. An entry of an
    /// earlier job whose digest has the same contents as this job's takes this
    /// job's digest in place, so the pages after the first find it by pointer
    /// and the contents are never compared twice; one whose digest differs can
    /// not serve this job and is dropped.
    fn find(&self, key: &Key, request: &Request<'_>) -> Option<Rc<Composed>> {
        let mut memo = self.memo.borrow_mut();
        let Memo {
            entries,
            held,
            compares,
            ..
        } = &mut *memo;
        [entries, held].into_iter().find_map(|lists| {
            lists
                .get_mut(&request.story)
                .and_then(|list| scan(list, key, request, compares))
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

    /// Keeps a composition a region allocation asked for, whole, past the
    /// memo's cap, until [`Shared::release_held`].
    pub fn hold(&self, run: &StoryRun, composed: Composed) {
        let mut memo = self.memo.borrow_mut();
        memo.compositions += 1;
        memo.held.entry(run.want.story).or_default().push(Entry {
            key: run.key.clone(),
            page: run.want.page,
            composed: Rc::new(composed),
        });
    }

    /// The allocation consumed what it asked for: the held compositions go to
    /// the memo, oldest first, as if they had been stored one by one, so the
    /// memo keeps the most recent [`MAX_ENTRIES`] of a story as it always did.
    pub fn release_held(&self) {
        let mut memo = self.memo.borrow_mut();
        let held = std::mem::take(&mut memo.held);
        for (story, list) in held {
            let entries = memo.entries.entry(story).or_default();
            for entry in list {
                if entries.len() >= MAX_ENTRIES {
                    entries.remove(0);
                }
                entries.push(entry);
            }
        }
    }

    /// Compositions held for a region allocation now.
    #[cfg(test)]
    pub fn held_count(&self) -> usize {
        self.memo.borrow().held.values().map(Vec::len).sum()
    }

    /// Forgets what is held, as a memo cleared under a job would.
    #[cfg(test)]
    pub fn lose_held(&self) {
        self.memo.borrow_mut().held.clear();
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

    /// Drops every cached composition and keeps the job's digests: what a memo
    /// at its [`MAX_ENTRIES`] does to its oldest entries when others arrive,
    /// done to all of them, so a test can lose what a look-ahead just made.
    #[cfg(test)]
    pub fn evict_compositions(&self) {
        self.memo.borrow_mut().entries.clear();
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

/// The entry of `list` that serves `request`, if any. An entry of an earlier
/// job whose digest has the same contents as this job's takes this job's in
/// place; one whose digest differs can not serve this job and is dropped.
fn scan(
    list: &mut Vec<Entry>,
    key: &Key,
    request: &Request<'_>,
    compares: &mut usize,
) -> Option<Rc<Composed>> {
    let mut found = None;
    let mut stale = Vec::new();
    for (i, entry) in list.iter_mut().enumerate() {
        if !entry.key.same_request(key) {
            continue;
        }
        if !Rc::ptr_eq(&entry.key.digest.0, &key.digest.0) {
            *compares = compares.saturating_add(1);
            if entry.key.digest.0.same_as(&key.digest.0) {
                entry.key.digest = key.digest.clone();
            } else {
                stale.push(i);
                continue;
            }
        }
        if still_valid(entry, request) {
            found = Some(entry.composed.clone());
            break;
        }
    }
    for i in stale.into_iter().rev() {
        list.remove(i);
    }
    found
}

impl Key {
    fn of(digest: &Rc<Digest>, want: &Want) -> Self {
        Key {
            slot: want.slot,
            width: want.width,
            depth: want.depth,
            page: want.page_size,
            digest: DigestRef(digest.clone()),
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
        })
        .with_scope(digest.clone());
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
