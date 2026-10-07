//! Page templates (decisions 05, 24 and 34): the page sizes and frames that
//! text flows through.
//!
//! A template is **authored** state: it is saved, undoable and merges between
//! replicas. Where a template's frames land on a given page is **derived** and
//! lives in the layout snapshot, never here (05).
//!
//! # Responsive templates
//!
//! A [`Dim`] is a length that may refer to the layout [`Medium`] (the viewport
//! or the paper), or to the page it sits on. The medium is a layout input, not
//! authored state, so it is engine configuration and is recorded in the
//! snapshot (38). [`Dim::resolve`] is the single entry point; the
//! expression language of decision 17 can replace the inside of `Dim` later
//! without touching its users.
//!
//! # Storage
//!
//! Each template is one JSON string in the `page_templates` map, keyed by its
//! name, inside a versioned envelope. A template this engine can't read (a
//! newer version, an unknown role, a stray property) is kept as it is and
//! reported by [`Document::page_templates`] as `Err(raw)`; it is never dropped
//! or rewritten (34). `deny_unknown_fields` is deliberate: laying out a
//! template while ignoring a property we don't understand would silently
//! differ from what the author asked.

use loro::{LoroMap, ValueOrContainer};
use reprise_geom::{Fixed, Length};
use serde::{Deserialize, Serialize};

use crate::{DocError, Document, get_str};

/// The version of the stored template envelope, for templates without the
/// version 3 writing modes.
const VERSION: u32 = 2;
/// The envelope version that reads `"vertical-lr"` as the downward mode and
/// knows `"sideways-lr"`. Written only for templates that use either, so every
/// other template stays readable by engines that know only version 2.
const VERSION_MODES: u32 = 3;

/// What the document is being laid out for: the viewport, or the paper. A
/// layout input, not authored state (05, 38).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Medium {
    pub width: Length,
    pub height: Length,
}

impl Medium {
    pub const fn new(width: Length, height: Length) -> Medium {
        Medium { width, height }
    }
}

/// What a [`Dim`] is a fraction of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Basis {
    MediumWidth,
    MediumHeight,
    /// The width of the page the frame is on. Not defined for the page's own size.
    PageWidth,
    /// The height of the page the frame is on. Not defined for the page's own size.
    PageHeight,
}

/// A length as authored in a template: absolute, or a fraction of the medium
/// or page plus an offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dim {
    Pt(Length),
    /// `plus + of × permille / 1000`.
    Fraction {
        of: Basis,
        permille: i32,
        #[serde(default)]
        plus: Length,
    },
}

impl Dim {
    pub const fn pt(pt: i32) -> Dim {
        Dim::Pt(Length::from_pt(pt))
    }

    /// A thousandth-part fraction of `of`.
    pub const fn fraction(of: Basis, permille: i32) -> Dim {
        Dim::Fraction {
            of,
            permille,
            plus: Length::ZERO,
        }
    }

    /// The same dimension moved by `by`.
    pub fn plus(self, by: Length) -> Dim {
        match self {
            Dim::Pt(l) => Dim::Pt(l + by),
            Dim::Fraction { of, permille, plus } => Dim::Fraction {
                of,
                permille,
                plus: plus + by,
            },
        }
    }

    /// The used length, or `None` when the basis is a page size that isn't
    /// known (`page` is `None`, as when resolving the page's own size).
    /// Saturating integer arithmetic only (19).
    pub fn resolve(&self, medium: &Medium, page: Option<(Length, Length)>) -> Option<Length> {
        match *self {
            Dim::Pt(l) => Some(l),
            Dim::Fraction { of, permille, plus } => {
                let reference = match of {
                    Basis::MediumWidth => medium.width,
                    Basis::MediumHeight => medium.height,
                    Basis::PageWidth => page?.0,
                    Basis::PageHeight => page?.1,
                };
                Some(plus + reference.mul_ratio(permille, 1000))
            }
        }
    }
}

/// What a frame is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FrameRole {
    /// Text threads through the frames of a named flow, in the order the
    /// template declares them, then on to the next page. Paragraphs flow in `"main"`.
    Flow(String),
    /// Receives blocks that relations place beside the text, such as notes
    /// that `reprise.follow` a line.
    Margin,
    /// The notes area (24): where `reprise.note` places footnotes. The frame
    /// is the most room notes may take on its page; notes sit at its bottom
    /// and the body frames above it give up exactly the depth they use. A
    /// frame as large as the page lets notes take the whole page over.
    Notes,
}

