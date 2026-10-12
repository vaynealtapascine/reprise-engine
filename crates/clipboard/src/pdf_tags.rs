//! The structure tree of the exported PDF (32, 33, 35).
//!
//! Layout gives the blocks in reading order and the document gives their
//! meaning. This module joins them into a `reprise_display::pdf::tags::Structure`:
//!
//! -   a block is what its role says (`H1`–`H6`, `Title`, `BlockQuote`,
//!     `Code`; see [`pdf_role`]); a body block is a `P`, or an `H1` to `H6`
//!     when its style is named like a heading (see [`heading_level`]); an
//!     image is a `Figure`;
//! -   consecutive list items are `L` > `LI` > (`Lbl`, `LBody`), a nested list
//!     inside its parent item's `LBody`;
//! -   a block placed by `reprise.note` is a `Note` with an ID, and the anchor
//!     of that note becomes a `Reference` linked to it where the anchor's
//!     glyphs can be cut out of their run;
//! -   a block placed by `reprise.float` is a `Div` (or the `Figure` itself);
//! -   blocks inside table, row and cell containers are `Table`, `TR`, `TD`
//!     (see [`cell_role`]).
//!
//! The order is [`LayoutSnapshot::reading_order`], never paint order, so
//! rotated, mirrored, vertical and spiral text, notes and floats each appear
//! once, where the reading order puts them.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::rc::Rc;

use reprise_diag::Note;
use reprise_display::pdf::tags::{
    self, CellRole, Child, Content, DocLink, Link, LinkTo, Node, Numbering, Role, Scope, Structure,
};
use reprise_doc::annotation::{AnnotationData, AnnotationState, AnnotationView, Kind, LinkTarget};
use reprise_doc::relation::builtin::{FLOAT, NOTE};
use reprise_doc::{Document, GridCell, NodeId, RangeState, TableGrid, TableRole};
use reprise_geom::{FrameSpace, Length, PageSpace, Point, Rect};
use reprise_layout::{
    LayoutSnapshot, LineRef, PdfReadingBlock, PdfReadingRun, RelationStatus, Resolution,
};

/// The most table ancestors followed from a block.
const MAX_ANCESTORS: usize = 64;
/// Longest derived title, in characters.
const MAX_TITLE: usize = 120;

/// Author-supplied metadata for the exported PDF.
#[derive(Clone, Debug, Default)]
pub struct PdfMetadata {
    /// The title. Default: the first heading, else the start of the first block.
    pub title: Option<String>,
    /// The language, a BCP 47 tag. The document model records none, so
    /// the default is `reprise_display::pdf::tags::DEFAULT_LANG`.
    pub lang: Option<String>,
}

/// The heading level a named style asks for: `h1` to `h6`, `heading1` to
/// `heading6`, with an optional `-`, `_` or space before the digit, in any case.
///
/// The document model has no heading block kind, so a style name is the only
/// authored signal. This is the one place that reads it.
pub fn heading_level(style: &str) -> Option<u8> {
    let lower = style.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("heading")
        .or_else(|| lower.strip_prefix('h'))?;
    let digit = rest.trim_start_matches(['-', '_', ' ']);
    match digit.as_bytes() {
        [d @ b'1'..=b'6'] => Some(d - b'0'),
        _ => None,
    }
}

/// The seam for table cell semantics: whether a cell is a `TD` or a `TH`, its
/// `/Scope`, and its `/RowSpan` and `/ColSpan`.
///
/// Everything about a cell's role is decided here and nowhere else, from the
/// cell's resolved grid entry (`Document::table_structure`). A cell that starts
/// in a header row is a `TH` with scope `Column`; spans are the grid's clamped
/// spans. A cell with no grid entry (an unresolvable table) is a plain `TD`.
pub fn cell_role(cell: Option<&GridCell>) -> CellRole {
    let Some(cell) = cell else {
        return CellRole::default();
    };
    let span = |n: usize| u32::try_from(n).unwrap_or(u32::MAX).max(1);
    CellRole {
        header: cell.header.then_some(Scope::Column),
        row_span: span(cell.rowspan),
        col_span: span(cell.colspan),
    }
}
/// The structure role a block's semantic role asks for; `None` for body text,
/// which falls back to [`heading_level`] on its style name.
pub fn pdf_role(role: reprise_doc::Role, title: &str) -> Option<Role> {
    use reprise_doc::Role as R;
    match role {
        R::Heading(level) => Some(Role::Heading {
            level,
            title: title.to_string(),
        }),
        R::Title => Some(Role::Title),
        R::Quote => Some(Role::BlockQuote),
        R::Code => Some(Role::Code),
        // PDF 1.7 has no subtitle or verse element: they are paragraphs.
        R::Subtitle | R::Verse => Some(Role::P),
        R::Body => None,
    }
}

