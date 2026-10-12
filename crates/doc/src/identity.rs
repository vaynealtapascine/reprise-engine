//! Durable document lineage. Copies author a fresh identity in fresh history.
use crate::{DocError, Document, get_str};

pub(crate) const ROOT: &str = "document1";

pub(crate) fn valid(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Document {
    /// Reads the durable identity without materialising a root in old documents.
    /// Missing identity is the legacy form; malformed identity is refused.
    pub fn document_lineage(&self) -> Result<Option<String>, DocError> {
        if self.doc.get_by_str_path(ROOT).is_none() {
            return Ok(None);
        }
        let id = get_str(&self.doc.get_map(ROOT), "id")
            .filter(|id| valid(id))
            .ok_or_else(|| DocError::Store("invalid document lineage".into()))?;
        Ok(Some(id))
    }

    /// Bind a fresh document to its package identity. A bound identity cannot
    /// be reassigned; an independent copy must start with a new document.
    pub fn bind_document_lineage(&self, id: &str) -> Result<(), DocError> {
        if !valid(id) {
            return Err(DocError::Store("invalid document lineage".into()));
        }
        match self.document_lineage()? {
            Some(have) if have != id => {
                Err(DocError::Store("document lineage is immutable".into()))
            }
            Some(_) => Ok(()),
            None => {
                self.doc.get_map(ROOT).insert("id", id)?;
                Ok(())
            }
        }
    }
}
