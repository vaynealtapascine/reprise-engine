//! Flow text: paragraphs as break markers in a host's text (06, 07, 11, 29).
//! See `docs/flow.md`.
//!
//! A host is a `paragraph` or `annotation` node without a table role. Its
//! text holds the head paragraph, then for each active break a U+FDD0
//! character followed by that paragraph's text. Everything here is a pure
//! function of the document; nothing is written on read.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use loro::cursor::Side;
use loro::{
    Container, Frontiers, ID, LoroDoc, LoroMap, LoroText, LoroTree, LoroValue, TreeID,
    TreeParentId, ValueOrContainer,
};
use reprise_text::{Anchor, BREAK, Resolved, Segment, Span, Text, TextError};

use crate::lifecycle::STAGE_ORIGIN;
use crate::{BlockKind, DocError, Document, NodeId, get_str};

/// How deeply embedded nodes nest inside flows. Deeper embeds are hidden.
pub const MAX_FLOW_DEPTH: usize = 32;

/// The root map of break records, keyed by the break paragraph's `NodeId`
/// (`host/mark`). A root map, so two peers making the first break of a host
/// at the same time can't overwrite each other's record container.
pub(crate) const BREAKS: &str = "breaks1";
/// Host metadata: `false` when the head paragraph is deleted.
pub(crate) const HEAD: &str = "head";
const VERSION: &str = "v";
const ACTIVE: &str = "active";
const EMBED: &str = "embed";
/// UTF-8 length of [`BREAK`].
const BREAK_BYTES: usize = 3;

#[derive(Default)]
pub(crate) struct FlowCache {
    frontiers: Option<Frontiers>,
    hosts: HashMap<TreeID, Option<Arc<HostFlow>>>,
}

/// One host's paragraphs as read now.
#[derive(Debug)]
pub(crate) struct HostFlow {
    pub(crate) head_live: bool,
    /// The head, then every active break, in text order.
    pub(crate) paras: Vec<Para>,
    index: HashMap<NodeId, usize>,
    /// Every break character with a readable break record, active or not,
    /// with its scalar index, in text order.
    pub(crate) marks: Vec<(NodeId, usize)>,
}

#[derive(Debug)]
pub(crate) struct Para {
    pub(crate) id: NodeId,
    pub(crate) segs: Vec<Segment>,
    /// Nodes embedded after this paragraph: the node, its embed's ID and the
    /// embed character's scalar index, in text order.
    pub(crate) embeds: Vec<(TreeID, ID, usize)>,
    /// The scalar index of this paragraph's break; `None` for the head.
    pub(crate) mark_at: Option<usize>,
    /// From just after its break (or the text's start) to the next active
    /// break (or the text's end): bytes and scalars.
    pub(crate) region: Range<(usize, usize)>,
}

impl HostFlow {
    pub(crate) fn para(&self, id: NodeId) -> Option<&Para> {
        self.index.get(&id).map(|&i| &self.paras[i])
    }

    /// Whether `id` is a live paragraph of this flow.
    pub(crate) fn is_live(&self, id: NodeId) -> bool {
        match self.index.get(&id) {
            Some(0) => self.head_live,
            Some(_) => true,
            None => false,
        }
    }

    /// The live paragraphs in text order.
    pub(crate) fn live(&self) -> impl Iterator<Item = &Para> {
        self.paras
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > 0 || self.head_live)
            .map(|(_, p)| p)
    }

    /// Whether anything other than the head is visible: a break paragraph
    /// or an embedded node.
    pub(crate) fn has_more_than_head(&self) -> bool {
        self.paras.len() > 1 || self.paras.iter().any(|p| !p.embeds.is_empty())
    }

    /// The paragraph an embedded node follows, and its embed.
    pub(crate) fn embed_of(&self, node: TreeID) -> Option<(&Para, ID, usize)> {
        self.paras.iter().find_map(|p| {
            p.embeds
                .iter()
                .find(|(n, _, _)| *n == node)
                .map(|&(_, id, at)| (p, id, at))
        })
    }
}

enum Mark {
    Hidden,
    Break,
    Embed(TreeID),
}

struct Builder {
    paras: Vec<Para>,
    /// Where the current visible run starts: bytes, scalars.
    start: (usize, usize),
}

