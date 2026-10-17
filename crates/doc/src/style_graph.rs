//! A bounded transfer of authored style roots, independent of clipboard appearance.
use std::collections::BTreeMap;

use loro::{Container, ContainerType, LoroMap, LoroValue, ValueOrContainer};

use crate::{DocError, Document};

const MAX_KEYS: usize = 1 << 16;
const MAX_BYTES: usize = 16 << 20;
const MAX_VALUE_DEPTH: usize = 64;

/// Complete existing style storage, including aliases, tombstones and unused
/// records. Opaque: no CRDT handles or source operation IDs cross this boundary.
#[derive(Clone, Debug)]
pub struct AuthoredStyleGraph {
    nested: Option<BTreeMap<String, BTreeMap<String, LoroValue>>>,
    flat: Option<BTreeMap<String, LoroValue>>,
    identities: Option<BTreeMap<String, LoroValue>>,
    legacy_builtins: bool,
    clocks: BTreeMap<u64, i64>,
}

fn invalid() -> DocError {
    DocError::Store("unreadable or excessive authored style graph".into())
}

#[derive(Default)]
struct Budget {
    keys: usize,
    bytes: usize,
}

impl Budget {
    fn key(&mut self, key: &str) -> Result<(), DocError> {
        self.keys = self.keys.saturating_add(1);
        self.add_bytes(key.len())?;
        if self.keys > MAX_KEYS {
            return Err(invalid());
        }
        Ok(())
    }

    fn add_bytes(&mut self, count: usize) -> Result<(), DocError> {
        self.bytes = self.bytes.saturating_add(count);
        if self.bytes > MAX_BYTES {
            return Err(invalid());
        }
        Ok(())
    }

    fn value(&mut self, value: &LoroValue, depth: usize) -> Result<(), DocError> {
        if depth > MAX_VALUE_DEPTH {
            return Err(invalid());
        }
        self.add_bytes(16)?;
        match value {
            LoroValue::String(v) => self.add_bytes(v.len()),
            LoroValue::Binary(v) => self.add_bytes(v.len()),
            LoroValue::List(v) => {
                for value in v.iter() {
                    self.key("")?;
                    self.value(value, depth.saturating_add(1))?;
                }
                Ok(())
            }
            LoroValue::Map(v) => {
                for (key, value) in v.iter() {
                    self.key(key)?;
                    self.value(value, depth.saturating_add(1))?;
                }
                Ok(())
            }
            LoroValue::Container(_) => Err(invalid()),
            _ => Ok(()),
        }
    }
}

fn properties(map: &LoroMap, budget: &mut Budget) -> Result<BTreeMap<String, LoroValue>, DocError> {
    if map.len() > MAX_KEYS {
        return Err(invalid());
    }
    let mut properties = BTreeMap::new();
    for key in map.keys() {
        budget.key(&key)?;
        let Some(ValueOrContainer::Value(value)) = map.get(&key) else {
            return Err(invalid());
        };
        budget.value(&value, 0)?;
        properties.insert(key.to_string(), value);
    }
    Ok(properties)
}

impl Document {
    fn style_graph_root(&self, name: &str) -> Result<Option<LoroMap>, DocError> {
        let LoroValue::Map(roots) = self.doc.get_value() else {
            return Err(invalid());
        };
        match roots.get(name) {
            None => Ok(None),
            Some(LoroValue::Container(id)) if id.container_type() == ContainerType::Map => {
                Ok(Some(self.doc.get_map(name)))
            }
            Some(_) => Err(invalid()),
        }
    }

    /// Read both authored style roots without creating missing roots or
    /// resolving their values. At most 65536 keys/entries, 16 MiB of keys and
    /// values, and 64 levels inside literal values. Unexpected containers refuse.
    pub fn authored_style_graph(&self) -> Result<AuthoredStyleGraph, DocError> {
        let mut budget = Budget::default();
        let nested = if let Some(root) = self.style_graph_root("styles")? {
            if root.len() > MAX_KEYS {
                return Err(invalid());
            }
            let mut records = BTreeMap::new();
            for name in root.keys() {
                budget.key(&name)?;
                let Some(ValueOrContainer::Container(Container::Map(map))) = root.get(&name) else {
                    return Err(invalid());
                };
                records.insert(name.to_string(), properties(&map, &mut budget)?);
            }
            Some(records)
        } else {
            None
        };
        let flat = self
            .style_graph_root("styles1")?
            .map(|root| properties(&root, &mut budget))
            .transpose()?;
        let identities = self
            .style_graph_root(crate::style_identity::ROOT)?
            .map(|root| properties(&root, &mut budget))
            .transpose()?;
        let mut clocks = BTreeMap::new();
        if self.has_style_identities() {
            for (peer, counter) in self.doc.oplog_vv().iter() {
                if let Ok(floor) = self.style_owner_floor(*peer) {
                    clocks.insert(
                        *peer,
                        floor.checked_add(i64::from(*counter)).ok_or_else(invalid)?,
                    );
                }
            }
            if let Some(properties) = &identities {
                for (key, value) in properties {
                    if let Some(peer) = key
                        .strip_prefix("clock/")
                        .and_then(|p| p.parse::<u64>().ok())
                        && let LoroValue::String(value) = value
                        && let Ok(floor) = value.parse::<i64>()
                        && floor >= 0
                    {
                        clocks.entry(peer).or_insert(floor);
                    }
                }
            }
            for (peer, counter) in &clocks {
                let key = format!("clock/{peer}");
                if identities.as_ref().is_none_or(|m| !m.contains_key(&key)) {
                    budget.key(&key)?;
                }
                budget.add_bytes(counter.to_string().len())?;
            }
        }
        if self.legacy_builtin_context()
            && identities
                .as_ref()
                .is_none_or(|m| !m.contains_key("legacy-builtins1"))
        {
            budget.key("legacy-builtins1")?;
            budget.add_bytes(1)?;
        }
        Ok(AuthoredStyleGraph {
            nested,
            flat,
            identities,
            legacy_builtins: self.legacy_builtin_context(),
            clocks,
        })
    }