/// A list's `/ListNumbering`, from its first item.
pub fn numbering(item: &reprise_doc::ListItem) -> Numbering {
    use reprise_doc::NumberFormat as F;
    match &item.format {
        F::Bullet => match item.marker.as_str() {
            "◦" | "○" => Numbering::Circle,
            "▪" | "■" => Numbering::Square,
            _ => Numbering::Disc,
        },
        F::Decimal | F::DecimalLeadingZero | F::Unknown(_) => Numbering::Decimal,
        F::LowerAlpha => Numbering::LowerAlpha,
        F::UpperAlpha => Numbering::UpperAlpha,
        F::LowerRoman => Numbering::LowerRoman,
        F::UpperRoman => Numbering::UpperRoman,
        F::None => Numbering::None,
    }
}

/// One open list level while building the structure.
struct OpenList {
    level: u8,
    list: reprise_doc::ListId,
    /// The block that opened this `L`: its key.
    key: NodeId,
    numbering: Numbering,
    /// The item whose `LI` is open at this level.
    item: NodeId,
}

/// What a block placed by a relation is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Placed {
    Note,
    Float,
}

/// A note whose anchor is a range of another block.
struct Anchor {
    bytes: Range<usize>,
    /// The note's number, in reading order, from 1.
    note: u32,
}

/// Where a document link goes, before keys are given to its target blocks.
#[derive(Clone)]
enum LinkSpec {
    Uri(String),
    Block(NodeId),
}

/// The part of a link in one block, from the annotation index.
#[derive(Clone)]
struct LinkCut {
    bytes: Range<usize>,
    to: LinkSpec,
}

/// What a stretch of a run is for.
#[derive(Clone, Copy)]
enum Cut {
    /// The anchor of note `anchors[i]`.
    Note(usize),
    /// Linked text: `links[i]`.
    Link(usize),
}

/// Keys for blocks that links go to start here, above any note number.
const LINK_KEY_BASE: u32 = 1 << 24;

struct Builder<'a> {
    doc: &'a Document,
    layout: &'a LayoutSnapshot,
    notes: Vec<Note>,
    grids: BTreeMap<NodeId, Option<Rc<TableGrid>>>,
    view: AnnotationView,
    /// Link parts by block.
    links: BTreeMap<NodeId, Vec<LinkCut>>,
    /// The key of each block a link goes to.
    keys: BTreeMap<NodeId, u32>,
}

/// The structure for `layout`, in reading order, with what was lost doing so.
pub(crate) fn structure(
    doc: &Document,
    layout: &LayoutSnapshot,
    meta: &PdfMetadata,
) -> (Structure, Vec<Note>) {
    let blocks = layout.pdf_reading_blocks(doc);
    let mut builder = Builder {
        doc,
        layout,
        notes: Vec::new(),
        grids: BTreeMap::new(),
        view: doc.annotation_view(),
        links: BTreeMap::new(),
        keys: BTreeMap::new(),
    };
    let (children, derived_title) = builder.tree(&blocks);
    let structure = Structure {
        title: meta.title.clone().or(derived_title).unwrap_or_default(),
        lang: meta.lang.clone().unwrap_or_default(),
        children,
        // Repeated table headers are derived copies: decoration, never content.
        artifacts: layout.pdf_artifact_runs(),
    };
    (structure, builder.notes)
}

