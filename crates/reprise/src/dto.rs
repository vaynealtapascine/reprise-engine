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
/// Imports a package as an independent document: a new identity and a fresh
/// history. `document_id` must differ from the package's own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ImportCopy {
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<SectionState>>", optional)]
    pub sections: Vec<SectionState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<StoryState>>", optional)]
    pub stories: Vec<StoryState>,
    pub document_id: String,
    pub peer_id: String,
    pub revision: Vec<Clock>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub page_setup: PageSetup,
    pub blocks: Vec<Block>,
    pub diagnostics: Vec<Diagnostic>,
    /// The live named styles, built-ins included, sorted by name (at most
    /// 1,024; a `bindings.limit` diagnostic says when more were left out).
    pub styles: Vec<NamedStyle>,
    /// The distinct resolved paragraph styles that `blocks[].style.resolved`
    /// indexes. Blocks sharing a style share an entry.
    pub resolved: Vec<ResolvedStyle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<NamedList>>", optional)]
    pub lists: Vec<NamedList>,
    /// Links and comment anchors, compact: where each is and what it is for,
    /// without a link's address (ask `annotation` for that). At most
    /// [`MAX_STATE_ANNOTATIONS`], in ID order; a `bindings.limit` diagnostic
    /// says when more were left out. The queries are not cut short that way.
    pub annotations: Vec<AnnotationBrief>,
}
/// Integer physical page setup; absent patch fields remain unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
pub struct PageSetupPatch {
    #[ts(optional = nullable)]
    pub width: Option<i32>,
    #[ts(optional = nullable)]
    pub height: Option<i32>,
    #[ts(optional = nullable)]
    pub top: Option<i32>,
    #[ts(optional = nullable)]
    pub right: Option<i32>,
    #[ts(optional = nullable)]
    pub bottom: Option<i32>,
    #[ts(optional = nullable)]
    pub left: Option<i32>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PageSetup {
    pub width: i32,
    pub height: i32,
    pub margins: PageMargins,
    pub orientation: PageOrientation,
    pub template: String,
    pub source: PageTemplateSource,
    pub patched: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PageMargins {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum PageOrientation {
    Portrait,
    Landscape,
    Square,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum PageTemplateSource {
    Builtin,
    Document,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Clock {
    pub peer: String,
    pub counter: i32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Block {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub starts: Option<SectionStart>,
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
    /// The block's paragraph style: the one additive per-block field.
    pub style: BlockStyle,
    #[serde(default, skip_serializing_if = "BlockRole::is_body")]
    #[ts(as = "Option<BlockRole>", optional)]
    pub role: BlockRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub list: Option<ListItem>,
    /// The live inline objects of this paragraph in text order, present only
    /// when there are some (`docs/inline.md`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<InlineEntry>>", optional)]
    pub inline: Vec<InlineEntry>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub alignment: Option<Alignment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub tabs: Option<TabStops>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub weight: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub slant: Option<FontStyle>,
    #[serde(default, skip_serializing_if = "Decoration::is_empty")]
    #[ts(as = "Option<Decoration>", optional)]
    pub decoration: Decoration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub color: Option<[u8; 4]>,

    pub families: Option<Vec<String>>,
    pub size: Option<String>,
    pub line_height: Option<String>,

    // Paragraph properties (`docs/paragraph-styles.md`). Lengths take the
    // style expression syntax (`12pt`, `1.5em`, `1lh`, `5%`, `calc`-like).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub indent_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub indent_end: Option<String>,
    /// Negative values hang, bounded by `indent_start`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub indent_first: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub space_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub space_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub keep_with_next: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub keep_together: Option<bool>,
    /// 1..=32 lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub widows: Option<u8>,
    /// 1..=32 lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub orphans: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub background: Option<[u8; 4]>,
}
/// One property, for `clear` lists: the single representation of "inherit"
/// across `set-style`, `update-style` and `format-text` (`set-alignment`'s
/// `alignment: null` is the same as clearing `alignment`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum StyleProperty {
    Parent,
    Families,
    Size,
    LineHeight,
    Alignment,
    Tabs,
    Weight,
    Slant,
    Underline,
    Strike,
    Color,
    IndentStart,
    IndentEnd,
    IndentFirst,
    SpaceBefore,
    SpaceAfter,
    KeepWithNext,
    KeepTogether,
    Widows,
    Orphans,
    Background,
    /// Character formatting only.
    Language,
    /// Character formatting only.
    Features,
}
/// A named style as `State` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct NamedStyle {
    pub name: String,
    pub parent: Option<String>,
    /// Engine data the document has not redefined.
    pub builtin: bool,
    /// The authored properties of this style alone (not inherited ones).
    pub style: Style,
}
/// A block's paragraph style in `State`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct BlockStyle {
    /// The named style, as stored (the empty name is the document base). A
    /// renamed style is reported under its new name.
    pub name: String,
    /// The block's direct overrides.
    pub direct: Style,
    /// Index into `State.resolved`.
    pub resolved: u32,
}
/// A resolved (used) paragraph style. Lengths are 1/1024 pt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ResolvedStyle {
    pub family: String,
    /// Empty for a legacy single-family chain.
    pub families: Vec<String>,
    pub size: i32,
    pub line_height: i32,
    pub alignment: Option<Alignment>,
    pub tabs: Option<TabStops>,
    pub indent_start: i32,
    pub indent_end: i32,
    pub indent_first: i32,
    pub space_before: i32,
    pub space_after: i32,
    pub keep_with_next: bool,
    pub keep_together: bool,
    pub widows: u8,
    pub orphans: u8,
    pub weight: Option<u16>,
    pub slant: Option<FontStyle>,
    pub decoration: Decoration,
    pub color: Option<[u8; 4]>,
    pub background: Option<[u8; 4]>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Command {
    SetPageSetup {
        setup: PageSetupPatch,
    },
    SwapPageOrientation,
    InsertLineBreak {
        node: String,
        at: u32,
    },
    InsertTab {
        node: String,
        at: u32,
    },
    /// `alignment: null` removes it: the paragraph's direct alignment for
    /// `at: null`, the line default for a point.
    SetAlignment {
        node: String,
        at: Option<u32>,
        alignment: Option<Alignment>,
    },
    SetTabStops {
        node: String,
        tabs: TabStops,
    },
    AddAnchor {
        node: String,
        at: u32,
        edge: LineEdge,
        target: String,
        target_at: u32,
        target_edge: AnchorEdge,
    },
    RemoveAnchor {
        id: String,
    },
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
    /// Without `clear`, replaces the direct overrides. With `clear` (even
    /// empty), patches: supplied fields are set, listed ones cleared (they
    /// inherit), the rest kept.
    SetStyle {
        node: String,
        style: Style,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional = nullable)]
        clear: Option<Vec<StyleProperty>>,
    },
    FormatText {
        node: String,
        start: u32,
        end: u32,
        style: TextStyle,
    },
    /// Defines `name` as exactly `style` with `parent`.
    DefineStyle {
        name: String,
        parent: Option<String>,
        style: Style,
    },
    /// Sets the supplied properties of a live style (built-ins included)
    /// and clears the listed ones. `parent` absent keeps it.
    UpdateStyle {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional = nullable)]
        parent: Option<String>,
        style: Style,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<StyleProperty>>", optional)]
        clear: Vec<StyleProperty>,
    },
    RenameStyle {
        from: String,
        to: String,
    },
    DeleteStyle {
        name: String,
    },
    /// `style: null` returns the block to the document base.
    ApplyStyle {
        node: String,
        style: Option<String>,
    },
    SetRole {
        node: String,
        role: BlockRole,
    },
    CreateList {
        definition: ListDefinition,
    },
    UpdateList {
        list: String,
        definition: ListDefinition,
    },
    DeleteList {
        list: ListRef,
    },
    SetListItem {
        node: String,
        list: ListRef,
        level: u8,
    },
    RemoveListItem {
        node: String,
    },
    SetListLevel {
        node: String,
        level: u8,
    },
    IndentListItem {
        node: String,
        by: i8,
    },
    /// Canonical signed i64 decimal string; null clears the restart.
    RestartList {
        node: String,
        value: Option<String>,
    },
    ContinueList {
        node: String,
    },
    Enter {
        node: String,
        at: u32,
    },
    BackspaceAtStart {
        node: String,
    },
    /// Adds a link or a comment anchor over `start..end` of a block (UTF-8
    /// bytes, not empty). Its ID is in `Applied.annotations`. A link does not
    /// style its text: Cmd+K is this command plus a `format-text` in one
    /// transaction.
    AddAnnotation {
        node: String,
        start: u32,
        end: u32,
        annotation: AnnotationPayload,
    },
    /// Replaces an annotation's payload; its kind cannot change.
    UpdateAnnotation {
        id: String,
        annotation: AnnotationPayload,
    },
    /// Removes an annotation. Undo brings it back with the same ID.
    RemoveAnnotation {
        id: String,
    },
    /// Inserts an inline object at byte `at` of a flow paragraph. The object
    /// keeps its identity through undo and redo (`docs/inline.md`).
    InsertInline {
        node: String,
        at: u32,
        object: InlineObject,
    },
    /// Changes the live inline object whose U+FFFC is at byte `at` of `node`.
    /// `null` values in `patch.params` remove parameters.
    UpdateInline {
        node: String,
        at: u32,
        patch: InlinePatch,
    },
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
pub struct TextStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub weight: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub slant: Option<FontStyle>,
    #[serde(default, skip_serializing_if = "Decoration::is_empty")]
    #[ts(as = "Option<Decoration>", optional)]
    pub decoration: Decoration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub color: Option<[u8; 4]>,

    pub families: Option<Vec<String>>,
    /// Positive font size in 1/1024 pt.
    pub size: Option<i32>,
    pub language: Option<String>,
    pub features: Option<Vec<TextFeature>>,
    pub reset: bool,
    /// Character fields this action clears (they reveal the paragraph
    /// style) without a full `reset`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<StyleProperty>>", optional)]
    pub clear: Vec<StyleProperty>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
