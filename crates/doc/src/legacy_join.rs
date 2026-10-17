//! Pin historical reference interpretation at replica joins (N3).
//!
//! Literal legacy built-in names are migrated to the already-versioned
//! builtin/owner reference grammar. Each side keeps its own interpretation;
//! an unrelated identity root never acts as a document-wide migration flag.
use crate::{DocError, Document, NodeId, get_str};
use loro::{Container, LoroMap, ValueOrContainer};

#[derive(Clone)]
enum Place {
    Block(NodeId),
    Override(NodeId),
    Flat(String),
    Nested(String),
}

pub(crate) struct Pin {
    place: Place,
    raw: String,
    reference: String,
}

impl Document {
    pub(crate) fn legacy_join_pins(&self, legacy: bool, until: Option<&[(u64, i32)]>) -> Vec<Pin> {
        let covered =
            |peer, counter| until.is_none_or(|v| v.iter().any(|(p, n)| *p == peer && counter < *n));
        let mut pins = Vec::new();
        let mut pin = |place, raw: Option<String>| {
            let Some(raw) = raw.filter(|s| crate::BUILTIN_STYLES.contains(&s.as_str())) else {
                return;
            };
            let builtin = legacy && self.raw_style_record(&raw, true).is_some_and(|r| r.builtin);
            let reference = if builtin {
                self.builtin_reference(&raw)
            } else {
                self.original_owner_reference(&raw)
            };
            pins.push(Pin {
                place,
                raw,
                reference,
            });
        };
        let mut nodes: Vec<_> = self
            .tree("content")
            .nodes()
            .into_iter()
            .map(NodeId::tree)
            .collect();
        if self.has_root(crate::flow::BREAKS) {
            nodes.extend(
                self.break_records()
                    .keys()
                    .filter_map(|k| NodeId::parse(&k)),
            );
        }
        for node in nodes {
            let id = node.mark.unwrap_or(loro::ID {
                peer: node.node.peer,
                counter: node.node.counter,
            });
            if !covered(id.peer, id.counter) {
                continue;
            }
            let Ok(meta) = self.meta_of(node) else {
                continue;
            };
            pin(Place::Block(node), get_str(&meta, "style"));
            if let Some(ValueOrContainer::Container(Container::Map(map))) = meta.get("overrides") {
                pin(Place::Override(node), get_str(&map, "parent"));
            }
        }
        if self.has_root("styles1") {
            let flat = self.doc.get_map("styles1");
            for key in flat.keys().filter(|k| k.starts_with("parent/")) {
                if until.is_none() || flat.get_last_editor(&key).is_some_and(|p| covered(p, 0)) {
                    pin(Place::Flat(key.to_string()), get_str(&flat, &key));
                }
            }
        }
        if self.has_root("styles") {
            let nested = self.doc.get_map("styles");
            for name in nested.keys() {
                if let Some(ValueOrContainer::Container(Container::Map(map))) = nested.get(&name)
                    && (until.is_none()
                        || map.get_last_editor("parent").is_some_and(|p| covered(p, 0)))
                {
                    pin(Place::Nested(name.to_string()), get_str(&map, "parent"));
                }
            }
        }
        pins
    }

    fn pin_map(&self, place: &Place) -> Option<(LoroMap, String)> {
        match place {
            Place::Block(node) => Some((self.meta_of(*node).ok()?, "style".into())),
            Place::Override(node) => match self.meta_of(*node).ok()?.get("overrides")? {
                ValueOrContainer::Container(Container::Map(map)) => Some((map, "parent".into())),
                _ => None,
            },
            Place::Flat(key) => Some((self.doc.get_map("styles1"), key.clone())),
            Place::Nested(name) => match self.doc.get_map("styles").get(name)? {
                ValueOrContainer::Container(Container::Map(map)) => Some((map, "parent".into())),
                _ => None,
            },
        }
    }

    pub(crate) fn apply_legacy_join_pins(
        &self,
        pins: impl Iterator<Item = Pin>,
    ) -> Result<(), DocError> {
        let mut writes = Vec::new();
        for pin in pins {
            if let Some((map, key)) = self.pin_map(&pin.place)
                && get_str(&map, &key).as_deref() == Some(&pin.raw)
            {
                writes.push((map, key, pin.reference));
            }
        }
        if writes.is_empty() {
            return Ok(());
        }
        // The entire join and these writes have already run on an isolated
        // same-peer preflight. Bounds include the added migration operations.
        let count = self.version_vector().iter().fold(0u64, |n, (_, c)| {
            n.saturating_add(u64::try_from(*c).unwrap_or(u64::MAX))
        });
        if count.saturating_add(writes.len() as u64) > crate::persist::MAX_PERSIST_OPS {
            return Err(DocError::Store(
                "legacy reference migration exceeds history limit".into(),
            ));
        }
        self.retain_style_identities();
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        for (map, key, reference) in writes {
            map.insert(&key, reference)?;
        }
        self.doc.commit();
        Ok(())
    }
}
