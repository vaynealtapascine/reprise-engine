use std::collections::{BTreeMap, BTreeSet};

use reprise_diag::Note;
use reprise_font::{Face, FaceId};
use serde::{Deserialize, Serialize};

use crate::{Container, FormatError, Limits, codes, ids};

/// `FaceId` supplies family and the 128-bit font content hash. The host pins
/// the distribution's version separately because FaceId has no version field.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontPin {
    pub face: FaceId,
    pub version: String,
    pub asset: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssetKind {
    Font,
    Image,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum ExternalLocation {
    Path(String),
    Url(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AssetSource {
    Bundled { section: u32 },
    External { location: ExternalLocation },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub kind: AssetKind,
    /// Full SHA-256, lowercase hex. FontPin retains FaceId's shorter hash too.
    pub hash: String,
    pub source: AssetSource,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Everything the host needs to resolve this resource without fetching in engine code.
#[derive(Clone)]
pub struct AssetNeed {
    pub id: String,
    pub kind: AssetKind,
    pub hash: String,
    pub location: Option<ExternalLocation>,
    pub font: Option<FontPin>,
}

pub struct AssetAvailability {
    /// All declared resources, including any pinned font without a table entry.
    pub needed: Vec<AssetNeed>,
    /// Only hash-verified bytes. Invalid pinned fonts are excluded from here too.
    pub bundled: BTreeMap<String, Vec<u8>>,
    /// Resources requiring host resolution: external, absent, or rejected bundles.
    pub missing: Vec<AssetNeed>,
    /// Validated, pinned bundled fonts; callers may add these to FontStore.
    pub fonts: Vec<Face>,
    pub notes: Vec<Note>,
}

pub fn content_hash(bytes: &[u8]) -> String {
    crate::container::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn valid_hash(hash: &str, len: usize) -> bool {
    hash.len() == len
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn metadata(
    container: &Container,
    limits: Limits,
) -> Result<(Vec<FontPin>, Vec<Asset>), FormatError> {
    let fonts: Vec<FontPin> = crate::json::parse(
        &container
            .sections
            .get(&ids::FONTS)
            .ok_or(FormatError::Missing(ids::FONTS))?
            .bytes,
        limits,
    )?;
    let assets: Vec<Asset> = crate::json::parse(
        &container
            .sections
            .get(&ids::ASSETS)
            .ok_or(FormatError::Missing(ids::ASSETS))?
            .bytes,
        limits,
    )?;
    let mut font_ids = BTreeSet::new();
    let mut font_assets = BTreeSet::new();
    let mut asset_ids = BTreeSet::new();
    let mut sections = BTreeSet::new();
    for pin in &fonts {
        if !valid_hash(&pin.face.hash, 32)
            || pin.face.family.is_empty()
            || pin.version.is_empty()
            || pin.asset.is_empty()
            || !font_assets.insert(&pin.asset)
            || !font_ids.insert((
                pin.face.family.clone(),
                pin.version.clone(),
                pin.face.hash.clone(),
            ))
        {
            return Err(FormatError::Metadata(
                "invalid or duplicate font pin".into(),
            ));
        }
    }
    for asset in &assets {
        if asset.id.is_empty() || !valid_hash(&asset.hash, 64) || !asset_ids.insert(&asset.id) {
            return Err(FormatError::Metadata("invalid or duplicate asset".into()));
        }
        if let AssetSource::Bundled { section } = asset.source
            && (!(ids::FIRST_ASSET..=ids::LAST_ASSET).contains(&section)
                || !sections.insert(section))
        {
            return Err(FormatError::Metadata(
                "invalid or shared bundled section".into(),
            ));
        }
    }
    Ok((fonts, assets))
}

pub(crate) fn inspect(
    container: &Container,
    limits: Limits,
) -> Result<AssetAvailability, FormatError> {
    let (mut pins, mut assets) = metadata(container, limits)?;
    pins.sort_by(|a, b| (&a.face, &a.version, &a.asset).cmp(&(&b.face, &b.version, &b.asset)));
    assets.sort_by(|a, b| a.id.cmp(&b.id));
    let mut result = AssetAvailability {
        needed: Vec::new(),
        bundled: BTreeMap::new(),
        missing: Vec::new(),
        fonts: Vec::new(),
        notes: Vec::new(),
    };
    for asset in &assets {
        let matching: Vec<_> = pins.iter().filter(|p| p.asset == asset.id).collect();
        let need = AssetNeed {
            id: asset.id.clone(),
            kind: asset.kind.clone(),
            hash: asset.hash.clone(),
            location: match &asset.source {
                AssetSource::External { location } => Some(location.clone()),
                _ => None,
            },
            font: matching.first().map(|p| (*p).clone()),
        };
        result.needed.push(need.clone());
        let bytes = match asset.source {
            AssetSource::Bundled { section } => {
                container.sections.get(&section).map(|s| s.bytes.as_slice())
            }
            AssetSource::External { .. } => None,
        };
        let Some(bytes) = bytes else {
            result.missing.push(need);
            result.notes.push(Note::info(
                codes::ASSET_MISSING,
                format!("host must resolve {}", asset.id),
            ));
            continue;
        };
        if content_hash(bytes) != asset.hash {
            result.missing.push(need);
            result.notes.push(Note::error(
                if asset.kind == AssetKind::Font {
                    codes::FONT_HASH
                } else {
                    codes::ASSET_HASH
                },
                format!("rejected bundle {}", asset.id),
            ));
            continue;
        }
        let mut faces = Vec::new();
        let mut usable = true;
        for pin in matching {
            let hash = crate::container::digest(bytes);
            let face_hash: String = hash.iter().take(16).map(|b| format!("{b:02x}")).collect();
            if asset.kind != AssetKind::Font || face_hash != pin.face.hash {
                result.notes.push(Note::error(
                    codes::FONT_HASH,
                    format!("rejected pin {}", asset.id),
                ));
                usable = false;
                break;
            }
            match Face::from_bytes(bytes) {
                Ok(face) if face.id() == &pin.face => faces.push(face),
                _ => {
                    result.notes.push(Note::error(
                        codes::FONT_UNREADABLE,
                        format!("unreadable or differently named font {}", asset.id),
                    ));
                    usable = false;
                    break;
                }
            }
        }
        if usable {
            result.bundled.insert(asset.id.clone(), bytes.to_vec());
            result.fonts.extend(faces);
        } else {
            result.missing.push(need);
        }
    }
    for pin in pins {
        if !assets.iter().any(|a| a.id == pin.asset) {
            let need = AssetNeed {
                id: pin.asset.clone(),
                kind: AssetKind::Font,
                hash: pin.face.hash.clone(),
                location: None,
                font: Some(pin),
            };
            result.needed.push(need.clone());
            result.missing.push(need);
            result.notes.push(Note::info(
                codes::ASSET_MISSING,
                "pinned font has no asset entry",
            ));
        }
    }
    result.needed.sort_by(|a, b| a.id.cmp(&b.id));
    result.missing.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}