impl Builder {
    fn new(host: TreeID) -> Builder {
        Builder {
            paras: vec![Para {
                id: NodeId::tree(host),
                segs: Vec::new(),
                embeds: Vec::new(),
                mark_at: None,
                region: (0, 0)..(0, 0),
            }],
            start: (0, 0),
        }
    }

    fn close_run(&mut self, byte: usize, u: usize) {
        if byte > self.start.0
            && let Some(p) = self.paras.last_mut()
        {
            p.segs.push(Segment {
                bytes: self.start.0..byte,
                chars: self.start.1..u,
            });
        }
    }

    fn hidden(&mut self, byte: usize, u: usize) {
        self.close_run(byte, u);
        self.start = (byte + BREAK_BYTES, u + 1);
    }

    fn embed(&mut self, node: TreeID, id: ID, byte: usize, u: usize) {
        self.hidden(byte, u);
        if let Some(p) = self.paras.last_mut() {
            p.embeds.push((node, id, u));
        }
    }

    fn brk(&mut self, id: NodeId, byte: usize, u: usize) {
        self.close_run(byte, u);
        if let Some(p) = self.paras.last_mut() {
            p.region.end = (byte, u);
        }
        self.start = (byte + BREAK_BYTES, u + 1);
        self.paras.push(Para {
            id,
            segs: Vec::new(),
            embeds: Vec::new(),
            mark_at: Some(u),
            region: self.start..self.start,
        });
    }

    fn finish(mut self, bytes: usize, chars: usize) -> Vec<Para> {
        self.close_run(bytes, chars);
        if let Some(p) = self.paras.last_mut() {
            p.region.end = (bytes, chars);
        }
        self.paras
    }
}

/// Makes a paragraph's segments cover its whole region: an empty segment
/// at the region's start or end when hidden characters are there, so every
/// position in the region maps to an offset of the paragraph, and text
/// typed at its start goes right after its break.
fn bound(segs: Vec<Segment>, region: &Range<(usize, usize)>) -> Vec<Segment> {
    let empty = |(b, u): (usize, usize)| Segment {
        bytes: b..b,
        chars: u..u,
    };
    let mut segs: Vec<Segment> = segs.into_iter().filter(|s| !s.bytes.is_empty()).collect();
    if segs.first().is_none_or(|s| s.chars.start != region.start.1) {
        segs.insert(0, empty(region.start));
    }
    if segs.last().is_some_and(|s| s.chars.end != region.end.1) {
        segs.push(empty(region.end));
    }
    segs
}

/// Supplies a paragraph view's segments from the current state.
struct ParaSpan {
    doc: LoroDoc,
    flows: Arc<Mutex<FlowCache>>,
    id: NodeId,
}

impl Span for ParaSpan {
    fn segments(&self) -> Result<Vec<Segment>, TextError> {
        let doc = Document::wrap_with(self.doc.clone(), self.flows.clone());
        let flow = doc.flow(self.id.node).ok_or(TextError::TextGone)?;
        if !flow.is_live(self.id) {
            return Err(TextError::TextGone);
        }
        flow.para(self.id)
            .map(|p| p.segs.clone())
            .ok_or(TextError::TextGone)
    }
}

/// Whether node metadata describes a flow host.
fn is_host_meta(meta: &LoroMap) -> bool {
    matches!(
        get_str(meta, "kind").and_then(|k| BlockKind::parse(&k)),
        Some(BlockKind::Paragraph | BlockKind::Annotation)
    ) && meta.get("table1").is_none()
}

fn get_i64(map: &LoroMap, key: &str) -> Option<i64> {
    match map.get(key)? {
        ValueOrContainer::Value(LoroValue::I64(n)) => Some(n),
        _ => None,
    }
}

impl Document {
    /// The break records map.
    pub(crate) fn break_records(&self) -> LoroMap {
        self.doc.get_map(BREAKS)
    }

    /// A break paragraph's record, if it has a readable one.
    pub(crate) fn break_record(&self, id: NodeId) -> Option<LoroMap> {
        id.mark?;
        match self.break_records().get(&id.to_string())? {
            ValueOrContainer::Container(Container::Map(m)) if get_i64(&m, VERSION) == Some(1) => {
                Some(m)
            }
            _ => None,
        }
    }