/// The name of the flow paragraphs belong to.
pub const MAIN_FLOW: &str = "main";

/// Authored clockwise rotation. Directions are normalised with exact integer
/// arithmetic; matrix coefficients are authored 16.16 rationals, never floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Rotation {
    #[default]
    None,
    Quarter(i32),
    Direction {
        dx: i32,
        dy: i32,
    },
    Matrix {
        xx: Fixed,
        yx: Fixed,
        xy: Fixed,
        yy: Fixed,
    },
}

/// Physical writing axes, as in CSS (see `docs/vertical.md`).
///
/// -   `vertical-rl`: lines run down, columns progress to the left.
/// -   `vertical-lr`: lines run down, columns progress to the right.
/// -   `sideways-lr`: lines run up, columns progress to the right, and every
///     glyph is sideways. Version 1 and 2 templates stored this mode as
///     `"vertical-lr"`; they are read as `SidewaysLr`.
///
/// In both vertical modes, glyphs follow the block's `text-orientation`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WritingMode {
    #[default]
    HorizontalTb,
    VerticalRl,
    VerticalLr,
    SidewaysLr,
}

impl WritingMode {
    /// True for the modes whose lines run down a column, where glyphs follow
    /// `text-orientation`.
    pub fn is_vertical(self) -> bool {
        matches!(self, WritingMode::VerticalRl | WritingMode::VerticalLr)
    }
}

/// A transform in physical frame coordinates, before placement at (x,y).
/// Apply mirrors, then rotation, about origin. Defaults preserve v1 layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameTransform {
    #[serde(default)]
    pub rotation: Rotation,
    #[serde(default)]
    pub mirror_x: bool,
    #[serde(default)]
    pub mirror_y: bool,
    #[serde(default = "zero_dim")]
    pub origin_x: Dim,
    #[serde(default = "zero_dim")]
    pub origin_y: Dim,
}

fn zero_dim() -> Dim {
    Dim::pt(0)
}

impl Default for FrameTransform {
    fn default() -> Self {
        Self {
            rotation: Rotation::None,
            mirror_x: false,
            mirror_y: false,
            origin_x: zero_dim(),
            origin_y: zero_dim(),
        }
    }
}

/// Archimedean spiral, expanded into bounded, threaded tangent frames.
/// The parent (x,y) is the centre; radius grows by `growth` per full turn.
/// `width` is ignored: each inline measure is the chord between two samples.
/// `height` is the physical strip depth. Text wraps through the strips and
/// onto the next page; path geometry never depends on text or font metrics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spiral {
    pub radius: Dim,
    pub growth: Dim,
    pub start_millidegrees: i64,
    pub sweep_millidegrees: i64,
    pub segments: u32,
}

/// A region of the page. Its position and size are relative to the page's
/// top-left corner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameTemplate {
    pub name: String,
    pub role: FrameRole,
    pub x: Dim,
    pub y: Dim,
    pub width: Dim,
    pub height: Dim,
    #[serde(default, skip_serializing_if = "is_default_transform")]
    pub transform: FrameTransform,
    #[serde(default, skip_serializing_if = "is_horizontal")]
    pub writing_mode: WritingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Spiral>,
}

fn is_default_transform(t: &FrameTransform) -> bool {
    *t == FrameTransform::default()
}
fn is_horizontal(m: &WritingMode) -> bool {
    *m == WritingMode::HorizontalTb
}

impl FrameTemplate {
    pub fn new(
        name: impl Into<String>,
        role: FrameRole,
        (x, y): (Dim, Dim),
        (width, height): (Dim, Dim),
    ) -> FrameTemplate {
        FrameTemplate {
            name: name.into(),
            role,
            x,
            y,
            width,
            height,
            transform: FrameTransform::default(),
            writing_mode: WritingMode::HorizontalTb,
            path: None,
        }
    }
}

/// A page's size and the frames on it. Every page of the document is made
/// from a template.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageTemplate {
    pub name: String,
    pub width: Dim,
    pub height: Dim,
    /// In threading order: a flow continues from one of its frames to the
    /// next one declared.
    pub frames: Vec<FrameTemplate>,
}