pub struct Decoration {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub underline: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub strike: Option<bool>,
}
impl Decoration {
    fn is_empty(&self) -> bool {
        self.underline.is_none() && self.strike.is_none()
    }
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<String>>", optional)]
    pub lists: Vec<String>,
    pub effects: Vec<Effect>,
    pub diagnostics: Vec<Diagnostic>,
    /// The annotations `add-annotation` created, in command order.
    pub annotations: Vec<String>,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum GenericFamily {
    Serif,
    SansSerif,
    Monospace,
    Script,
}
/// Host configuration; omitted mappings keep engine defaults. No document state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FontSettings {
    #[ts(type = "Partial<Record<GenericFamily, string>>")]
    pub generics: std::collections::BTreeMap<GenericFamily, String>,
    pub fallbacks: Vec<String>,
    pub synthetic_oblique: bool,
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
    /// The line's UTF-8 byte range in its block's text (`State.blocks[].text`),
    /// trailing whitespace included, so a reader can slice without `move`.
    pub start: u32,
    pub end: u32,
    #[serde(default, skip_serializing_if = "BlockRole::is_body")]
    #[ts(as = "Option<BlockRole>", optional)]
    pub role: BlockRole,
    /// A derived label on the first line; byte ranges still refer only to authored text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub marker: Option<String>,
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
/// Replaces an anchored selection with a native clipboard fragment (the bytes
/// travel beside the request): one transaction, one undo step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ReplaceSelection {
    pub selection: StableSelection,
}
/// Replaces an anchored selection with plain text or HTML, like
/// [`ReplaceSelection`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ReplaceText {
    pub selection: StableSelection,
    pub text: String,
    pub html: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Replaced {
    /// The deletion's effects, then the paste's: one pass maps a position.
    pub applied: Applied,
    /// A collapsed selection at the end of the inserted content (or at the
    /// start of the replaced range when nothing was inserted).
    pub selection: Option<Selection>,
}
/// Several commands as one transaction and one undo step, all or nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub commands: Vec<Command>,
    /// Refuse with `bindings.stale`, writing nothing, unless the document is
    /// at exactly this revision (`State.revision`).
    pub expected_revision: Option<Vec<Clock>>,
    /// Mapped through the batch's effects and returned in `BatchReport`.
    pub selection: Option<Selection>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct BatchReport {
    pub applied: Applied,
    /// `Batch.selection` as the batch left it; `None` when it was not given or
    /// its block was deleted.
    pub selection: Option<Selection>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub synthetic_oblique: Option<bool>,
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
/// The renderer passes the display run's synthetic flag when fetching outlines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct StyledGlyphRequest {
    pub face: Face,
    pub glyphs: Vec<u32>,
    pub synthetic_oblique: bool,
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
    // Conservative integer design-unit envelope, y up. Styled requests only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ink_bounds: Option<GlyphBounds>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct GlyphBounds {
    pub x_min: i32,
    pub y_min: i32,
    pub x_max: i32,
    pub y_max: i32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DisplayPage {
    pub token: LayoutToken,
    pub page: u32,
    pub settled: bool,
    pub complete: bool,
    pub display: DisplayList,
    /// The links drawn on this page, for hit-testing and hover cards. The
    /// display list is unchanged by links.
    pub links: Vec<PageLink>,
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

/// All geometry is integer page units (1/1024 pt); offsets are UTF-8 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Alignment {
    Start,
    Centre,
    End,
    /// Paragraphs only; not a tab stop alignment.
    Justify,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum LineEdge {
    Start,
    End,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AnchorEdge {
    Position,
    GapStart,
    GapEnd,
    LineStart,
    LineEnd,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TabStop {
    pub position: Option<i32>,
    pub alignment: Alignment,
    pub leader: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TabStops {
    pub interval: i32,
    pub stops: Vec<TabStop>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum MarkKind {
    ParagraphEnd,
    LineBreak,
    Gap,
    Alignment,
    Anchor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum RelationState {
    Valid,
    Rebound,
    Ambiguous,
    Missing,
    OwnerDeleted,
    Deleted,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Mark {
    pub kind: MarkKind,
    pub node: String,
    pub offset: u32,
    pub line: u32,
    pub from: Point,
    pub to: Option<Point>,
    pub target_page: Option<u32>,
    pub alignment: Option<Alignment>,
    pub relation: Option<String>,
    pub state: Option<RelationState>,
    pub applied: bool,
    /// Anchors: the authored source edge.
    pub edge: Option<LineEdge>,
    /// Anchors: the target's block and UTF-8 byte offset, when it resolved.
    pub target_node: Option<String>,
    pub target_offset: Option<u32>,
    /// Anchors: the authored target edge.
    pub target_edge: Option<AnchorEdge>,
    /// Anchors: the stable code of the layout diagnostic that explains an
    /// unapplied or suspect pin (`relation.alignment-cycle`, `-limit`,
    /// `-outside`, `-conflict` or `-invalid`).
    pub diagnostic: Option<String>,
}
/// Which diagnostics `diagnostics_for` returns. Every field narrows the result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticFilter {
    /// Keep codes starting with this text, such as `relation.` or
    /// `relation.alignment-cycle`.
    pub code_prefix: Option<String>,
    /// Keep this severity and worse.
    pub min_severity: Option<Severity>,
    /// Keep one subject: `relation:<id>`, `node:<id>`, `range:<id>`,
    /// `document`, or a bare id.
    pub subject: Option<String>,
    /// At most this many (default and ceiling 1000).
    pub limit: Option<u32>,
}
/// The most diagnostics one `diagnostics_for` call returns.
pub const MAX_DIAGNOSTICS: u32 = 1000;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DiagnosticList {
    pub diagnostics: Vec<Diagnostic>,
    /// More matched than `limit` allowed.
    pub truncated: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MarksPage {
    pub token: LayoutToken,
    pub page: u32,
    pub marks: Vec<Mark>,
}

/// Semantic tokens. An absent State role means body; appearance is independent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum BlockRole {
    #[default]
    Body,
    #[serde(rename = "heading-1")]
    Heading1,
    #[serde(rename = "heading-2")]
    Heading2,
    #[serde(rename = "heading-3")]
    Heading3,
    #[serde(rename = "heading-4")]
    Heading4,
    #[serde(rename = "heading-5")]
    Heading5,
    #[serde(rename = "heading-6")]
    Heading6,
    Title,
    Subtitle,
    Quote,
    Code,
    Verse,
}
impl BlockRole {
    fn is_body(&self) -> bool {
        *self == Self::Body
    }
}
/// Existing list identity, or zero-based CreateList index in this transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ListRef {
    Existing { id: String },
    New { index: u32 },
}
/// Unset properties inherit from base and then engine defaults. Updating replaces
/// the authored definition; unchanged properties do not emit CRDT writes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
pub struct ListDefinition {
    pub base: Option<String>,
    pub levels: Vec<ListLevel>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
pub struct ListLevel {
    pub level: u8,
    /// bullet, decimal, decimal-leading-zero, lower/upper-alpha, lower/upper-roman, none.
    pub format: Option<String>,
    pub text: Option<String>,
    /// Canonical signed i64 decimal string.
    pub start: Option<String>,
    /// Integer 1/1024 pt, logical inline start.
    pub indent: Option<i32>,
    pub hanging: Option<i32>,
    pub marker: Option<TextStyle>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct NamedList {
    pub id: String,
    pub definition: Option<ListDefinition>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ListItem {
    pub id: String,
    pub level: u8,
    pub value: String,
    pub restart: bool,
    pub marker: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct OutlineEntry {
    pub node: String,
    pub level: u8,
    pub text: String,
    pub page: Option<u32>,
}

/// The most annotations `State.annotations` lists.
pub const MAX_STATE_ANNOTATIONS: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AnnotationKind {
    Link,
    Comment,
}

/// Where a link goes: a URL (`http`, `https`, `mailto` or `tel`; anything
/// else is refused with `annotation.link-scheme`), or inside the document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LinkDestination {
    Url { url: String },
    Node { node: String },
    Heading { node: String },
    Range { range: String },
}

/// What an annotation is. A comment names its thread only; the thread's
/// messages are not stored here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AnnotationPayload {
    Link { target: LinkDestination },
    Comment { thread: String },
}

/// Whether an annotation is over text, or all of its text was deleted and
/// its kind keeps a point (a comment).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AnnotationStatus {
    Active,
    Orphaned,
}

/// One paragraph's part of an annotation, in UTF-8 bytes. An orphan has one
/// empty span (`start == end`) where its point is, or none if its block is gone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AnnotationSpan {
    pub node: String,
    pub start: u32,
    pub end: u32,
}

/// An annotation, compact: what `State.annotations` lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AnnotationBrief {
    pub id: String,
    pub kind: AnnotationKind,
    pub status: AnnotationStatus,
    pub spans: Vec<AnnotationSpan>,
    /// A comment's thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub thread: Option<String>,
}

/// An annotation with its payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Annotation {
    pub id: String,
    /// The peer that created it.
    pub author: String,
    pub status: AnnotationStatus,
    pub spans: Vec<AnnotationSpan>,
    pub payload: AnnotationPayload,
}

/// A bounded query result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AnnotationList {
    pub annotations: Vec<Annotation>,
    /// More matched than one query returns (4,096).
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AnnotationsAt {
    pub node: String,
    pub at: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AnnotationsIn {
    pub node: String,
    pub start: u32,
    pub end: u32,
}

/// One stretch of one line fragment of a span, three ways: in its frame's
/// logical space, as page bounds, and as the page positions of its corners
/// (exact in rotated, mirrored, vertical and spiral frames). All lengths are
/// 1/1024 pt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SpanRect {
    pub page: u32,
    pub frame: u32,
    pub node: String,
    pub line: u32,
    pub frame_rect: Rect,
    pub bounds: Rect,
    /// Inline start and block start, inline end and block start, inline end
    /// and block end, inline start and block end.
    pub corners: [Point; 4],
    /// On a derived copy of a repeated table header row.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub repeated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SpanTop {
    pub page: u32,
    pub y: i32,
}

/// Where an annotation is on the pages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AnnotationGeometry {
    pub id: String,
    pub rects: Vec<SpanRect>,
    /// The first page it is on and the smallest y there, to align a sidebar.
    pub top: Option<SpanTop>,
    pub truncated: bool,
    /// Pieces with no line on any page.
    pub unplaced: u32,
}

/// A link on one page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PageLink {
    pub id: String,
    pub target: LinkDestination,
    pub rects: Vec<SpanRect>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PageLinks {
    pub token: LayoutToken,
    pub page: u32,
    pub links: Vec<PageLink>,
    pub truncated: bool,
}

/// Current-lifetime values retain unknown keys. Raw i64s use decimal strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SectionState {
    pub id: String,
    pub life: String,
    pub first_block: Option<String>,
    pub end: Option<String>,
    #[ts(type = "Record<string, unknown>")]
    pub own: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown", optional)]
    pub resolved: Option<serde_json::Value>,
    pub bindings: Vec<SectionBinding>,
    pub unknown_keys: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SectionBinding {
    pub slot: String,
    pub variant: String,
    pub authored: String,
    pub story: Option<String>,
    pub resolved: Option<String>,
    pub from_section: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct StoryState {
    pub id: String,
    pub bound_by: Vec<StoryBinding>,
    pub blocks: Vec<Block>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct StoryBinding {
    pub section: String,
    pub slot: String,
    pub variant: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SectionStart {
    pub kind: String,
    pub life: String,
    #[ts(type = "Record<string, unknown>")]
    pub own: serde_json::Value,
}
/// Where an inline object sits relative to the line's baseline (default
/// `baseline`; `docs/inline.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum InlineAlign {
    #[default]
    Baseline,
    Middle,
    TextTop,
    TextBottom,
}
/// An inline object as stored: a kind (`image`, `field`, or any other name a
/// later engine knows), an alignment and string parameters. Unknown kinds
/// round-trip unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
pub struct InlineObject {
    pub kind: String,
    pub align: InlineAlign,
    pub params: std::collections::BTreeMap<String, String>,
}
/// A change to an inline object's parameters. A parameter mapped to `null`
/// is removed; `align` absent keeps the current alignment.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
pub struct InlinePatch {
    #[ts(optional = nullable)]
    pub align: Option<InlineAlign>,
    pub params: std::collections::BTreeMap<String, Option<String>>,
}
/// One live inline object of a paragraph, as `Block.inline` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct InlineEntry {
    /// `host/char`: stable through undo, redo and collaboration, new for a copy.
    pub id: String,
    /// Byte offset of the object's U+FFFC in the paragraph's text.
    pub at: u32,
    pub object: InlineObject,
}
/// A persistent position for a `page-of` field's `target` parameter: the
/// paragraph `node` at byte `at`. See `DocumentSession::field_target`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FieldTarget {
    pub node: String,
    pub at: u32,
}