/// Collapses whitespace and cuts at `MAX_TITLE` characters.
fn snippet(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let joined = words.join(" ");
    if joined.chars().count() <= MAX_TITLE {
        return joined;
    }
    let mut cut: String = joined.chars().take(MAX_TITLE).collect();
    cut.push('…');
    cut
}

impl Builder<'_> {
    fn tree(&mut self, blocks: &[PdfReadingBlock]) -> (Vec<Node>, Option<String>) {
        // What each placed block is, from the relations layout applied.
        let mut placed: BTreeMap<NodeId, Placed> = BTreeMap::new();
        let mut note_anchor: BTreeMap<NodeId, (NodeId, Range<usize>)> = BTreeMap::new();
        for rel in &self.layout.relations {
            let Some(owner) = rel.owner else { continue };
            if !rel.applied
                || !matches!(rel.status, RelationStatus::Valid | RelationStatus::Rebound)
            {
                continue;
            }
            if rel.schema == NOTE {
                placed.insert(owner, Placed::Note);
                let anchor = rel.targets.iter().find(|t| t.role == "anchor");
                if let Some(Resolution::Range { node, bytes }) =
                    anchor.and_then(|t| t.resolved.clone())
                {
                    note_anchor.insert(owner, (node, bytes));
                }
            } else if rel.schema == FLOAT {
                placed.insert(owner, Placed::Float);
            }
        }
        // Number the notes in reading order, then collect each block's anchors.
        let mut numbers: BTreeMap<NodeId, u32> = BTreeMap::new();
        for block in blocks {
            if placed.get(&block.node) == Some(&Placed::Note) {
                let next = numbers.len() as u32 + 1;
                numbers.insert(block.node, next);
            }
        }
        self.collect_links(blocks, &numbers);
        let mut anchors: BTreeMap<NodeId, Vec<Anchor>> = BTreeMap::new();
        for (note, (node, bytes)) in &note_anchor {
            if let Some(&number) = numbers.get(note) {
                anchors.entry(*node).or_default().push(Anchor {
                    bytes: bytes.clone(),
                    note: number,
                });
            }
        }
        for list in anchors.values_mut() {
            list.sort_by_key(|a| (a.bytes.start, a.bytes.end, a.note));
        }

        let mut title = None;
        let mut first_text = None;
        let mut open = Open::default();
        let mut title_role: Option<String> = None;
        let mut lists: Vec<OpenList> = Vec::new();
        let mut list_base: Option<Vec<(NodeId, Part)>> = None;
        for block in blocks {
            if block.runs.is_empty() {
                continue;
            }
            let chain = self.table_chain(block.node);
            // Lists nest only among blocks of one container (a cell, the root).
            let keys: Vec<_> = chain.iter().map(|c| c.key).collect();
            if list_base.as_ref() != Some(&keys) {
                lists.clear();
                list_base = Some(keys);
            }
            // A placed note or float interrupts a list; it is not an item of it.
            let item = self
                .doc
                .list_item(block.node)
                .filter(|_| !placed.contains_key(&block.node));
            let (labels, rest): (Vec<_>, Vec<_>) = if item.is_some() {
                block.runs.iter().cloned().partition(|r| r.label)
            } else {
                (Vec::new(), block.runs.clone())
            };
            let block = &PdfReadingBlock {
                node: block.node,
                runs: rest,
            };
            if let Some(Role::Title) = self.semantic_role(block.node)
                && let Some(b) = self.layout.block(block.node)
            {
                let s = snippet(&b.text);
                if !s.is_empty() {
                    title_role.get_or_insert(s);
                }
            }
            let mut element = self.element(
                block,
                placed.get(&block.node).copied(),
                numbers.get(&block.node).copied(),
                anchors
                    .get(&block.node)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
            // The block a link goes to is where the destination lands.
            if let (Some(key), Some(element)) = (self.keys.get(&block.node), element.as_mut())
                && element.key.is_none()
            {
                element.key = Some(*key);
            }
            if let Some(Role::Heading { title: t, .. }) = element.as_ref().map(|e| e.role.clone()) {
                title.get_or_insert(t);
            }
            if first_text.is_none()
                && let Some(b) = self.layout.block(block.node)
                && block.runs.iter().any(|r| r.bytes.is_some())
            {
                let s = snippet(&b.text);
                if !s.is_empty() {
                    first_text = Some(s);
                }
            }
            let Some(element) = element.take() else {
                continue;
            };
            let Some(item) = item else {
                lists.clear();
                open.place(chain, element);
                continue;
            };
            if lists.first().is_some_and(|l| l.list != item.list) {
                lists.clear();
            }
            while lists.last().is_some_and(|l| l.level > item.level) {
                lists.pop();
            }
            let (key, list_numbering) = match lists.last() {
                Some(l) if l.level == item.level => {
                    let reused = (l.key, l.numbering);
                    lists.pop();
                    reused
                }
                _ => (block.node, numbering(&item)),
            };
            let mut full = chain;
            for l in &lists {
                full.push(Container {
                    key: (l.key, Part::List),
                    role: Role::List(l.numbering),
                });
                full.push(Container {
                    key: (l.item, Part::Item),
                    role: Role::ListItem,
                });
                full.push(Container {
                    key: (l.item, Part::ListBody),
                    role: Role::ListBody,
                });
            }
            full.push(Container {
                key: (key, Part::List),
                role: Role::List(list_numbering),
            });
            full.push(Container {
                key: (block.node, Part::Item),
                role: Role::ListItem,
            });
            lists.push(OpenList {
                level: item.level,
                list: item.list,
                key,
                numbering: list_numbering,
                item: block.node,
            });
            if !labels.is_empty() {
                let leaves = labels
                    .into_iter()
                    .map(|r| Child::Content(Content::whole(r.run)))
                    .collect();
                open.place(full.clone(), Node::new(Role::Label, leaves));
            }
            full.push(Container {
                key: (block.node, Part::ListBody),
                role: Role::ListBody,
            });
            open.place(full, element);
        }
        (open.finish(), title_role.or(title).or(first_text))
    }

    /// The table containers above `node`, outermost first: `Table`, then for
    /// a table with header rows `THead` or `TBody`, then `TR` and the cell.
    fn table_chain(&mut self, node: NodeId) -> Vec<Container> {
        let mut chain = Vec::new();
        let mut at = self.doc.parent_of(node).flatten();
        let mut steps = 0;
        while let Some(parent) = at {
            steps += 1;
            if steps > MAX_ANCESTORS {
                break;
            }
            if let Ok(Some(role)) = self.doc.table_role(parent) {
                chain.push((parent, role));
            }
            at = self.doc.parent_of(parent).flatten();
        }
        chain.reverse();
        let mut out = Vec::new();
        let mut table = None;
        for (id, role) in chain {
            match role {
                TableRole::Table(_) => {
                    table = Some(id);
                    out.push(Container {
                        key: (id, Part::Node),
                        role: Role::Table,
                    });
                }
                TableRole::Row(_) => {
                    if let Some(table) = table
                        && let Some(grid) = self.grid(table)
                        && grid.repeating_header_rows > 0
                    {
                        let head = grid
                            .rows
                            .iter()
                            .position(|r| r.node == id)
                            .is_some_and(|i| i < grid.repeating_header_rows);
                        out.push(if head {
                            Container {
                                key: (table, Part::Head),
                                role: Role::TableHead,
                            }
                        } else {
                            Container {
                                key: (table, Part::Body),
                                role: Role::TableBody,
                            }
                        });
                    }
                    out.push(Container {
                        key: (id, Part::Node),
                        role: Role::Row,
                    });
                }
                TableRole::Cell(_) => {
                    let grid = table.and_then(|t| self.grid(t));
                    let cell = grid.as_ref().and_then(|g| g.cell(id));
                    out.push(Container {
                        key: (id, Part::Node),
                        role: Role::Cell(cell_role(cell)),
                    });
                }
            }
        }
        out
    }

    /// A table's resolved grid, computed once per export.
    fn grid(&mut self, table: NodeId) -> Option<Rc<TableGrid>> {
        let doc = self.doc;
        self.grids
            .entry(table)
            .or_insert_with(|| doc.table_structure(table).ok().flatten().map(Rc::new))
            .clone()
    }
    /// The element for one block: its role around its runs, with note
    /// references cut out of the runs they anchor to.
    fn element(
        &mut self,
        block: &PdfReadingBlock,
        placed: Option<Placed>,
        note_number: Option<u32>,
        anchors: &[Anchor],
    ) -> Option<Node> {
        let layout_block = self.layout.block(block.node)?;
        let at = block.runs.first().and_then(|r| self.page_point(r.line));
        let mut node = if let Some(image) = &layout_block.image {
            let leaves = block
                .runs
                .iter()
                .map(|r| Child::Content(Content::whole(r.run.clone())))
                .collect();
            let figure = Node::new(
                Role::Figure {
                    alt: image.alt.clone(),
                },
                leaves,
            );
            if placed == Some(Placed::Float) {
                // A floated image is its own element; a Div would add nothing.
                return Some(figure);
            }
            figure
        } else {
            let children = self.content(block, anchors);
            let style = self
                .doc
                .block(block.node)
                .ok()
                .and_then(|b| b.style)
                .and_then(|s| heading_level(&s));
            let semantic = self.doc.role(block.node).unwrap_or_default();
            match (pdf_role(semantic, &snippet(&layout_block.text)), style) {
                (Some(role), _) => Node::new(role, children),
                (None, Some(level)) => Node::new(
                    Role::Heading {
                        level,
                        title: snippet(&layout_block.text),
                    },
                    children,
                ),
                (None, None) => Node::new(Role::P, children),
            }
        };
        node.at = at;
        match placed {
            Some(Placed::Note) => {
                let mut note = Node::new(
                    Role::Note {
                        id: note_number.map(|n| format!("note-{n}")),
                    },
                    vec![Child::Node(node)],
                );
                note.key = note_number;
                note.at = at;
                Some(note)
            }
            Some(Placed::Float) => Some(Node::new(Role::Div, vec![Child::Node(node)])),
            None => Some(node),
        }
    }

    /// The structure role a block's semantic role gives it, if any.
    fn semantic_role(&self, node: NodeId) -> Option<Role> {
        pdf_role(self.doc.role(node).ok()?, "")
    }

    /// The top left of a line, in page space.
    fn page_point(&self, line: LineRef) -> Option<Point<PageSpace>> {
        let l = self.layout.line(line)?;
        let frame = self.layout.frame(l.frame)?;
        Some(
            frame
                .to_page
                .apply(Point::<FrameSpace>::new(l.rect.origin.x, l.rect.origin.y)),
        )
    }

    /// The links of the blocks in `blocks`: each active link annotation's part
    /// in each block, and a key for every block a link goes to.
    fn collect_links(&mut self, blocks: &[PdfReadingBlock], numbers: &BTreeMap<NodeId, u32>) {
        let placed: BTreeSet<NodeId> = blocks
            .iter()
            .filter(|b| !b.runs.is_empty())
            .map(|b| b.node)
            .collect();
        let mut next = LINK_KEY_BASE;
        let mut unresolved = 0usize;
        for block in blocks {
            // Copy the pieces out: the view is borrowed while keys are given.
            let pieces: Vec<_> = self.view.pieces(block.node).to_vec();
            for (bytes, id) in pieces {
                let Some(annotation) = self.view.get(id) else {
                    continue;
                };
                let (Kind::Link, AnnotationState::Active { .. }, AnnotationData::Link(target)) =
                    (annotation.kind(), &annotation.state, &annotation.data)
                else {
                    continue;
                };
                let to = match target {
                    LinkTarget::Url { url } => Some(LinkSpec::Uri(url.clone())),
                    LinkTarget::Node { node } | LinkTarget::Heading { node } => {
                        Some(LinkSpec::Block(*node))
                    }
                    LinkTarget::Range { range } => match self.doc.resolve_range(*range) {
                        RangeState::Valid { node, .. } | RangeState::Rebound { node, .. } => {
                            Some(LinkSpec::Block(node))
                        }
                        _ => None,
                    },
                };
                // A block target must be on a page to be landed on.
                let to = to.filter(|to| match to {
                    LinkSpec::Block(node) => placed.contains(node),
                    LinkSpec::Uri(_) => true,
                });
                let Some(to) = to else {
                    unresolved += 1;
                    continue;
                };
                if let LinkSpec::Block(node) = &to
                    && !self.keys.contains_key(node)
                {
                    // A note is keyed by its number; any other block gets a new key.
                    let key = numbers.get(node).copied().unwrap_or_else(|| {
                        next += 1;
                        next - 1
                    });
                    self.keys.insert(*node, key);
                }
                self.links
                    .entry(block.node)
                    .or_default()
                    .push(LinkCut { bytes, to });
            }
        }
        if unresolved > 0 {
            self.notes.push(Note::warning(
                tags::LINK_UNRESOLVED,
                format!("{unresolved} links go to text that is not on a page and were not linked"),
            ));
        }
    }

    /// The leaves of a block, with each anchored range as a `Reference` and
    /// each linked range as a `Link` over the glyphs that draw it.
    fn content(&mut self, block: &PdfReadingBlock, anchors: &[Anchor]) -> Vec<Child> {
        let Some(text) = self.layout.block(block.node).map(|b| b.text.as_str()) else {
            return Vec::new();
        };
        let links = self.links.get(&block.node).cloned().unwrap_or_default();
        let mut out = Vec::new();
        // Anchors that a run has already used or that overlap an earlier one.
        let mut used = vec![false; anchors.len()];
        let mut overlapped = false;
        for run in &block.runs {
            let Some(bytes) = run.bytes.clone() else {
                out.push(Child::Content(Content::whole(run.run.clone())));
                continue;
            };
            // The anchored bytes inside this run, in order, without overlaps.
            let mut cuts: Vec<(Range<usize>, Cut)> = Vec::new();
            for (i, anchor) in anchors.iter().enumerate() {
                let range = anchor_chars(text, &anchor.bytes);
                let (start, end) = (range.start.max(bytes.start), range.end.min(bytes.end));
                if start >= end || used[i] {
                    continue;
                }
                if cuts.last().is_some_and(|(last, _)| start < last.end) {
                    self.unlinked("two note anchors overlap; the second is not linked");
                    used[i] = true;
                    continue;
                }
                if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                    self.unlinked("a note anchor is not on character boundaries");
                    used[i] = true;
                    continue;
                }
                // An anchor that goes on into the next run keeps its end here.
                used[i] = range.end <= bytes.end;
                cuts.push((start..end, Cut::Note(i)));
            }
            // Linked text takes what the note anchors leave, and the first of
            // two overlapping links keeps the text they share.
            for (j, link) in links.iter().enumerate() {
                let start = link.bytes.start.max(bytes.start);
                let end = link.bytes.end.min(bytes.end);
                if start >= end {
                    continue;
                }
                if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                    overlapped = true;
                    continue;
                }
                let mut free = Vec::new();
                let mut at = start;
                for (taken, _) in cuts.iter().filter(|(r, _)| r.start < end && start < r.end) {
                    if at < taken.start {
                        free.push(at..taken.start);
                    }
                    at = at.max(taken.end);
                }
                if at < end {
                    free.push(at..end);
                }
                if free.iter().map(|r| r.len()).sum::<usize>() < end - start {
                    overlapped = true;
                }
                cuts.extend(free.into_iter().map(|r| (r, Cut::Link(j))));
                cuts.sort_by_key(|(r, _)| (r.start, r.end));
            }
            if cuts.is_empty() {
                out.push(Child::Content(Content::whole(run.run.clone())));
                continue;
            }
            let part = |r: Range<usize>| Content {
                run: run.run.clone(),
                part: Some(r.start - bytes.start..r.end - bytes.start),
            };
            let mut at = bytes.start;
            for (range, cut) in cuts {
                if at < range.start {
                    out.push(Child::Content(part(at..range.start)));
                }
                let inner = vec![Child::Content(part(range.clone()))];
                match cut {
                    Cut::Note(i) => {
                        let link = self.link(run, &range, anchors[i].note);
                        out.push(Child::Node(Node::new(Role::Reference { link }, inner)));
                    }
                    Cut::Link(j) => match links
                        .get(j)
                        .and_then(|l| self.doc_link(run, &range, text, l))
                    {
                        Some(link) => out.push(Child::Node(Node::new(
                            Role::Link { link: Some(link) },
                            inner,
                        ))),
                        None => out.extend(inner),
                    },
                }
                at = range.end;
            }
            if at < bytes.end {
                out.push(Child::Content(part(at..bytes.end)));
            }
        }
        if overlapped {
            self.notes.push(Note::warning(
                tags::LINK_UNRESOLVED,
                "overlapping links share text; the first keeps it and the others cover the rest",
            ));
        }
        out
    }

    /// The annotation for linked text: its area, its target and its alt text.
    fn doc_link(
        &mut self,
        run: &PdfReadingRun,
        range: &Range<usize>,
        text: &str,
        link: &LinkCut,
    ) -> Option<DocLink> {
        let Some(rect) = self.glyph_rect(run, range) else {
            self.notes.push(Note::warning(
                tags::LINK_UNRESOLVED,
                "linked text has no glyphs to make a link area from",
            ));
            return None;
        };
        let label = snippet(text.get(link.bytes.clone()).unwrap_or_default());
        let (to, alt) = match &link.to {
            LinkSpec::Uri(uri) => (
                LinkTo::Uri(uri.clone()),
                if label.is_empty() {
                    uri.clone()
                } else {
                    format!("{label}: {uri}")
                },
            ),
            LinkSpec::Block(node) => {
                let key = *self.keys.get(node)?;
                let target = self
                    .layout
                    .block(*node)
                    .map(|b| snippet(&b.text))
                    .unwrap_or_default();
                (
                    LinkTo::Node(key),
                    if target.is_empty() {
                        label
                    } else {
                        format!("{label}: go to {target}")
                    },
                )
            }
        };
        Some(DocLink { rect, to, alt })
    }

    fn unlinked(&mut self, message: &str) {
        self.notes
            .push(Note::info(tags::REFERENCE_UNLINKED, message.to_string()));
    }

    /// The link from the glyphs of `range` in `run` to note `number`.
    fn link(&mut self, run: &PdfReadingRun, range: &Range<usize>, number: u32) -> Option<Link> {
        let rect = self.glyph_rect(run, range);
        if rect.is_none() {
            self.unlinked("a note anchor has no glyphs to make a link area from");
        }
        Some(Link {
            rect: rect?,
            target: number,
            alt: format!("Go to note {number}"),
        })
    }

    /// The page-space box of the glyphs of `run` whose clusters start in `range`.
    fn glyph_rect(&self, run: &PdfReadingRun, range: &Range<usize>) -> Option<Rect<PageSpace>> {
        let line = self.layout.line(run.line)?;
        let positioned = line
            .runs
            .iter()
            .find(|r| Some(&r.range) == run.bytes.as_ref())?;
        let frame = self.layout.frame(line.frame)?;
        let mut x = positioned.x;
        let mut span: Option<(Length, Length)> = None;
        for glyph in &positioned.glyphs {
            let next = x + glyph.advance;
            let cluster = glyph.cluster as usize;
            if range.start <= cluster && cluster < range.end {
                span = Some(match span {
                    Some((a, b)) => (a.min(x), b.max(next)),
                    None => (x, next),
                });
            }
            x = next;
        }
        let (from, to) = span?;
        let area = Rect::<FrameSpace>::new(
            Point::new(from.min(to), line.rect.origin.y),
            (to - from).max(Length::ZERO).max(Length::from_pt(1)),
            line.rect.height,
        );
        Some(frame.to_page.bounds(&area))
    }
}

