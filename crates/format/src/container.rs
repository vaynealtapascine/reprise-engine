use std::collections::BTreeMap;

use reprise_diag::Note;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::codes;

pub const CURRENT_VERSION: u32 = 1;
pub(crate) const MAGIC: &[u8; 8] = b"REPRISE\0";
pub(crate) const HEADER_BYTES: usize = 80;
pub(crate) const SECTION_HEADER_BYTES: usize = 52;

pub mod ids {
    pub const DOCUMENT: u32 = 1;
    pub const SETTINGS: u32 = 2;
    pub const FONTS: u32 = 3;
    pub const ASSETS: u32 = 4;
    pub const EXTENSIONS: u32 = 6;
    pub const CACHE: u32 = 0x7fff_ffff;
    pub const FIRST_ASSET: u32 = 0x1_0000;
    pub const LAST_ASSET: u32 = 0x1_ffff;
}

/// A host-assigned persistent identity, independent of peers and revisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DocumentId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeatureFlags {
    pub required: u64,
    pub optional: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub document_id: DocumentId,
    pub features: FeatureFlags,
}

/// Only bit zero is defined: the section must be understood to open the file.
/// Optional unknown sections preserve every flag, codec and payload byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub flags: u32,
    pub codec: u32,
    pub bytes: Vec<u8>,
}

impl Section {
    pub fn raw(bytes: Vec<u8>) -> Self {
        Self {
            flags: 0,
            codec: 0,
            bytes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Container {
    pub header: Header,
    pub sections: BTreeMap<u32, Section>,
}

/// Limits are hard ceilings, including when the caller supplies larger values.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub file_bytes: usize,
    pub section_bytes: usize,
    pub sections: usize,
    pub manifest_bytes: usize,
    pub manifest_depth: usize,
    pub entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            file_bytes: 128 * 1024 * 1024,
            section_bytes: 64 * 1024 * 1024,
            sections: 4096,
            manifest_bytes: 1024 * 1024,
            manifest_depth: 32,
            entries: 4096,
        }
    }
}

