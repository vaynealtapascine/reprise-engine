use std::collections::BTreeMap;

use reprise_diag::Note;
use reprise_doc::{Document, DocumentAt, PersistenceMode, Revision, SnapshotRef};
use reprise_shape::AdapterInfo;
use serde::{Deserialize, Serialize};

use crate::{
    Asset, AssetAvailability, CURRENT_VERSION, Container, DocumentId, FeatureFlags, FontPin,
    FormatError, Header, Limits, MigrationRegistry, MigrationStep, Section, codes, ids,
};

/// Always bind a saved snapshot reference to its source document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotReference {
    pub document_id: DocumentId,
    pub reference: SnapshotRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheTags {
    pub document_id: DocumentId,
    pub revision: Revision,
    pub engine_version: String,
    pub adapter: AdapterInfo,
    /// SHA-256 of a host-defined canonical engine configuration.
    pub configuration_hash: String,
}

pub type CacheContext = CacheTags;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedCache {
    pub tags: CacheTags,
    pub bytes: Vec<u8>,
}

fn revision_valid(revision: &Revision) -> bool {
    revision.0.iter().all(|(_, counter)| *counter >= 0)
        && revision.0.windows(2).all(|pair| match pair {
            [a, b] => a.0 < b.0,
            _ => false,
        })
}

fn validate_tags(tags: &CacheTags) -> Result<(), FormatError> {
    if !revision_valid(&tags.revision)
        || tags.configuration_hash.len() != 64
        || !tags
            .configuration_hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || tags.engine_version.is_empty()
        || tags.adapter.name.is_empty()
        || tags.adapter.version.is_empty()
    {
        return Err(FormatError::Metadata("invalid cache tags".into()));
    }
    Ok(())
}

impl DerivedCache {
    pub fn validate(&self, expected: &CacheContext) -> bool {
        self.tags == *expected && validate_tags(expected).is_ok()
    }

    fn encode(&self, limits: Limits) -> Result<Vec<u8>, FormatError> {
        validate_tags(&self.tags)?;
        let tags = crate::json::encode(&self.tags, limits)?;
        let len = u32::try_from(tags.len()).map_err(|_| FormatError::Limit("cache tags"))?;
        let size = tags
            .len()
            .checked_add(4)
            .and_then(|s| s.checked_add(self.bytes.len()))
            .ok_or(FormatError::Limit("cache bytes"))?;
        if size > limits.bounded().section_bytes {
            return Err(FormatError::Limit("cache bytes"));
        }
        let mut bytes = Vec::with_capacity(size);
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&tags);
        bytes.extend_from_slice(&self.bytes);
        Ok(bytes)
    }

    fn decode(bytes: &[u8], limits: Limits) -> Result<Self, FormatError> {
        let head: [u8; 4] = bytes
            .get(..4)
            .ok_or(FormatError::Truncated)?
            .try_into()
            .map_err(|_| FormatError::Truncated)?;
        let len = usize::try_from(u32::from_le_bytes(head))
            .map_err(|_| FormatError::Limit("cache tags"))?;
        let end = len.checked_add(4).ok_or(FormatError::Limit("cache tags"))?;
        let tags: CacheTags =
            crate::json::parse(bytes.get(4..end).ok_or(FormatError::Truncated)?, limits)?;
        validate_tags(&tags)?;
        Ok(Self {
            tags,
            bytes: bytes.get(end..).ok_or(FormatError::Truncated)?.to_vec(),
        })
    }
}

// Preserve unrecognized fields while refreshing the known declaration fields.
// The incoming typed declaration has at most three levels, independent of any
// unknown stored subtrees; recursion follows only that known schema.
fn overlay_declaration(stored: &mut serde_json::Value, incoming: serde_json::Value) {
    if let (Some(target), serde_json::Value::Object(fields)) = (stored.as_object_mut(), &incoming) {
        for (name, value) in fields {
            overlay_declaration(
                target
                    .entry(name.clone())
                    .or_insert(serde_json::Value::Null),
                value.clone(),
            );
        }
    } else {
        *stored = incoming;
    }
}

/// Raw manifests are retained on open so unknown fields and extension bytes
/// survive without being normalized. Setters replace only their own section.
#[derive(Clone)]
pub struct Package {
    pub(crate) container: Container,
    pub(crate) limits: Limits,
    pub(crate) revision: Revision,
}