impl PageTemplate {
    pub fn new(name: impl Into<String>, width: Dim, height: Dim) -> PageTemplate {
        PageTemplate {
            name: name.into(),
            width,
            height,
            frames: Vec::new(),
        }
    }

    pub fn with_frame(mut self, frame: FrameTemplate) -> PageTemplate {
        self.frames.push(frame);
        self
    }

    /// The template a document without one gets: 420×300 pt, a main frame at
    /// (36, 36) that is 220 pt wide, and a margin frame 18 pt after it,
    /// 110 pt wide. It is the geometry of the end-to-end spike (40).
    pub fn builtin() -> PageTemplate {
        let depth = Dim::pt(300 - 36 - 36);
        PageTemplate::new("default", Dim::pt(420), Dim::pt(300))
            .with_frame(FrameTemplate::new(
                "main",
                FrameRole::Flow(MAIN_FLOW.into()),
                (Dim::pt(36), Dim::pt(36)),
                (Dim::pt(220), depth),
            ))
            .with_frame(FrameTemplate::new(
                "margin",
                FrameRole::Margin,
                (Dim::pt(36 + 220 + 18), Dim::pt(36)),
                (Dim::pt(110), depth),
            ))
    }
}

/// What is stored in the document for one template name.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    template: PageTemplate,
}

/// One entry of [`Document::page_templates`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTemplate {
    /// The key the template is stored under.
    pub name: String,
    /// The template, or the stored text when this engine can't read it. It
    /// stays in the document either way (34).
    pub template: Result<PageTemplate, String>,
}

/// Before version 3, `"vertical-lr"` meant the upward mode now called
/// `sideways-lr`; reading it that way keeps stored documents laid out as they
/// were (34).
fn legacy_modes(mut template: PageTemplate) -> PageTemplate {
    for frame in &mut template.frames {
        if frame.writing_mode == WritingMode::VerticalLr {
            frame.writing_mode = WritingMode::SidewaysLr;
        }
    }
    template
}

/// Which template layout uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateChoice {
    /// The document has no template: use the built-in one.
    Builtin,
    Template(PageTemplate),
    /// The chosen template is stored but unreadable. Layout reports it and
    /// uses the built-in one.
    Unreadable {
        name: String,
        raw: String,
    },
}

impl Document {
    fn templates_map(&self) -> LoroMap {
        self.doc.get_map("page_templates")
    }

    /// Stores `template` under its name, replacing a template of the same
    /// name. Concurrent definitions of one name resolve to one of them, the
    /// same way on every replica. It does not change which template is in use;
    /// see [`Document::use_page_template`].
    pub fn define_page_template(&self, template: &PageTemplate) -> Result<(), DocError> {
        let modes = template.frames.iter().any(|f| {
            matches!(
                f.writing_mode,
                WritingMode::VerticalLr | WritingMode::SidewaysLr
            )
        });
        let json = serde_json::to_string(&Envelope {
            version: if modes { VERSION_MODES } else { VERSION },
            template: template.clone(),
        })
        .map_err(|e| DocError::Store(e.to_string()))?;
        self.templates_map().insert(&template.name, json)?;
        Ok(())
    }

    /// Stores `raw` under `name` exactly as given, without reading it. For
    /// tools that carry templates between engine versions (migrations, 34) and
    /// for tests of what happens when one can't be read; use
    /// [`Document::define_page_template`] otherwise.
    pub fn store_raw_page_template(&self, name: &str, raw: &str) -> Result<(), DocError> {
        self.templates_map().insert(name, raw)?;
        Ok(())
    }

    /// Defines `template` and makes it the one in use.
    pub fn set_page_template(&self, template: &PageTemplate) -> Result<(), DocError> {
        self.define_page_template(template)?;
        self.use_page_template(&template.name)
    }

    /// Makes the template called `name` the one layout uses. Naming a template
    /// that doesn't exist is allowed: layout then falls back to the built-in
    /// one and says so, since the template may arrive by a merge.
    pub fn use_page_template(&self, name: &str) -> Result<(), DocError> {
        self.doc.get_map("page_setup").insert("template", name)?;
        Ok(())
    }

