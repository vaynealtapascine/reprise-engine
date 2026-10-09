//! The derived layout result and its queries (05, 13, 16, 30).
//!
//! A snapshot has pages, frames on those pages, and blocks whose lines sit in
//! frames. Line geometry is in its frame's logical [`FrameSpace`]; each frame
//! has a transform to its page, which carries any writing mode, rotation or
//! mirroring (20). Use [`LayoutSnapshot::line_to_page`] or
//! [`LayoutSnapshot::line_bounds`] for page coordinates.

use std::ops::Range;

use reprise_compose::Explanation;
use reprise_diag::{Code, Note, Severity};
use reprise_doc::{
    BlockKind, ComputedStyle, FrameRole, Medium, NodeId, RangeId, RelationId, Revision, SchemaId,
    SnapshotContent, WritingMode,
};
use reprise_font::FaceId;
use reprise_geom::{FrameSpace, Length, LineSpace, Matrix, PageSpace, Rect, Transform};
use reprise_shape::{AdapterInfo, ShapedGlyph};
use serde::Serialize;

use crate::FlowSettings;

/// Everything layout derived from one document revision. Can be thrown away
/// and recomputed at any time (05).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LayoutSnapshot {
    pub revision: Revision,
    pub adapter: AdapterInfo,
    pub composer: String,
    /// What the document was laid out for: the viewport or the paper (38).
    pub medium: Medium,
    /// The engine settings that shaped the flow (38).
    pub settings: FlowSettings,
    /// The page template every page was made from.
    pub template: TemplateUsed,
    pub pages: Vec<PageLayout>,
    pub frames: Vec<FrameLayout>,
    /// Blocks in the order layout placed them: flowed blocks in flow order,
    /// then blocks placed by relations in relation order.
    pub blocks: Vec<BlockLayout>,
    pub relations: Vec<RelationLayout>,
    /// Derived copies of repeating table header rows on continuation frames
    /// (24, 33). They are not in `blocks`, so block queries, reading order and
    /// the editing kernel never see them as authored text. Left out of JSON
    /// when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub repeated_headers: Vec<RepeatedHeader>,
    pub diagnostics: Vec<Diagnostic>,
}

/// One repeated copy of a table's header rows on a continuation frame. The
/// blocks carry the authored header blocks' node IDs and byte ranges, so a
/// consumer can map any position in a copy to the authored header.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RepeatedHeader {
    pub table: NodeId,
    /// Index into [`LayoutSnapshot::frames`].
    pub frame: usize,
    pub blocks: Vec<BlockLayout>,
}

/// Which page template layout used, and where it came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TemplateUsed {
    pub name: String,
    pub source: TemplateSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateSource {
    /// Stored in the document.
    Document,
    /// The engine's built-in template: the document has none, or its
    /// template couldn't be used (a diagnostic says which).
    Builtin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PageLayout {
    pub width: Length,
    pub height: Length,
}

/// A region that lines sit in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FrameLayout {
    /// The template's name for the frame, such as `main` or `margin`.
    pub name: String,
    /// What the frame is for: which flow threads through it, or margin.
    pub role: FrameRole,
    /// Index into [`LayoutSnapshot::pages`].
    pub page: usize,
    pub to_page: Transform<FrameSpace, PageSpace>,
    /// The frame's extent; its origin is the frame's start corner.
    pub rect: Rect<FrameSpace>,
    /// How glyphs sit in the frame's lines (20): needed to draw upright runs
    /// and the reflected `vertical-lr` frame map. Left out of JSON when
    /// horizontal.
    #[serde(skip_serializing_if = "is_horizontal")]
    pub writing_mode: WritingMode,
}

fn is_horizontal(mode: &WritingMode) -> bool {
    *mode == WritingMode::HorizontalTb
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BlockLayout {
    pub node: NodeId,
    pub kind: BlockKind,
    pub style: ComputedStyle,
    /// The text the block was laid out from. Line, run and glyph positions
    /// are byte offsets into it. Left out of JSON snapshots.
    #[serde(skip)]
    pub text: String,
    /// In logical order. A block's lines may sit in several frames.
    pub lines: Vec<LineLayout>,
    /// The paragraph's resolved bidi base level (22): 0 for left to right,
    /// 1 for right to left. Carets, visual movement and line alignment need
    /// it. Left out of JSON snapshots when 0.
    #[serde(skip_serializing_if = "is_zero")]
    pub base_level: u8,
    /// Positioned image, absent for text blocks and omitted from their JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageLayout>,
}

/// One indivisible image box in logical frame space, sharing a line-like
/// geometry entry so reading order and layout queries also address images.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImageLayout {
    pub asset: String,
    pub alt: String,
    pub frame: usize,
    pub rect: Rect<FrameSpace>,
    pub placeholder: bool,
}

