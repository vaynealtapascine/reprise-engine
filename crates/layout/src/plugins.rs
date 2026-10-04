//! Safe adapters for sandboxed extensions. Registrations are immutable during a layout job.
use std::collections::{BTreeMap, BTreeSet};

use reprise_diag::Note;
use reprise_doc::{RelationSchema, SchemaId};
use reprise_plugin::{CallContext, Envelope, GeometryComposer, Phase, Plugin, abi, codes};
use serde::{Deserialize, Serialize};

use crate::{Engine, RelationStatus, Resolution, TargetLayout};

#[derive(Clone, Debug)]
pub struct RelationBinding {
    /// None explicitly represents a missing/refused module; targets still resolve.
    pub plugin: Option<Plugin>,
    pub operation: u32,
}

#[derive(Clone, Debug, Default)]
pub struct PluginRegistry {
    pub relations: BTreeMap<SchemaId, RelationBinding>,
    modules: BTreeSet<Envelope>,
    geometry: Option<(Envelope, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginEnvelope {
    pub abi_version: u32,
    pub modules: Vec<Envelope>,
    pub geometry: Option<(Envelope, u32)>,
    pub relations: Vec<(SchemaId, Option<Envelope>, u32)>,
}

impl PluginRegistry {
    pub fn envelope(&self) -> PluginEnvelope {
        let mut modules = self.modules.clone();
        for binding in self.relations.values() {
            if let Some(p) = &binding.plugin {
                modules.insert(p.envelope().clone());
            }
        }
        PluginEnvelope {
            abi_version: reprise_plugin::ABI_VERSION,
            modules: modules.into_iter().collect(),
            geometry: self.geometry.clone(),
            relations: self
                .relations
                .iter()
                .map(|(id, b)| {
                    (
                        id.clone(),
                        b.plugin.as_ref().map(|p| p.envelope().clone()),
                        b.operation,
                    )
                })
                .collect(),
        }
    }
}

impl Engine {
    /// Registers functions atomically and retains the complete module pin for cache tags.
    pub fn install_plugin_functions(
        &mut self,
        plugin: &Plugin,
    ) -> Result<(), reprise_doc::function::RegistryError> {
        plugin.register_functions(&mut self.functions)?;
        self.plugins.modules.insert(plugin.envelope().clone());
        Ok(())
    }

    /// Applies a plugin shape through an existing conforming composer.
    pub fn install_plugin_geometry(
        &mut self,
        plugin: Plugin,
        operation: u32,
        composer: Box<dyn reprise_compose::Composer>,
    ) -> Result<(), Note> {
        if plugin.envelope().phase != Phase::Layout {
            return Err(Note::warning(
                codes::CAPABILITY,
                "editing plugins cannot provide layout geometry",
            ));
        }
        self.plugins.modules.insert(plugin.envelope().clone());
        self.plugins.geometry = Some((plugin.envelope().clone(), operation));
        self.composer = Box::new(GeometryComposer {
            plugin: Some(plugin),
            operation,
            composer,
        });
        Ok(())
    }

    /// Schemas remain authored declarations; the plugin only acknowledges resolved targets.
    pub fn install_plugin_relation(
        &mut self,
        schema: RelationSchema,
        binding: RelationBinding,
    ) -> Result<(), reprise_doc::relation::SchemaError> {
        let id = schema.id.clone();
        self.schemas.register(schema)?;
        self.plugins.relations.insert(id, binding);
        Ok(())
    }
}

impl RelationBinding {
    pub(crate) fn report(&self, targets: &[TargetLayout]) -> Result<bool, Note> {
        let Some(plugin) = &self.plugin else {
            return Err(Note::warning(
                codes::UNAVAILABLE,
                "plugin relation unavailable; resolved but not applied",
            ));
        };
        if plugin.envelope().phase != Phase::Layout {
            return Err(Note::warning(
                codes::CAPABILITY,
                "editing plugins cannot report layout relations",
            ));
        }
        if targets.len() > 256 {
            return Err(Note::warning(
                codes::LIMIT,
                "too many plugin relation targets",
            ));
        }
        let mut input = Vec::new();
        abi::push_i32(&mut input, targets.len() as i32);
        for target in targets {
            if target.role.len() > 256 {
                return Err(Note::warning(
                    codes::LIMIT,
                    "plugin relation role is too long",
                ));
            }
            abi::push_i32(&mut input, target.role.len() as i32);
            input.extend_from_slice(target.role.as_bytes());
            let status = match target.status {
                RelationStatus::Valid => 0,
                RelationStatus::Rebound => 1,
                RelationStatus::Ambiguous => 2,
                RelationStatus::Missing => 3,
                RelationStatus::OwnerDeleted => 4,
                RelationStatus::Deleted => 5,
            };
            let (kind, count) = match &target.resolved {
                None => (0, 0),
                Some(Resolution::Node(_)) => (1, 1),
                Some(Resolution::Range { .. }) => (2, 1),
                Some(Resolution::Line(_)) => (3, 1),
                Some(Resolution::Nodes(n)) => (4, n.len()),
                Some(Resolution::Lines(n)) => (5, n.len()),
                Some(Resolution::Frame(_)) => (6, 1),
                Some(Resolution::Page(_)) => (7, 1),
                Some(Resolution::Snapshot(_)) => (8, 1),
            };
            for v in [
                status,
                kind,
                i32::try_from(count)
                    .map_err(|_| Note::warning(codes::LIMIT, "too many resolved subjects"))?,
            ] {
                abi::push_i32(&mut input, v);
            }
        }
        let bytes = plugin
            .call(self.operation, &input, &CallContext::default())?
            .bytes;
        if bytes.len() != 4 {
            return Err(Note::warning(
                codes::RESULT,
                "relation result must be one acknowledgement word",
            ));
        }
        match abi::read_i32(&bytes, 0)? {
            0 => Ok(false),
            1 => Ok(targets
                .iter()
                .all(|t| matches!(t.status, RelationStatus::Valid | RelationStatus::Rebound))),
            _ => Err(Note::warning(
                codes::RESULT,
                "invalid relation acknowledgement",
            )),
        }
    }
}