    /// Removes the template called `name`. Not a tombstone: the name can be
    /// defined again, and the map's own history keeps the old value for undo.
    pub fn remove_page_template(&self, name: &str) -> Result<(), DocError> {
        self.templates_map().delete(name)?;
        Ok(())
    }

    /// Every stored template, sorted by name. Unreadable ones are `Err` with
    /// their stored text.
    pub fn page_templates(&self) -> Vec<StoredTemplate> {
        let map = self.templates_map();
        let mut names: Vec<String> = map.keys().map(|k| k.to_string()).collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|name| {
                let raw = match map.get(&name)? {
                    ValueOrContainer::Value(loro::LoroValue::String(s)) => s.to_string(),
                    // Not a string at all: keep what we can show of it.
                    other => format!("{:?}", other.get_deep_value()),
                };
                let template = match serde_json::from_str::<Envelope>(&raw) {
                    Ok(e) if (1..=VERSION).contains(&e.version) => Ok(legacy_modes(e.template)),
                    Ok(e) if e.version == VERSION_MODES => Ok(e.template),
                    _ => Err(raw),
                };
                Some(StoredTemplate { name, template })
            })
            .collect()
    }

    /// The template layout uses: the one named by [`Document::use_page_template`]
    /// if there is one. With none chosen, the first stored template by name
    /// that can be read, or the built-in one when the document has none.
    pub fn page_template(&self) -> TemplateChoice {
        let all = self.page_templates();
        let chosen = get_str(&self.doc.get_map("page_setup"), "template");
        let found = match &chosen {
            Some(name) => all.into_iter().find(|t| &t.name == name),
            None => all.into_iter().find(|t| t.template.is_ok()),
        };
        // A choice that isn't there (yet) falls back to the built-in template.
        match found {
            Some(StoredTemplate {
                template: Ok(t), ..
            }) => TemplateChoice::Template(t),
            Some(StoredTemplate {
                name,
                template: Err(raw),
            }) => TemplateChoice::Unreadable { name, raw },
            None => TemplateChoice::Builtin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_column() -> PageTemplate {
        let col = |name: &str, x: i32| {
            FrameTemplate::new(
                name,
                FrameRole::Flow(MAIN_FLOW.into()),
                (Dim::pt(x), Dim::pt(30)),
                (Dim::pt(100), Dim::pt(140)),
            )
        };
        PageTemplate::new("two-column", Dim::pt(300), Dim::pt(200))
            .with_frame(col("left", 30))
            .with_frame(col("right", 170))
    }

    #[test]
    fn dims_resolve_against_the_medium_and_page() {
        let medium = Medium::new(Length::from_pt(1000), Length::from_pt(500));
        let page = Some((Length::from_pt(200), Length::from_pt(100)));
        let half = Dim::fraction(Basis::MediumWidth, 500).plus(Length::from_pt(-10));
        assert_eq!(half.resolve(&medium, None), Some(Length::from_pt(490)));
        let of_page = Dim::fraction(Basis::PageHeight, 250);
        assert_eq!(of_page.resolve(&medium, page), Some(Length::from_pt(25)));
        assert_eq!(of_page.resolve(&medium, None), None, "no page yet");
    }

    #[test]
    fn dims_saturate_instead_of_overflowing() {
        let medium = Medium::new(Length::MAX, Length::MIN);
        let huge = Dim::fraction(Basis::MediumWidth, i32::MAX).plus(Length::MAX);
        assert_eq!(huge.resolve(&medium, None), Some(Length::MAX));
        let low = Dim::fraction(Basis::MediumHeight, i32::MAX).plus(Length::MIN);
        assert_eq!(low.resolve(&medium, None), Some(Length::MIN));
    }

    #[test]
    fn templates_round_trip_through_loro() {
        let doc = Document::new(1).unwrap();
        assert_eq!(doc.page_template(), TemplateChoice::Builtin);
        let responsive = PageTemplate::new(
            "screen",
            Dim::fraction(Basis::MediumWidth, 1000),
            Dim::fraction(Basis::MediumHeight, 1000),
        );
        doc.define_page_template(&two_column()).unwrap();
        doc.define_page_template(&responsive).unwrap();
        doc.commit();
        let all = doc.page_templates();
        assert_eq!(
            all.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["screen", "two-column"],
            "sorted by name"
        );
        assert_eq!(all[1].template.as_ref().unwrap(), &two_column());
        // Nothing chosen: the first readable one by name.
        assert_eq!(doc.page_template(), TemplateChoice::Template(responsive));
        doc.use_page_template("two-column").unwrap();
        assert_eq!(doc.page_template(), TemplateChoice::Template(two_column()));
        doc.remove_page_template("two-column").unwrap();
        assert_eq!(
            doc.page_template(),
            TemplateChoice::Builtin,
            "a missing choice falls back to the built-in template"
        );
    }

    #[test]
    fn templates_survive_fork_and_merge() {
        let a = Document::new(1).unwrap();
        a.define_page_template(&two_column()).unwrap();
        a.commit();
        let b = a.fork(2).unwrap();
        // Each peer adds its own template, and both rename the same one.
        a.define_page_template(&PageTemplate::new("a-only", Dim::pt(1), Dim::pt(1)))
            .unwrap();
        b.define_page_template(&PageTemplate::new("b-only", Dim::pt(2), Dim::pt(2)))
            .unwrap();
        let mut shrunk = two_column();
        shrunk.width = Dim::pt(250);
        a.define_page_template(&shrunk).unwrap();
        let mut grown = two_column();
        grown.width = Dim::pt(350);
        b.define_page_template(&grown).unwrap();
        a.merge(&b).unwrap();
        b.merge(&a).unwrap();
        assert_eq!(a.page_templates(), b.page_templates(), "replicas converge");
        let names: Vec<_> = a.page_templates().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["a-only", "b-only", "two-column"]);
    }

    #[test]
    fn unreadable_templates_are_kept_and_reported() {
        let doc = Document::new(1).unwrap();
        for (name, raw) in [
            ("future", r#"{"version":2,"template":{"name":"future"}}"#),
            ("garbage", "not json"),
            (
                "stray-field",
                r#"{"version":1,"template":{"name":"x","width":{"pt":1},"height":{"pt":1},
                   "frames":[],"rotation":90}}"#,
            ),
            (
                "future-role",
                r#"{"version":1,"template":{"name":"x","width":{"pt":1},"height":{"pt":1},
                   "frames":[{"name":"f","role":"float","x":{"pt":0},"y":{"pt":0},
                   "width":{"pt":1},"height":{"pt":1}}]}}"#,
            ),
        ] {
            doc.templates_map().insert(name, raw).unwrap();
        }
        let all = doc.page_templates();
        assert_eq!(all.len(), 4);
        for stored in &all {
            let raw = stored.template.as_ref().unwrap_err();
            assert!(!raw.is_empty());
        }
        // Kept byte for byte.
        assert!(
            all.iter()
                .any(|t| t.template.as_ref().is_err_and(|r| r == "not json"))
        );
        doc.use_page_template("garbage").unwrap();
        assert!(matches!(
            doc.page_template(),
            TemplateChoice::Unreadable { name, .. } if name == "garbage"
        ));
    }

    #[test]
    fn a_template_that_isnt_a_string_is_kept_too() {
        let doc = Document::new(1).unwrap();
        doc.templates_map().insert("odd", 12).unwrap();
        let all = doc.page_templates();
        assert_eq!(all.len(), 1);
        assert!(all[0].template.is_err());
    }
}