impl BlockLayout {
    pub(crate) fn sync_image(&mut self) {
        if let (Some(image), Some(line)) = (&mut self.image, self.lines.first()) {
            image.frame = line.frame;
            image.rect = line.rect;
        }
    }
}

fn is_zero(level: &u8) -> bool {
    *level == 0
}

/// One line fragment: a line's text in one interval of one frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LineLayout {
    /// Index into [`LayoutSnapshot::frames`].
    pub frame: usize,
    /// Source bytes of the block's text, including trailing whitespace.
    pub text: Range<usize>,
    /// The line's text, for reading snapshots.
    pub preview: String,
    /// The line box, in frame space.
    pub rect: Rect<FrameSpace>,
    /// The baseline's offset on the frame's block axis.
    pub baseline: Length,
    /// The width of the content, without trailing whitespace.
    pub width: Length,
    pub explanation: Explanation,
    /// In visual order, left to right along the inline axis.
    pub runs: Vec<PositionedRun>,
}

/// A run of glyphs placed on a line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PositionedRun {
    #[serde(skip_serializing_if = "is_black")]
    pub color: [u8; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub underline: Option<DecorationLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strike: Option<DecorationLine>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub upright: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub combined: bool,
    #[serde(skip)]
    pub horizontal_scale: reprise_geom::Fixed,
    pub range: Range<usize>,
    pub face: FaceId,
    pub size: Length,
    pub level: u8,
    /// Where the run starts on the frame's inline axis.
    pub x: Length,
    pub width: Length,
    /// In visual order. Each glyph's pen position is `x` plus the advances
    /// of the glyphs before it. Left out of JSON snapshots, which the display
    /// list snapshot covers.
    #[serde(skip)]
    pub glyphs: Vec<ShapedGlyph>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DecorationLine {
    /// Signed distance below the baseline, along the logical block axis.
    pub offset: Length,
    pub thickness: Length,
}
fn is_black(color: &[u8; 4]) -> bool {
    *color == [0, 0, 0, 255]
}

/// A line within a snapshot: a block and the index of one of its lines.
/// Only meaningful for the snapshot it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct LineRef {
    pub node: NodeId,
    pub line: usize,
}

/// How a relation's target resolved (15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelationStatus {
    Valid,
    /// The target lost part of itself and was rebound to the nearest
    /// surviving position.
    Rebound,
    /// Several candidates matched and none was preferred.
    Ambiguous,
    Missing,
    /// The relation is owned and its owner was deleted, so it is no longer in
    /// effect (07, 14). It stays in the document until tombstones are compacted.
    OwnerDeleted,
    /// A target was deleted and the schema's `OnTargetDeleted` is `Delete`, so
    /// the relation is no longer in effect (14). Like `OwnerDeleted`, it stays
    /// in the document; it is decided from tombstones at resolution time, so a
    /// deletion merged from another peer gives the same status.
    Deleted,
}

/// What a target resolved to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    Node(NodeId),
    Range {
        node: NodeId,
        bytes: Range<usize>,
    },
    Line(LineRef),
    /// Several blocks: the matches of a structural query that can have
    /// several, or the candidates of an ambiguous target.
    Nodes(Vec<NodeId>),
    /// Several lines, in order.
    Lines(Vec<LineRef>),
    /// A frame, by index into [`LayoutSnapshot::frames`].
    Frame(usize),
    /// A page, by index into [`LayoutSnapshot::pages`].
    Page(usize),
    /// Content as it was at an earlier version, and whether it still exists.
    Snapshot(SnapshotContent),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TargetLayout {
    pub role: String,
    pub status: RelationStatus,
    pub resolved: Option<Resolution>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RelationLayout {
    pub id: RelationId,
    pub schema: SchemaId,
    pub owner: Option<NodeId>,
    /// The worst status of any target.
    pub status: RelationStatus,
    /// True when layout acted on the relation. A relation can be valid and
    /// still not applied, for example when its schema is unknown.
    pub applied: bool,
    pub targets: Vec<TargetLayout>,
}

/// What a diagnostic is about.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "kebab-case")]
pub enum Subject {
    Document,
    Node(NodeId),
    Relation(RelationId),
    Range(RangeId),
}

/// A problem layout worked around (37), or a note about a decision it made
/// (39). Match on `code`, never on `message`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Code,
    pub subject: Subject,
    pub message: String,
    /// Bytes of the subject's text, when the subject is a node.
    pub bytes: Option<Range<usize>>,
}

