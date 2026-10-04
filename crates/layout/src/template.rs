//! Resolving the document's page template against the medium (24, 34, 38).
//!
//! The template is authored; what it comes to on this medium is derived. This
//! module turns the first into the second, and decides what to do with a
//! template that can't be laid out: it is reported and the built-in template
//! stands in, so layout still produces pages (37).

use reprise_diag::Severity;
use reprise_doc::{
    Dim, Document, FrameRole, MAIN_FLOW, Medium, PageTemplate, StoredTemplate, TemplateChoice,
};
use reprise_geom::{FrameSpace, Length, PageSpace, Point, Rect, Transform};

use crate::{Diagnostic, Engine, Subject, TemplateSource, codes};

/// A template frame on this medium. Position and size are in page space.
#[derive(Clone, Debug)]
pub(crate) struct ResolvedFrame {
    pub name: String,
    pub role: FrameRole,
    pub x: Length,
    pub y: Length,
    /// Never negative.
    pub width: Length,
    /// Never negative.
    pub depth: Length,
}

impl ResolvedFrame {
    /// The frame's extent in its own space: its origin is its start corner.
    pub fn rect(&self) -> Rect<FrameSpace> {
        Rect::new(Point::origin(), self.width, self.depth)
    }

    /// Where the frame's space sits on the page. A translation today. Rotated
    /// and mirrored frames and writing modes (20) change only this function.
    pub fn to_page(&self) -> Transform<FrameSpace, PageSpace> {
        Transform::translate(self.x, self.y)
    }

    /// Whether lines can be put in it: a frame with no depth is full already.
    pub fn takes_text(&self) -> bool {
        self.depth > Length::ZERO
    }

    pub fn is_main_flow(&self) -> bool {
        self.role == FrameRole::Flow(MAIN_FLOW.into())
    }
}

/// A page template on this medium: every page is made from it.
#[derive(Clone, Debug)]
pub(crate) struct ResolvedTemplate {
    pub name: String,
    pub source: TemplateSource,
    pub width: Length,
    pub height: Length,
    /// In the template's threading order.
    pub frames: Vec<ResolvedFrame>,
}

impl ResolvedTemplate {
    /// The indices of the frames the main flow threads through, in order.
    pub fn main_thread(&self) -> Vec<usize> {
        (0..self.frames.len())
            .filter(|&i| self.frames[i].is_main_flow() && self.frames[i].takes_text())
            .collect()
    }

    /// The first margin frame's width: what blocks that relations place are
    /// first composed at, before the page they land on is known.
    pub fn margin_width(&self) -> Option<Length> {
        self.frames
            .iter()
            .find(|f| f.role == FrameRole::Margin)
            .map(|f| f.width)
    }
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    severity: Severity,
    code: reprise_diag::Code,
    message: String,
) {
    diagnostics.push(Diagnostic::new(severity, code, Subject::Document, message));
}

/// The template layout uses for `doc` on `engine.medium`. Problems with it
/// are added to `diagnostics`; the result is always usable.
pub(crate) fn resolve(
    engine: &Engine,
    doc: &Document,
    diagnostics: &mut Vec<Diagnostic>,
) -> ResolvedTemplate {
    let chosen = doc.page_template();
    let (template, source) = match &chosen {
        TemplateChoice::Builtin => (PageTemplate::builtin(), TemplateSource::Builtin),
        TemplateChoice::Template(t) => (t.clone(), TemplateSource::Document),
        TemplateChoice::Unreadable { name, .. } => {
            report(
                diagnostics,
                Severity::Warning,
                codes::TEMPLATE_UNREADABLE,
                format!(
                    "page template `{name}` can't be read by this engine; the built-in one is used"
                ),
            );
            (PageTemplate::builtin(), TemplateSource::Builtin)
        }
    };
    // Other templates this engine can't read stay in the document, unused.
    let in_use = match &chosen {
        TemplateChoice::Unreadable { name, .. } => Some(name.as_str()),
        _ => None,
    };
    for StoredTemplate { name, template } in doc.page_templates() {
        if template.is_err() && Some(name.as_str()) != in_use {
            report(
                diagnostics,
                Severity::Info,
                codes::TEMPLATE_UNREADABLE,
                format!("page template `{name}` can't be read by this engine; kept, not used"),
            );
        }
    }

    match resolve_template(&template, source, &engine.medium) {
        Ok((resolved, notes)) => {
            diagnostics.extend(notes);
            resolved
        }
        Err(why) => {
            report(
                diagnostics,
                Severity::Warning,
                codes::TEMPLATE_UNUSABLE,
                format!(
                    "page template `{}` can't be used: {why}; the built-in one is used",
                    template.name
                ),
            );
            builtin(&engine.medium)
        }
    }
}