    /// Where a block's own metadata is: its tree node's, or its break record.
    pub(crate) fn meta_of(&self, id: NodeId) -> Result<LoroMap, DocError> {
        match id.mark {
            None => Ok(self.tree("content").get_meta(id.node)?),
            Some(_) => self.break_record(id).ok_or(DocError::NoNode(id)),
        }
    }

    /// The host's paragraphs now, if `host` is a flow host. Cached per
    /// revision; read fresh while a transaction is open.
    pub(crate) fn flow(&self, host: TreeID) -> Option<Arc<HostFlow>> {
        if self.doc.get_pending_txn_len() > 0 {
            return self.read_flow(host).map(Arc::new);
        }
        let frontiers = self.doc.state_frontiers();
        {
            let mut cache = self.flows.lock().unwrap_or_else(|e| e.into_inner());
            if cache.frontiers.as_ref() != Some(&frontiers) {
                cache.hosts.clear();
                cache.frontiers = Some(frontiers.clone());
            }
            if let Some(hit) = cache.hosts.get(&host) {
                return hit.clone();
            }
        }
        let read = self.read_flow(host).map(Arc::new);
        let mut cache = self.flows.lock().unwrap_or_else(|e| e.into_inner());
        if cache.frontiers.as_ref() == Some(&frontiers) {
            cache.hosts.insert(host, read.clone());
        }
        read
    }

    fn read_flow(&self, host: TreeID) -> Option<HostFlow> {
        let tree = self.tree("content");
        if !tree.contains(host) {
            return None;
        }
        let meta = tree.get_meta(host).ok()?;
        if !is_host_meta(&meta) {
            return None;
        }
        let Some(ValueOrContainer::Container(Container::Text(text))) = meta.get("text") else {
            return None;
        };
        let head_live = !matches!(
            meta.get(HEAD),
            Some(ValueOrContainer::Value(LoroValue::Bool(false)))
        );
        let s = text.to_string();
        let mut b = Builder::new(host);
        let mut marks = Vec::new();
        if s.contains(BREAK) {
            let records = self.break_records();
            let mut seen = HashSet::new();
            let mut u = 0;
            for (byte, c) in s.char_indices() {
                if c == BREAK {
                    let id = text.get_cursor(u, Side::Left).and_then(|c| c.id);
                    let (mark, recorded) = match id {
                        Some(id) => self.classify(&tree, &records, host, id, &mut seen),
                        None => (Mark::Hidden, false),
                    };
                    let nid = id.map(|id| NodeId::at_break(host, id));
                    if recorded && let Some(nid) = nid {
                        marks.push((nid, u));
                    }
                    match (mark, id, nid) {
                        (Mark::Break, _, Some(nid)) => b.brk(nid, byte, u),
                        (Mark::Embed(node), Some(id), _) => b.embed(node, id, byte, u),
                        _ => b.hidden(byte, u),
                    }
                }
                u += 1;
            }
            let mut paras = b.finish(s.len(), u);
            if !head_live && paras.len() > 1 {
                // Text before the first active break joins it.
                let head = std::mem::take(&mut paras[0].segs);
                let start = paras[0].region.start;
                let own = std::mem::take(&mut paras[1].segs);
                paras[1].segs = head.into_iter().chain(own).collect();
                paras[1].region.start = start;
            }
            for p in &mut paras {
                p.segs = bound(std::mem::take(&mut p.segs), &p.region);
            }
            return Some(finish(head_live, paras, marks));
        }
        let mut paras = b.finish(s.len(), text.len_unicode());
        for p in &mut paras {
            p.segs = bound(std::mem::take(&mut p.segs), &p.region);
        }
        Some(finish(head_live, paras, marks))
    }