impl Diagnostic {
    pub fn new(
        severity: Severity,
        code: Code,
        subject: Subject,
        message: impl Into<String>,
    ) -> Diagnostic {
        Diagnostic {
            severity,
            code,
            subject,
            message: message.into(),
            bytes: None,
        }
    }

    /// Attaches a subject to a note from a lower library.
    pub fn from_note(note: Note, subject: Subject) -> Diagnostic {
        Diagnostic {
            severity: note.severity,
            code: note.code,
            subject,
            message: note.message,
            bytes: note.bytes,
        }
    }
}

impl LayoutSnapshot {
    pub fn block(&self, node: NodeId) -> Option<&BlockLayout> {
        self.blocks.iter().find(|b| b.node == node)
    }

    pub fn line(&self, at: LineRef) -> Option<&LineLayout> {
        self.block(at.node)?.lines.get(at.line)
    }

    pub fn frame(&self, index: usize) -> Option<&FrameLayout> {
        self.frames.get(index)
    }

    /// The indices of the frames on `page`, in the template's order.
    pub fn frames_on(&self, page: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.frames.len()).filter(move |&i| self.frames[i].page == page)
    }

    /// The first margin frame on `page`: where relations place blocks beside
    /// the text.
    pub fn margin_frame_on(&self, page: usize) -> Option<usize> {
        self.frames_on(page)
            .find(|&i| self.frames[i].role == FrameRole::Margin)
    }

    pub fn relation(&self, id: RelationId) -> Option<&RelationLayout> {
        self.relations.iter().find(|r| r.id == id)
    }

    /// The `LineContaining` query (13): the line holding byte `at` of `node`.
    /// A position at a line break belongs to the line after it, except at the
    /// end of the block, which belongs to the last line.
    pub fn line_containing(&self, node: NodeId, at: usize) -> Option<LineRef> {
        let block = self.block(node)?;
        let last = block.lines.len().checked_sub(1)?;
        let line = block
            .lines
            .iter()
            .position(|l| l.text.start <= at && at < l.text.end)
            .or_else(|| (at == block.lines[last].text.end).then_some(last))?;
        Some(LineRef { node, line })
    }

    /// The line before `at` in its block, or `None` on the first line.
    pub fn previous_line(&self, at: LineRef) -> Option<LineRef> {
        let line = at.line.checked_sub(1)?;
        self.line(LineRef { line, ..at })
            .map(|_| LineRef { line, ..at })
    }

    /// The line after `at` in its block, or `None` on the last line.
    pub fn next_line(&self, at: LineRef) -> Option<LineRef> {
        let next = LineRef {
            line: at.line + 1,
            ..at
        };
        self.line(next).map(|_| next)
    }

    /// Every line of `node` holding part of `bytes`, in order (16). An empty
    /// range finds the line containing it.
    pub fn lines_in(&self, node: NodeId, bytes: Range<usize>) -> Vec<LineRef> {
        if bytes.is_empty() {
            return self
                .line_containing(node, bytes.start)
                .into_iter()
                .collect();
        }
        let Some(block) = self.block(node) else {
            return Vec::new();
        };
        block
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.text.start < bytes.end && bytes.start < l.text.end)
            .map(|(line, _)| LineRef { node, line })
            .collect()
    }

    /// The transform from a line's own space (origin at its start edge on its
    /// baseline) to its page.
    pub fn line_to_page(&self, at: LineRef) -> Option<Transform<LineSpace, PageSpace>> {
        let line = self.line(at)?;
        let frame = self.frame(line.frame)?;
        let to_frame: Transform<LineSpace, FrameSpace> =
            Transform::new(Matrix::translate(line.rect.origin.x, line.baseline));
        Some(to_frame.then(&frame.to_page))
    }

    /// The page and page-space bounds of a line's box.
    pub fn line_bounds(&self, at: LineRef) -> Option<(usize, Rect<PageSpace>)> {
        let line = self.line(at)?;
        let frame = self.frame(line.frame)?;
        Some((frame.page, frame.to_page.bounds(&line.rect)))
    }

    /// Diagnostics with a given code, in order.
    pub fn diagnostics_with(&self, code: &str) -> impl Iterator<Item = &Diagnostic> {
        let code = code.to_owned();
        self.diagnostics
            .iter()
            .filter(move |d| d.code == code.as_str())
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("snapshots serialize")
    }
}
