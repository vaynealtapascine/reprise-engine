use std::collections::{BTreeMap, BTreeSet};

use reprise_diag::Code;
use reprise_doc::{Document, NodeId, SchemaRegistry, TableRole};
use reprise_font::FontStore;
use reprise_layout::{DisplayOptions, LayoutSnapshot};
use serde::{Deserialize, Serialize};

use crate::{ClipboardError, copy_all};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Feature {
    Relations,
    ReadingOrder,
    Transforms,
    NotesAndFloats,
    Tables,
    Styles,
    Bidi,
    Fonts,
    Assets,
    EditingStructure,
}
impl Feature {
    pub const ALL: [Self; 10] = [
        Self::Relations,
        Self::ReadingOrder,
        Self::Transforms,
        Self::NotesAndFloats,
        Self::Tables,
        Self::Styles,
        Self::Bidi,
        Self::Fonts,
        Self::Assets,
        Self::EditingStructure,
    ];
    pub fn code(self) -> Code {
        Code::new(match self {
            Self::Relations => "export.relations",
            Self::ReadingOrder => "export.reading-order",
            Self::Transforms => "export.transforms",
            Self::NotesAndFloats => "export.notes-floats",
            Self::Tables => "export.tables",
            Self::Styles => "export.styles",
            Self::Bidi => "export.bidi",
            Self::Fonts => "export.fonts",
            Self::Assets => "export.assets",
            Self::EditingStructure => "export.editing-structure",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Disposition {
    Preserved,
    Approximated,
    Dropped,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loss {
    pub feature: Feature,
    pub code: Code,
    pub disposition: Disposition,
    pub detail: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossReport {
    pub features: Vec<Loss>,
    pub notes: Vec<reprise_diag::Note>,
}
pub struct ExportResult {
    pub bytes: Vec<u8>,
    pub losses: LossReport,
}

/// Explicit inputs keep export pure. A layout must belong to `doc`'s current revision.
pub struct ExportOptions<'a> {
    pub source_namespace: &'a str,
    pub schemas: &'a SchemaRegistry,
    pub fonts: Option<&'a FontStore>,
}

/// Extension point for future DOCX/EPUB exporters. Failures produce no misleading bytes.
pub trait Exporter {
    fn export(
        &self,
        doc: &Document,
        layout: Option<&LayoutSnapshot>,
        options: &ExportOptions<'_>,
    ) -> Result<ExportResult, ClipboardError>;
}
pub struct PlainText;
pub struct Html;
pub struct Native;
pub struct Pdf;

fn report(dispositions: [Disposition; 10], details: [&str; 10]) -> LossReport {
    LossReport {
        features: Feature::ALL
            .into_iter()
            .zip(dispositions)
            .zip(details)
            .map(|((feature, disposition), detail)| Loss {
                feature,
                code: feature.code(),
                disposition,
                detail: detail.into(),
            })
            .collect(),
        notes: Vec::new(),
    }
}

fn validate_layout(doc: &Document, layout: Option<&LayoutSnapshot>) -> Result<(), ClipboardError> {
    if layout.is_some_and(|l| l.revision != doc.revision()) {
        return Err(ClipboardError::Invalid("export snapshot is stale".into()));
    }
    Ok(())
}

pub(crate) fn semantic(doc: &Document) -> Result<Vec<NodeId>, ClipboardError> {
    let mut out = Vec::new();
    let mut stack: Vec<_> = doc
        .blocks()
        .into_iter()
        .rev()
        .map(|n| (n, 0usize))
        .collect();
    while let Some((node, depth)) = stack.pop() {
        if out.len() >= reprise_doc::fragment::MAX_FRAGMENT_BLOCKS
            || depth > reprise_doc::fragment::MAX_FRAGMENT_DEPTH
        {
            return Err(ClipboardError::Limit("export tree"));
        }
        out.push(node);
        stack.extend(
            doc.children(Some(node))
                .into_iter()
                .rev()
                .map(|n| (n, depth + 1)),
        );
    }
    Ok(out)
}

fn cell(doc: &Document, mut node: NodeId) -> Option<(NodeId, NodeId)> {
    for _ in 0..=reprise_doc::fragment::MAX_FRAGMENT_DEPTH {
        if matches!(doc.table_role(node), Ok(Some(TableRole::Cell(_)))) {
            return Some((node, doc.parent_of(node).flatten()?));
        }
        node = doc.parent_of(node).flatten()?;
    }
    None
}
fn separator(doc: &Document, previous: NodeId, next: NodeId) -> &'static str {
    match (cell(doc, previous), cell(doc, next)) {
        (Some((a, row_a)), Some((b, row_b))) if a != b && row_a == row_b => "\t",
        (Some((a, row_a)), Some((b, row_b))) if a != b && row_a != row_b => "\n",
        _ => "\n\n",
    }
}
fn append(out: &mut String, text: &str) -> Result<(), ClipboardError> {
    if out.len().saturating_add(text.len()) > reprise_doc::fragment::MAX_FRAGMENT_BYTES {
        return Err(ClipboardError::Limit("export bytes"));
    }
    out.push_str(text);
    Ok(())
}

impl Exporter for PlainText {
    fn export(
        &self,
        doc: &Document,
        layout: Option<&LayoutSnapshot>,
        _: &ExportOptions<'_>,
    ) -> Result<ExportResult, ClipboardError> {
        validate_layout(doc, layout)?;
        let mut out = String::new();
        let mut previous = None;
        let mut emitted = BTreeMap::<NodeId, usize>::new();
        if let Some(layout) = layout {
            for step in layout.reading_order(doc) {
                let Some(block) = layout.blocks.iter().find(|b| b.node == step.line.node) else {
                    continue;
                };
                let Some(line) = block.lines.get(step.line.line) else {
                    continue;
                };
                if let Some(previous) = previous
                    && previous != block.node
                {
                    append(&mut out, separator(doc, previous, block.node))?;
                }
                append(
                    &mut out,
                    block
                        .text
                        .get(line.text.clone())
                        .ok_or_else(|| ClipboardError::Invalid("layout text range".into()))?,
                )?;
                previous = Some(block.node);
                let end = emitted.entry(block.node).or_default();
                *end = (*end).max(line.text.end);
            }
        }
        let mut unplaced = false;
        for node in semantic(doc)? {
            if doc
                .table_role(node)
                .map_err(|e| ClipboardError::Invalid(e.to_string()))?
                .is_some()
            {
                continue;
            }
            let block = doc
                .block(node)
                .map_err(|e| ClipboardError::Invalid(e.to_string()))?;
            let text = block.text.to_string();
            let start = emitted.get(&node).copied().unwrap_or(0);
            if emitted.contains_key(&node) && start >= text.len() {
                continue;
            }
            if let Some(previous) = previous
                && previous != node
            {
                append(&mut out, separator(doc, previous, node))?;
            }
            append(
                &mut out,
                text.get(start..)
                    .ok_or_else(|| ClipboardError::Invalid("unplaced UTF-8 range".into()))?,
            )?;
            previous = Some(node);
            unplaced |= layout.is_some();
        }
        use Disposition::*;
        let mut losses = report(
            [
                Dropped,
                if unplaced { Approximated } else { Preserved },
                Dropped,
                Approximated,
                Approximated,
                Dropped,
                Preserved,
                Dropped,
                Dropped,
                Dropped,
            ],
            [
                "relations are flattened to their text",
                "layout reading order when supplied; semantic order otherwise; unplaced blocks appended",
                "transforms and spirals are omitted",
                "notes and floats become ordinary paragraphs",
                "cells use tabs; rows use newlines; paragraphs use two newlines; authored breaks stay literal",
                "style metadata is omitted",
                "logical Unicode text and bidi controls are retained",
                "font bytes and identities are omitted",
                "non-text assets are omitted",
                "IDs, ranges, undo and collaboration history are omitted",
            ],
        );
        if let Some(layout) = layout {
            losses.notes.extend(
                layout
                    .reading_order_report(doc)
                    .diagnostics
                    .into_iter()
                    .map(|d| reprise_diag::Note::new(d.severity, d.code, d.message)),
            );
        }
        Ok(ExportResult {
            bytes: out.into_bytes(),
            losses,
        })
    }
}

pub(crate) fn escape(text: &str) -> String {
    text.chars().fold(String::new(), |mut s, c| {
        s.push_str(match c {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&#39;",
            _ => {
                s.push(c);
                return s;
            }
        });
        s
    })
}
pub(crate) fn points(length: reprise_geom::Length) -> String {
    let n = i64::from(length.0);
    let magnitude = n.unsigned_abs();
    let whole = magnitude / 1024;
    let fraction = (magnitude % 1024) * 9_765_625;
    if fraction == 0 {
        format!("{}{whole}", if n < 0 { "-" } else { "" })
    } else {
        format!(
            "{}{whole}.{}",
            if n < 0 { "-" } else { "" },
            format!("{fraction:010}").trim_end_matches('0')
        )
    }
}
fn paragraph(
    doc: &Document,
    node: NodeId,
    layout: Option<&LayoutSnapshot>,
    out: &mut String,
) -> Result<(), ClipboardError> {
    let block = doc
        .block(node)
        .map_err(|e| ClipboardError::Invalid(e.to_string()))?;
    let text = block.text.to_string();
    let style = doc
        .computed_style(node)
        .map_err(|e| ClipboardError::Invalid(e.to_string()))?;
    let base = layout
        .and_then(|l| l.blocks.iter().find(|b| b.node == node))
        .map(|b| b.base_level)
        .unwrap_or_else(|| {
            unicode_bidi::BidiInfo::new(&text, None)
                .paragraphs
                .first()
                .map_or(0, |p| p.level.number())
        });
    // CSS string quoting also escapes backslashes/newlines, so a family can't inject CSS.
    let family = style
        .family
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r', '\u{c}'], " ");
    append(
        out,
        &format!(
            "<p dir=\"{}\" style=\"font-family: &quot;{}&quot;; font-size: {}pt; line-height: {}pt\">",
            if base % 2 == 1 { "rtl" } else { "ltr" },
            escape(&family),
            points(style.size),
            points(style.line_height)
        ),
    )?;
    append(out, &escape(&text).replace(['\n', '\u{2028}'], "<br>"))?;
    append(out, "</p>")
}
fn html_node(
    doc: &Document,
    node: NodeId,
    layout: Option<&LayoutSnapshot>,
    out: &mut String,
    depth: usize,
) -> Result<(), ClipboardError> {
    if depth > reprise_doc::fragment::MAX_FRAGMENT_DEPTH {
        return Err(ClipboardError::Limit("HTML tree"));
    }
    let role = doc
        .table_role(node)
        .map_err(|e| ClipboardError::Invalid(e.to_string()))?;
    let tag = match role {
        Some(TableRole::Table(_)) => Some("table"),
        Some(TableRole::Row(_)) => Some("tr"),
        Some(TableRole::Cell(_)) => Some("td"),
        None => None,
    };
    if let Some(tag) = tag {
        append(out, &format!("<{tag}>"))?;
    } else {
        paragraph(doc, node, layout, out)?;
    }
    for child in doc.children(Some(node)) {
        html_node(doc, child, layout, out, depth + 1)?;
    }
    if let Some(tag) = tag {
        append(out, &format!("</{tag}>"))?;
    }
    Ok(())
}
impl Exporter for Html {
    fn export(
        &self,
        doc: &Document,
        layout: Option<&LayoutSnapshot>,
        _: &ExportOptions<'_>,
    ) -> Result<ExportResult, ClipboardError> {
        validate_layout(doc, layout)?;
        semantic(doc)?;
        let mut roots = Vec::new();
        let mut seen = BTreeSet::new();
        if let Some(layout) = layout {
            for step in layout.reading_order(doc) {
                let mut root = step.line.node;
                for _ in 0..=reprise_doc::fragment::MAX_FRAGMENT_DEPTH {
                    match doc.parent_of(root).flatten() {
                        Some(parent) => root = parent,
                        None => break,
                    }
                }
                if seen.insert(root) {
                    roots.push(root);
                }
            }
        }
        for root in doc.blocks() {
            if seen.insert(root) {
                roots.push(root);
            }
        }
        let mut out = String::from("<!doctype html><html><body>");
        for node in roots {
            html_node(doc, node, layout, &mut out, 0)?;
        }
        append(&mut out, "</body></html>")?;
        use Disposition::*;
        let mut losses = report(
            [
                Dropped,
                Approximated,
                Dropped,
                Approximated,
                Approximated,
                Approximated,
                Preserved,
                Approximated,
                Dropped,
                Dropped,
            ],
            [
                "authored relations are omitted",
                "root paragraphs ordered from the layout; table/internal order remains semantic; no precedence graph",
                "transforms and spirals are omitted",
                "notes and floats become paragraphs",
                "table/row/cell semantics retained; column constraints and headers are omitted",
                "computed family, size and line height retained; symbolic values and named inheritance omitted",
                "logical Unicode plus paragraph dir retained",
                "CSS family names retained; pinned identities and font bytes omitted",
                "assets have no HTML representation",
                "IDs, ranges, undo and collaboration history omitted",
            ],
        );
        if let Some(layout) = layout {
            losses.notes.extend(
                layout
                    .reading_order_report(doc)
                    .diagnostics
                    .into_iter()
                    .map(|d| reprise_diag::Note::new(d.severity, d.code, d.message)),
            );
        }
        Ok(ExportResult {
            bytes: out.into_bytes(),
            losses,
        })
    }
}

impl Exporter for Native {
    fn export(
        &self,
        doc: &Document,
        layout: Option<&LayoutSnapshot>,
        options: &ExportOptions<'_>,
    ) -> Result<ExportResult, ClipboardError> {
        validate_layout(doc, layout)?;
        let fragment = copy_all(
            doc,
            options.source_namespace,
            options.schemas,
            layout,
            options.fonts,
        )?;
        use Disposition::*;
        let mut losses = report(
            [
                Approximated,
                Preserved,
                Preserved,
                Preserved,
                Preserved,
                Preserved,
                Preserved,
                if fragment
                    .notes
                    .iter()
                    .any(|n| n.code == crate::codes::RESOURCE_MISSING)
                {
                    Approximated
                } else {
                    Preserved
                },
                Approximated,
                Approximated,
            ],
            [
                "schema copy policies apply; external targets require the source document; historical targets carry no history",
                "authored reading overrides retained",
                "authored page transforms and spirals retained",
                "owned relations and block kinds retained",
                "topology and raw table metadata retained",
                "named styles, inheritance, unparsed values and direct overrides retained",
                "Unicode text and controls retained",
                "available used font bytes and pinned identities bundled; missing resources reported",
                "host may attach assets by content hash; no authored image usage graph yet",
                "live authored structure retained; new IDs allocated on paste; tombstones and undo/history omitted",
            ],
        );
        losses.notes = fragment.notes.clone();
        Ok(ExportResult {
            bytes: fragment.encode()?,
            losses,
        })
    }
}
impl Exporter for Pdf {
    fn export(
        &self,
        doc: &Document,
        layout: Option<&LayoutSnapshot>,
        options: &ExportOptions<'_>,
    ) -> Result<ExportResult, ClipboardError> {
        validate_layout(doc, layout)?;
        let layout = layout.ok_or_else(|| ClipboardError::Export("PDF requires layout".into()))?;
        let fonts = options
            .fonts
            .ok_or_else(|| ClipboardError::Export("PDF requires fonts".into()))?;
        let lists = layout.to_display_lists(DisplayOptions::default());
        let order = layout.pdf_reading_order(doc);
        let bytes = reprise_display::pdf::render_ordered(&lists, fonts, &order)
            .map_err(|e| ClipboardError::Export(e.to_string()))?;
        use Disposition::*;
        let mut losses = report(
            [
                Dropped,
                Approximated,
                Preserved,
                Preserved,
                Preserved,
                Approximated,
                Approximated,
                Preserved,
                Approximated,
                Dropped,
            ],
            [
                "relation graph omitted; visible results retained",
                "text extraction order supplied; no PDF/UA structure tree",
                "rendered transforms and spiral strips retained",
                "visible placed notes/floats retained; unplaced content omitted",
                "laid-out cells retained; editable table semantics omitted",
                "visible computed styles retained; authored expressions and inheritance omitted",
                "positioned glyphs and ActualText retained; viewer extraction support varies",
                "rendered used fonts embedded by the PDF backend",
                "displayed content only; resource ownership omitted",
                "IDs, ranges, editable tree, undo and collaboration history omitted",
            ],
        );
        losses.notes.extend(
            layout
                .diagnostics
                .iter()
                .map(|d| reprise_diag::Note::new(d.severity, d.code.clone(), d.message.clone())),
        );
        losses.notes.extend(
            layout
                .reading_order_report(doc)
                .diagnostics
                .into_iter()
                .map(|d| reprise_diag::Note::new(d.severity, d.code, d.message)),
        );
        Ok(ExportResult { bytes, losses })
    }
}