#[cfg(test)]
mod geometry_storage_tests {
    use super::*;
    #[test]
    fn transforms_paths_and_modes_round_trip_and_v1_reads() {
        let doc = Document::new(1).unwrap();
        let mut t = PageTemplate::builtin();
        t.frames[0].transform.rotation = Rotation::Direction {
            dx: i32::MIN,
            dy: i32::MAX,
        };
        t.frames[0].transform.mirror_y = true;
        t.frames[0].transform.origin_x = Dim::fraction(Basis::PageWidth, 500);
        t.frames[0].writing_mode = WritingMode::VerticalRl;
        t.frames[0].path = Some(Spiral {
            radius: Dim::pt(10),
            growth: Dim::pt(5),
            start_millidegrees: i64::MIN,
            sweep_millidegrees: i64::MAX,
            segments: u32::MAX,
        });
        doc.set_page_template(&t).unwrap();
        doc.commit();
        assert_eq!(doc.page_template(), TemplateChoice::Template(t.clone()));
        let raw = get_str(&doc.templates_map(), &t.name).unwrap();
        assert_eq!(serde_json::from_str::<Envelope>(&raw).unwrap().version, 2);
        for mode in [WritingMode::VerticalLr, WritingMode::SidewaysLr] {
            let mut moded = t.clone();
            moded.name = format!("{mode:?}");
            moded.frames[1].writing_mode = mode;
            doc.define_page_template(&moded).unwrap();
            let raw = get_str(&doc.templates_map(), &moded.name).unwrap();
            assert_eq!(serde_json::from_str::<Envelope>(&raw).unwrap().version, 3);
            let read = doc
                .page_templates()
                .into_iter()
                .find(|s| s.name == moded.name)
                .unwrap();
            assert_eq!(read.template, Ok(moded));
        }
        let replica = doc.fork(2).unwrap();
        assert_eq!(replica.page_template(), doc.page_template());
        let mut legacy = serde_json::to_value(Envelope {
            version: 1,
            template: PageTemplate::builtin(),
        })
        .unwrap();
        doc.store_raw_page_template("legacy", &legacy.to_string())
            .unwrap();
        assert!(
            doc.page_templates()
                .iter()
                .any(|s| s.name == "legacy" && s.template.is_ok())
        );
        legacy["template"]["frames"][0]["future-transform"] = serde_json::json!(true);
        let raw = legacy.to_string();
        doc.store_raw_page_template("unknown", &raw).unwrap();
        assert!(
            doc.page_templates()
                .iter()
                .any(|s| s.name == "unknown" && s.template == Err(raw.clone()))
        );
    }
}

