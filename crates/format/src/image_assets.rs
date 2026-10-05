//! Embed live authored image references without disturbing other package assets.
use crate::{
    Asset, AssetAvailability, AssetKind, AssetSource, DocumentId, FormatError, Package, ids,
};
use reprise_display::AssetStore;
use reprise_doc::{BlockKind, Document, PersistenceMode};
use std::collections::BTreeSet;

impl Package {
    /// Embed every live authored image, including an image omitted by a page
    /// limit. Missing bytes stay declared and are returned in SHA-256 order.
    pub fn embed_document_images(
        &mut self,
        doc: &Document,
        store: &AssetStore,
    ) -> Result<Vec<String>, FormatError> {
        self.writable()?;
        if doc.revision() != self.revision {
            return Err(FormatError::Metadata(
                "document revision differs from package".into(),
            ));
        }
        let used: BTreeSet<_> = doc
            .document_order()
            .into_iter()
            .filter(|&node| doc.kind_of(node) == Some(BlockKind::Image))
            .filter_map(|node| doc.image(node).ok().map(|i| i.asset))
            .collect();
        if used.len() > self.limits.entries {
            return Err(FormatError::Limit("used images"));
        }
        let (pins, mut assets) = crate::assets::metadata(&self.container, self.limits)?;
        let mut candidate = self.container.clone();
        let mut missing = Vec::new();
        for hash in used {
            let existing = assets
                .iter()
                .position(|a| a.kind == AssetKind::Image && a.hash == hash);
            let reserved: BTreeSet<_> = assets
                .iter()
                .filter_map(|a| match a.source {
                    AssetSource::Bundled { section } => Some(section),
                    _ => None,
                })
                .collect();
            let old_section = existing
                .and_then(|i| assets.get(i))
                .and_then(|a| match a.source {
                    AssetSource::Bundled { section } => Some(section),
                    _ => None,
                });
            let section = old_section
                .or_else(|| {
                    (ids::FIRST_ASSET..=ids::LAST_ASSET)
                        .find(|s| !candidate.sections.contains_key(s) && !reserved.contains(s))
                })
                .ok_or(FormatError::Limit("image sections"))?;
            let mut asset = if let Some(index) = existing {
                assets.remove(index)
            } else {
                let mut id = format!("image-{hash}");
                for _ in 0..=self.limits.entries {
                    if !assets.iter().any(|a| a.id == id) {
                        break;
                    }
                    id.push('-');
                }
                if assets.iter().any(|a| a.id == id) {
                    return Err(FormatError::Limit("image names"));
                }
                Asset {
                    id,
                    kind: AssetKind::Image,
                    hash: hash.clone(),
                    source: AssetSource::Bundled { section },
                    extra: Default::default(),
                }
            };
            if let Some(bytes) = store.get(&hash) {
                if bytes.len() > self.limits.section_bytes {
                    return Err(FormatError::Limit("image bytes"));
                }
                candidate
                    .sections
                    .insert(section, crate::Section::raw(bytes.to_vec()));
                asset.source = AssetSource::Bundled { section };
            } else if old_section
                .and_then(|s| candidate.sections.get(&s))
                .is_none_or(|s| crate::content_hash(&s.bytes) != hash)
            {
                missing.push(hash);
            }
            assets.push(asset);
        }
        assets.sort_by(|a, b| a.id.cmp(&b.id));
        candidate.sections.insert(
            ids::FONTS,
            crate::Section::raw(crate::json::encode(&pins, self.limits)?),
        );
        candidate.sections.insert(
            ids::ASSETS,
            crate::Section::raw(crate::json::encode(&assets, self.limits)?),
        );
        crate::assets::metadata(&candidate, self.limits)?;
        candidate.encoded_len(self.limits)?;
        self.container = candidate;
        Ok(missing)
    }

    pub fn new_with_resources(
        doc: &Document,
        id: DocumentId,
        mode: PersistenceMode,
        layout: &reprise_layout::LayoutSnapshot,
        fonts: &reprise_font::FontStore,
        assets: &AssetStore,
    ) -> Result<Self, FormatError> {
        let mut package = Self::new_with_layout(doc, id, mode, layout, fonts)?;
        package.embed_document_images(doc, assets)?;
        Ok(package)
    }

    pub fn open_with_resources(
        bytes: &[u8],
        peer: u64,
        limits: crate::Limits,
        migrations: &crate::MigrationRegistry,
        fonts: &mut reprise_font::FontStore,
        assets: &mut AssetStore,
    ) -> Result<crate::OpenedFile, FormatError> {
        let mut opened = Self::open_with_fonts(bytes, peer, limits, migrations, fonts)?;
        opened.notes.extend(opened.assets.restore_images(assets));
        Ok(opened)
    }
}