    /// What a break character is: (meaning, whether it has a readable break
    /// record).
    fn classify(
        &self,
        tree: &LoroTree,
        records: &LoroMap,
        host: TreeID,
        id: ID,
        seen: &mut HashSet<TreeID>,
    ) -> (Mark, bool) {
        let key = NodeId::at_break(host, id).to_string();
        let Some(ValueOrContainer::Container(Container::Map(rec))) = records.get(&key) else {
            return (Mark::Hidden, false);
        };
        if get_i64(&rec, VERSION) != Some(1) {
            return (Mark::Hidden, false);
        }
        let active = matches!(
            rec.get(ACTIVE),
            Some(ValueOrContainer::Value(LoroValue::Bool(true)))
        );
        match rec.get(EMBED) {
            None => (if active { Mark::Break } else { Mark::Hidden }, true),
            Some(ValueOrContainer::Value(LoroValue::String(raw))) => {
                let Ok(node) = TreeID::try_from(raw.as_str()) else {
                    return (Mark::Hidden, false);
                };
                let valid = active
                    && tree.parent(node) == Some(TreeParentId::Node(host))
                    && self.live(tree, node)
                    && seen.insert(node);
                (
                    if valid {
                        Mark::Embed(node)
                    } else {
                        Mark::Hidden
                    },
                    false,
                )
            }
            Some(_) => (Mark::Hidden, false),
        }
    }

    /// A view of one paragraph of a flow.
    pub(crate) fn paragraph_text(&self, id: NodeId, text: LoroText) -> Text {
        Text::paragraph(
            text,
            Arc::new(ParaSpan {
                doc: self.doc.clone(),
                flows: self.flows.clone(),
                id,
            }),
        )
    }

    /// The host's text container, if `host` is a flow host.
    pub(crate) fn host_text(&self, host: TreeID) -> Option<LoroText> {
        let meta = self.tree("content").get_meta(host).ok()?;
        if !is_host_meta(&meta) {
            return None;
        }
        match meta.get("text")? {
            ValueOrContainer::Container(Container::Text(t)) => Some(t),
            _ => None,
        }
    }

    /// Expands one live tree node into the items it contributes to its
    /// parent's children: itself, or for a host its paragraphs with their
    /// embedded nodes.
    pub(crate) fn expand(&self, node: TreeID, depth: usize, out: &mut Vec<NodeId>) {
        let Some(flow) = self.flow(node) else {
            out.push(NodeId::tree(node));
            return;
        };
        for (i, p) in flow.paras.iter().enumerate() {
            if i > 0 || flow.head_live {
                out.push(p.id);
            }
            if depth < MAX_FLOW_DEPTH {
                for &(e, _, _) in &p.embeds {
                    self.expand(e, depth + 1, out);
                }
            }
        }
    }

    /// Whether a tree node is embedded in its parent's flow.
    pub(crate) fn is_embedded(&self, node: TreeID) -> bool {
        let tree = self.tree("content");
        match tree.parent(node) {
            Some(TreeParentId::Node(host)) => {
                self.flow(host).is_some_and(|f| f.embed_of(node).is_some())
            }
            _ => false,
        }
    }

    /// Writes `active` on a break or embed record. Does not commit.
    pub(crate) fn set_record_active(&self, id: NodeId, active: bool) -> Result<(), DocError> {
        let rec = self.break_record(id).ok_or(DocError::NoNode(id))?;
        rec.insert(ACTIVE, active)?;
        Ok(())
    }

    /// Commits what is pending as part of the current step, then inserts
    /// `BREAK` at scalar `at` of `host`'s text and writes its record, all
    /// in a commit outside the undo history.
    fn stage_mark(
        &self,
        host: TreeID,
        at: usize,
        write: impl FnOnce(&LoroMap) -> Result<(), DocError>,
    ) -> Result<NodeId, DocError> {
        let text = self
            .host_text(host)
            .ok_or(DocError::Malformed(NodeId::tree(host), "not a flow host"))?;
        if at > text.len_unicode() {
            return Err(DocError::BadIndex {
                index: at,
                len: text.len_unicode(),
            });
        }
        self.commit_part();
        self.doc.set_next_commit_origin(STAGE_ORIGIN);
        let staged = (|| {
            text.insert(at, &BREAK.to_string())?;
            let mark = text
                .get_cursor(at, Side::Left)
                .and_then(|c| c.id)
                .ok_or_else(|| DocError::Store("the break has no identity".into()))?;
            let id = NodeId::at_break(host, mark);
            let rec = self
                .break_records()
                .insert_container(&id.to_string(), LoroMap::new())?;
            rec.insert(VERSION, 1)?;
            rec.insert(ACTIVE, false)?;
            write(&rec)?;
            Ok(id)
        })();
        self.doc.commit();
        staged
    }

