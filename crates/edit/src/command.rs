//! Commands: small typed descriptions of an edit (29).
//!
//! A command is data. It names blocks by [`NodeId`] and text by UTF-8 byte
//! offsets, and does nothing until a [`crate::Transaction`] holding it is
//! applied. A command can only refer to blocks that exist before its
//! transaction; the blocks a transaction creates are named by the
//! [`crate::Applied`] it returns.

use std::ops::Range;

use reprise_doc::{ListId, NewBlock, NodeId, Relation, RelationId, Style};

/// One edit. Offsets are byte offsets on character boundaries; a block's
/// children are indexed among the *live* siblings.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Command {
    /// Patch only supplied properties. Medium is an explicit validation input.
    SetPageSetup {
        patch: reprise_doc::page_setup::PageSetupPatch,
        medium: reprise_doc::Medium,
    },
    /// Swap physical dimensions, retaining the physical margins.
    SwapPageOrientation {
        medium: reprise_doc::Medium,
    },
    InsertLineBreak {
        node: NodeId,
        at: usize,
    },
    InsertTab {
        node: NodeId,
        at: usize,
    },
    /// None changes the paragraph; Some pins the default of the containing visual line.
    SetAlignment {
        node: NodeId,
        at: Option<usize>,
        alignment: reprise_doc::marks::Alignment,
    },
    SetTabStops {
        node: NodeId,
        tabs: reprise_doc::marks::TabStops,
    },
    AddAnchor {
        node: NodeId,
        at: usize,
        edge: reprise_doc::marks::LineEdge,
        target: NodeId,
        target_at: usize,
        target_edge: reprise_doc::marks::AnchorEdge,
    },

    /// A complete paste transaction. Must be the only command in its transaction.
    Paste {
        fragment: Box<reprise_doc::fragment::Fragment>,
        at: Option<(NodeId, usize)>,
        target_namespace: String,
    },
    InsertText {
        node: NodeId,
        at: usize,
        text: String,
    },
    DeleteText {
        node: NodeId,
        range: Range<usize>,
    },
    /// The text from `at` on becomes a new block right after `node`, with
    /// the same kind, style and overrides. In a flow this is a break, so no
    /// text moves (see `docs/flow.md`).
    SplitBlock {
        node: NodeId,
        at: usize,
    },
    /// `second`'s text is appended to `first` and `second` is deleted. For
    /// adjacent paragraphs of one flow only the break between them goes, so
    /// no text moves and ranges and anchors stay valid. Node targets follow
    /// the recorded succession only under `Rebind` policy.
    JoinBlocks {
        first: NodeId,
        second: NodeId,
    },
    InsertBlock {
        parent: Option<NodeId>,
        index: usize,
        block: NewBlock,
    },
    /// Flags the block as deleted; its descendants inherit deletion.
    DeleteBlock {
        node: NodeId,
    },
    /// Moves the block to `index` among `parent`'s other live children. A
    /// paragraph of a flow moves by copying: see [`Effect::Moved`].
    MoveBlock {
        node: NodeId,
        parent: Option<NodeId>,
        index: usize,
    },
    /// Replaces the block's direct style overrides.
    SetStyleOverride {
        node: NodeId,
        style: Style,
    },
    /// Adds anchored text formatting. A reset patch reveals paragraph defaults.
    /// Boundaries expand when typing at either end; empty selections are refused.
    FormatText {
        node: NodeId,
        range: Range<usize>,
        style: reprise_doc::formatting::TextStyle,
    },
    AddRelation {
        relation: Relation,
    },
    RemoveRelation {
        id: RelationId,
    },

    // Named paragraph styles (`docs/paragraph-styles.md`).
    /// Defines `name` as exactly `style` (including its parent); a built-in
    /// of that name no longer applies.
    DefineStyle {
        name: String,
        style: Style,
    },
    /// Sets the supplied properties of `name` and clears the listed ones.
    PatchStyle {
        name: String,
        patch: reprise_doc::StylePatch,
    },
    /// `from` forwards to `to`, which gets `from`'s effective properties.
    RenameStyle {
        from: String,
        to: String,
    },
    /// Blocks and styles naming it resolve through its parent.
    DeleteStyle {
        name: String,
    },
    /// Names the block's style; the empty name is the document base.
    SetStyleName {
        node: NodeId,
        name: String,
    },
    /// Sets and clears direct overrides property by property, keeping the rest.
    PatchOverrides {
        node: NodeId,
        patch: reprise_doc::StylePatch,
    },
    /// `None` clears the paragraph's direct alignment; a point removes the
    /// line defaults of the authored line containing it.
    ClearAlignment {
        node: NodeId,
        at: Option<usize>,
    },
    // Roles and lists (see `docs/roles-and-lists.md`).
    /// Sets what a paragraph is. Appearance is a separate style change.
    SetRole {
        node: NodeId,
        role: reprise_doc::Role,
    },
    /// Creates a list definition. Later commands of the transaction name it
    /// [`ListRef::New`], counting `CreateList` commands from 0.
    CreateList {
        definition: reprise_doc::ListDefinition,
    },
    /// Writes the properties of a definition that changed.
    UpdateList {
        list: ListId,
        definition: reprise_doc::ListDefinition,
    },
    /// Deletes a definition; its items become plain paragraphs.
    DeleteList {
        list: ListRef,
    },
    /// Makes a text block an item of `list` at `level` (0 to 8).
    SetListItem {
        node: NodeId,
        list: ListRef,
        level: u8,
    },
    /// Takes a block out of its list; its text and role stay.
    RemoveListItem {
        node: NodeId,
    },
    SetListLevel {
        node: NodeId,
        level: u8,
    },
    /// Tab (`by: 1`) and Shift+Tab (`by: -1`): the level moves, clamped to
    /// 0 to 8. Nothing happens to a block that is not an item.
    IndentListItem {
        node: NodeId,
        by: i8,
    },
    /// Restarts numbering at an item with `value`, or (`None`) continues it.
    RestartList {
        node: NodeId,
        value: Option<i64>,
    },
    /// Continues the previous list: clears the item's restart and, when the
    /// nearest earlier item belongs to another list, moves this item and every
    /// later item of its list into that list. Reads document order as it was when
    /// the transaction started; membership reflects earlier commands.
    ContinueList {
        node: NodeId,
    },
    /// The Enter key: on an empty list item, outdent one level, or leave the
    /// list at level 0; otherwise a split, which keeps role, list and level.
    Enter {
        node: NodeId,
        at: usize,
    },
    /// Backspace at a block's start: on a list item, remove its marker (it
    /// leaves the list); otherwise join it onto the previous sibling, if any.
    BackspaceAtStart {
        node: NodeId,
    },
    /// Adds an annotation (a link or comment anchor) over a nonempty span,
    /// with its kind's edge policy. Its ID is in [`crate::Applied::annotations`].
    AddAnnotation {
        node: NodeId,
        range: Range<usize>,
        data: reprise_doc::annotation::AnnotationData,
    },
    /// Replaces an annotation's payload; its kind cannot change.
    UpdateAnnotation {
        id: reprise_doc::RangeId,
        data: reprise_doc::annotation::AnnotationData,
    },
    /// Removes an annotation, readable or not. Undo restores it with its ID.
    RemoveAnnotation {
        id: reprise_doc::RangeId,
    },

    // Inline objects (`docs/inline.md`).
    /// Puts an inline object at byte `at` of a flow paragraph. It is staged
    /// and then shown in its step, so it keeps its identity through undo and
    /// redo.
    InsertInline {
        node: NodeId,
        at: usize,
        object: reprise_doc::InlineObject,
    },
    /// Changes the parameters of the object whose U+FFFC is at byte `at` of
    /// `node`, as the transaction leaves the text at that point. Positions
    /// are the ones the earlier commands in the transaction leave.
    UpdateInline {
        node: NodeId,
        at: usize,
        patch: reprise_doc::InlinePatch,
    },
}

/// A list a command names: one that exists, or the `n`th made by this
/// transaction's `CreateList` commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ListRef {
    Existing(ListId),
    New(usize),
}

/// What a command did to positions, so that a caret or selection held by a UI
/// can follow the edit. See [`crate::Applied::map_position`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Effect {
    /// `removed` bytes at `at` were replaced by `inserted` bytes.
    Text {
        node: NodeId,
        at: usize,
        removed: usize,
        inserted: usize,
    },
    /// The text from `at` on moved to `new`.
    Split {
        node: NodeId,
        at: usize,
        new: NodeId,
    },
    /// `second`'s text now follows the first `at` bytes of `first`.
    Join {
        first: NodeId,
        second: NodeId,
        at: usize,
    },
    /// The block and its subtree were deleted.
    Deleted(NodeId),
    /// A paragraph of a flow moved by copying: `node` was deleted, and its
    /// text is now `new`'s, at the same offsets.
    Moved { node: NodeId, new: NodeId },
}