#[cfg(test)]
mod writing_mode_storage_tests {
    use super::*;

    fn moded(mode: WritingMode) -> PageTemplate {
        let mut t = PageTemplate::builtin();
        t.name = "moded".into();
        t.frames[0].writing_mode = mode;
        t
    }

    fn stored(version: u32, mode: &str) -> String {
        let mut value = serde_json::to_value(Envelope {
            version: 2,
            template: moded(WritingMode::VerticalRl),
        })
        .unwrap();
        value["version"] = serde_json::json!(version);
        value["template"]["frames"][0]["writing_mode"] = serde_json::json!(mode);
        value.to_string()
    }

    fn read(raw: &str) -> Result<PageTemplate, String> {
        let doc = Document::new(1).unwrap();
        doc.store_raw_page_template("moded", raw).unwrap();
        doc.page_templates().remove(0).template
    }

    #[test]
    fn version_two_vertical_lr_is_read_as_the_upward_sideways_mode() {
        for version in [1, 2] {
            let t = read(&stored(version, "vertical-lr")).unwrap();
            assert_eq!(t.frames[0].writing_mode, WritingMode::SidewaysLr);
            let t = read(&stored(version, "vertical-rl")).unwrap();
            assert_eq!(t.frames[0].writing_mode, WritingMode::VerticalRl);
        }
        let t = read(&stored(3, "vertical-lr")).unwrap();
        assert_eq!(t.frames[0].writing_mode, WritingMode::VerticalLr);
        let t = read(&stored(3, "sideways-lr")).unwrap();
        assert_eq!(t.frames[0].writing_mode, WritingMode::SidewaysLr);
    }

    #[test]
    fn templates_without_new_modes_stay_version_two_for_older_engines() {
        let doc = Document::new(1).unwrap();
        for (mode, version) in [
            (WritingMode::HorizontalTb, 2),
            (WritingMode::VerticalRl, 2),
            (WritingMode::VerticalLr, 3),
            (WritingMode::SidewaysLr, 3),
        ] {
            doc.define_page_template(&moded(mode)).unwrap();
            let raw = get_str(&doc.templates_map(), "moded").unwrap();
            let envelope: Envelope = serde_json::from_str(&raw).unwrap();
            assert_eq!(envelope.version, version, "{mode:?}");
            assert_eq!(doc.page_template(), TemplateChoice::Template(moded(mode)));
        }
    }

    #[test]
    fn unknown_versions_and_modes_are_kept_unread() {
        for raw in [
            stored(4, "vertical-lr"),
            stored(3, "sideways-rl"),
            stored(0, "vertical-rl"),
        ] {
            assert_eq!(read(&raw), Err(raw.clone()));
        }
    }
}
