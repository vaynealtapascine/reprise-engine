use std::collections::BTreeMap;

use crate::{CURRENT_VERSION, Container, FormatError, Limits, Section, ids};

/// A migration is a pure function. It advances the header by exactly one and
/// must preserve identity, feature flags and unknown sections.
pub type Migration = fn(Container) -> Result<Container, FormatError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationStep {
    pub from: u32,
    pub to: u32,
}

#[derive(Default)]
pub struct MigrationRegistry {
    steps: BTreeMap<u32, Migration>,
}

impl MigrationRegistry {
    pub fn builtin() -> Self {
        let mut registry = Self::default();
        registry.steps.insert(0, version_zero);
        registry
    }

    pub fn register(&mut self, from: u32, migration: Migration) -> Result<(), FormatError> {
        if self.steps.contains_key(&from) {
            return Err(FormatError::Metadata("duplicate migration".into()));
        }
        self.steps.insert(from, migration);
        Ok(())
    }

    pub fn migrate(
        &self,
        mut container: Container,
    ) -> Result<(Container, Vec<MigrationStep>), FormatError> {
        let mut ran = Vec::new();
        while container.header.version < CURRENT_VERSION {
            let from = container.header.version;
            let step = self
                .steps
                .get(&from)
                .ok_or(FormatError::MissingMigration(from))?;
            let identity = container.header.document_id;
            let flags = container.header.features;
            let unknown: BTreeMap<_, _> = container
                .sections
                .iter()
                .filter(|(id, _)| **id == ids::EXTENSIONS || !crate::container::known(**id))
                .map(|(id, s)| (*id, s.clone()))
                .collect();
            container = step(container)?;
            container.encoded_len(Limits::default())?;
            if container.header.version != from.saturating_add(1)
                || container.header.document_id != identity
                || container.header.features != flags
                || unknown
                    .iter()
                    .any(|(id, s)| container.sections.get(id) != Some(s))
            {
                return Err(FormatError::BadMigration);
            }
            ran.push(MigrationStep {
                from,
                to: container.header.version,
            });
        }
        Ok((container, ran))
    }
}

/// Synthetic v0 had no font or asset declarations. Its settings and document
/// are unchanged; metadata defaults are the only container-level migration.
fn version_zero(mut container: Container) -> Result<Container, FormatError> {
    if !container.sections.contains_key(&ids::DOCUMENT) {
        return Err(FormatError::Missing(ids::DOCUMENT));
    }
    for id in [ids::FONTS, ids::ASSETS] {
        container
            .sections
            .entry(id)
            .or_insert_with(|| Section::raw(b"[]".to_vec()));
    }
    container
        .sections
        .entry(ids::SETTINGS)
        .or_insert_with(|| Section::raw(b"{}".to_vec()));
    container.header.version = 1;
    // Check migration-produced lengths as well as input lengths.
    container.encoded_len(Limits::default())?;
    Ok(container)
}