impl AssetAvailability {
    /// Only verified bundled image bytes enter the host store; external needs
    /// remain for the host to resolve without any engine I/O.
    pub fn restore_images(&self, store: &mut AssetStore) -> Vec<reprise_diag::Note> {
        let mut notes = Vec::new();
        for need in &self.needed {
            if need.kind != AssetKind::Image {
                continue;
            }
            if let Some(bytes) = self.bundled.get(&need.id)
                && let Err(error) = store.insert(bytes.as_slice())
            {
                notes.push(reprise_diag::Note::warning(
                    crate::codes::ASSET_MISSING,
                    error,
                ));
            }
        }
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExternalLocation, Section};
    use reprise_doc::image::ImageData;
    use std::collections::BTreeMap;

    #[test]
    fn damaged_bundles_are_missing_and_repaired_without_losing_other_metadata() {
        let bytes = b"host image bytes";
        let mut store = AssetStore::default();
        let hash = store.insert(bytes.as_slice()).unwrap();
        let doc = Document::new(1).unwrap();
        doc.append_image("", &ImageData::new(&hash), "alt").unwrap();
        doc.commit();
        let mut package =
            Package::new(&doc, DocumentId([1; 16]), PersistenceMode::History).unwrap();
        let image = Asset {
            id: "existing-image".into(),
            kind: AssetKind::Image,
            hash: hash.clone(),
            source: AssetSource::Bundled {
                section: ids::FIRST_ASSET,
            },
            extra: BTreeMap::from([("future".into(), serde_json::json!({"keep": [1, 2]}))]),
        };
        let external = Asset {
            id: "external-other".into(),
            kind: AssetKind::Other,
            hash: "1".repeat(64),
            source: AssetSource::External {
                location: ExternalLocation::Url("https://example.invalid/asset".into()),
            },
            extra: BTreeMap::from([("opaque".into(), serde_json::json!(true))]),
        };
        package
            .set_assets(
                vec![image.clone(), external.clone()],
                BTreeMap::from([(ids::FIRST_ASSET, b"wrong bytes".to_vec())]),
            )
            .unwrap();
        let opaque = Section {
            flags: 2,
            codec: 77,
            bytes: b"unrecognized".to_vec(),
        };
        package.container.sections.insert(99, opaque.clone());
        assert_eq!(
            package
                .embed_document_images(&doc, &AssetStore::default())
                .unwrap(),
            std::slice::from_ref(&hash)
        );
        assert!(
            package
                .assets()
                .unwrap()
                .missing
                .iter()
                .any(|n| n.hash == hash)
        );
        assert!(
            package
                .embed_document_images(&doc, &store)
                .unwrap()
                .is_empty()
        );
        let (_, assets) = crate::assets::metadata(&package.container, package.limits).unwrap();
        assert!(assets.contains(&image));
        assert!(assets.contains(&external));
        assert_eq!(package.container.sections.get(&99), Some(&opaque));
        assert_eq!(
            package.assets().unwrap().bundled.get(&image.id).unwrap(),
            bytes
        );
        let before = package.save().unwrap();
        assert!(
            package
                .embed_document_images(&doc, &AssetStore::default())
                .unwrap()
                .is_empty()
        );
        assert_eq!(before, package.save().unwrap());
    }

    #[test]
    fn absent_bundles_reserve_distinct_sections_and_external_images_stay_external() {
        let doc = Document::new(1).unwrap();
        for digit in ["1", "2", "3"] {
            doc.append_image("", &ImageData::new(digit.repeat(64)), "alt")
                .unwrap();
        }
        doc.commit();
        let mut package =
            Package::new(&doc, DocumentId([2; 16]), PersistenceMode::History).unwrap();
        let external = Asset {
            id: "external-image".into(),
            kind: AssetKind::Image,
            hash: "1".repeat(64),
            source: AssetSource::External {
                location: ExternalLocation::Path("host/image.png".into()),
            },
            extra: Default::default(),
        };
        package
            .set_assets(vec![external.clone()], BTreeMap::new())
            .unwrap();
        let missing = package
            .embed_document_images(&doc, &AssetStore::default())
            .unwrap();
        assert_eq!(missing, ["1".repeat(64), "2".repeat(64), "3".repeat(64)]);
        let (_, assets) = crate::assets::metadata(&package.container, package.limits).unwrap();
        assert!(assets.contains(&external));
        let sections: BTreeSet<_> = assets
            .iter()
            .filter_map(|a| match a.source {
                AssetSource::Bundled { section } => Some(section),
                _ => None,
            })
            .collect();
        assert_eq!(sections.len(), 2);
        assert_eq!(package.assets().unwrap().missing.len(), 3);
    }
}
