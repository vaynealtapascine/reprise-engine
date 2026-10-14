//! Durable document lineage. Copies author a fresh identity in fresh history.
use crate::{DocError, Document, get_str};

pub(crate) const ROOT: &str = "document1";
const PREFIX: &str = "document1:";

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
        let mut stored = match self.doc.get_by_str_path(ROOT) {
            None => None,
            Some(loro::ValueOrContainer::Container(loro::Container::Map(map))) => Some(
                get_str(&map, "id")
                    .filter(|id| valid(id))
                    .ok_or_else(|| DocError::Store("invalid document lineage".into()))?,
            ),
            Some(_) => return Err(DocError::Store("invalid document lineage".into())),
        };
        // Shallow root values are container IDs, not recursive document data.
        // Root declarations persist without consuming an author operation.
        let loro::LoroValue::Map(roots) = self.doc.get_value() else {
            return Err(DocError::Store("invalid document roots".into()));
        };
        for (name, value) in roots.iter() {
            let Some(id) = name.strip_prefix(PREFIX) else {
                continue;
            };
            if !valid(id)
                || !matches!(
                    value,
                    loro::LoroValue::Container(loro::ContainerID::Root {
                        container_type: loro::ContainerType::Map,
                        ..
                    })
                )
                || stored.as_deref().is_some_and(|have| have != id)
            {
                return Err(DocError::Store("invalid document lineage".into()));
            }
            stored = Some(id.to_owned());
        }
        Ok(stored)
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
                self.doc.get_map(format!("{PREFIX}{id}"));
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockKind, PersistenceMode};

    #[test]
    fn binding_retains_identity_without_consuming_content_counters() {
        let id = "00112233445566778899aabbccddeeff";
        for mode in [PersistenceMode::History, PersistenceMode::Shallow] {
            let doc = Document::new(1).unwrap();
            let before = doc.revision();
            doc.bind_document_lineage(id).unwrap();
            assert_eq!(doc.revision(), before);
            let node = doc.append_block(BlockKind::Paragraph, "", "hello").unwrap();
            assert_eq!(node.to_string(), "0@1");
            doc.commit();
            let reopened = Document::import(&doc.try_export(mode).unwrap(), 2).unwrap();
            assert_eq!(reopened.document_lineage().unwrap().as_deref(), Some(id));
            assert!(
                reopened
                    .bind_document_lineage("ffeeddccbbaa99887766554433221100")
                    .is_err()
            );
        }
    }

    #[test]
    fn conflicting_or_malformed_root_names_are_refused() {
        for other in ["bad", "ffeeddccbbaa99887766554433221100"] {
            let doc = Document::new(1).unwrap();
            doc.bind_document_lineage("00112233445566778899aabbccddeeff")
                .unwrap();
            doc.doc.get_map(format!("{PREFIX}{other}"));
            assert!(doc.document_lineage().is_err());
        }
    }
}