impl Package {
    pub fn new(
        document: &Document,
        id: DocumentId,
        mode: PersistenceMode,
    ) -> Result<Self, FormatError> {
        let mut sections = BTreeMap::new();
        sections.insert(ids::DOCUMENT, Section::raw(document.try_export(mode)?));
        sections.insert(ids::SETTINGS, Section::raw(b"{}".to_vec()));
        sections.insert(ids::FONTS, Section::raw(b"[]".to_vec()));
        sections.insert(ids::ASSETS, Section::raw(b"[]".to_vec()));
        Ok(Self {
            container: Container {
                header: Header {
                    version: CURRENT_VERSION,
                    document_id: id,
                    features: FeatureFlags::default(),
                },
                sections,
            },
            limits: Limits::default(),
            revision: document.revision(),
        })
    }

    /// A save-ready package embedding precisely faces with at least one glyph
    /// in the supplied current layout, including glyph zero. Empty runs and
    /// style-only names are excluded. The old metadata-only constructor remains.
    pub fn new_with_layout(
        document: &Document,
        id: DocumentId,
        mode: PersistenceMode,
        layout: &reprise_layout::LayoutSnapshot,
        fonts: &reprise_font::FontStore,
    ) -> Result<Self, FormatError> {
        let mut package = Self::new(document, id, mode)?;
        package.embed_layout_fonts(layout, fonts)?;
        Ok(package)
    }

    /// Add/refresh used font bundles without dropping other assets, font pins,
    /// unknown metadata or opaque sections. Failure leaves this package intact.
    pub fn embed_layout_fonts(
        &mut self,
        layout: &reprise_layout::LayoutSnapshot,
        fonts: &reprise_font::FontStore,
    ) -> Result<(), FormatError> {
        self.writable()?;
        if layout.revision != self.revision {
            return Err(FormatError::Metadata(
                "layout revision differs from package".into(),
            ));
        }
        let used: std::collections::BTreeSet<_> = layout
            .blocks
            .iter()
            .flat_map(|block| &block.lines)
            .flat_map(|line| &line.runs)
            .filter(|run| !run.glyphs.is_empty())
            .map(|run| run.face.clone())
            .collect();
        if used.len() > self.limits.entries {
            return Err(FormatError::Limit("used fonts"));
        }
        let (mut pins, mut assets) = crate::assets::metadata(&self.container, self.limits)?;
        let mut candidate = self.container.clone();
        for id in used {
            let face = fonts
                .get(&id)
                .map_err(|error| FormatError::Metadata(error.to_string()))?;
            let previous = pins.iter().position(|pin| pin.face == id);
            let mut pin = if let Some(index) = previous {
                pins.remove(index)
            } else {
                FontPin {
                    face: id,
                    version: face.version().into(),
                    asset: String::new(),
                    extra: BTreeMap::new(),
                }
            };
            if pin.asset.is_empty()
                || assets
                    .iter()
                    .any(|asset| asset.id == pin.asset && asset.kind != crate::AssetKind::Font)
            {
                pin.asset = format!(
                    "font-{}-{}",
                    crate::assets::content_hash(pin.face.family.as_bytes()),
                    pin.face.hash
                );
                // Bounded collision repair preserves host assets with the same ID.
                for _ in 0..=self.limits.entries {
                    if !assets.iter().any(|entry| entry.id == pin.asset) {
                        break;
                    }
                    pin.asset.push('-');
                }
                if assets.iter().any(|entry| entry.id == pin.asset) {
                    return Err(FormatError::Limit("font asset names"));
                }
            }
            let existing = assets.iter().position(|asset| asset.id == pin.asset);
            let section = existing
                .and_then(|index| match assets.get(index)?.source {
                    crate::AssetSource::Bundled { section } => Some(section),
                    _ => None,
                })
                .or_else(|| {
                    (ids::FIRST_ASSET..=ids::LAST_ASSET)
                        .find(|id| !candidate.sections.contains_key(id))
                })
                .ok_or(FormatError::Limit("font bundle sections"))?;
            let mut asset = existing
                .map(|index| assets.remove(index))
                .unwrap_or_else(|| Asset {
                    id: pin.asset.clone(),
                    kind: crate::AssetKind::Font,
                    hash: String::new(),
                    source: crate::AssetSource::Bundled { section },
                    extra: BTreeMap::new(),
                });
            asset.kind = crate::AssetKind::Font;
            asset.hash = crate::assets::content_hash(face.data());
            asset.source = crate::AssetSource::Bundled { section };
            pin.version = face.version().into();
            let declared = serde_json::to_value(face.declaration())
                .map_err(|error| FormatError::Metadata(error.to_string()))?;
            overlay_declaration(
                pin.extra
                    .entry("font-declaration1".into())
                    .or_insert(serde_json::Value::Null),
                declared,
            );
            candidate
                .sections
                .insert(section, Section::raw(face.data().to_vec()));
            pins.push(pin);
            assets.push(asset);
        }
        pins.sort_by(|a, b| (&a.face, &a.version, &a.asset).cmp(&(&b.face, &b.version, &b.asset)));
        assets.sort_by(|a, b| a.id.cmp(&b.id));
        candidate.sections.insert(
            ids::FONTS,
            Section::raw(crate::json::encode(&pins, self.limits)?),
        );
        candidate.sections.insert(
            ids::ASSETS,
            Section::raw(crate::json::encode(&assets, self.limits)?),
        );
        crate::assets::metadata(&candidate, self.limits)?;
        candidate.encoded_len(self.limits)?;
        self.container = candidate;
        Ok(())
    }

