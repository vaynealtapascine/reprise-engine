//! Commands: small typed descriptions of an edit (29).
//!
//! A command is data. It names blocks by [`NodeId`] and text by UTF-8 byte
//! offsets, and does nothing until a [`crate::Transaction`] holding it is
//! applied. A command can only refer to blocks that exist before its
//! transaction; the blocks a transaction creates are named by the
//! [`crate::Applied`] it returns.

use std::ops::Range;

use reprise_doc::{NewBlock, NodeId, Relation, RelationId, Style};

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
