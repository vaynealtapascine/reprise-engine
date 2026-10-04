//! JSON is inspected iteratively before serde can allocate or recurse.
use serde::{Serialize, de::DeserializeOwned};

use crate::{FormatError, Limits};

/// Bound already-materialized caller metadata before recursive serialization.
pub(crate) fn check_values<'a>(
    values: impl IntoIterator<Item = &'a serde_json::Value>,
    limits: Limits,
) -> Result<(), FormatError> {
    let limits = limits.bounded();
    let mut pending = Vec::new();
    let mut visited = 0_usize;
    for value in values {
        if pending.len() >= limits.entries {
            return Err(FormatError::Limit("manifest entries"));
        }
        pending.push((value, 1_usize));
    }
    while let Some((value, depth)) = pending.pop() {
        visited = visited.saturating_add(1);
        if visited > limits.entries {
            return Err(FormatError::Limit("manifest entries"));
        }
        if depth > limits.manifest_depth {
            return Err(FormatError::Limit("manifest depth"));
        }
        let children: Box<dyn Iterator<Item = &serde_json::Value> + '_> = match value {
            serde_json::Value::Array(values) => Box::new(values.iter()),
            serde_json::Value::Object(values) => Box::new(values.values()),
            _ => continue,
        };
        for child in children {
            if pending.len().saturating_add(visited) >= limits.entries {
                return Err(FormatError::Limit("manifest entries"));
            }
            pending.push((child, depth.saturating_add(1)));
        }
    }
    Ok(())
}

pub(crate) fn check(bytes: &[u8], limits: Limits) -> Result<(), FormatError> {
    let limits = limits.bounded();
    if bytes.len() > limits.manifest_bytes {
        return Err(FormatError::Limit("manifest bytes"));
    }
    std::str::from_utf8(bytes).map_err(|e| FormatError::Metadata(e.to_string()))?;
    let mut depth = 0_usize;
    let mut items = 1_usize;
    let mut string = false;
    let mut escaped = false;
    for &byte in bytes {
        if string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
            }
        } else {
            match byte {
                b'"' => string = true,
                b'{' | b'[' => {
                    depth = depth.saturating_add(1);
                    items = items.saturating_add(1);
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                b',' => items = items.saturating_add(1),
                _ => {}
            }
        }
        if depth > limits.manifest_depth {
            return Err(FormatError::Limit("manifest depth"));
        }
        if items > limits.entries {
            return Err(FormatError::Limit("manifest entries"));
        }
    }
    Ok(())
}

pub(crate) fn parse<T: DeserializeOwned>(bytes: &[u8], limits: Limits) -> Result<T, FormatError> {
    check(bytes, limits)?;
    serde_json::from_slice(bytes).map_err(|e| FormatError::Metadata(e.to_string()))
}

pub(crate) fn encode<T: Serialize>(value: &T, limits: Limits) -> Result<Vec<u8>, FormatError> {
    // Value maps are ordered (serde_json's preserve_order feature is disabled).
    // Parsing the result also bounds caller-created nested metadata on save.
    let value = serde_json::to_value(value).map_err(|e| FormatError::Metadata(e.to_string()))?;
    let bytes = serde_json::to_vec(&value).map_err(|e| FormatError::Metadata(e.to_string()))?;
    check(&bytes, limits)?;
    Ok(bytes)
}
