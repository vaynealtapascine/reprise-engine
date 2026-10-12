//! Inline objects at the clipboard boundary (`docs/inline.md`): what each
//! object reads as in plain text, and how it is written to and read from
//! HTML. Everything here is a function of the document and an optional
//! layout; nothing is stored.
use std::collections::BTreeMap;
use std::ops::Range;

use reprise_doc::inline::{FIELD, IMAGE};
use reprise_doc::text::OBJECT;
use reprise_doc::{Document, InlineAlign, InlineId, InlineObject, InlineSize, NodeId};
use reprise_layout::LayoutSnapshot;

use crate::export::{append, escape, points};
use crate::{ClipboardError, html};

/// Holds an object's full stored form as JSON: every parameter, the
/// alignment and sizes in any unit. The visible markup is only a fallback
/// for HTML written by other tools.
pub(crate) const JSON_ATTR: &str = "data-reprise-inline";
/// A field's function, on its `span`.
pub(crate) const FIELD_ATTR: &str = "data-field";
/// An image's asset, as its SHA-256.
pub(crate) const ASSET_ATTR: &str = "data-reprise-asset";

/// What a field shows when no pagination has resolved it.
pub(crate) const UNRESOLVED: &str = "?";

/// A live object of a paragraph, as export reads it.
pub(crate) struct Placed {
    /// Byte offset of its U+FFFC in the paragraph's text.
    pub at: usize,
    pub object: InlineObject,
    /// The layout's resolved text when the layout places the object, else
    /// [`fallback`].
    pub text: String,
}

/// The live objects of `node`, in text order.
pub(crate) fn placed(
    doc: &Document,
    node: NodeId,
    layout: Option<&LayoutSnapshot>,
) -> Result<Vec<Placed>, ClipboardError> {
    if !doc.has_inline_objects() {
        return Ok(Vec::new());
    }
    let entries = doc
        .inline_objects(node)
        .map_err(|e| ClipboardError::Invalid(e.to_string()))?;
    let resolved: BTreeMap<InlineId, String> = layout
        .and_then(|l| l.blocks.iter().find(|b| b.node == node))
        .map(|b| {
            b.lines
                .iter()
                .flat_map(|l| &l.atoms)
                .map(|a| (a.id, a.text()))
                .collect()
        })
        .unwrap_or_default();
    Ok(entries
        .into_iter()
        .map(|e| {
            let text = resolved
                .get(&e.id)
                .cloned()
                .unwrap_or_else(|| fallback(&e.object));
            Placed {
                at: e.at,
                object: e.object,
                text,
            }
        })
        .collect())
}

/// What an object reads as without a layout that places it: an image's alt
/// text, a `text` field's own text, `?` for a field that needs pagination,
/// and nothing for a kind this engine doesn't know.
pub(crate) fn fallback(object: &InlineObject) -> String {
    match object.kind.as_str() {
        IMAGE => object.param("alt").unwrap_or_default().into(),
        FIELD if object.param("field") == Some("text") => {
            object.param("text").unwrap_or_default().into()
        }
        FIELD => UNRESOLVED.into(),
        _ => String::new(),
    }
}

/// The objects whose U+FFFC lies inside `bytes`, in text order.
pub(crate) fn within(placed: &[Placed], bytes: Range<usize>) -> &[Placed] {
    let start = placed.partition_point(|p| p.at < bytes.start);
    let end = placed.partition_point(|p| p.at < bytes.end);
    placed.get(start..end).unwrap_or_default()
}

/// A paragraph's plain text, each object's U+FFFC replaced by its text.
pub(crate) fn plain(text: &str, placed: &[Placed]) -> Result<String, ClipboardError> {
    let mut out = String::new();
    let mut cursor = 0;
    for p in placed {
        let before = text.get(cursor..p.at).unwrap_or_default();
        append(&mut out, &before.replace(OBJECT, ""))?;
        append(&mut out, &p.text)?;
        cursor = p.at.saturating_add(OBJECT.len_utf8());
    }
    append(
        &mut out,
        &text.get(cursor..).unwrap_or_default().replace(OBJECT, ""),
    )?;
    Ok(out)
}

/// An object as HTML. An image is an `img`; any other object is a `span`,
/// holding a field's resolved text. Both keep the full stored form in
/// [`JSON_ATTR`], so import restores it exactly. The caller puts the result
/// inside the surrounding run's span, so a field takes that run's styling.
pub(crate) fn html(p: &Placed) -> Result<String, ClipboardError> {
    let json = serde_json::to_string(&p.object)
        .map_err(|e| ClipboardError::Invalid(e.to_string()))?;
    let json = escape(&json);
    if let Ok(image) = p.object.as_image() {
        let mut style = String::new();
        for (name, size) in [("width", image.width), ("height", image.height)] {
            if let Some(InlineSize::Pt(length)) = size {
                style.push_str(&format!("{name}: {}pt; ", points(length)));
            }
        }
        if p.object.align != InlineAlign::Baseline {
            style.push_str(&format!("vertical-align: {}; ", p.object.align.as_str()));
        }
        let style = if style.is_empty() {
            String::new()
        } else {
            format!(" style=\"{}\"", escape(style.trim_end()))
        };
        return Ok(format!(
            "<img {ASSET_ATTR}=\"{}\" alt=\"{}\" {JSON_ATTR}=\"{json}\"{style}>",
            escape(&image.asset),
            escape(&image.alt),
        ));
    }
    match (p.object.kind.as_str(), p.object.param("field")) {
        (FIELD, Some(function)) => Ok(format!(
            "<span {FIELD_ATTR}=\"{}\" {JSON_ATTR}=\"{json}\">{}</span>",
            escape(function),
            escape(&p.text),
        )),
        _ => Ok(format!("<span {JSON_ATTR}=\"{json}\"></span>")),
    }
}

/// The object an HTML element stands for, if it is one. The stored JSON is
/// read first; failing that, the visible attributes. `None` for anything
/// else, and for an object that would not validate.
pub(crate) fn from_html(name: &str, attrs: &BTreeMap<String, String>) -> Option<InlineObject> {
    let stored = attrs
        .get(JSON_ATTR)
        .and_then(|json| serde_json::from_str::<InlineObject>(json).ok());
    stored
        .or_else(|| visible(name, attrs))
        .filter(|object| object.validate().is_ok())
}

/// An image from its asset, alt text, and the CSS width, height and
/// vertical alignment of its `style`; a field from its function. Sizes in
/// units other than points and an `em`-based alignment are not visible, so
/// they come only from the JSON.
fn visible(name: &str, attrs: &BTreeMap<String, String>) -> Option<InlineObject> {
    match name {
        "img" => {
            let alt = attrs.get("alt").map_or("", String::as_str);
            let mut object = InlineObject::image(attrs.get(ASSET_ATTR)?, alt);
            for declaration in attrs.get("style").map_or("", String::as_str).split(';') {
                let Some((key, value)) = declaration.split_once(':') else {
                    continue;
                };
                let key = key.trim().to_ascii_lowercase();
                let value = value.trim();
                match key.as_str() {
                    "width" | "height" => {
                        if let Some(length) = html::css_length(value) {
                            object = object.with(&key, InlineSize::Pt(length).to_string());
                        }
                    }
                    "vertical-align" => {
                        if let Some(align) = InlineAlign::parse(value) {
                            object.align = align;
                        }
                    }
                    _ => {}
                }
            }
            Some(object)
        }
        "span" => Some(InlineObject::field(attrs.get(FIELD_ATTR)?)),
        _ => None,
    }
}
