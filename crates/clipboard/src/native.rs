use std::collections::{BTreeMap, BTreeSet};

use reprise_diag::Note;
use reprise_doc::fragment::{CopyBlock, Fragment, MAX_FRAGMENT_BYTES};
use reprise_doc::relation::{CopyAction, CopySet, plan_copy};
use reprise_doc::{Document, SchemaRegistry};
use reprise_edit::{Editor, Navigator, Pasted, Selection};
use reprise_font::{Face, FaceId, FontDeclaration, FontStore};
use reprise_format::content_hash;
use reprise_layout::LayoutSnapshot;
use serde::{Deserialize, Serialize};

use crate::{ClipboardError, codes};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceKind {
    /// A face and the declaration it was registered with: the family alias,
    /// descriptors and collection index are part of its identity (21), so
    /// the bytes alone can't reproduce it.
    Font {
        pin: FaceId,
        declaration: FontDeclaration,
    },
    Asset,
}

/// Where a resource sits in [`NativeFragment::resources`]: its content hash,
/// and for a font also its face, since one file (a collection, or one file
/// declared under several families) can carry several faces.
pub fn resource_key(resource: &EmbeddedResource) -> String {
    match &resource.kind {
        ResourceKind::Font { pin, declaration } => format!(
            "{}#{}#{}#{}",
            resource.hash, pin.family, pin.hash, declaration.face_index
        ),
        ResourceKind::Asset => resource.hash.clone(),
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddedResource {
    pub kind: ResourceKind,
    pub hash: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeFragment {
    pub fragment: Fragment,
    /// Sorted resource keys. Hosts supply image bytes through `attach_images`
    /// and font bytes through the copy helpers.
    pub resources: BTreeMap<String, EmbeddedResource>,
    pub notes: Vec<Note>,
}

impl NativeFragment {
    /// Attach the selected image blocks' bytes through the native asset path.
    /// Missing bytes produce a warning; their authored references remain intact.
    pub fn attach_images(
        &mut self,
        store: &reprise_display::AssetStore,
    ) -> Result<(), ClipboardError> {
        let hashes: BTreeSet<_> = self
            .fragment
            .blocks
            .iter()
            .filter_map(|b| b.image.as_deref())
            .filter_map(|raw| {
                reprise_doc::image::ImageData::parse(raw)
                    .ok()
                    .map(|i| i.asset)
            })
            .collect();
        let mut candidate = self.clone();
        for hash in hashes {
            if candidate.resources.values().any(|r| r.hash == hash) {
                continue;
            }
            if let Some(bytes) = store.get(&hash) {
                candidate.attach_asset(bytes.to_vec())?;
            } else {
                let message = format!("image asset {hash} is missing");
                if !candidate
                    .notes
                    .iter()
                    .any(|n| n.code == codes::RESOURCE_MISSING && n.message == message)
                {
                    candidate
                        .notes
                        .push(Note::warning(codes::RESOURCE_MISSING, message));
                }
            }
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    pub fn install_assets(
        &self,
        store: &mut reprise_display::AssetStore,
    ) -> Result<(), ClipboardError> {
        self.validate()?;
        let mut candidate = store.clone();
        for resource in self.resources.values() {
            if resource.kind == ResourceKind::Asset {
                candidate
                    .insert(resource.bytes.as_slice())
                    .map_err(|e| ClipboardError::Invalid(e.into()))?;
            }
        }
        *store = candidate;
        Ok(())
    }
    pub fn validate(&self) -> Result<(), ClipboardError> {
        self.fragment.validate()?;
        if self.resources.len() > 1024 {
            return Err(ClipboardError::Limit("resources"));
        }
        let mut total = 0usize;
        for (key, resource) in &self.resources {
            total = total.saturating_add(resource.bytes.len());
            if total > MAX_FRAGMENT_BYTES {
                return Err(ClipboardError::Limit("resource bytes"));
            }
            if *key != resource_key(resource) || content_hash(&resource.bytes) != resource.hash {
                return Err(ClipboardError::ResourceHash);
            }
            if let ResourceKind::Font { pin, declaration } = &resource.kind {
                let face = Face::declared(resource.bytes.clone(), Some(declaration.clone()))
                    .map_err(|e| ClipboardError::Invalid(e.to_string()))?;
                if face.id() != pin {
                    return Err(ClipboardError::ResourceHash);
                }
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, ClipboardError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| ClipboardError::Invalid(e.to_string()))?;
        if bytes.len() > 96 << 20 {
            return Err(ClipboardError::Limit("encoded bytes"));
        }
        Ok(bytes)
    }

    /// Serde's recursion guard remains enabled; the byte cap is checked before parsing.
    pub fn decode(bytes: &[u8]) -> Result<Self, ClipboardError> {
        if bytes.len() > 96 << 20 {
            return Err(ClipboardError::Limit("encoded bytes"));
        }
        let fragment: Self =
            serde_json::from_slice(bytes).map_err(|e| ClipboardError::Invalid(e.to_string()))?;
        fragment.validate()?;
        Ok(fragment)
    }

    pub fn attach_asset(&mut self, bytes: Vec<u8>) -> Result<String, ClipboardError> {
        let hash = content_hash(&bytes);
        // Bytes the fragment already carries, as a font or an asset, aren't
        // duplicated: the existing resource's key is returned.
        if let Some((key, _)) = self.resources.iter().find(|(_, r)| r.hash == hash) {
            return Ok(key.clone());
        }
        let mut resources = self.resources.clone();
        resources.entry(hash.clone()).or_insert(EmbeddedResource {
            kind: ResourceKind::Asset,
            hash: hash.clone(),
            bytes,
        });
        let candidate = Self {
            fragment: self.fragment.clone(),
            resources,
            notes: self.notes.clone(),
        };
        candidate.validate()?;
        self.resources = candidate.resources;
        Ok(hash)
    }

    /// Resources are returned with the paste rather than stored in the Loro document.
    /// The host owns FontStore and its content-addressed asset store.
    pub fn paste(
        &self,
        editor: &mut Editor,
        at: Option<(reprise_doc::NodeId, usize)>,
        target: &str,
    ) -> Result<Pasted, ClipboardError> {
        self.validate()?;
        let mut result = editor
            .paste(&self.fragment, at, target)
            .map_err(ClipboardError::Edit)?;
        result.notes.extend(self.notes.clone());
        Ok(result)
    }

    pub fn install_fonts(&self, fonts: &mut FontStore) -> Result<(), ClipboardError> {
        self.validate()?;
        for resource in self.resources.values() {
            if let ResourceKind::Font { declaration, .. } = &resource.kind {
                fonts.add(
                    Face::declared(resource.bytes.clone(), Some(declaration.clone()))
                        .map_err(|e| ClipboardError::Invalid(e.to_string()))?,
                );
            }
        }
        Ok(())
    }
}

fn bundle(
    doc: &Document,
    fragment: Fragment,
    schemas: &SchemaRegistry,
    layout: Option<&LayoutSnapshot>,
    fonts: Option<&FontStore>,
) -> NativeFragment {
    let copied = CopySet {
        nodes: fragment.blocks.iter().map(|b| b.id).collect(),
        ranges: fragment.ranges.iter().map(|r| r.id).collect(),
    };
    let mut notes = fragment.notes.clone();
    for entry in plan_copy(schemas, &doc.relations(), &copied) {
        if matches!(entry.action, CopyAction::Drop(_)) {
            notes.push(Note::error(
                codes::RELATION_DROPPED,
                format!("copy policy omitted relation {}", entry.relation),
            ));
        }
    }
    if doc.relations().iter().any(|(_, r)| r.is_err()) {
        notes.push(Note::error(
            codes::RELATION_DROPPED,
            "unreadable relations cannot be assigned to the selection",
        ));
    }
    let mut resources = BTreeMap::new();
    let mut pins = BTreeSet::new();
    let mut families = BTreeSet::new();
    if let Some(layout) = layout.filter(|l| l.revision == doc.revision()) {
        for block in &layout.blocks {
            if copied.nodes.contains(&block.node) {
                for run in block.lines.iter().flat_map(|l| &l.runs) {
                    pins.insert(run.face.clone());
                }
            }
        }
    } else {
        for block in &fragment.blocks {
            if block.kind == reprise_doc::BlockKind::Image {
                continue;
            }
            if let Ok(style) = doc.computed_style(block.id) {
                families.insert(style.family);
            }
        }
    }
    for family in families {
        if let Some(face) = fonts.and_then(|f| f.by_family(&family)) {
            pins.insert(face.id().clone());
        } else {
            notes.push(Note::warning(
                codes::RESOURCE_MISSING,
                format!("host must supply font family {family}"),
            ));
        }
    }
    for pin in pins {
        if let Some(face) = fonts.and_then(|fonts| fonts.get(&pin).ok()) {
            let bytes = face.data().to_vec();
            let hash = content_hash(&bytes);
            let resource = EmbeddedResource {
                kind: ResourceKind::Font {
                    pin,
                    declaration: face.declaration().clone(),
                },
                hash,
                bytes,
            };
            resources.insert(resource_key(&resource), resource);
        } else {
            notes.push(Note::warning(
                codes::RESOURCE_MISSING,
                "host must supply a used pinned font",
            ));
        }
    }
    NativeFragment {
        fragment,
        resources,
        notes,
    }
}

pub fn copy_blocks(
    doc: &Document,
    source: &str,
    selection: &[CopyBlock],
    schemas: &SchemaRegistry,
    layout: Option<&LayoutSnapshot>,
    fonts: Option<&FontStore>,
) -> Result<NativeFragment, ClipboardError> {
    let fragment = doc.copy_fragment(source, selection, schemas)?;
    Ok(bundle(doc, fragment, schemas, layout, fonts))
}

pub fn copy_all(
    doc: &Document,
    source: &str,
    schemas: &SchemaRegistry,
    layout: Option<&LayoutSnapshot>,
    fonts: Option<&FontStore>,
) -> Result<NativeFragment, ClipboardError> {
    let fragment = doc.copy_all_fragment(source, schemas)?;
    Ok(bundle(doc, fragment, schemas, layout, fonts))
}

pub fn copy_selection(
    doc: &Document,
    source: &str,
    selection: &Selection,
    layout: &LayoutSnapshot,
    schemas: &SchemaRegistry,
    fonts: Option<&FontStore>,
) -> Result<NativeFragment, ClipboardError> {
    if layout.revision != doc.revision() {
        return Err(ClipboardError::Invalid(
            "selection snapshot is stale".into(),
        ));
    }
    let nav = Navigator::semantic(layout, doc);
    let ranges = nav.selection_ranges(selection);
    if ranges.is_empty() {
        return Err(ClipboardError::Invalid(
            "selection endpoints are absent".into(),
        ));
    }
    if ranges.len() == 1 && ranges.first().is_some_and(|r| r.bytes.is_empty()) {
        return copy_blocks(doc, source, &[], schemas, Some(layout), fonts);
    }
    let selected: BTreeMap<_, _> = ranges.iter().map(|r| (r.node, r.bytes.clone())).collect();
    let mut candidates = BTreeSet::new();
    let mut in_table = BTreeSet::new();
    for range in &ranges {
        let mut up = Some(range.node);
        for _ in 0..=reprise_doc::fragment::MAX_FRAGMENT_DEPTH {
            let Some(node) = up else { break };
            if matches!(
                doc.table_role(node),
                Ok(Some(reprise_doc::TableRole::Table(_)))
            ) {
                candidates.insert(node);
                in_table.insert(range.node);
            }
            up = doc.parent_of(node).flatten();
        }
        if up.is_some() {
            return Err(ClipboardError::Limit("selection ancestors"));
        }
    }
    let mut promoted = BTreeMap::new();
    let mut covered = BTreeSet::new();
    for table in candidates {
        if covered.contains(&table) {
            continue;
        }
        let mut members = Vec::new();
        let mut todo = vec![(table, 0usize)];
        let mut full = true;
        while let Some((node, depth)) = todo.pop() {
            if depth > reprise_doc::fragment::MAX_FRAGMENT_DEPTH
                || members.len() >= reprise_doc::fragment::MAX_FRAGMENT_BLOCKS
            {
                return Err(ClipboardError::Limit("selection table"));
            }
            members.push(node);
            if matches!(doc.table_role(node), Ok(None)) {
                let block = doc
                    .block(node)
                    .map_err(|e| ClipboardError::Invalid(e.to_string()))?;
                if selected.get(&node) != Some(&(0..block.text.len())) {
                    full = false;
                }
            }
            todo.extend(
                doc.children(Some(node))
                    .into_iter()
                    .rev()
                    .map(|n| (n, depth + 1)),
            );
        }
        if full {
            for node in members {
                covered.insert(node);
                if selected.contains_key(&node) {
                    promoted.insert(node, table);
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut blocks = Vec::new();
    let flattened = in_table.iter().any(|n| !promoted.contains_key(n));
    for range in ranges {
        if let Some(&table) = promoted.get(&range.node) {
            if seen.insert(table) {
                blocks.push(CopyBlock {
                    node: table,
                    bytes: None,
                });
            }
        } else {
            blocks.push(CopyBlock {
                node: range.node,
                bytes: Some(range.bytes),
            });
        }
    }
    let mut fragment = copy_blocks(doc, source, &blocks, schemas, Some(layout), fonts)?;
    if flattened {
        fragment.notes.push(Note::warning(
            codes::SELECTION_TABLE,
            "partial table selections become independent paragraph blocks",
        ));
    }
    Ok(fragment)
}