    pub fn container(&self) -> &Container {
        &self.container
    }
    pub fn document_id(&self) -> DocumentId {
        self.container.header.document_id
    }

    pub(crate) fn writable(&self) -> Result<(), FormatError> {
        if self.container.header.version > CURRENT_VERSION {
            Err(FormatError::ReadOnly)
        } else {
            Ok(())
        }
    }

    pub fn set_optional_features(&mut self, optional: u64) -> Result<(), FormatError> {
        self.writable()?;
        self.container.header.features.optional = optional;
        Ok(())
    }

    pub fn settings(&self) -> Result<BTreeMap<String, serde_json::Value>, FormatError> {
        crate::json::parse(
            &self
                .container
                .sections
                .get(&ids::SETTINGS)
                .ok_or(FormatError::Missing(ids::SETTINGS))?
                .bytes,
            self.limits,
        )
    }

    pub fn set_settings(
        &mut self,
        settings: &BTreeMap<String, serde_json::Value>,
    ) -> Result<(), FormatError> {
        self.writable()?;
        crate::json::check_values(settings.values(), self.limits)?;
        self.container.sections.insert(
            ids::SETTINGS,
            Section::raw(crate::json::encode(settings, self.limits)?),
        );
        Ok(())
    }

    pub fn set_fonts(&mut self, mut pins: Vec<FontPin>) -> Result<(), FormatError> {
        self.writable()?;
        crate::json::check_values(pins.iter().flat_map(|pin| pin.extra.values()), self.limits)?;
        pins.sort_by(|a, b| (&a.face, &a.version, &a.asset).cmp(&(&b.face, &b.version, &b.asset)));
        let mut candidate = self.container.clone();
        candidate.sections.insert(
            ids::FONTS,
            Section::raw(crate::json::encode(&pins, self.limits)?),
        );
        crate::assets::metadata(&candidate, self.limits)?;
        self.container = candidate;
        Ok(())
    }

    /// Bundled section IDs are explicit and stable. Missing bundles are allowed
    /// and reported; references are never opened or fetched by this library.
    pub fn set_assets(
        &mut self,
        mut assets: Vec<Asset>,
        bundles: BTreeMap<u32, Vec<u8>>,
    ) -> Result<(), FormatError> {
        self.writable()?;
        crate::json::check_values(
            assets.iter().flat_map(|asset| asset.extra.values()),
            self.limits,
        )?;
        assets.sort_by(|a, b| a.id.cmp(&b.id));
        let mut candidate = self.container.clone();
        candidate
            .sections
            .retain(|id, _| !(ids::FIRST_ASSET..=ids::LAST_ASSET).contains(id));
        for (id, bytes) in bundles {
            if !(ids::FIRST_ASSET..=ids::LAST_ASSET).contains(&id) {
                return Err(FormatError::Metadata(
                    "bundle section ID out of range".into(),
                ));
            }
            if bytes.len() > self.limits.section_bytes {
                return Err(FormatError::Limit("asset bytes"));
            }
            candidate.sections.insert(id, Section::raw(bytes));
        }
        candidate.sections.insert(
            ids::ASSETS,
            Section::raw(crate::json::encode(&assets, self.limits)?),
        );
        crate::assets::metadata(&candidate, self.limits)?;
        self.container = candidate;
        Ok(())
    }

