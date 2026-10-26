//! The sections of a document as HTML (sections §15), and back.
//!
//! Each section is a `<section data-reprise-section='{v, props, bindings}'>`
//! with the CSS a viewer needs (`columns`, `column-gap`, `break-before`). A
//! story is written once per document under a document-local label: as visible
//! `<header>` or `<footer>` content in the first section that binds it as the
//! default variant of that slot, else in a `<template data-reprise-story>`. A
//! story no section binds is an orphan, and is written the same inert way
//! after the last section, so authored text is never left out. A
//! binding keeps its three states, absent (same as previous), `""` (none) and
//! the label. A page or column break is `break-before: page | column` on its
//! block; the geometry mode record is `data-reprise-geometry` on the root
//! element. The JSON is what an import reads back; the CSS is for viewers.

use std::collections::{BTreeMap, BTreeSet};

use reprise_doc::sections::{BreakKind, SectionId, SectionProperties, SectionView, binding_key};
use reprise_doc::{Document, Domain, NodeId};
use reprise_geom::Length;
use serde_json::{Map, Value, json};

use crate::export::{escape, points};

/// The attribute that carries a section's stored form.
pub(crate) const SECTION_ATTR: &str = "data-reprise-section";
/// The attribute that names a story, on its `header`, `footer` or `template`.
pub(crate) const STORY_ATTR: &str = "data-reprise-story";
/// The attribute that carries the geometry-mode record, on the root element.
pub(crate) const GEOMETRY_ATTR: &str = "data-reprise-geometry";
/// The version of the JSON in [`SECTION_ATTR`].
pub(crate) const VERSION: u64 = 1;

/// How a story is written: visible in a section, or inert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    Header,
    Footer,
    Template,
}

impl Shown {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Header => "header",
            Self::Footer => "footer",
            Self::Template => "template",
        }
    }
}

/// A story the export writes, in the section it is first bound by.
pub(crate) struct Home {
    pub label: String,
    pub story: NodeId,
    pub shown: Shown,
}

/// What a document's sections come to as HTML.
pub(crate) struct Plan {
    pub views: Vec<SectionView>,
    /// Stories by label, in the order the sections first refer to them.
    labels: BTreeMap<NodeId, String>,
    /// The stories written in each section.
    homes: Vec<Vec<Home>>,
    /// Page and column breaks on top-level body blocks.
    pub breaks: BTreeMap<NodeId, BreakKind>,
    pub geometry: Option<SectionProperties>,
    /// Where each section starts among the body's top-level blocks.
    starts: Vec<usize>,
    /// The stories no section binds, each in an inert template after the last
    /// section. They get the labels after the bound ones.
    pub orphans: Vec<Home>,
    /// Sections whose columns are not equal, which CSS cannot say.
    pub unequal_columns: bool,
    /// Bindings to something that is not a live story, kept as written.
    pub raw_bindings: bool,
}

/// The plan, or `None` for a document with nothing of sections to say: it is
/// then written exactly as it always was.
pub(crate) fn plan(doc: &Document) -> Option<Plan> {
    let starts = doc.effective_starts();
    let views = doc.section_table();
    let first_keys = views.first().is_some_and(|v| !v.own.is_empty());
    let geometry = doc.geometry_mode_record();
    if starts.is_empty() && !first_keys && geometry.is_none() && doc.stories().is_empty() {
        return None;
    }
    let live: BTreeSet<NodeId> = doc.stories().into_iter().collect();
    let top = doc.top(Domain::Body);
    let index: BTreeMap<NodeId, usize> = top.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut labels = BTreeMap::new();
    let mut order = Vec::new();
    let mut homes: Vec<Vec<Home>> = Vec::new();
    let mut raw_bindings = false;
    let mut unequal_columns = false;
    for view in &views {
        let mut here = Vec::new();
        for slot in ["header", "footer", "background"] {
            for variant in ["default", "first", "even"] {
                let key = format!("region.{slot}.{variant}");
                let Some(value) = view.own.get(&key) else {
                    continue;
                };
                let story = value
                    .as_str()
                    .and_then(NodeId::parse)
                    .filter(|s| live.contains(s));
                let Some(story) = story else {
                    raw_bindings |= value.as_str().is_some_and(|s| !s.is_empty());
                    continue;
                };
                if labels.contains_key(&story) {
                    continue;
                }
                let label = format!("s{}", order.len());
                labels.insert(story, label.clone());
                order.push(story);
                here.push(Home {
                    label,
                    story,
                    shown: match (slot, variant) {
                        ("header", "default") => Shown::Header,
                        ("footer", "default") => Shown::Footer,
                        _ => Shown::Template,
                    },
                });
            }
        }
        unequal_columns |= view
            .own
            .get("columns.widths")
            .and_then(Value::as_str)
            .is_some_and(|w| !w.is_empty());
        homes.push(here);
    }
    let breaks = starts
        .iter()
        .filter_map(|r| match (r.id, r.kind) {
            (SectionId::Block(node), Some(kind @ (BreakKind::Page | BreakKind::Column))) => {
                Some((node, kind))
            }
            _ => None,
        })
        .collect();
    let starts: Vec<usize> = views
        .iter()
        .map(|v| {
            v.first_block
                .filter(|_| v.id != SectionId::First)
                .and_then(|n| index.get(&n).copied())
                .unwrap_or(0)
        })
        .collect();
    let orphans = doc
        .stories()
        .into_iter()
        .filter(|story| !labels.contains_key(story))
        .enumerate()
        .map(|(i, story)| Home {
            label: format!("s{}", order.len().saturating_add(i)),
            story,
            shown: Shown::Template,
        })
        .collect();
    Some(Plan {
        orphans,
        views,
        labels,
        homes,
        breaks,
        geometry,
        starts,
        unequal_columns,
        raw_bindings,
    })
}