/// The text an anchor marks: its range, or for a point the character before it
/// (a note mark follows the word it belongs to), else the one after.
fn anchor_chars(text: &str, bytes: &Range<usize>) -> Range<usize> {
    if bytes.start != bytes.end {
        return bytes.clone();
    }
    let at = bytes.start.min(text.len());
    if let Some(c) = text.get(..at).and_then(|t| t.chars().next_back()) {
        return at - c.len_utf8()..at;
    }
    match text.get(at..).and_then(|t| t.chars().next()) {
        Some(c) => at..at + c.len_utf8(),
        None => bytes.clone(),
    }
}

/// Which part of a document node a container is: the node itself, or a
/// `THead` or `TBody` section the table's rows are grouped into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Node,
    Head,
    Body,
    /// An `L`, named by the item that opened it.
    List,
    /// An item's `LI`.
    Item,
    /// An item's `LBody`.
    ListBody,
}

#[derive(Clone)]
struct Container {
    key: (NodeId, Part),
    role: Role,
}

/// The containers (table, row, cell) open around the elements placed so far.
#[derive(Default)]
struct Open {
    root: Vec<Node>,
    /// Open containers, outermost first, with the document node each was made for.
    stack: Vec<((NodeId, Part), Node)>,
}

impl Open {
    fn close_to(&mut self, depth: usize) {
        while self.stack.len() > depth {
            let Some((_, done)) = self.stack.pop() else {
                return;
            };
            match self.stack.last_mut() {
                Some((_, parent)) => parent.children.push(Child::Node(done)),
                None => self.root.push(done),
            }
        }
    }

