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
    /// Reads identity from the store or attached package context, without
    /// materialising a root in old documents. The two must agree when present.
    pub fn document_lineage(&self) -> Result<Option<String>, DocError> {
        let stored = self.stored_document_lineage()?;
        if let (Some(stored), Some(scope)) = (&stored, &self.sync_scope)
            && stored != scope
        {
            return Err(DocError::Store(
                "document lineage differs from package scope".into(),
            ));
        }
        Ok(stored.or_else(|| self.sync_scope.clone()))
    }

    /// Attach a trusted package ID to sync, including unmarked legacy stores.
    /// Does not author metadata, commit pending writes, or change revision.
    /// A document's declared identity cannot subsequently be reassigned.
    pub fn set_sync_scope(&mut self, id: &str) -> Result<(), DocError> {
        if !valid(id) {
            return Err(DocError::Store("invalid document scope".into()));
        }
        if self.document_lineage()?.is_some_and(|have| have != id) {
            return Err(DocError::Store("document lineage is immutable".into()));
        }
        self.sync_scope = Some(id.to_owned());
        Ok(())
    }

    fn stored_document_lineage(&self) -> Result<Option<String>, DocError> {
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
        if self.sync_scope.as_deref().is_some_and(|scope| scope != id) {
            return Err(DocError::Store("document lineage is immutable".into()));
        }
        match self.stored_document_lineage()? {
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