    /// Stages a break at byte `at` of paragraph `id`: an inactive break,
    /// outside the undo history, whose record copies the paragraph's kind,
    /// style and overrides. Commits what was pending first, as part of the
    /// current step. [`Document::activate_break`] makes it a paragraph.
    pub fn stage_break(&self, id: NodeId, at: usize) -> Result<NodeId, DocError> {
        let block = self.block(id)?;
        if self.flow(id.node).is_none() {
            return Err(DocError::Malformed(id, "not in a flow"));
        }
        let pos = block.text.to_shared(at)?;
        self.stage_mark(id.node, pos, |rec| {
            rec.insert("kind", block.kind.as_str())?;
            rec.insert("style", block.style.clone().unwrap_or_default())?;
            if block.overrides != crate::Style::default() {
                block
                    .overrides
                    .write(&rec.insert_container("overrides", LoroMap::new())?)?;
            }
            Ok(())
        })
    }

    /// Makes a staged or joined break a paragraph again. Does not commit.
    pub fn activate_break(&self, id: NodeId) -> Result<(), DocError> {
        if !id.is_break() {
            return Err(DocError::Malformed(id, "not a break"));
        }
        self.set_record_active(id, true)
    }

    /// Splits a flow paragraph with a break: the text from `at` on becomes
    /// a new paragraph, without moving any text. Stages the break (see
    /// [`Document::stage_break`]) and activates it.
    pub(crate) fn split_flow(&self, id: NodeId, at: usize) -> Result<NodeId, DocError> {
        let new = self.stage_break(id, at)?;
        self.activate_break(new)?;
        Ok(new)
    }

    /// The live paragraph just before `id` in its host's text, if `id` is a
    /// live break paragraph.
    pub(crate) fn flow_predecessor(&self, id: NodeId) -> Option<NodeId> {
        if !id.is_break() {
            return None;
        }
        let flow = self.flow(id.node)?;
        let live: Vec<NodeId> = flow.live().map(|p| p.id).collect();
        let at = live.iter().position(|&p| p == id)?;
        at.checked_sub(1).map(|i| live[i])
    }

    /// Deletes a flow paragraph: its visible text, then its break (or the
    /// head flag). `Ok(false)` when `id` is a head with nothing else in its
    /// flow, which is deleted as a block instead.
    pub(crate) fn delete_flow_para(&self, id: NodeId) -> Result<bool, DocError> {
        let Some(flow) = self.flow(id.node) else {
            return Ok(false);
        };
        if !flow.is_live(id) {
            return Err(DocError::NoNode(id));
        }
        if !id.is_break() && !flow.has_more_than_head() {
            return Ok(false);
        }
        let text = self.block(id)?.text;
        text.delete(0..text.len())?;
        if id.is_break() {
            self.set_record_active(id, false)?;
        } else {
            self.tree("content")
                .get_meta(id.node)?
                .insert(HEAD, false)?;
        }
        Ok(true)
    }

    /// Where a block goes at `index` among `parent`'s children (without
    /// `moving`): in the tree before a node, at the end, or into a flow, as
    /// an embed at a scalar index of a host's text.
    pub(crate) fn placement(
        &self,
        parent: Option<NodeId>,
        index: usize,
        moving: Option<NodeId>,
    ) -> Result<Placement, DocError> {
        if let Some(p) = parent {
            if p.is_break() {
                return Err(DocError::Malformed(p, "a break paragraph has no children"));
            }
            if !self.is_live(p) {
                return Err(DocError::NoNode(p));
            }
        }
        let siblings: Vec<NodeId> = self
            .children(parent)
            .into_iter()
            .filter(|&s| Some(s) != moving)
            .collect();
        if index > siblings.len() {
            return Err(DocError::BadIndex {
                index,
                len: siblings.len(),
            });
        }
        let tree_parent = parent.map_or(TreeParentId::Root, |p| TreeParentId::Node(p.node));
        let Some(&next) = siblings.get(index) else {
            return Ok(Placement::Tree {
                parent: tree_parent,
                before: None,
            });
        };
        if next.is_break() {
            let flow = self.flow(next.node).ok_or(DocError::NoNode(next))?;
            let at = flow
                .para(next)
                .and_then(|p| p.mark_at)
                .ok_or(DocError::NoNode(next))?;
            return Ok(Placement::Embed {
                host: next.node,
                at,
            });
        }
        let tree = self.tree("content");
        if let Some(TreeParentId::Node(host)) = tree.parent(next.node)
            && let Some(flow) = self.flow(host)
            && let Some((_, _, at)) = flow.embed_of(next.node)
        {
            return Ok(Placement::Embed { host, at });
        }
        Ok(Placement::Tree {
            parent: tree_parent,
            before: Some(next.node),
        })
    }

