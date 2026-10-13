//! Frozen, read-only section resolution shared by body and region passes.
mod geometry;
mod spaces;
pub(crate) use spaces::{FloatSpaces, name as float_space_name};
#[cfg(test)]
mod tests;

use crate::template::ResolvedTemplate;
use crate::{Diagnostic, Engine, LayoutSnapshot, Subject, TemplateSource};
use geometry::Warnings;
pub(crate) use geometry::{Columns, Geometry};
use reprise_diag::{Code, Severity};
use reprise_doc::page_setup::PageDimensions;
use reprise_doc::sections::{BreakKind, SectionId, SectionProperties, SectionView};
use reprise_doc::{
    Document, Domain, FrameRole, NodeId, PageTemplate, Rotation, TemplateChoice, WritingMode,
};
use reprise_geom::{Length, Matrix, PageSpace, Rect};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Start {
    #[default]
    NewPage,
    OddPage,
    EvenPage,
    Continuous,
}

#[derive(Clone, Debug)]
pub(crate) struct Section {
    pub view: SectionView,
    pub template: Arc<ResolvedTemplate>,
    pub geometry: Geometry,
    pub columns: Option<Columns>,
    pub rules: Vec<Rect<PageSpace>>,
    pub start: Start,
    pub restart: Option<i64>,
    pub format: String,
    pub filler_running: bool,
    pub mode: String,
}
impl Section {
    pub fn id(&self) -> SectionId {
        self.view.id
    }
    pub fn life(&self) -> &str {
        &self.view.life
    }
    pub fn state(&self) -> Value {
        let d = self.geometry.dimensions;
        json!({
            "template": self.template.name,
            "template_source": self.template.source,
            "mode": self.mode,
            "rendered_mode": "paged",
            "geometry": { "width": d.width, "height": d.height, "top": d.top,
                "right": d.right, "bottom": d.bottom, "left": d.left,
                "provenance": self.geometry.provenance },
            "columns": self.columns.as_ref().map_or_else(|| {
                let widths: Vec<_> = self.template.frames.iter().filter(|f| f.is_main_flow()).map(|f| f.width).collect();
                json!({"widths": widths, "count": widths.len(), "thread": self.template.main_thread(), "gap": null, "rule": 0, "generated": false})
            }, |c| json!({"widths": c.widths,
                "count": c.widths.len(), "thread": c.thread, "gap": c.gap, "rule": c.rule, "generated": true})),
            "direction": if self.geometry.rtl { "rtl" } else { "ltr" },
            "writing-mode": self.geometry.writing_mode,
            "header.distance": self.geometry.header_distance,
            "footer.distance": self.geometry.footer_distance,
            "numbering": { "restart": self.restart.map(|n| n.to_string()), "format": self.format },
            "filler.regions": if self.filler_running { "running" } else { "blank" }
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Table {
    pub sections: Vec<Section>,
    pub starts: BTreeMap<NodeId, BreakKind>,
    pub by_id: BTreeMap<SectionId, usize>,
    pub initial: usize,
    pub enabled: bool,
    pub has_continuous: bool,
    pub float_spaces: FloatSpaces,
    pub diagnostics: Vec<Diagnostic>,
    diagnostic_order: BTreeMap<SectionId, usize>,
}
type Base = (Arc<PageTemplate>, Arc<ResolvedTemplate>);

impl Table {
    pub fn resolve(engine: &Engine, doc: &Document, default: &ResolvedTemplate) -> Arc<Self> {
        let mut views = doc.section_table();
        if views.is_empty() {
            views.push(SectionView {
                id: SectionId::First,
                life: "0".into(),
                first_block: doc.top(Domain::Body).first().copied(),
                end: None,
                own: SectionProperties::new(),
                bindings: Vec::new(),
                unknown_keys: Vec::new(),
            });
        }
        let raw_default = match doc.page_template() {
            TemplateChoice::Template(t) if default.source == TemplateSource::Document => t,
            _ => PageTemplate::builtin(),
        };
        let raw_default = doc
            .patch_page_template(&raw_default, &engine.medium)
            .ok()
            .flatten()
            .unwrap_or(raw_default);
        let base: Base = (Arc::new(raw_default), Arc::new(default.clone()));
        let fallback = default.page_setup.unwrap_or(PageDimensions {
            width: default.width,
            height: default.height,
            top: Length::ZERO,
            right: Length::ZERO,
            bottom: Length::ZERO,
            left: Length::ZERO,
        });
        let stored: BTreeMap<_, _> = doc
            .page_templates()
            .into_iter()
            .map(|t| (t.name, t.template))
            .collect();
        let mode_record = doc.geometry_mode_record();
        let mut bases: BTreeMap<String, Result<Base, String>> = BTreeMap::new();
        let mut diagnostics = Vec::new();
        let mut sections = Vec::with_capacity(views.len());
        let mut format = "decimal".to_owned();
        for view in views {
            let mut notes = Warnings::new(view.id);
            if view.id == SectionId::First
                && mode_record.as_ref().is_some_and(|r| {
                    r.get("v").and_then(Value::as_i64) != Some(1)
                        || !matches!(
                            r.get("mode").and_then(Value::as_str),
                            Some("paged" | "pageless")
                        )
                })
            {
                notes.warn(
                    "mode",
                    "section.mode-unsupported",
                    "unknown geometry mode or version is retained; paged layout is used",
                );
            }
            let selected = match view.own.get("template") {
                None => base.clone(),
                Some(Value::String(name)) => {
                    let selected = bases.entry(name.clone()).or_insert_with(|| {
                        let raw = stored
                            .get(name)
                            .ok_or_else(|| "missing template".to_owned())?
                            .as_ref()
                            .map_err(|_| "unreadable template".to_owned())?
                            .clone();
                        let raw = doc
                            .patch_page_template(&raw, &engine.medium)
                            .ok()
                            .flatten()
                            .unwrap_or(raw);
                        let (resolved, extra) = crate::template::resolve_template(
                            &raw,
                            TemplateSource::Document,
                            &engine.medium,
                        )?;
                        diagnostics.extend(extra);
                        Ok((Arc::new(raw), Arc::new(resolved)))
                    });
                    match selected {
                        Ok(base) => base.clone(),
                        Err(_) => {
                            notes.warn("template", "section.template", "selected template is missing or unusable; the document default is used");
                            base.clone()
                        }
                    }
                }
                Some(_) => {
                    notes.warn(
                        "template",
                        "section.property",
                        "template name is unreadable; the document default is used",
                    );
                    base.clone()
                }
            };
            let base_mode = selected
                .1
                .frames
                .iter()
                .find(|f| f.is_main_flow())
                .map_or(WritingMode::HorizontalTb, |f| f.writing_mode);
            let geometry = geometry::resolve(
                &view.own,
                selected.1.page_setup.unwrap_or(fallback),
                fallback,
                base_mode,
                &mut notes,
            );
            let columns = geometry::columns(
                &view.own,
                geometry.inline_extent(),
                selected.1.main_thread().len(),
                &mut notes,
            );
            let geometry_keys = [
                "width",
                "height",
                "top",
                "right",
                "bottom",
                "left",
                "orientation",
            ];
            let project = geometry_keys.iter().any(|k| view.own.contains_key(*k));
            let mode_key = view
                .own
                .get("writing-mode")
                .and_then(Value::as_str)
                .filter(|s| matches!(*s, "horizontal-tb" | "vertical-rl" | "vertical-lr"));
            let mut rules = Vec::new();
            let template = if project || mode_key.is_some() || columns.is_some() {
                let mut raw = (*selected.0).clone();
                let d = geometry.dimensions;
                if project || columns.is_some() {
                    raw.width = reprise_doc::Dim::Pt(d.width);
                    raw.height = reprise_doc::Dim::Pt(d.height);
                    if let Some(frame) = raw
                        .frames
                        .iter_mut()
                        .find(|f| f.role == FrameRole::Flow(reprise_doc::MAIN_FLOW.into()))
                    {
                        frame.x = reprise_doc::Dim::Pt(d.left);
                        frame.y = reprise_doc::Dim::Pt(d.top);
                        frame.width = reprise_doc::Dim::Pt(d.width - d.left - d.right);
                        frame.height = reprise_doc::Dim::Pt(d.height - d.top - d.bottom);
                    }
                }
                if mode_key.is_some() {
                    for frame in &mut raw.frames {
                        if frame.role == FrameRole::Flow(reprise_doc::MAIN_FLOW.into()) {
                            frame.writing_mode = geometry.writing_mode;
                        }
                    }
                }
                let mut resolved =
                    crate::template::resolve_template(&raw, selected.1.source, &engine.medium)
                        .map(|(t, extra)| {
                            diagnostics.extend(extra);
                            t
                        })
                        .unwrap_or_else(|_| (*base.1).clone());
                if let Some(columns) = &columns {
                    if selected.0.frames.iter().any(|f| {
                        f.role == FrameRole::Flow(reprise_doc::MAIN_FLOW.into()) && transformed(f)
                    }) {
                        notes.warn(
                            "columns",
                            "section.template",
                            "generated columns replace an authored path, transform or mirror",
                        );
                    }
                    let (generated, column_rules) = geometry::frames(&geometry, columns);
                    rules = column_rules;
                    let mut frames = Vec::new();
                    let mut insertion = None;
                    for frame in resolved.frames {
                        if frame.is_main_flow() {
                            if insertion.is_none() {
                                insertion = Some(frames.len());
                                frames.extend(generated.iter().cloned());
                            }
                        } else {
                            frames.push(frame);
                        }
                    }
                    let at = insertion.unwrap_or_else(|| {
                        let at = frames.len();
                        frames.extend(generated);
                        at
                    });
                    resolved.frames = frames;
                    resolved.thread = Some(
                        columns
                            .thread
                            .iter()
                            .map(|i| at.saturating_add(*i))
                            .collect(),
                    );
                }
                Arc::new(resolved)
            } else {
                selected.1
            };
            let start = if view.id == SectionId::First {
                Start::NewPage
            } else {
                match text(
                    &view.own,
                    "start",
                    &["new-page", "odd-page", "even-page", "continuous"],
                    &mut notes,
                ) {
                    Some("odd-page") => Start::OddPage,
                    Some("even-page") => Start::EvenPage,
                    Some("continuous") => Start::Continuous,
                    _ => Start::NewPage,
                }
            };
            let restart = view.own.get("numbering.restart").and_then(|value| {
                let n = value.as_i64().or_else(|| {
                    value
                        .as_str()
                        .filter(|s| s.len() <= 20)
                        .and_then(|s| s.parse::<i64>().ok().filter(|n| n.to_string() == s))
                });
                if n.is_none() {
                    notes.warn(
                        "numbering.restart",
                        "section.property",
                        "restart is not a signed 64-bit integer; numbering continues",
                    );
                }
                n
            });
            if let Some(value) = view.own.get("numbering.format") {
                use reprise_doc::lists::NumberFormat;
                match value.as_str().map(NumberFormat::parse) {
                    Some(NumberFormat::Bullet | NumberFormat::None) | None => notes.warn(
                        "numbering.format",
                        "section.property",
                        "page numbering requires an ordered format; the preceding format is used",
                    ),
                    Some(NumberFormat::Unknown(token)) => {
                        notes.warn(
                            "numbering.format",
                            "section.property",
                            "unknown format is retained and renders decimal",
                        );
                        format = token;
                    }
                    Some(value) => format = value.token().into(),
                }
            }
            let filler_running = text(
                &view.own,
                "filler.regions",
                &["blank", "running"],
                &mut notes,
            ) == Some("running");
            let default_mode = mode_record.as_ref().and_then(|record| {
                (record.get("v").and_then(Value::as_i64) == Some(1))
                    .then(|| record.get("mode").and_then(Value::as_str))
                    .flatten()
            });
            let mode = match view.own.get("mode") {
                Some(Value::String(s)) if matches!(s.as_str(), "paged" | "pageless") => s.clone(),
                Some(_) => {
                    notes.warn(
                        "mode",
                        "section.mode-unsupported",
                        "unknown section mode; paged layout is used",
                    );
                    "paged".into()
                }
                None => match default_mode {
                    Some("paged") => "paged".into(),
                    Some("pageless") => "pageless".into(),
                    _ => {
                        if mode_record.is_some() {
                            notes.warn(
                                "mode",
                                "section.mode-unsupported",
                                "unknown geometry mode or version; paged layout is used",
                            );
                        }
                        "paged".into()
                    }
                },
            };
            if mode == "pageless" {
                notes.warn(
                    "mode",
                    "section.mode-unsupported",
                    "pageless mode is retained; paged layout is used",
                );
            }
            for key in &view.unknown_keys {
                if let Some(tail) = key.strip_prefix("region.") {
                    let slot = tail.split('.').next().unwrap_or_default();
                    if !matches!(slot, "header" | "footer" | "background" | "first" | "even") {
                        notes.warn(
                            key,
                            "section.region-slot",
                            "unknown running slot is retained and ignored",
                        );
                    }
                }
            }
            diagnostics.extend(notes.finish());
            sections.push(Section {
                view,
                template,
                geometry,
                columns,
                rules,
                start,
                restart,
                format: format.clone(),
                filler_running,
                mode,
            });
        }
        let starts: BTreeMap<_, _> = doc
            .effective_starts()
            .into_iter()
            .filter_map(|r| match (r.id, r.kind) {
                (SectionId::Block(node), Some(kind)) => Some((node, kind)),
                _ => None,
            })
            .collect();
        let body = doc.top(Domain::Body);
        let diagnostic_order: BTreeMap<_, _> = std::iter::once((SectionId::First, 0))
            .chain(
                body.iter()
                    .enumerate()
                    .map(|(i, node)| (SectionId::Block(*node), i.saturating_add(1))),
            )
            .collect();
        for record in doc.start_records() {
            if record.active
                && record.kind.is_none()
                && matches!(record.id, SectionId::Block(_))
                && diagnostic_order.contains_key(&record.id)
                && record.own.contains_key("break")
            {
                diagnostics.push(diagnostic(
                    record.id,
                    "break",
                    "section.property",
                    Severity::Warning,
                    "unknown or unreadable break is retained and ignored",
                ));
            }
        }
        let by_id: BTreeMap<_, _> = sections
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id(), i))
            .collect();
        let initial = doc
            .top(Domain::Body)
            .first()
            .and_then(|n| by_id.get(&SectionId::Block(*n)))
            .copied()
            .unwrap_or(0);
        let enabled = !starts.is_empty()
            || mode_record.is_some()
            || sections.first().is_some_and(|s| {
                s.view.own.keys().any(|k| {
                    reprise_doc::sections::known_property(k)
                        && !matches!(k.as_str(), "break" | "start")
                })
            });
        // Orphan stories and inactive retained starts are authored state, but
        // they do not add section metadata to an otherwise legacy snapshot.
        let has_continuous = sections.iter().any(|s| s.start == Start::Continuous);
        let float_spaces = FloatSpaces::resolve(
            &sections,
            has_continuous
                && doc.relations().iter().any(|(_, r)| {
                    r.as_ref().is_ok_and(|r| {
                        r.schema == reprise_doc::relation::builtin::FLOAT
                            && r.owner.is_some()
                            && engine.schemas.get(&r.schema).is_some()
                    })
                }),
        );
        Arc::new(Self {
            sections,
            starts,
            by_id,
            initial,
            enabled,
            has_continuous,
            float_spaces,
            diagnostics,
            diagnostic_order,
        })
    }
    pub fn get(&self, id: SectionId) -> Option<&Section> {
        self.sections.get(*self.by_id.get(&id)?)
    }
    pub fn state(&self) -> BTreeMap<SectionId, Value> {
        self.sections.iter().map(|s| (s.id(), s.state())).collect()
    }
    pub fn sort_diagnostics(&self, snapshot: &mut LayoutSnapshot) {
        let mut section = Vec::new();
        snapshot.diagnostics.retain(|d| {
            if d.code.as_str().starts_with("section.") {
                section.push(d.clone());
                false
            } else {
                true
            }
        });
        section.sort_by_key(|d| {
            let id = match d.subject {
                Subject::Node(node) => SectionId::Block(node),
                _ => SectionId::First,
            };
            (
                self.diagnostic_order
                    .get(&id)
                    .copied()
                    .unwrap_or(usize::MAX),
                d.message.split(':').next().unwrap_or_default().to_owned(),
                d.code.as_str().to_owned(),
            )
        });
        snapshot.diagnostics.extend(section);
    }
}

fn text<'a>(
    own: &'a SectionProperties,
    key: &str,
    accepted: &[&str],
    notes: &mut Warnings,
) -> Option<&'a str> {
    let value = own.get(key)?;
    let value = value.as_str().filter(|s| accepted.contains(s));
    if value.is_none() {
        notes.warn(
            key,
            "section.property",
            "invalid value; the default is used",
        );
    }
    value
}
fn transformed(frame: &reprise_doc::FrameTemplate) -> bool {
    let rotation_identity = match frame.transform.rotation {
        Rotation::None => true,
        Rotation::Quarter(n) => n.rem_euclid(4) == 0,
        Rotation::Direction { dx, dy } => dx > 0 && dy == 0,
        Rotation::Matrix { xx, yx, xy, yy } => {
            xx == Matrix::IDENTITY.xx
                && yy == Matrix::IDENTITY.yy
                && xy == Matrix::IDENTITY.xy
                && yx == Matrix::IDENTITY.yx
        }
    };
    frame.path.is_some()
        || frame.transform.mirror_x
        || frame.transform.mirror_y
        || !rotation_identity
}

pub(crate) fn diagnostic(
    id: SectionId,
    key: &str,
    code: &'static str,
    severity: Severity,
    why: &str,
) -> Diagnostic {
    Diagnostic::new(
        severity,
        Code::new(code),
        match id {
            SectionId::First => Subject::Document,
            SectionId::Block(node) => Subject::Node(node),
        },
        format!("{key}: {why}"),
    )
}