/// The built-in template, which is absolute and positive and so always resolves.
fn builtin(medium: &Medium) -> ResolvedTemplate {
    match resolve_template(&PageTemplate::builtin(), TemplateSource::Builtin, medium) {
        Ok((resolved, _)) => resolved,
        // Unreachable, but layout must not panic: an empty page set still
        // lays out (every block is reported as unplaced).
        Err(_) => ResolvedTemplate {
            name: String::new(),
            source: TemplateSource::Builtin,
            width: Length::ZERO,
            height: Length::ZERO,
            frames: Vec::new(),
        },
    }
}

/// Resolves every dimension. `Err` says why the template can't be used at
/// all; `Ok` carries the diagnostics about frames that were clamped or empty.
fn resolve_template(
    template: &PageTemplate,
    source: TemplateSource,
    medium: &Medium,
) -> Result<(ResolvedTemplate, Vec<Diagnostic>), String> {
    let width = template
        .width
        .resolve(medium, None)
        .ok_or("the page width refers to the page")?;
    let height = template
        .height
        .resolve(medium, None)
        .ok_or("the page height refers to the page")?;
    if width <= Length::ZERO || height <= Length::ZERO {
        return Err(format!(
            "the page size {width:?} × {height:?} isn't positive"
        ));
    }

    let mut notes = Vec::new();
    let page = Some((width, height));
    let mut frames = Vec::new();
    for frame in &template.frames {
        let at = |dim: &Dim| dim.resolve(medium, page).unwrap_or(Length::ZERO);
        let mut size = |what: &str, dim: &Dim| {
            let used = at(dim);
            let (severity, note) = if used < Length::ZERO {
                (Severity::Warning, "is negative; clamped to zero")
            } else if used == Length::ZERO {
                (Severity::Info, "is zero")
            } else {
                return used;
            };
            report(
                &mut notes,
                severity,
                codes::DEGENERATE_FRAME,
                format!("frame `{}`: its {what} {note}", frame.name),
            );
            used.max(Length::ZERO)
        };
        let frame_width = size("width", &frame.width);
        let depth = size("height", &frame.height);
        frames.push(ResolvedFrame {
            name: frame.name.clone(),
            role: frame.role.clone(),
            x: at(&frame.x),
            y: at(&frame.y),
            width: frame_width,
            depth,
        });
    }
    let resolved = ResolvedTemplate {
        name: template.name.clone(),
        source,
        width,
        height,
        frames,
    };
    if resolved.main_thread().is_empty() {
        return Err("it has no frame with room for the main flow".into());
    }
    Ok((resolved, notes))
}

#[cfg(test)]
mod tests {
    use reprise_doc::{Basis, FrameTemplate};

    use super::*;

    fn medium() -> Medium {
        Medium::new(Length::from_pt(600), Length::from_pt(400))
    }

    fn column(width: Dim, height: Dim) -> FrameTemplate {
        FrameTemplate::new(
            "col",
            FrameRole::Flow(MAIN_FLOW.into()),
            (Dim::pt(0), Dim::pt(0)),
            (width, height),
        )
    }

    fn resolve_one(template: &PageTemplate) -> Result<(ResolvedTemplate, Vec<Diagnostic>), String> {
        resolve_template(template, TemplateSource::Document, &medium())
    }

    #[test]
    fn the_builtin_template_is_the_spikes_geometry() {
        let (t, notes) = resolve_one(&PageTemplate::builtin()).unwrap();
        assert!(notes.is_empty());
        assert_eq!(
            (t.width, t.height),
            (Length::from_pt(420), Length::from_pt(300))
        );
        let (main, margin) = (&t.frames[0], &t.frames[1]);
        assert_eq!((main.x, main.y), (Length::from_pt(36), Length::from_pt(36)));
        assert_eq!(
            (main.width, main.depth),
            (Length::from_pt(220), Length::from_pt(228))
        );
        assert_eq!(margin.x, Length::from_pt(274));
        assert_eq!(margin.width, Length::from_pt(110));
    }

