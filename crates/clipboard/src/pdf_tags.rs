//! The structure tree of the exported PDF (32, 33, 35).
//!
//! Layout gives the blocks in reading order and the document gives their
//! meaning. This module joins them into a `reprise_display::pdf::tags::Structure`:
//!
//! -   a block is a `P`, an `H1` to `H6` when its style is named like a heading
//!     (see [`heading_level`]), or a `Figure` for an image;
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

use std::collections::BTreeMap;
use std::ops::Range;

use reprise_diag::Note;
use reprise_display::pdf::tags::{self, CellRole, Child, Content, Link, Node, Role, Structure};
use reprise_doc::relation::builtin::{FLOAT, NOTE};
use reprise_doc::{CellInfo, Document, NodeId, RowInfo, TableColumns, TableRole};
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
/// table's columns, the cell's row and the cell itself. Today every cell is a
/// plain `TD` spanning one row and column. The `tables` workstream adds header
/// rows and spans: when merging, return `header: Some(Scope::Column)` for a cell
/// of a header row (`row.header`) and the real spans, and nothing else changes,
/// because the renderer already writes `TH`, `/Scope`, `RowSpan` and `ColSpan`.
pub fn cell_role(_table: &TableColumns, _row: &RowInfo, _cell: &CellInfo) -> CellRole {
    CellRole::default()
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

struct Builder<'a> {
    doc: &'a Document,
    layout: &'a LayoutSnapshot,
    notes: Vec<Note>,
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
    };
    let (children, derived_title) = builder.tree(&blocks);
    let structure = Structure {
        title: meta.title.clone().or(derived_title).unwrap_or_default(),
        lang: meta.lang.clone().unwrap_or_default(),
        children,
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
        for block in blocks {
            if block.runs.is_empty() {
                continue;
            }
            let chain = self.table_chain(block.node);
            let mut element = self.element(
                block,
                placed.get(&block.node).copied(),
                numbers.get(&block.node).copied(),
                anchors
                    .get(&block.node)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
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
            if let Some(element) = element.take() {
                open.place(chain, element);
            }
        }
        (open.finish(), title.or(first_text))
    }

    /// The table, row and cell containers above `node`, outermost first.
    fn table_chain(&self, node: NodeId) -> Vec<Container> {
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
        let mut table = None;
        let mut row = None;
        chain
            .into_iter()
            .map(|(id, role)| {
                let role = match role {
                    TableRole::Table(columns) => {
                        table = Some(columns);
                        Role::Table
                    }
                    TableRole::Row(info) => {
                        row = Some(info);
                        Role::Row
                    }
                    TableRole::Cell(cell) => Role::Cell(cell_role(
                        &table.clone().unwrap_or(TableColumns {
                            columns: Vec::new(),
                        }),
                        &row.clone().unwrap_or(RowInfo { header: false }),
                        &cell,
                    )),
                };
                Container { id, role }
            })
            .collect()
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
            match style {
                Some(level) => Node::new(
                    Role::Heading {
                        level,
                        title: snippet(&layout_block.text),
                    },
                    children,
                ),
                None => Node::new(Role::P, children),
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

    /// The leaves of a block, with each anchored range as a `Reference` over
    /// the glyphs that draw it.
    fn content(&mut self, block: &PdfReadingBlock, anchors: &[Anchor]) -> Vec<Child> {
        let Some(text) = self.layout.block(block.node).map(|b| b.text.as_str()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // Anchors that a run has already used or that overlap an earlier one.
        let mut used = vec![false; anchors.len()];
        for run in &block.runs {
            let Some(bytes) = run.bytes.clone() else {
                out.push(Child::Content(Content::whole(run.run.clone())));
                continue;
            };
            // The anchored bytes inside this run, in order, without overlaps.
            let mut cuts: Vec<(Range<usize>, usize)> = Vec::new();
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
                cuts.push((start..end, i));
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
            for (range, i) in cuts {
                if at < range.start {
                    out.push(Child::Content(part(at..range.start)));
                }
                let link = self.link(run, &range, anchors[i].note);
                out.push(Child::Node(Node::new(
                    Role::Reference { link },
                    vec![Child::Content(part(range.clone()))],
                )));
                at = range.end;
            }
            if at < bytes.end {
                out.push(Child::Content(part(at..bytes.end)));
            }
        }
        out
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

struct Container {
    id: NodeId,
    role: Role,
}

/// The containers (table, row, cell) open around the elements placed so far.
#[derive(Default)]
struct Open {
    root: Vec<Node>,
    /// Open containers, outermost first, with the document node each was made for.
    stack: Vec<(NodeId, Node)>,
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
            .take_while(|((open, _), c)| *open == c.id)
            .count();
        self.close_to(shared);
        for container in chain.into_iter().skip(shared) {
            self.stack
                .push((container.id, Node::new(container.role, Vec::new())));
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
