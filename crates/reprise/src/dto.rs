//! Facade-owned wire data. Only envelopes are standalone payloads; nested records
//! inherit their envelope's version. IDs and 64-bit counters are decimal strings.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const API_VERSION: u32 = 1;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Payload<T> {
    pub version: u32,
    pub data: T,
}
impl<T> Payload<T> {
    pub fn new(data: T) -> Self {
        Self {
            version: API_VERSION,
            data,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Bytes {
    #[serde(with = "byte_buffer")]
    #[ts(type = "Uint8Array")]
    pub bytes: Vec<u8>,
}
mod byte_buffer {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        Vec::deserialize(d)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub subject: Option<String>,
    pub start: Option<u32>,
    pub end: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ErrorPayload {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub command: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Create {
    pub document_id: String,
    pub peer_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Open {
    pub peer_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct State {
    pub document_id: String,
    pub peer_id: String,
    pub revision: Vec<Clock>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub blocks: Vec<Block>,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Clock {
    pub peer: String,
    pub counter: i32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Block {
    pub id: String,
    pub parent: Option<String>,
    pub kind: BlockKind,
    pub text: String,
    /// Resolved character formatting as disjoint byte runs covering `text`,
    /// present only when some formatting action covers this block. A run's
    /// absent fields inherit the paragraph style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub formatting: Option<Vec<TextRun>>,
}
/// Character formatting over `start..end`, UTF-8 byte offsets into the block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TextRun {
    pub start: u32,
    pub end: u32,
    pub style: TextStyle,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum BlockKind {
    Paragraph,
    Annotation,
    Image,
}
/// One transactional image insertion; `at: None` appends at document end.
/// Physical dimensions use 1/1024 pt; absent dimensions use image metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ImageInsert {
    pub at: Option<Caret>,
    pub asset: String,
    pub alt: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub style: Style,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Style {
    pub families: Option<Vec<String>>,
    pub size: Option<String>,
    pub line_height: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Command {
    /// Must be the only command, like native fragment paste in the kernel.
    InsertImage {
        image: ImageInsert,
    },
    AddRelation {
        relation: Relation,
    },
    RemoveRelation {
        id: String,
    },
    InsertText {
        node: String,
        at: u32,
        text: String,
    },
    DeleteText {
        node: String,
        start: u32,
        end: u32,
    },
    SplitBlock {
        node: String,
        at: u32,
    },
    JoinBlocks {
        first: String,
        second: String,
    },
    InsertBlock {
        parent: Option<String>,
        index: u32,
        block_kind: BlockKind,
        text: String,
        style: Style,
    },
    DeleteBlock {
        node: String,
    },
    MoveBlock {
        node: String,
        parent: Option<String>,
        index: u32,
    },
    SetStyle {
        node: String,
        style: Style,
    },
    FormatText {
        node: String,
        start: u32,
        end: u32,
        style: TextStyle,
    },
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
pub struct TextStyle {
    pub families: Option<Vec<String>>,
    /// Positive font size in 1/1024 pt.
    pub size: Option<i32>,
    pub language: Option<String>,
    pub features: Option<Vec<TextFeature>>,
    pub reset: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TextFeature {
    /// Exactly four ASCII graphic characters.
    pub tag: String,
    pub value: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Transaction {
    pub commands: Vec<Command>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Effect {
    Text {
        node: String,
        at: u32,
        removed: u32,
        inserted: u32,
    },
    Split {
        node: String,
        at: u32,
        new: String,
    },
    Join {
        first: String,
        second: String,
        at: u32,
    },
    Deleted {
        node: String,
    },
    Moved {
        node: String,
        new: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Applied {
    pub blocks: Vec<String>,
    pub relations: Vec<String>,
    pub effects: Vec<Effect>,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum FontStyle {
    Normal,
    Italic,
    Oblique,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FontDeclaration {
    pub family: String,
    pub weight: u16,
    pub style: FontStyle,
    pub stretch: u16,
    pub face_index: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Face {
    pub family: String,
    pub hash: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AssetDeclaration {
    pub id: String,
    pub kind: AssetKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AssetKind {
    Image,
    Other,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LayoutOptions {
    pub width: i32,
    pub height: i32,
    pub max_pages: u32,
    pub viewport_start: u32,
    pub viewport_end: u32,
}
impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            width: 420 * 1024,
            height: 300 * 1024,
            max_pages: 1000,
            viewport_start: 0,
            viewport_end: 1,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LayoutToken {
    pub document_id: String,
    pub revision: Vec<Clock>,
    pub generation: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Pass {
    Flow,
    RegionFeedback,
    Relations,
    ReadingOrder,
    Complete,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LayoutProgress {
    pub token: LayoutToken,
    pub used: u32,
    pub pass: Pass,
    pub viewport_ready: bool,
    pub complete: bool,
    pub settled: bool,
    pub outside_document: bool,
    pub pages: Vec<u32>,
    pub counters: Counters,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Counters {
    pub units: u32,
    pub shapes: u32,
    pub compositions: u32,
    pub reused_compositions: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Affinity {
    Upstream,
    Downstream,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Caret {
    pub node: String,
    pub offset: u32,
    pub affinity: Affinity,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub caret: Caret,
    pub goal_x: Option<i32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Movement {
    NextGrapheme,
    PreviousGrapheme,
    NextWord,
    PreviousWord,
    VisualRight,
    VisualLeft,
    InlineForward,
    InlineBackward,
    LineUp,
    LineDown,
    LineStart,
    LineEnd,
    LineLeftmost,
    LineRightmost,
    LineInlineStart,
    LineInlineEnd,
    BlockStart,
    BlockEnd,
    DocumentStart,
    DocumentEnd,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Move {
    pub cursor: Cursor,
    pub movement: Movement,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub anchor: Caret,
    pub focus: Caret,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PageRect {
    pub page: u32,
    pub rect: Rect,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HitTest {
    pub page: u32,
    pub x: i32,
    pub y: i32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Hit {
    pub caret: Caret,
    pub page: u32,
    pub inside: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ReadingStep {
    pub node: String,
    pub line: u32,
    pub page: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ExportFormat {
    PlainText,
    Html,
    Native,
    Pdf,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Loss {
    pub code: String,
    pub disposition: Disposition,
    pub detail: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Disposition {
    Preserved,
    Approximated,
    Dropped,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Exported {
    pub content: Bytes,
    pub losses: Vec<Loss>,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Paste {
    pub at: Option<Caret>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TextImport {
    pub text: String,
    pub html: bool,
    pub at: Option<Caret>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SyncInfo {
    pub document_id: String,
    pub peer_id: String,
    pub vector: Vec<Clock>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SyncUpdate {
    pub document_id: String,
    pub from_peer: String,
    pub vector: Vec<Clock>,
    pub content: Bytes,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Awareness {
    pub document_id: String,
    pub peer_id: String,
    pub content: Bytes,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    ReadText,
    InsertText,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum PluginPhase {
    Layout,
    Editing,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Dimension {
    Length,
    Number,
    Percentage,
    Ratio,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PluginFunction {
    pub name: String,
    pub operation: u32,
    pub params: Vec<Dimension>,
    pub variadic: Option<Dimension>,
    pub returns: Dimension,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PluginSpec {
    pub name: String,
    pub plugin_version: String,
    pub sha256: String,
    pub phase: PluginPhase,
    pub imports: Vec<Capability>,
    pub grants: Vec<Capability>,
    pub functions: Vec<PluginFunction>,
    pub fuel: u32,
    pub memory_pages: u32,
    pub table_elements: u32,
    pub buffer_bytes: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PluginInstall {
    Relation {
        schema: RelationSchema,
        operation: u32,
    },
    Functions,
    Geometry {
        operation: u32,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PluginEdit {
    pub operation: u32,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}
/// Display geometry mirrors the frozen renderer JSON (navigation Rect is flat).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DisplayRect {
    pub origin: Point,
    pub width: i32,
    pub height: i32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Matrix {
    pub xx: i32,
    pub yx: i32,
    pub xy: i32,
    pub yy: i32,
    pub tx: i32,
    pub ty: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    Content,
    Debug,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ByteRange {
    pub start: u32,
    pub end: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Glyph {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub text: ByteRange,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct GlyphRun {
    pub face: Face,
    pub size: i32,
    pub color: [u8; 4],
    pub text: String,
    pub glyphs: Vec<Glyph>,
    pub layer: Layer,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Segment {
    Move(Point),
    Line(Point),
    Quad(Point, Point),
    Cubic(Point, Point, Point),
    Close,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Stroke {
    pub color: [u8; 4],
    pub width: i32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DisplayItem {
    Glyphs(GlyphRun),
    Image {
        asset: String,
        rect: DisplayRect,
        alt: String,
        layer: Layer,
    },
    Path {
        path: Vec<Segment>,
        fill: Option<[u8; 4]>,
        stroke: Option<Stroke>,
        layer: Layer,
    },
    Group {
        transform: Matrix,
        clip: Option<Vec<Segment>>,
        items: Vec<DisplayItem>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DisplayList {
    pub width: i32,
    pub height: i32,
    pub items: Vec<DisplayItem>,
}
/// Glyph outlines a renderer needs for one face: at most 4,096 IDs per request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GlyphRequest {
    pub face: Face,
    pub glyphs: Vec<u32>,
}
/// Outlines in font design units, y up, as SVG path data (`M`, `L`, `Q`, `C`,
/// `Z`, absolute). Scale by `size / units_per_em` and flip y to draw. An ID the
/// face lacks has an empty path. Rendering only: never feeds layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct GlyphOutlines {
    pub units_per_em: u32,
    pub glyphs: Vec<GlyphOutline>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct GlyphOutline {
    pub id: u32,
    pub path: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DisplayPage {
    pub token: LayoutToken,
    pub page: u32,
    pub settled: bool,
    pub complete: bool,
    pub display: DisplayList,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    pub schema: String,
    pub owner: Option<String>,
    pub targets: std::collections::BTreeMap<String, Vec<Target>>,
    pub params: std::collections::BTreeMap<String, Param>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Node(String),
    Range(String),
    Structural(StructuralQuery),
    Layout(LayoutQuery),
    Snapshot(SnapshotReference),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "query", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StructuralQuery {
    NextSibling {
        from: String,
        kind: Option<BlockKind>,
    },
    PreviousSibling {
        from: String,
        kind: Option<BlockKind>,
    },
    NthChild {
        of: Option<String>,
        index: u32,
        from_end: bool,
        kind: Option<BlockKind>,
    },
    FirstChild {
        of: Option<String>,
        kind: Option<BlockKind>,
    },
    LastChild {
        of: Option<String>,
        kind: Option<BlockKind>,
    },
    Parent {
        of: String,
    },
    Children {
        of: Option<String>,
        kind: Option<BlockKind>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "query", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LayoutQuery {
    LineContaining { range: String },
    PreviousLine { range: String },
    NextLine { range: String },
    FirstLine { node: String },
    LastLine { node: String },
    LinesIn { range: String },
    FrameContaining { range: String },
    PageContaining { range: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SnapshotReference {
    pub revision: Vec<Clock>,
    pub of: SnapshotSubject,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotSubject {
    Node(String),
    Range(String),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Param {
    Length(RelativeLength),
    Int(String),
    Bool(bool),
    Text(String),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum RelativeLength {
    Pt(i32),
    Em(i32),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum TargetClass {
    Node,
    Range,
    Structural,
    Layout,
    Snapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ParamKind {
    Length,
    Int,
    Bool,
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Ownership {
    Owned,
    Independent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum OnTargetDeleted {
    Rebind,
    KeepMissing,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CopyInside {
    Duplicate,
    Drop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CopyCrossing {
    KeepOutside,
    Drop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CopyPolicy {
    pub inside: CopyInside,
    pub crossing: CopyCrossing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoleSpec {
    pub name: String,
    pub accepts: Vec<TargetClass>,
    pub min: u32,
    pub max: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    pub name: String,
    pub kind: ParamKind,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RelationSchema {
    pub id: String,
    pub version: u32,
    pub ownership: Ownership,
    pub roles: Vec<RoleSpec>,
    pub params: Vec<ParamSpec>,
    pub on_target_deleted: OnTargetDeleted,
    pub on_copy: CopyPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CopyAs {
    pub selection: Selection,
    pub format: CopyFormat,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Resource {
    pub id: String,
    pub kind: ResourceKind,
    pub hash: String,
    pub available: bool,
    pub font: Option<Face>,
    pub location: Option<ResourceLocation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceKind {
    Font,
    Image,
    Other,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum ResourceLocation {
    Path(String),
    Url(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CopyFormat {
    PlainText,
    Html,
}
/// Asks for a sync packet (format 2). `since: None` is a full snapshot for a
/// first join; a vector is a delta of everything that vector lacks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SyncRequest {
    pub since: Option<Vec<Clock>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SyncKind {
    Delta,
    Snapshot,
}
/// A sync packet. `format`, `features`, `kind`, `since` and `vector` mirror
/// the frame in `content`, so a transport can route without decoding; import
/// checks that they agree with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SyncPacket {
    pub document_id: String,
    pub from_peer: String,
    pub format: u32,
    pub features: Vec<String>,
    pub kind: SyncKind,
    pub since: Vec<Clock>,
    pub vector: Vec<Clock>,
    pub content: Bytes,
}
/// What an import, undo or redo changed. `blocks` may name more than
/// changed, never less; `styles` means every block may have restyled.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Changes {
    pub blocks: Vec<String>,
    pub structure: bool,
    pub styles: bool,
    pub relations: Vec<String>,
    pub ranges: Vec<String>,
    pub other: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SyncReport {
    pub info: SyncInfo,
    pub changed: bool,
    pub changes: Changes,
    /// The anchored local selection after the import, if one is set.
    pub selection: Option<Selection>,
    /// Whether an end of it had to take a fallback.
    pub selection_moved: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct EditReport {
    pub changed: bool,
    pub changes: Changes,
    pub selection: Option<Selection>,
    pub selection_moved: bool,
}
/// A caret anchored to a character. `anchor` is opaque lowercase hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct StableCaret {
    pub node: String,
    pub affinity: Affinity,
    pub anchor: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct StableSelection {
    pub anchor: StableCaret,
    pub focus: StableCaret,
}
/// This peer's presence: its selection and bounded opaque host metadata
/// (at most 16 entries, keys of 64 bytes, values of 1 KiB).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Presence {
    pub selection: Option<Selection>,
    pub meta: std::collections::BTreeMap<String, String>,
}
/// A peer's presence resolved on the current layout. Stale or garbage
/// presence gives empty fields, never an error.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PresenceView {
    pub peer_id: String,
    pub meta: std::collections::BTreeMap<String, String>,
    pub selection: Option<Selection>,
    pub caret: Option<PageRect>,
    pub rects: Vec<PageRect>,
}
