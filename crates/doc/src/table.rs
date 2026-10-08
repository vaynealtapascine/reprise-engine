//! Authored table topology on the movable content tree (05, 24).
//!
//! Table/row/cell containers retain the ordinary block envelope for old readers.
//! Their versioned metadata declares their role; only cell descendants are text.
use loro::LoroText;
use reprise_geom::Length;
use serde::{Deserialize, Serialize};

use crate::{BlockKind, DocError, Document, NodeId, get_str};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColumnWidth {
    Fixed(Length),
    Proportional(u32),
    /// Min/max content are measured from the cell's shaped text.
    Content,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub width: ColumnWidth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableColumns {
    pub columns: Vec<Column>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowInfo {
    pub header: bool,
}

/// A cell's declared column and optional spans. Spans are stored as authored
/// (zero and oversized values included); [`TableGrid`](crate::TableGrid)
/// decides what layout and export actually use. A span of one is omitted from
/// the stored record so spanless tables keep their original bytes, and a record
/// with spans is rejected by readers that predate them rather than misread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellInfo {
    pub column: u32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub colspan: u32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub rowspan: u32,
}

fn one() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

impl CellInfo {
    pub fn new(column: u32) -> Self {
        Self {
            column,
            colspan: 1,
            rowspan: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TableRole {
    Table(TableColumns),
    Row(RowInfo),
    Cell(CellInfo),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    role: TableRole,
}

impl Document {
    /// `Err` preserves unsupported metadata rather than treating it as text.
    pub fn table_role(&self, node: NodeId) -> Result<Option<TableRole>, DocError> {
        if !self.is_live(node) {
            return Err(DocError::NoNode(node));
        }
        if node.is_break() {
            // A paragraph of a flow is never a table container.
            return Ok(None);
        }
        let meta = self.tree("content").get_meta(node.node)?;
        if meta.get("table1").is_none() {
            return Ok(None);
        }
        let raw = get_str(&meta, "table1").ok_or(DocError::Malformed(node, "table1"))?;
        let stored: Stored =
            serde_json::from_str(&raw).map_err(|_| DocError::Malformed(node, "table1"))?;
        if stored.version != 1 {
            return Err(DocError::Malformed(node, "table1 version"));
        }
        Ok(Some(stored.role))
    }

    pub fn append_table(&self, columns: TableColumns) -> Result<NodeId, DocError> {
        self.table_container(None, TableRole::Table(columns))
    }

    pub fn append_table_row(&self, table: NodeId, header: bool) -> Result<NodeId, DocError> {
        if !matches!(self.table_role(table)?, Some(TableRole::Table(_))) {
            return Err(DocError::Malformed(table, "table parent"));
        }
        self.table_container(Some(table), TableRole::Row(RowInfo { header }))
    }

    pub fn append_table_cell(&self, row: NodeId, column: u32) -> Result<NodeId, DocError> {
        if !matches!(self.table_role(row)?, Some(TableRole::Row(_))) {
            return Err(DocError::Malformed(row, "row parent"));
        }
        self.table_container(Some(row), TableRole::Cell(CellInfo::new(column)))
    }

    /// Appends a cell that spans `colspan` columns and `rowspan` rows. Values
    /// are stored as given; layout clamps or drops what cannot be honoured.
    pub fn append_table_cell_spanned(
        &self,
        row: NodeId,
        column: u32,
        colspan: u32,
        rowspan: u32,
    ) -> Result<NodeId, DocError> {
        if !matches!(self.table_role(row)?, Some(TableRole::Row(_))) {
            return Err(DocError::Malformed(row, "row parent"));
        }
        self.table_container(
            Some(row),
            TableRole::Cell(CellInfo {
                column,
                colspan,
                rowspan,
            }),
        )
    }

    /// Marks or unmarks a row as a header row. Only the leading run of header
    /// rows repeats after a break; the repeated copies are derived, never stored.
    pub fn set_table_row_header(&self, row: NodeId, header: bool) -> Result<(), DocError> {
        if !matches!(self.table_role(row)?, Some(TableRole::Row(_))) {
            return Err(DocError::Malformed(row, "row parent"));
        }
        self.write_table_role(row, TableRole::Row(RowInfo { header }))
    }

    /// Replaces a cell's spans, keeping its column. Concurrent edits to one cell
    /// resolve last-writer-wins on the whole record; overlaps between different
    /// cells are resolved by layout, identically on every replica.
    pub fn set_table_cell_span(
        &self,
        cell: NodeId,
        colspan: u32,
        rowspan: u32,
    ) -> Result<(), DocError> {
        let Some(TableRole::Cell(info)) = self.table_role(cell)? else {
            return Err(DocError::Malformed(cell, "cell parent"));
        };
        self.write_table_role(
            cell,
            TableRole::Cell(CellInfo {
                colspan,
                rowspan,
                ..info
            }),
        )
    }

    fn write_table_role(&self, node: NodeId, role: TableRole) -> Result<(), DocError> {
        let raw = serde_json::to_string(&Stored { version: 1, role })
            .map_err(|_| DocError::Malformed(node, "table1"))?;
        self.tree("content")
            .get_meta(node.node)?
            .insert("table1", raw)?;
        Ok(())
    }

    /// Cell content is ordinary blocks, so styles, anchors and relations work unchanged.
    pub fn append_cell_block(
        &self,
        cell: NodeId,
        kind: BlockKind,
        style: &str,
        text: &str,
    ) -> Result<NodeId, DocError> {
        if !matches!(self.table_role(cell)?, Some(TableRole::Cell(_))) {
            return Err(DocError::Malformed(cell, "cell parent"));
        }
        self.table_block(Some(cell), kind, style, text)
    }

    fn table_container(&self, parent: Option<NodeId>, role: TableRole) -> Result<NodeId, DocError> {
        let node = self.table_block(parent, BlockKind::Paragraph, "", "")?;
        self.write_table_role(node, role)?;
        Ok(node)
    }

    fn table_block(
        &self,
        parent: Option<NodeId>,
        kind: BlockKind,
        style: &str,
        text: &str,
    ) -> Result<NodeId, DocError> {
        crate::check_authored(text)?;
        let tree = self.tree("content");
        let id = tree.create(parent.map(|n| n.node))?;
        let meta = tree.get_meta(id)?;
        meta.insert("kind", kind.as_str())?;
        meta.insert("style", style)?;
        meta.insert_container("text", LoroText::new())?
            .insert_utf8(0, text)?;
        Ok(NodeId::tree(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn topology_and_metadata_survive_replication() {
        let doc = Document::new(1).unwrap();
        let table = doc.append_table(TableColumns { columns: vec![] }).unwrap();
        let row = doc.append_table_row(table, false).unwrap();
        let cell = doc.append_table_cell(row, u32::MAX).unwrap();
        let text = doc
            .append_cell_block(cell, BlockKind::Paragraph, "", "λ")
            .unwrap();
        assert_eq!(doc.children(Some(table)), vec![row]);
        assert_eq!(doc.parent_of(text), Some(Some(cell)));
        let other = doc.fork(2).unwrap();
        assert_eq!(
            other.table_role(cell).unwrap(),
            doc.table_role(cell).unwrap()
        );
        assert_eq!(other.block(text).unwrap().text.to_string(), "λ");
        assert!(doc.append_table_cell(table, 0).is_err());
    }

    #[test]
    fn unknown_and_malformed_metadata_remain_authored() {
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "rest").unwrap();
        let meta = doc.tree("content").get_meta(node.node).unwrap();
        for raw in ["{", "{\"version\":2,\"role\":\"future\"}"] {
            meta.insert("table1", raw).unwrap();
            assert!(doc.table_role(node).is_err());
            assert_eq!(get_str(&meta, "table1").as_deref(), Some(raw));
        }
        meta.insert("table1", 123).unwrap();
        assert!(doc.table_role(node).is_err());
    }
}