    pub fn assets(&self) -> Result<AssetAvailability, FormatError> {
        crate::assets::inspect(&self.container, self.limits)
    }

    pub fn set_extensions(&mut self, bytes: Vec<u8>) -> Result<(), FormatError> {
        self.writable()?;
        if bytes.len() > self.limits.section_bytes {
            return Err(FormatError::Limit("extensions"));
        }
        self.container
            .sections
            .insert(ids::EXTENSIONS, Section::raw(bytes));
        Ok(())
    }

    pub fn extensions(&self) -> Option<&[u8]> {
        self.container
            .sections
            .get(&ids::EXTENSIONS)
            .map(|s| s.bytes.as_slice())
    }

    pub fn set_unknown_section(&mut self, id: u32, section: Section) -> Result<(), FormatError> {
        self.writable()?;
        if crate::container::known(id) {
            return Err(FormatError::Metadata("reserved section ID".into()));
        }
        if section.flags & 1 != 0 {
            return Err(FormatError::RequiredSection(id));
        }
        if section.bytes.len() > self.limits.section_bytes {
            return Err(FormatError::Limit("unknown section bytes"));
        }
        self.container.sections.insert(id, section);
        Ok(())
    }

    pub fn set_cache(&mut self, cache: Option<DerivedCache>) -> Result<(), FormatError> {
        self.writable()?;
        if let Some(cache) = cache {
            self.container
                .sections
                .insert(ids::CACHE, Section::raw(cache.encode(self.limits)?));
        } else {
            self.container.sections.remove(&ids::CACHE);
        }
        Ok(())
    }

    /// Cache bytes are exposed only through full tag validation.
    pub fn usable_cache(&self, expected: &CacheContext) -> (Option<Vec<u8>>, Vec<Note>) {
        let Some(section) = self.container.sections.get(&ids::CACHE) else {
            return (None, Vec::new());
        };
        match DerivedCache::decode(&section.bytes, self.limits) {
            Ok(cache)
                if expected.document_id == self.document_id()
                    && expected.revision == self.revision
                    && cache.validate(expected) =>
            {
                (Some(cache.bytes), Vec::new())
            }
            _ => (
                None,
                vec![Note::info(
                    codes::CACHE_IGNORED,
                    "cache tags do not match the requested inputs",
                )],
            ),
        }
    }

    /// Save the captured package, always at the current format version.
    /// To capture document edits after opening use `OpenedFile::save`.
    pub fn save(&self) -> Result<Vec<u8>, FormatError> {
        self.writable()?;
        self.settings()?;
        crate::assets::metadata(&self.container, self.limits)?;
        let mut container = self.container.clone();
        container.header.version = CURRENT_VERSION;
        container.encode(self.limits)
    }

    /// Open a package and restore every verified embedded face into the supplied
    /// frontend store. Missing/corrupt pins remain in the returned missing list.
    pub fn open_with_fonts(
        bytes: &[u8],
        peer: u64,
        limits: Limits,
        migrations: &MigrationRegistry,
        fonts: &mut reprise_font::FontStore,
    ) -> Result<OpenedFile, FormatError> {
        let mut opened = Self::open(bytes, peer, limits, migrations)?;
        opened.restore_fonts(fonts);
        Ok(opened)
    }

