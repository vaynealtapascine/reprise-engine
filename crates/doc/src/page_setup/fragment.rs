//! Native fragments carry authored scalars, never a resolved page rectangle.
use std::collections::BTreeMap;

use loro::{LoroMap, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{DocError, Document};

use super::ROOT;

const MAX_BYTES: usize = 16 << 10;
const MAX_PROPERTIES: usize = 64;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPatch {
    version: u32,
    properties: BTreeMap<String, Value>,
}

fn invalid() -> DocError {
    DocError::Store("unreadable or excessive native page setup properties".into())
}

fn scalar(value: Value) -> Result<LoroValue, DocError> {
    match value {
        Value::Null => Ok(LoroValue::Null),
        Value::Bool(v) => Ok(LoroValue::Bool(v)),
        Value::String(v) => Ok(LoroValue::String(v.into())),
        Value::Number(v) => {
            if let Some(integer) = v.as_i64() {
                Ok(LoroValue::I64(integer))
            } else if v.is_f64() {
                // An unreadable authored scalar is transported, not used as
                // geometry. The page interpreter still accepts only I64.
                v.as_f64().map(LoroValue::Double).ok_or_else(invalid)
            } else {
                Err(invalid())
            }
        }
        Value::Array(_) | Value::Object(_) => Err(invalid()),
    }
}

fn read(raw: &str) -> Result<BTreeMap<String, LoroValue>, DocError> {
    if raw.len() > MAX_BYTES {
        return Err(invalid());
    }
    let patch: RawPatch = serde_json::from_str(raw).map_err(|_| invalid())?;
    if patch.version != 1 || patch.properties.len() > MAX_PROPERTIES {
        return Err(invalid());
    }
    patch
        .properties
        .into_iter()
        .map(|(key, value)| scalar(value).map(|value| (key, value)))
        .collect()
}

impl Document {
    fn native_page_map(&self) -> Result<Option<LoroMap>, DocError> {
        let LoroValue::Map(roots) = self.doc.get_value() else {
            return Err(invalid());
        };
        match roots.get(ROOT) {
            None => Ok(None),
            Some(LoroValue::Container(id)) if id.container_type() == loro::ContainerType::Map => {
                let map = self.doc.get_map(ROOT);
                if map.len() > MAX_PROPERTIES {
                    return Err(invalid());
                }
                Ok(Some(map))
            }
            Some(_) => Err(invalid()),
        }
    }

    /// Capture at most 64 raw scalar properties in a 16 KiB versioned JSON
    /// envelope. Absent roots stay absent; unsupported containers are refused.
    pub fn raw_page_setup(&self) -> Result<Option<String>, DocError> {
        let Some(map) = self.native_page_map()? else {
            return Ok(None);
        };
        let mut properties = BTreeMap::new();
        let mut bytes = 0usize;
        for key in map.keys() {
            let Some(ValueOrContainer::Value(value)) = map.get(&key) else {
                return Err(invalid());
            };
            let value = match value {
                LoroValue::Null => Value::Null,
                LoroValue::Bool(v) => Value::Bool(v),
                LoroValue::I64(v) => Value::Number(v.into()),
                LoroValue::Double(v) => {
                    Value::Number(serde_json::Number::from_f64(v).ok_or_else(invalid)?)
                }
                LoroValue::String(v) => {
                    bytes = bytes.saturating_add(v.len());
                    if bytes > MAX_BYTES {
                        return Err(invalid());
                    }
                    Value::String(v.to_string())
                }
                _ => return Err(invalid()),
            };
            bytes = bytes.saturating_add(key.len());
            if bytes > MAX_BYTES {
                return Err(invalid());
            }
            properties.insert(key.to_string(), value);
        }
        let raw = serde_json::to_string(&RawPatch {
            version: 1,
            properties,
        })
        .map_err(|_| invalid())?;
        if raw.len() > MAX_BYTES {
            return Err(invalid());
        }
        Ok(Some(raw))
    }

    /// Validate a portable raw envelope without touching authored state.
    pub fn validate_raw_page_setup(raw: &str) -> Result<(), DocError> {
        read(raw).map(|_| ())
    }

    /// Preflight replacement before the editing kernel stages a native paste.
    pub fn validate_page_setup_import(&self, raw: &str) -> Result<(), DocError> {
        Self::validate_raw_page_setup(raw)?;
        self.native_page_map().map(|_| ())
    }

    /// Replace raw authored properties as part of the caller's undo step.
    /// Unknown or conflicting scalars remain raw and receive layout fallback.
    pub fn restore_raw_page_setup(&self, raw: &str) -> Result<(), DocError> {
        let properties = read(raw)?;
        let existing = self.native_page_map()?;
        let map = existing.unwrap_or_else(|| self.doc.get_map(ROOT));
        let mut keys: Vec<_> = map.keys().collect();
        keys.sort();
        for key in keys {
            map.delete(&key)?;
        }
        for (key, value) in properties {
            map.insert(&key, value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_setup::{PageSetupError, PageSetupPatch};
    use reprise_geom::Length;

    #[test]
    fn raw_transfer_preserves_unknown_and_unreadable_scalars_deterministically() {
        let doc = Document::new(1).unwrap();
        let revision = doc.revision();
        assert_eq!(doc.raw_page_setup().unwrap(), None);
        Document::validate_raw_page_setup(r#"{"version":1,"properties":{}}"#).unwrap();
        assert_eq!(doc.revision(), revision);
        assert!(!doc.has_page_setup());
        let raw = r#"{"version":1,"properties":{"width":612000,"left":-1,"future":null,"top":1.5,"right":"unreadable","bottom":9223372036854775807,"height":true}}"#;
        doc.restore_raw_page_setup(raw).unwrap();
        let captured = doc.raw_page_setup().unwrap().unwrap();
        let other = Document::new(2).unwrap();
        other.restore_raw_page_setup(&captured).unwrap();
        assert_eq!(
            other.raw_page_setup().unwrap().as_deref(),
            Some(captured.as_str())
        );
        assert_eq!(other.page_setup_patch(), Err(PageSetupError::Unreadable));
    }

    #[test]
    fn hostile_raw_envelopes_refuse_before_writing() {
        let doc = Document::new(1).unwrap();
        let revision = doc.revision();
        for raw in [
            "{",
            r#"{"version":2,"properties":{}}"#,
            r#"{"version":1,"properties":{"width":{}}}"#,
            r#"{"version":1,"properties":{"width":[]}}"#,
            r#"{"version":1,"properties":{"width":18446744073709551615}}"#,
            r#"{"version":1,"properties":{},"future":true}"#,
        ] {
            assert!(doc.restore_raw_page_setup(raw).is_err());
            assert!(!doc.has_page_setup());
            assert_eq!(doc.revision(), revision);
        }
        let deep = format!(
            "{{\"version\":1,\"properties\":{{\"width\":{}0{}}}}}",
            "[".repeat(1000),
            "]".repeat(1000)
        );
        assert!(doc.restore_raw_page_setup(&deep).is_err());
        assert!(
            doc.restore_raw_page_setup(&" ".repeat(MAX_BYTES + 1))
                .is_err()
        );
        let huge = RawPatch {
            version: 1,
            properties: (0..=MAX_PROPERTIES)
                .map(|i| (i.to_string(), Value::Null))
                .collect(),
        };
        assert!(
            doc.restore_raw_page_setup(&serde_json::to_string(&huge).unwrap())
                .is_err()
        );
        assert!(!doc.has_page_setup());
    }

    #[test]
    fn unsupported_authored_root_types_are_never_repaired_by_transfer() {
        let doc = Document::new(1).unwrap();
        doc.doc.get_text(ROOT).insert(0, "future root").unwrap();
        doc.commit();
        let revision = doc.revision();
        assert!(doc.raw_page_setup().is_err());
        assert!(
            doc.restore_raw_page_setup(r#"{"version":1,"properties":{}}"#)
                .is_err()
        );
        assert_eq!(doc.revision(), revision);
    }

    #[test]
    fn raw_replacement_removes_old_keys_and_rejects_nested_authored_containers() {
        let doc = Document::new(1).unwrap();
        doc.set_page_setup_patch(PageSetupPatch {
            left: Some(Length(10)),
            ..Default::default()
        })
        .unwrap();
        doc.restore_raw_page_setup(r#"{"version":1,"properties":{"width":626688}}"#)
            .unwrap();
        assert_eq!(doc.page_setup_patch().unwrap().left, None);
        let mut nested = doc
            .doc
            .get_map(ROOT)
            .insert_container("future", LoroMap::new())
            .unwrap();
        for _ in 0..256 {
            nested = nested.insert_container("child", LoroMap::new()).unwrap();
        }
        let revision = doc.revision();
        assert!(doc.raw_page_setup().is_err());
        assert_eq!(doc.revision(), revision);
        for i in 0..MAX_PROPERTIES {
            doc.doc
                .get_map(ROOT)
                .insert(&format!("future-{i}"), 0)
                .unwrap();
        }
        assert!(
            doc.validate_page_setup_import(r#"{"version":1,"properties":{}}"#)
                .is_err()
        );
    }
}