impl Plan {
    /// The stories written in section `i`.
    pub fn homes(&self, i: usize) -> &[Home] {
        self.homes.get(i).map_or(&[], Vec::as_slice)
    }

    /// The section a body block at position `at` among the top-level blocks is in.
    pub fn section_of(&self, at: usize) -> usize {
        // Section 0 is `first`; later sections start where they say.
        (1..self.starts.len())
            .rev()
            .find(|&i| self.starts.get(i).is_some_and(|s| *s <= at))
            .unwrap_or(0)
    }

    /// The root element's geometry attribute, with its leading space.
    pub fn geometry_attr(&self) -> String {
        match &self.geometry {
            Some(record) => {
                let object: Map<String, Value> =
                    record.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                format!(
                    " {GEOMETRY_ATTR}=\"{}\"",
                    escape(&Value::Object(object).to_string())
                )
            }
            None => String::new(),
        }
    }

    /// Section `i`'s opening tag.
    pub fn open(&self, i: usize) -> String {
        let Some(view) = self.views.get(i) else {
            return "<section>".into();
        };
        let mut props = Map::new();
        let mut bindings = Map::new();
        for (key, value) in &view.own {
            if key == "break" {
                continue;
            }
            if binding_key(key) && value.is_string() {
                bindings.insert(key.clone(), self.binding(value));
            } else {
                props.insert(key.clone(), value.clone());
            }
        }
        let mut stored = Map::new();
        stored.insert("v".into(), json!(VERSION));
        if view.id == SectionId::First {
            stored.insert("first".into(), json!(true));
        }
        stored.insert("props".into(), Value::Object(props));
        stored.insert("bindings".into(), Value::Object(bindings));
        let mut css = Vec::new();
        let new_page = view.id != SectionId::First
            && view.own.get("start").and_then(Value::as_str) != Some("continuous");
        if new_page {
            css.push("break-before: page".to_owned());
        }
        let number = |key: &str| view.own.get(key).and_then(Value::as_i64);
        let length = |v: i64| i32::try_from(v).ok().map(Length);
        if let Some(count) = number("columns.count").filter(|n| *n > 1) {
            css.push(format!("columns: {}", count.min(64)));
            let gap = number("columns.gap")
                .and_then(length)
                .map_or_else(|| "18".to_owned(), points);
            css.push(format!("column-gap: {gap}pt"));
        }
        let style = if css.is_empty() {
            String::new()
        } else {
            format!(" style=\"{}\"", escape(&css.join("; ")))
        };
        format!(
            "<section {SECTION_ATTR}=\"{}\"{style}>",
            escape(&Value::Object(stored).to_string())
        )
    }

    /// A binding's stored form: `""` for none, the story's label, or the raw
    /// value of one that names no live story.
    fn binding(&self, value: &Value) -> Value {
        match value.as_str() {
            Some("") => json!(""),
            Some(raw) => match NodeId::parse(raw).and_then(|s| self.labels.get(&s)) {
                Some(label) => json!(label),
                None => json!({ "raw": raw }),
            },
            None => value.clone(),
        }
    }
}

/// A section's stored form as an import reads it.
pub(crate) struct Stored {
    pub first: bool,
    pub props: SectionProperties,
    pub bindings: BTreeMap<String, Bound>,
}

/// A binding as an import reads it.
pub(crate) enum Bound {
    None,
    Label(String),
    Raw(String),
}

/// Reads [`SECTION_ATTR`]. `None` if it is not a version-1 section form; the
/// caller then reads the element as the ordinary markup it is.
pub(crate) fn read(raw: &str) -> Option<Stored> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    if object.get("v").and_then(Value::as_u64) != Some(VERSION) {
        return None;
    }
    let scalar = |v: &Value| matches!(v, Value::String(_) | Value::Bool(_)) || v.as_i64().is_some();
    let props = object
        .get("props")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(k, v)| scalar(v) && !binding_key(k) && k.as_str() != "break")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    let bindings = object
        .get("bindings")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(k, _)| binding_key(k))
                .filter_map(|(k, v)| {
                    let bound = match v {
                        Value::String(s) if s.is_empty() => Bound::None,
                        Value::String(s) => Bound::Label(s.clone()),
                        Value::Object(o) => Bound::Raw(o.get("raw")?.as_str()?.to_owned()),
                        _ => return None,
                    };
                    Some((k.clone(), bound))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Stored {
        first: object.get("first").and_then(Value::as_bool) == Some(true),
        props,
        bindings,
    })
}

/// Reads [`GEOMETRY_ATTR`]: the record's scalar keys, as written, and how many
/// keys held something else, which the record can't hold.
pub(crate) fn read_geometry(raw: &str) -> Option<(SectionProperties, usize)> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    let record: SectionProperties = object
        .iter()
        .filter(|(_, v)| matches!(v, Value::String(_) | Value::Bool(_)) || v.as_i64().is_some())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let left_out = object.len().saturating_sub(record.len());
    Some((record, left_out))
}