    fn place(&mut self, chain: Vec<Container>, element: Node) {
        let shared = self
            .stack
            .iter()
            .zip(&chain)
            .take_while(|((open, _), c)| *open == c.key)
            .count();
        self.close_to(shared);
        for container in chain.into_iter().skip(shared) {
            self.stack
                .push((container.key, Node::new(container.role, Vec::new())));
        }
        match self.stack.last_mut() {
            Some((_, parent)) => parent.children.push(Child::Node(element)),
            None => self.root.push(element),
        }
    }

    fn finish(mut self) -> Vec<Node> {
        self.close_to(0);
        self.root
    }
}

/// Renders `layout` as a tagged PDF. Returns the bytes, what the backend and
/// the structure builder report, and whether the file claims PDF/UA-1.
pub(crate) fn render(
    doc: &Document,
    layout: &LayoutSnapshot,
    fonts: &reprise_font::FontStore,
    assets: &reprise_display::AssetStore,
    meta: &PdfMetadata,
) -> Result<(Vec<u8>, Vec<Note>, bool), crate::ClipboardError> {
    let lists = layout.to_display_lists(reprise_layout::DisplayOptions::default());
    let (structure, mut notes) = structure(doc, layout, meta);
    let tagged = reprise_display::pdf::render_tagged(&lists, fonts, assets, &structure)
        .map_err(|e| crate::ClipboardError::Export(e.to_string()))?;
    notes.extend(tagged.notes);
    Ok((tagged.bytes, notes, tagged.ua))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_levels_read_style_names() {
        for (name, level) in [
            ("h1", Some(1)),
            ("H6", Some(6)),
            ("heading3", Some(3)),
            ("Heading-2", Some(2)),
            ("heading 4", Some(4)),
            ("h7", None),
            ("h0", None),
            ("heading", None),
            ("header", None),
            ("body", None),
            ("", None),
            ("h12", None),
        ] {
            assert_eq!(heading_level(name), level, "{name}");
        }
    }

    #[test]
    fn point_anchors_mark_the_character_before() {
        assert_eq!(anchor_chars("héllo", &(3..3)), 1..3);
        assert_eq!(anchor_chars("héllo", &(0..0)), 0..1);
        assert_eq!(anchor_chars("ab", &(1..2)), 1..2);
        assert_eq!(anchor_chars("", &(0..0)), 0..0);
        assert_eq!(anchor_chars("ab", &(9..9)), 1..2);
    }

    #[test]
    fn snippets_collapse_whitespace_and_are_bounded() {
        assert_eq!(snippet("  a \n b  "), "a b");
        let long = "word ".repeat(200);
        assert_eq!(snippet(&long).chars().count(), MAX_TITLE + 1);
    }
}