    /// Places a live or staged tree node per [`Document::placement`]. An
    /// embed is staged (committing what was pending as part of the step)
    /// and activated. Clears an embed the node had before.
    pub(crate) fn put(&self, id: NodeId, placement: Placement) -> Result<(), DocError> {
        let tree = self.tree("content");
        let previous = self.embed_record_of(id.node);
        match placement {
            Placement::Tree { parent, before } => match before {
                Some(next) => tree.mov_before(id.node, next)?,
                None => {
                    let last = tree
                        .children(parent)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|&s| s != id.node && self.live(&tree, s))
                        .last();
                    match last {
                        Some(last) => tree.mov_after(id.node, last)?,
                        None => tree.mov(id.node, parent)?,
                    }
                }
            },
            Placement::Embed { host, at } => {
                let raw = id.node.to_string();
                let embed = self.stage_mark(host, at, |rec| {
                    rec.insert(EMBED, raw.as_str())?;
                    Ok(())
                })?;
                tree.mov(id.node, TreeParentId::Node(host))?;
                self.set_record_active(embed, true)?;
            }
        }
        if let Some(old) = previous {
            self.set_record_active(old, false)?;
        }
        Ok(())
    }

    /// The active embed record that currently places `node`, if any.
    pub(crate) fn embed_record_of(&self, node: TreeID) -> Option<NodeId> {
        let tree = self.tree("content");
        let Some(TreeParentId::Node(host)) = tree.parent(node) else {
            return None;
        };
        let flow = self.flow(host)?;
        flow.embed_of(node)
            .map(|(_, id, _)| NodeId::at_break(host, id))
    }
}

impl Document {
    /// Which live block an anchor made in `node` is in now, and where. In a
    /// flow the anchor's character may have moved to another paragraph (a
    /// split or a join, perhaps a collaborator's), and this finds it there.
    /// `None` when no live block holds it.
    pub fn locate(&self, node: NodeId, anchor: &Anchor) -> Option<(NodeId, Resolved)> {
        let Some(flow) = self.flow(node.node) else {
            let block = self.block(node).ok()?;
            return block.text.resolve(anchor).ok().map(|r| (node, r));
        };
        if !self.live(&self.tree("content"), node.node) {
            return None;
        }
        let shared = Text::from_loro(self.host_text(node.node)?);
        let u = shared.resolve_shared(anchor).ok()?.offset();
        let para = flow
            .live()
            .find(|p| p.region.start.1 <= u && u <= p.region.end.1)?;
        let block = self.block(para.id).ok()?;
        block.text.resolve(anchor).ok().map(|r| (para.id, r))
    }

    /// The order of two blocks of the same flow, by text position. Blocks
    /// of different flows, or not in a flow, compare equal only to
    /// themselves and otherwise by ID.
    pub(crate) fn flow_cmp(&self, a: NodeId, b: NodeId) -> std::cmp::Ordering {
        if a == b {
            return std::cmp::Ordering::Equal;
        }
        if a.node == b.node
            && let Some(flow) = self.flow(a.node)
            && let (Some(&i), Some(&j)) = (flow.index.get(&a), flow.index.get(&b))
        {
            return i.cmp(&j);
        }
        a.cmp(&b)
    }
}

/// Where [`Document::placement`] puts a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// Among `parent`'s tree children: before `before`, or last.
    Tree {
        parent: TreeParentId,
        before: Option<TreeID>,
    },
    /// In `host`'s flow, as an embed at scalar index `at`.
    Embed { host: TreeID, at: usize },
}

fn finish(head_live: bool, paras: Vec<Para>, marks: Vec<(NodeId, usize)>) -> HostFlow {
    let index = paras.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    HostFlow {
        head_live,
        paras,
        index,
        marks,
    }
}