impl Limits {
    pub(crate) fn bounded(self) -> Self {
        let hard = Self::default();
        Self {
            file_bytes: self.file_bytes.min(hard.file_bytes),
            section_bytes: self.section_bytes.min(hard.section_bytes),
            sections: self.sections.min(hard.sections),
            manifest_bytes: self.manifest_bytes.min(hard.manifest_bytes),
            manifest_depth: self.manifest_depth.min(hard.manifest_depth),
            entries: self.entries.min(hard.entries),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("file magic is not Reprise")]
    Magic,
    #[error("file is truncated")]
    Truncated,
    #[error("trailing bytes after the declared sections")]
    Trailing,
    #[error("integrity check failed for section {0:?} (None means header)")]
    Integrity(Option<u32>),
    #[error("{0} exceeds the format limit")]
    Limit(&'static str),
    #[error("unknown required features: {0:#x}")]
    RequiredFeatures(u64),
    #[error("unknown required section {0}")]
    RequiredSection(u32),
    #[error("unsupported section encoding {0}")]
    Encoding(u32),
    #[error("duplicate section {0}")]
    Duplicate(u32),
    #[error("missing section {0}")]
    Missing(u32),
    #[error("invalid metadata: {0}")]
    Metadata(String),
    #[error("missing migration from version {0}")]
    MissingMigration(u32),
    #[error("migration did not advance exactly one version")]
    BadMigration,
    #[error("newer file is read-only")]
    ReadOnly,
    #[error(transparent)]
    Document(#[from] reprise_doc::DocError),
}

impl FormatError {
    pub fn note(&self) -> Note {
        if matches!(self, Self::Limit(_)) {
            Note::error(codes::LIMIT, self.to_string())
        } else {
            Note::error(codes::INVALID, self.to_string())
        }
    }
}

pub(crate) fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(crate) fn known(id: u32) -> bool {
    matches!(
        id,
        ids::DOCUMENT | ids::SETTINGS | ids::FONTS | ids::ASSETS | ids::EXTENSIONS | ids::CACHE
    ) || (ids::FIRST_ASSET..=ids::LAST_ASSET).contains(&id)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], FormatError> {
        let end = self
            .at
            .checked_add(len)
            .ok_or(FormatError::Limit("offset"))?;
        let bytes = self.bytes.get(self.at..end).ok_or(FormatError::Truncated)?;
        self.at = end;
        Ok(bytes)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        self.take(N)?.try_into().map_err(|_| FormatError::Truncated)
    }
    fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, FormatError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}

impl Container {
    /// Parse sequentially, validating framing before allocation. Corrupt cache
    /// content is recoverable; ambiguous framing and authored corruption are not.
    pub fn decode(bytes: &[u8], limits: Limits) -> Result<(Self, Vec<Note>), FormatError> {
        let limits = limits.bounded();
        if bytes.len() > limits.file_bytes {
            return Err(FormatError::Limit("file bytes"));
        }
        let mut r = Reader { bytes, at: 0 };
        if r.take(8)? != MAGIC {
            return Err(FormatError::Magic);
        }
        let version = r.u32()?;
        let document_id = DocumentId(r.array()?);
        let features = FeatureFlags {
            required: r.u64()?,
            optional: r.u64()?,
        };
        let count = usize::try_from(r.u32()?).map_err(|_| FormatError::Limit("sections"))?;
        let checksum = r.array::<32>()?;
        if digest(bytes.get(..48).ok_or(FormatError::Truncated)?) != checksum {
            return Err(FormatError::Integrity(None));
        }
        if features.required != 0 {
            return Err(FormatError::RequiredFeatures(features.required));
        }
        if count > limits.sections {
            return Err(FormatError::Limit("sections"));
        }
        let mut sections = BTreeMap::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut notes = Vec::new();
        for index in 0..count {
            let start = r.at;
            let id = r.u32()?;
            let flags = r.u32()?;
            let codec = r.u32()?;
            let len = usize::try_from(r.u64()?).map_err(|_| FormatError::Limit("section bytes"))?;
            let checksum = r.array::<32>()?;
            if !seen.insert(id) {
                return Err(FormatError::Duplicate(id));
            }
            if !known(id) && flags & 1 != 0 {
                return Err(FormatError::RequiredSection(id));
            }
            if len > limits.section_bytes {
                return Err(FormatError::Limit("section bytes"));
            }
            let payload = match r.take(len) {
                Ok(payload) => payload,
                Err(FormatError::Truncated)
                    if id == ids::CACHE && index.saturating_add(1) == count =>
                {
                    notes.push(Note::info(
                        codes::CACHE_DROPPED,
                        "truncated final cache discarded",
                    ));
                    r.at = bytes.len();
                    continue;
                }
                Err(err) => return Err(err),
            };
            let mut hash = Sha256::new();
            hash.update(
                bytes
                    .get(start..start.saturating_add(20))
                    .ok_or(FormatError::Truncated)?,
            );
            hash.update(payload);
            let actual: [u8; 32] = hash.finalize().into();
            if actual != checksum || (known(id) && (codec != 0 || flags & !1 != 0)) {
                if id == ids::CACHE {
                    notes.push(Note::info(
                        codes::CACHE_DROPPED,
                        "corrupt or unsupported cache discarded",
                    ));
                    continue;
                }
                if actual != checksum {
                    return Err(FormatError::Integrity(Some(id)));
                }
                return Err(FormatError::Encoding(id));
            }
            sections.insert(
                id,
                Section {
                    flags,
                    codec,
                    bytes: payload.to_vec(),
                },
            );
        }
        if r.at != bytes.len() {
            return Err(FormatError::Trailing);
        }
        Ok((
            Self {
                header: Header {
                    version,
                    document_id,
                    features,
                },
                sections,
            },
            notes,
        ))
    }

    pub(crate) fn encoded_len(&self, limits: Limits) -> Result<usize, FormatError> {
        let limits = limits.bounded();
        if self.sections.len() > limits.sections {
            return Err(FormatError::Limit("sections"));
        }
        let size = self
            .sections
            .values()
            .try_fold(HEADER_BYTES, |sum, section| {
                if section.bytes.len() > limits.section_bytes {
                    return Err(FormatError::Limit("section bytes"));
                }
                sum.checked_add(SECTION_HEADER_BYTES)
                    .and_then(|s| s.checked_add(section.bytes.len()))
                    .ok_or(FormatError::Limit("file bytes"))
            })?;
        if size > limits.file_bytes {
            return Err(FormatError::Limit("file bytes"));
        }
        Ok(size)
    }

    /// Internal versioned encoder, also used to pin old-version fixtures.
    pub(crate) fn encode(&self, limits: Limits) -> Result<Vec<u8>, FormatError> {
        let size = self.encoded_len(limits)?;
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.header.version.to_le_bytes());
        out.extend_from_slice(&self.header.document_id.0);
        out.extend_from_slice(&self.header.features.required.to_le_bytes());
        out.extend_from_slice(&self.header.features.optional.to_le_bytes());
        let count =
            u32::try_from(self.sections.len()).map_err(|_| FormatError::Limit("sections"))?;
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&digest(&out));
        for (&id, section) in &self.sections {
            let mut head = Vec::with_capacity(20);
            head.extend_from_slice(&id.to_le_bytes());
            head.extend_from_slice(&section.flags.to_le_bytes());
            head.extend_from_slice(&section.codec.to_le_bytes());
            let len = u64::try_from(section.bytes.len())
                .map_err(|_| FormatError::Limit("section bytes"))?;
            head.extend_from_slice(&len.to_le_bytes());
            let mut hash = Sha256::new();
            hash.update(&head);
            hash.update(&section.bytes);
            let checksum: [u8; 32] = hash.finalize().into();
            out.extend_from_slice(&head);
            out.extend_from_slice(&checksum);
            out.extend_from_slice(&section.bytes);
        }
        Ok(out)
    }
}