    pub fn open(
        bytes: &[u8],
        peer: u64,
        limits: Limits,
        migrations: &MigrationRegistry,
    ) -> Result<OpenedFile, FormatError> {
        let limits = limits.bounded();
        let (container, mut notes) = Container::decode(bytes, limits)?;
        let newer = container.header.version > CURRENT_VERSION;
        let (container, ran) = migrations.migrate(container)?;
        container.encoded_len(limits)?;
        let doc_bytes = &container
            .sections
            .get(&ids::DOCUMENT)
            .ok_or(FormatError::Missing(ids::DOCUMENT))?
            .bytes;
        let document = Document::import(doc_bytes, peer)?;
        let mut package = Self {
            container,
            limits,
            revision: document.revision(),
        };
        package.settings()?;
        let assets = package.assets()?;
        notes.extend(assets.notes.clone());
        if let Some(section) = package.container.sections.get(&ids::CACHE) {
            let valid = DerivedCache::decode(&section.bytes, limits).is_ok_and(|cache| {
                cache.tags.document_id == package.document_id()
                    && cache.tags.revision == document.revision()
            });
            if !valid {
                package.container.sections.remove(&ids::CACHE);
                notes.push(Note::info(
                    codes::CACHE_DROPPED,
                    "malformed or wrong-document/revision cache discarded",
                ));
            }
        }
        for step in &ran {
            notes.push(Note::info(
                codes::MIGRATED,
                format!("migrated {} to {}", step.from, step.to),
            ));
        }
        if newer {
            notes.push(Note::info(
                codes::READ_ONLY,
                "compatible newer document opened read-only",
            ));
        }
        Ok(OpenedFile {
            package,
            document,
            read_only: newer,
            notes,
            migrations: ran,
            assets,
        })
    }
}

pub struct OpenedFile {
    package: Package,
    document: Document,
    read_only: bool,
    pub notes: Vec<Note>,
    pub migrations: Vec<MigrationStep>,
    pub assets: AssetAvailability,
}

impl OpenedFile {
    pub fn save_with_resources(
        &self,
        mode: PersistenceMode,
        layout: &reprise_layout::LayoutSnapshot,
        fonts: &reprise_font::FontStore,
        assets: &reprise_display::AssetStore,
    ) -> Result<Vec<u8>, FormatError> {
        if self.read_only {
            return Err(FormatError::ReadOnly);
        }
        let mut package = self.package.clone();
        package.revision = self.document.revision();
        package
            .container
            .sections
            .insert(ids::DOCUMENT, Section::raw(self.document.try_export(mode)?));
        package.container.sections.remove(&ids::CACHE);
        package.embed_layout_fonts(layout, fonts)?;
        package.embed_document_images(&self.document, assets)?;
        package.save()
    }
    pub fn missing_fonts(&self) -> Vec<&FontPin> {
        self.assets.missing_fonts()
    }
    pub fn restore_fonts(
        &mut self,
        store: &mut reprise_font::FontStore,
    ) -> Vec<reprise_font::FaceId> {
        self.assets.restore_fonts(store)
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }
    pub fn package(&self) -> &Package {
        &self.package
    }
    pub fn editable_document(&self) -> Result<&Document, FormatError> {
        if self.read_only {
            Err(FormatError::ReadOnly)
        } else {
            Ok(&self.document)
        }
    }
    pub fn view(&self) -> Result<DocumentAt, reprise_doc::VersionError> {
        self.document.at(&self.document.revision())
    }
    pub fn into_document(self) -> Result<Document, FormatError> {
        if self.read_only {
            Err(FormatError::ReadOnly)
        } else {
            Ok(self.document)
        }
    }
    /// Save edits and refresh fonts used by their current layout atomically.
    pub fn save_with_layout(
        &self,
        mode: PersistenceMode,
        layout: &reprise_layout::LayoutSnapshot,
        fonts: &reprise_font::FontStore,
    ) -> Result<Vec<u8>, FormatError> {
        if self.read_only {
            return Err(FormatError::ReadOnly);
        }
        let mut package = self.package.clone();
        package.revision = self.document.revision();
        package
            .container
            .sections
            .insert(ids::DOCUMENT, Section::raw(self.document.try_export(mode)?));
        package.container.sections.remove(&ids::CACHE);
        package.embed_layout_fonts(layout, fonts)?;
        package.save()
    }

    pub fn save(&self, mode: PersistenceMode) -> Result<Vec<u8>, FormatError> {
        if self.read_only {
            return Err(FormatError::ReadOnly);
        }
        let mut package = self.package.clone();
        package.revision = self.document.revision();
        package
            .container
            .sections
            .insert(ids::DOCUMENT, Section::raw(self.document.try_export(mode)?));
        if let Some(section) = package.container.sections.get(&ids::CACHE) {
            let stale = !DerivedCache::decode(&section.bytes, package.limits)
                .is_ok_and(|cache| cache.tags.revision == self.document.revision());
            if stale {
                package.container.sections.remove(&ids::CACHE);
            }
        }
        package.save()
    }
}