    /// Install an opaque graph in fresh history. Refuses before writing if
    /// either target style root already exists. Does not commit.
    pub fn restore_authored_style_graph(&self, graph: &AuthoredStyleGraph) -> Result<(), DocError> {
        if self.has_root("styles") || self.has_root("styles1") || self.has_style_identities() {
            return Err(invalid());
        }
        if let Some(records) = &graph.nested {
            let root = self.doc.get_map("styles");
            for (name, properties) in records {
                let record = root.insert_container(name, LoroMap::new())?;
                for (key, value) in properties {
                    record.insert(key, value.clone())?;
                }
            }
        }
        if let Some(properties) = &graph.flat {
            let root = self.doc.get_map("styles1");
            for (key, value) in properties {
                root.insert(key, value.clone())?;
            }
        }
        if let Some(properties) = &graph.identities {
            self.retain_style_identities();
            let root = self.doc.get_map(crate::style_identity::ROOT);
            for (key, value) in properties {
                root.insert(key, value.clone())?;
            }
        }
        if !graph.clocks.is_empty() || graph.legacy_builtins {
            self.retain_style_identities();
        }
        for (peer, counter) in &graph.clocks {
            self.doc
                .get_map(crate::style_identity::ROOT)
                .insert(&format!("clock/{peer}"), counter.to_string())?;
        }
        if graph.legacy_builtins {
            self.doc
                .get_map(crate::style_identity::ROOT)
                .insert("legacy-builtins1", "1")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PersistenceMode, Style};

    #[test]
    fn capture_is_read_pure_and_restores_unreadable_properties_in_new_history() {
        let empty = Document::new(1).unwrap();
        let revision = empty.revision();
        empty.authored_style_graph().unwrap();
        assert_eq!(revision, empty.revision());
        assert!(!empty.has_root("styles") && !empty.has_root("styles1"));
        let old = Document::new(1).unwrap();
        old.define_style("legacy", &Style::default()).unwrap();
        let root = old.doc.get_map("styles");
        let Some(ValueOrContainer::Container(Container::Map(record))) = root.get("legacy") else {
            panic!()
        };
        record.insert("weight", "future-weight").unwrap();
        old.doc
            .get_map("styles1")
            .insert("state/gone", "deleted")
            .unwrap();
        old.doc
            .get_map("styles1")
            .insert("future/gone", LoroValue::I64(17))
            .unwrap();
        old.commit();
        let before = old.try_export(PersistenceMode::History).unwrap();
        let graph = old.authored_style_graph().unwrap();
        assert_eq!(before, old.try_export(PersistenceMode::History).unwrap());
        let fresh = Document::new(2).unwrap();
        fresh.restore_authored_style_graph(&graph).unwrap();
        assert_eq!(old.style_record("legacy"), fresh.style_record("legacy"));
        assert!(matches!(
            fresh.doc.get_map("styles1").get("future/gone"),
            Some(ValueOrContainer::Value(LoroValue::I64(17)))
        ));
        fresh.commit();
        assert_eq!(
            fresh
                .version_vector()
                .iter()
                .map(|(peer, _)| *peer)
                .collect::<Vec<_>>(),
            vec![2]
        );
        let before = fresh.try_export(PersistenceMode::History).unwrap();
        assert!(fresh.restore_authored_style_graph(&graph).is_err());
        assert_eq!(before, fresh.try_export(PersistenceMode::History).unwrap());
    }

    #[test]
    fn capture_refuses_unknown_containers_and_excessive_values_without_mutation() {
        for huge in [false, true] {
            let doc = Document::new(1).unwrap();
            let root = doc.doc.get_map("styles1");
            if huge {
                root.insert("future/name", "x".repeat(MAX_BYTES + 1))
                    .unwrap();
            } else {
                root.insert_container("future/name", LoroMap::new())
                    .unwrap();
            }
            doc.commit();
            let revision = doc.revision();
            assert!(doc.authored_style_graph().is_err());
            assert_eq!(revision, doc.revision());
        }
    }
}
