use std::collections::{BTreeMap, BTreeSet};

use reprise_diag::Note;
use reprise_doc::fragment::{CopyBlock, Fragment, MAX_FRAGMENT_BYTES};
use reprise_doc::relation::{CopyAction, CopySet, plan_copy};
use reprise_doc::{Document, SchemaRegistry};
use reprise_edit::{Editor, Navigator, Pasted, Selection};
use reprise_font::{Face, FaceId, FontStore};
use reprise_format::content_hash;
use reprise_layout::LayoutSnapshot;
use serde::{Deserialize, Serialize};

use crate::{ClipboardError, codes};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceKind {
    Font(FaceId),
    Asset,
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
    /// Sorted by SHA-256. Hosts attach content-addressed assets explicitly because
    /// the authored engine currently has no image/asset usage graph.
    pub resources: BTreeMap<String, EmbeddedResource>,
    pub notes: Vec<Note>,
}

impl NativeFragment {
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
            if *key != resource.hash || content_hash(&resource.bytes) != resource.hash {
                return Err(ClipboardError::ResourceHash);
            }
            if let ResourceKind::Font(pin) = &resource.kind {
                let face = Face::from_bytes(resource.bytes.clone())
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
        let mut resources = self.resources.clone();
        resources.insert(
            hash.clone(),
            EmbeddedResource {
                kind: ResourceKind::Asset,
                hash: hash.clone(),
                bytes,
            },
        );
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
            .map_err(|e| ClipboardError::Invalid(e.to_string()))?;
        result.notes.extend(self.notes.clone());
        Ok(result)
    }

    pub fn install_fonts(&self, fonts: &mut FontStore) -> Result<(), ClipboardError> {
        self.validate()?;
        for resource in self.resources.values() {
            if matches!(resource.kind, ResourceKind::Font(_)) {
                fonts.add(
                    Face::from_bytes(resource.bytes.clone())
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
            resources.insert(
                hash.clone(),
                EmbeddedResource {
                    kind: ResourceKind::Font(pin),
                    hash,
                    bytes,
                },
            );
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
    let blocks: Vec<_> = ranges
        .into_iter()
        .map(|r| CopyBlock {
            node: r.node,
            bytes: Some(r.bytes),
        })
        .collect();
    copy_blocks(doc, source, &blocks, schemas, Some(layout), fonts)
}
