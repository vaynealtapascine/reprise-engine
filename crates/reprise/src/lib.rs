//! Stable native Reprise editing facade (04, 41).
//! All crossing data is facade-owned and versioned; see docs/bindings.md.
#![forbid(unsafe_code)]
mod convert;
mod dto;
mod error;
mod session;
pub mod typescript;
pub use dto::*;
pub use error::{Error, Result};
pub use session::{DocumentSession, LayoutJob, Plugin, Workspace};
/// Largest JSON request (byte resources use their own explicit limits).
pub const MAX_PAYLOAD_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_ASSET_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_AWARENESS_BYTES: usize = 64 * 1024;
pub fn validate<T>(payload: &Payload<T>) -> Result<&T> {
    if payload.version != API_VERSION {
        return Err(Error::Version(payload.version));
    }
    Ok(&payload.data)
}
/// Bounded JSON entry point for native IPC. Unknown fields and invalid types fail.
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<Payload<T>> {
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(Error::Limit("JSON bytes".into()));
    }
    let mut depth = 0u32;
    let mut string = false;
    let mut escaped = false;
    for b in bytes {
        if string {
            if escaped {
                escaped = false;
            } else if *b == b'\\' {
                escaped = true;
            } else if *b == b'"' {
                string = false;
            }
        } else {
            match b {
                b'"' => string = true,
                b'{' | b'[' => {
                    depth = depth.saturating_add(1);
                    if depth > 64 {
                        return Err(Error::Limit("JSON depth".into()));
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    // Validate the envelope version before interpreting its current-version data.
    #[derive(serde::Deserialize)]
    struct Header {
        version: u32,
    }
    let header: Header =
        serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
    if header.version != API_VERSION {
        return Err(Error::Version(header.version));
    }
    serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))
}
