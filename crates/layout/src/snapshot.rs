//! The derived layout result and its queries.

use std::ops::Range;

use reprise_compose::BreakReason;
use reprise_doc::{BlockKind, ComputedStyle, NodeId, RelationId, RelationKind, Revision};
use reprise_font::FaceId;
use reprise_geom::{Length, PageSpace, Rect};
use reprise_shape::{AdapterInfo, ShapedGlyph};
use serde::{Deserialize, Serialize};

use crate::PageSettings;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineLayout {
    pub text: Range<usize>,
    /// The line's text, for reading snapshots.
    pub preview: String,
    /// The line box.
    pub rect: Rect<PageSpace>,
    pub baseline: Length,
    pub width: Length,
    pub break_reason: BreakReason,
    #[serde(skip)]
    pub(crate) glyphs: Vec<ShapedGlyph>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockLayout {
    pub node: NodeId,
    pub kind: BlockKind,
    pub face: FaceId,
    pub style: ComputedStyle,
    pub frame: Rect<PageSpace>,
    pub lines: Vec<LineLayout>,
}

/// How a relation resolved (15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelationStatus {
    Valid,
    /// The target range lost an end and was rebound to the nearest position.
    Rebound,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationLayout {
    pub id: RelationId,
    pub kind: RelationKind,
    pub source: NodeId,
    pub status: RelationStatus,
    /// The block and line index the query matched.
    pub target_line: Option<(NodeId, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub node: Option<NodeId>,
    pub message: String,
}

/// Everything layout derived from one document revision. Can be thrown away
/// and recomputed at any time (05).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutSnapshot {
    pub revision: Revision,
    pub adapter: AdapterInfo,
    pub composer: String,
    pub page: PageSettings,
    pub blocks: Vec<BlockLayout>,
    pub relations: Vec<RelationLayout>,
    pub diagnostics: Vec<Diagnostic>,
}

impl LayoutSnapshot {
    pub fn block(&self, node: NodeId) -> Option<&BlockLayout> {
        self.blocks.iter().find(|b| b.node == node)
    }

    /// The `LineContaining` query: the line holding byte `at` of `node`.
    /// A position at a line's end belongs to that line, not the next.
    pub fn line_containing(&self, node: NodeId, at: usize) -> Option<usize> {
        let block = self.block(node)?;
        block
            .lines
            .iter()
            .position(|l| at < l.text.end || (at == l.text.end && l.text.end == block_end(block)))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("snapshots serialize")
    }
}

fn block_end(block: &BlockLayout) -> usize {
    block.lines.last().map_or(0, |l| l.text.end)
}