    #[test]
    fn page_sizes_that_are_not_positive_are_unusable() {
        for (w, h) in [(0, 100), (100, 0), (-5, 100), (100, -5), (0, 0)] {
            let t = PageTemplate::new("t", Dim::pt(w), Dim::pt(h))
                .with_frame(column(Dim::pt(10), Dim::pt(10)));
            assert!(resolve_one(&t).is_err(), "{w}×{h}");
        }
        let extreme = PageTemplate::new("t", Dim::Pt(Length::MIN), Dim::Pt(Length::MAX))
            .with_frame(column(Dim::pt(10), Dim::pt(10)));
        assert!(resolve_one(&extreme).is_err());
    }

    #[test]
    fn a_page_size_may_not_refer_to_the_page() {
        let t = PageTemplate::new("t", Dim::fraction(Basis::PageWidth, 500), Dim::pt(100))
            .with_frame(column(Dim::pt(10), Dim::pt(10)));
        assert!(resolve_one(&t).is_err());
    }

    #[test]
    fn a_page_as_big_as_the_medium_follows_it() {
        let t = PageTemplate::new(
            "t",
            Dim::fraction(Basis::MediumWidth, 500),
            Dim::fraction(Basis::MediumHeight, 1000),
        )
        .with_frame(column(Dim::fraction(Basis::PageWidth, 500), Dim::pt(10)));
        let (r, _) = resolve_one(&t).unwrap();
        assert_eq!(
            (r.width, r.height),
            (Length::from_pt(300), Length::from_pt(400))
        );
        assert_eq!(r.frames[0].width, Length::from_pt(150));
        let small = resolve_template(
            &t,
            TemplateSource::Document,
            &Medium::new(Length::from_pt(100), Length::from_pt(100)),
        )
        .unwrap()
        .0;
        assert_eq!(small.frames[0].width, Length::from_pt(25));
    }

    #[test]
    fn frames_with_no_size_are_reported_and_clamped() {
        let t = PageTemplate::new("t", Dim::pt(300), Dim::pt(300))
            .with_frame(column(Dim::pt(-10), Dim::pt(50)))
            .with_frame(column(Dim::pt(0), Dim::pt(0)))
            .with_frame(column(Dim::pt(100), Dim::pt(-1)))
            .with_frame(column(Dim::pt(100), Dim::pt(100)));
        let (r, notes) = resolve_one(&t).unwrap();
        assert!(
            r.frames
                .iter()
                .all(|f| f.width >= Length::ZERO && f.depth >= Length::ZERO)
        );
        assert_eq!(r.frames[0].width, Length::ZERO);
        assert_eq!(notes.len(), 4);
        let warnings = notes
            .iter()
            .filter(|n| n.severity == Severity::Warning)
            .count();
        assert_eq!(
            warnings, 2,
            "only the negative sizes differ from what was asked"
        );
        assert_eq!(
            r.main_thread(),
            [0, 3],
            "frames with no depth are not in the thread"
        );
    }

    #[test]
    fn a_template_without_room_for_the_main_flow_is_unusable() {
        let none = PageTemplate::new("t", Dim::pt(100), Dim::pt(100));
        assert!(resolve_one(&none).is_err());
        let margin_only =
            PageTemplate::new("t", Dim::pt(100), Dim::pt(100)).with_frame(FrameTemplate::new(
                "m",
                FrameRole::Margin,
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(10), Dim::pt(10)),
            ));
        assert!(resolve_one(&margin_only).is_err());
        let flat = PageTemplate::new("t", Dim::pt(100), Dim::pt(100))
            .with_frame(column(Dim::pt(10), Dim::pt(0)));
        assert!(resolve_one(&flat).is_err(), "its only frame has no depth");
    }

    #[test]
    fn extreme_dimensions_saturate() {
        let t = PageTemplate::new("t", Dim::Pt(Length::MAX), Dim::Pt(Length::MAX))
            .with_frame(FrameTemplate::new(
                "extreme",
                FrameRole::Flow(MAIN_FLOW.into()),
                (Dim::Pt(Length::MIN), Dim::Pt(Length::MAX)),
                (
                    Dim::fraction(Basis::PageWidth, i32::MAX).plus(Length::MAX),
                    Dim::fraction(Basis::PageHeight, i32::MIN),
                ),
            ))
            .with_frame(column(Dim::pt(1), Dim::pt(1)));
        let (r, notes) = resolve_one(&t).unwrap();
        assert_eq!(r.frames[0].width, Length::MAX);
        assert_eq!(
            r.frames[0].depth,
            Length::ZERO,
            "a huge negative depth is clamped"
        );
        assert_eq!(notes.len(), 1);
    }
}
