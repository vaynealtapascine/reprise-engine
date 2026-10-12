//! Inline objects in layout (see `docs/inline.md`): the kind registry, each
//! kind's metrics, paint and text, the field functions, and the resolution
//! of field text from a finished pass (26).
//!
//! Everything is integer `Length` arithmetic; nothing here touches Loro.

use std::collections::BTreeMap;

use reprise_diag::{Code, Severity};
use reprise_display::assets::{HeaderError, image_header};
use reprise_doc::inline::{FIELD, IMAGE};
use reprise_doc::{Document, InlineAlign, InlineId, InlineObject, NodeId};
use reprise_font::FaceId;
use reprise_geom::{FrameSpace, Length, Rect};
use reprise_shape::{Item, ShapedGlyph};
use serde::Serialize;

use crate::{Engine, LayoutSnapshot, codes};

/// The inline extent of a box that has no size of its own: one em.
const DEFAULT_EM: i32 = 1000;

/// An object's box: its inline advance, and its extent above and below the
/// baseline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AtomBox {
    pub advance: Length,
    pub ascent: Length,
    pub descent: Length,
}

/// What an object draws. Kinds paint with ordinary display items only: an
/// image, or glyphs of resolved text in the surrounding style.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "paint", rename_all = "kebab-case")]
pub enum AtomPaint {
    /// Nothing: a zero or unreadable object still has caret positions.
    None,
    Image {
        asset: String,
        alt: String,
        /// The gray box of a missing or unreadable image.
        placeholder: bool,
    },
    /// A field's resolved text, shaped in the surrounding face.
    Text {
        text: String,
        face: FaceId,
        size: Length,
        level: u8,
        /// In visual order; positioned from the object's inline start.
        #[serde(skip)]
        glyphs: Vec<ShapedGlyph>,
    },
}

/// What reading order and tagged export make of an object (33).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomRole {
    /// A figure with alternative text.
    Figure,
    /// Text of its paragraph.
    Text,
}

/// One object placed on a line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AtomLayout {
    pub id: InlineId,
    /// Byte offset of its U+FFFC in the block's text.
    pub at: usize,
    pub kind: String,
    /// The box in frame space: inline start and top, advance by height.
    pub rect: Rect<FrameSpace>,
    pub baseline: Length,
    /// Paint color for text, from the surrounding run.
    #[serde(skip)]
    pub color: [u8; 4],
    #[serde(flatten)]
    pub paint: AtomPaint,
}

impl AtomLayout {
    /// What copy, plain-text export and accessibility read for it.
    pub fn text(&self) -> String {
        match &self.paint {
            AtomPaint::Image { alt, .. } => alt.clone(),
            AtomPaint::Text { text, .. } => text.clone(),
            AtomPaint::None => String::new(),
        }
    }

    pub fn role(&self) -> AtomRole {
        kind(&self.kind).map_or(AtomRole::Text, |k| k.role())
    }
}

/// Everything an object's measurement may depend on.
pub(crate) struct MeasureEnv<'a> {
    pub engine: &'a Engine,
    /// The item (face, size, direction) the object's character is in.
    pub item: Option<&'a Item>,
    /// The surrounding font size: what `em` means.
    pub em: Length,
    /// The surrounding face's ascent and descent at `em`.
    pub text_ascent: Length,
    pub text_descent: Length,
    /// The frame's inline extent (`%` basis and inline clamp) and the
    /// deepest frame's block extent (block clamp).
    pub frame_inline: Length,
    pub frame_block: Length,
    pub vertical: bool,
    /// A field's text, already chosen from the plan or a guess.
    pub field_text: Option<String>,
}

/// A measured object and what measuring it reported.
pub(crate) struct Measured {
    pub size: AtomBox,
    pub paint: AtomPaint,
    pub notes: Vec<(Severity, Code, String)>,
}

/// The extension point: one implementation per stored `kind`.
pub(crate) trait InlineKind: Sync {
    fn name(&self) -> &'static str;
    /// Whether its measurement depends on pagination, so it is measured per
    /// composition rather than once per preparation.
    fn paginated(&self) -> bool {
        false
    }
    /// Its box and paint.
    fn measure(&self, object: &InlineObject, env: &MeasureEnv<'_>) -> Measured;
    fn role(&self) -> AtomRole;
}

/// A field function: what a `field` object's text is (see `FIELDS`).
pub(crate) trait FieldFunction: Sync {
    fn name(&self) -> &'static str;
    /// The text to assume before any pass has resolved it.
    fn guess(&self, object: &InlineObject, env: &FieldEnv<'_>) -> String;
    /// The text a finished pass resolves, or why it can't.
    fn resolve(&self, object: &InlineObject, at: &FieldContext<'_>) -> Result<String, String>;
}

pub(crate) static KINDS: &[&dyn InlineKind] = &[&ImageKind, &FieldKind];
pub(crate) static FIELDS: &[&dyn FieldFunction] = &[&PageNumber, &PageCount, &PageOf, &TextField];

pub(crate) fn kind(name: &str) -> Option<&'static dyn InlineKind> {
    KINDS.iter().copied().find(|k| k.name() == name)
}

fn field_function(object: &InlineObject) -> Option<&'static dyn FieldFunction> {
    let name = object.param("field")?;
    FIELDS.iter().copied().find(|f| f.name() == name)
}

/// Whether an object's box depends on pagination.
pub(crate) fn paginated(object: &InlineObject) -> bool {
    kind(&object.kind).is_some_and(|k| k.paginated())
}

/// Measures any object; unknown kinds are zero boxes with a Warning.
pub(crate) fn measure(object: &InlineObject, env: &MeasureEnv<'_>) -> Measured {
    match kind(&object.kind) {
        Some(k) => k.measure(object, env),
        None => Measured {
            size: AtomBox::default(),
            paint: AtomPaint::None,
            notes: vec![(
                Severity::Warning,
                codes::INLINE_KIND,
                format!(
                    "unknown inline object kind {:?} kept and not drawn",
                    object.kind
                ),
            )],
        },
    }
}

/// Places a box of `width` by `height` against the baseline per `align`.
/// Vertical lines centre upright boxes on the baseline: the inline advance
/// is the box's height and its width spans the line.
fn aligned(width: Length, height: Length, align: InlineAlign, env: &MeasureEnv<'_>) -> AtomBox {
    if env.vertical {
        let half = width.mul_ratio(1, 2);
        return AtomBox {
            advance: height,
            ascent: half,
            descent: width - half,
        };
    }
    let (ascent, descent) = match align {
        InlineAlign::Baseline => (height, Length::ZERO),
        InlineAlign::Middle => {
            // Centred a quarter em above the baseline: half an x-height of
            // half an em, the CSS fallback.
            let centre = env.em.mul_ratio(1, 4);
            let half = height.mul_ratio(1, 2);
            (centre + half, height - half - centre)
        }
        InlineAlign::TextTop => (env.text_ascent, height - env.text_ascent),
        InlineAlign::TextBottom => (height - env.text_descent, env.text_descent),
    };
    AtomBox {
        advance: width,
        ascent,
        descent,
    }
}

struct ImageKind;

impl InlineKind for ImageKind {
    fn name(&self) -> &'static str {
        IMAGE
    }

    fn role(&self) -> AtomRole {
        AtomRole::Figure
    }

    fn measure(&self, object: &InlineObject, env: &MeasureEnv<'_>) -> Measured {
        let mut notes = Vec::new();
        let image = match object.as_image() {
            Ok(image) => Some(image),
            Err(why) => {
                notes.push((Severity::Warning, codes::INLINE_KIND, why));
                None
            }
        };
        if image.is_none() {
            return Measured {
                size: AtomBox::default(),
                paint: AtomPaint::None,
                notes,
            };
        }
        if ["width", "height"]
            .iter()
            .filter_map(|name| object.param(name))
            .filter_map(reprise_doc::InlineSize::parse)
            .any(|s| match s {
                reprise_doc::InlineSize::Pt(l) => l < Length::ZERO,
                reprise_doc::InlineSize::Em(n) | reprise_doc::InlineSize::Percent(n) => n < 0,
            })
        {
            notes.push((
                Severity::Warning,
                codes::IMAGE_SIZE,
                "negative inline image size clamped to zero".into(),
            ));
        }
        let intrinsic = image.as_ref().and_then(|image| {
            let Some(bytes) = env.engine.assets.get(&image.asset) else {
                notes.push((
                    Severity::Warning,
                    codes::IMAGE_MISSING,
                    "inline image asset missing; drawing a placeholder".into(),
                ));
                return None;
            };
            match image_header(bytes) {
                Ok(header) => Some(header),
                Err(error) => {
                    notes.push((
                        Severity::Warning,
                        if error == HeaderError::Limit {
                            codes::IMAGE_LIMIT
                        } else {
                            codes::IMAGE_HEADER
                        },
                        "inline image header unavailable; drawing a placeholder".into(),
                    ));
                    None
                }
            }
        });
        let em = env.em.mul_ratio(DEFAULT_EM, 1000);
        let (natural_w, natural_h) =
            intrinsic.map_or((em, em), |h| (h.physical_width, h.physical_height));
        let resolve =
            |s: Option<reprise_doc::InlineSize>| s.map(|s| s.resolve(env.em, env.frame_inline));
        let authored_w = resolve(image.as_ref().and_then(|i| i.width));
        let authored_h = resolve(image.as_ref().and_then(|i| i.height));
        let (mut width, mut height) = match (authored_w, authored_h) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (
                w,
                intrinsic.map_or_else(
                    || w.mul_ratio(natural_h.0, natural_w.0.max(1)),
                    |h| h.height_for_width(w),
                ),
            ),
            (None, Some(h)) => (
                intrinsic.map_or_else(
                    || h.mul_ratio(natural_w.0, natural_h.0.max(1)),
                    |header| header.width_for_height(h),
                ),
                h,
            ),
            _ => (natural_w, natural_h),
        };
        width = width.max(Length::ZERO);
        height = height.max(Length::ZERO);
        // Clamp to the frame, keeping the aspect ratio: along the line to
        // the frame's inline extent, across it to the deepest frame.
        let (along, across) = if env.vertical {
            (&mut height, &mut width)
        } else {
            (&mut width, &mut height)
        };
        let inline_limit = env.frame_inline.max(Length::ZERO);
        if *along > inline_limit {
            *across = across.mul_ratio(inline_limit.0, along.0);
            *along = inline_limit;
            notes.push((
                Severity::Warning,
                codes::IMAGE_SIZE,
                "inline image scaled to the frame's inline extent".into(),
            ));
        }
        let block_limit = env.frame_block.max(Length::ZERO);
        if *across > block_limit {
            *along = along.mul_ratio(block_limit.0, across.0);
            *across = block_limit;
            notes.push((
                Severity::Warning,
                codes::IMAGE_SIZE,
                "inline image scaled to the frame's block extent".into(),
            ));
        }
        Measured {
            size: aligned(width, height, object.align, env),
            paint: AtomPaint::Image {
                asset: image.as_ref().map_or_else(String::new, |i| i.asset.clone()),
                alt: image.as_ref().map_or_else(String::new, |i| i.alt.clone()),
                placeholder: intrinsic.is_none(),
            },
            notes,
        }
    }
}

struct FieldKind;

impl InlineKind for FieldKind {
    fn name(&self) -> &'static str {
        FIELD
    }

    fn paginated(&self) -> bool {
        true
    }

    fn role(&self) -> AtomRole {
        AtomRole::Text
    }

    fn measure(&self, object: &InlineObject, env: &MeasureEnv<'_>) -> Measured {
        let mut notes = Vec::new();
        if field_function(object).is_none() {
            notes.push((
                Severity::Warning,
                codes::INLINE_KIND,
                format!(
                    "unknown field function {:?} kept and not drawn",
                    object.param("field").unwrap_or_default()
                ),
            ));
        }
        let text = env.field_text.clone().unwrap_or_default();
        let Some(item) = env.item.filter(|_| !text.is_empty()) else {
            return Measured {
                size: AtomBox {
                    advance: Length::ZERO,
                    ascent: env.text_ascent,
                    descent: env.text_descent,
                },
                paint: AtomPaint::None,
                notes,
            };
        };
        // The field's own text decides its direction; page numbers are
        // left to right even in a right-to-left paragraph.
        let rtl = text.chars().find_map(strong_rtl).unwrap_or(false);
        let item = Item {
            range: 0..text.len(),
            level: u8::from(rtl),
            ..item.clone()
        };
        let glyphs = crate::shaping::shape_request(
            &text,
            &item,
            0..text.len(),
            &env.engine.fonts,
            env.engine.shaper.as_ref(),
        );
        let advance = glyphs.iter().map(|g| g.advance).sum::<Length>();
        Measured {
            size: AtomBox {
                advance,
                ascent: env.text_ascent,
                descent: env.text_descent,
            },
            paint: AtomPaint::Text {
                text,
                face: item.face.clone(),
                size: item.size,
                level: item.level,
                glyphs,
            },
            notes,
        }
    }
}

/// `Some(true)` for a strong right-to-left character, `Some(false)` for a
/// strong left-to-right one.
fn strong_rtl(c: char) -> Option<bool> {
    if c.is_ascii_alphabetic() {
        return Some(false);
    }
    match c as u32 {
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF => Some(true),
        _ if c.is_alphabetic() => Some(false),
        _ => None,
    }
}

/// What a field may assume while a pass composes: the page being composed,
/// the pages made so far, and texts a previous pass resolved.
#[derive(Clone, Copy)]
pub(crate) struct FieldEnv<'a> {
    /// 0-based.
    pub page: usize,
    pub pages: usize,
    pub values: Option<&'a BTreeMap<InlineId, String>>,
}

impl FieldEnv<'_> {
    /// For compositions whose page isn't known yet (annotations).
    pub(crate) const UNPLACED: FieldEnv<'static> = FieldEnv {
        page: 0,
        pages: 1,
        values: None,
    };

    /// The text a field composes with.
    pub(crate) fn text(&self, id: InlineId, object: &InlineObject) -> String {
        if let Some(text) = self.values.and_then(|v| v.get(&id)) {
            return text.clone();
        }
        field_function(object).map_or_else(String::new, |f| f.guess(object, self))
    }
}

/// Where a field landed in a finished pass.
pub(crate) struct FieldContext<'a> {
    pub page: usize,
    pub doc: &'a Document,
    pub snapshot: &'a LayoutSnapshot,
}

impl FieldContext<'_> {
    /// The 1-based page of a byte of a placed block.
    fn page_of(&self, node: NodeId, at: usize) -> Option<usize> {
        let line = self.snapshot.line_containing(node, at)?;
        let frame = self.snapshot.line(line)?.frame;
        Some(self.snapshot.frame(frame)?.page + 1)
    }
}

struct PageNumber;
impl FieldFunction for PageNumber {
    fn name(&self) -> &'static str {
        "page-number"
    }
    fn guess(&self, _: &InlineObject, env: &FieldEnv<'_>) -> String {
        (env.page + 1).to_string()
    }
    fn resolve(&self, _: &InlineObject, at: &FieldContext<'_>) -> Result<String, String> {
        Ok((at.page + 1).to_string())
    }
}

struct PageCount;
impl FieldFunction for PageCount {
    fn name(&self) -> &'static str {
        "page-count"
    }
    fn guess(&self, _: &InlineObject, env: &FieldEnv<'_>) -> String {
        env.pages.max(env.page + 1).to_string()
    }
    fn resolve(&self, _: &InlineObject, at: &FieldContext<'_>) -> Result<String, String> {
        Ok(at.snapshot.pages.len().max(1).to_string())
    }
}

/// The page of a persistent position: the first cross-reference.
struct PageOf;
impl FieldFunction for PageOf {
    fn name(&self) -> &'static str {
        "page-of"
    }
    fn guess(&self, _: &InlineObject, _: &FieldEnv<'_>) -> String {
        "?".into()
    }
    fn resolve(&self, object: &InlineObject, at: &FieldContext<'_>) -> Result<String, String> {
        let target = object.param("target").ok_or("page-of has no target")?;
        let (node, offset) = at
            .doc
            .resolve_field_target(target)
            .ok_or("page-of target is unreadable or gone")?;
        at.page_of(node, offset)
            .map(|p| p.to_string())
            .ok_or_else(|| "page-of target is not placed".into())
    }
}

/// Its `text` parameter, verbatim: a field with no pagination dependency.
struct TextField;
impl FieldFunction for TextField {
    fn name(&self) -> &'static str {
        "text"
    }
    fn guess(&self, object: &InlineObject, _: &FieldEnv<'_>) -> String {
        object.param("text").unwrap_or_default().into()
    }
    fn resolve(&self, object: &InlineObject, _: &FieldContext<'_>) -> Result<String, String> {
        Ok(object.param("text").unwrap_or_default().into())
    }
}

/// Every field's text as a finished pass places it, with the fields that
/// couldn't resolve (they keep the text they were composed with).
pub(crate) fn resolve_fields(
    doc: &Document,
    snapshot: &LayoutSnapshot,
) -> (BTreeMap<InlineId, String>, Vec<(NodeId, String)>) {
    let mut out = BTreeMap::new();
    let mut failed = Vec::new();
    for block in &snapshot.blocks {
        let Ok(objects) = doc.inline_objects(block.node) else {
            continue;
        };
        if objects.is_empty() {
            continue;
        }
        let objects: BTreeMap<_, _> = objects.into_iter().map(|e| (e.id, e)).collect();
        for line in &block.lines {
            let Some(page) = snapshot.frame(line.frame).map(|f| f.page) else {
                continue;
            };
            for atom in line.atoms.iter().filter(|a| a.kind == FIELD) {
                let Some(entry) = objects.get(&atom.id) else {
                    continue;
                };
                let Some(function) = field_function(&entry.object) else {
                    continue;
                };
                let at = FieldContext {
                    page,
                    doc,
                    snapshot,
                };
                match function.resolve(&entry.object, &at) {
                    Ok(text) => {
                        out.insert(atom.id, text);
                    }
                    Err(why) => {
                        out.insert(atom.id, atom.text());
                        failed.push((block.node, why));
                    }
                }
            }
        }
    }
    (out, failed)
}

/// The field texts a pass composed with, read back from its atoms.
pub(crate) fn assumed_fields(snapshot: &LayoutSnapshot) -> BTreeMap<InlineId, String> {
    snapshot
        .blocks
        .iter()
        .flat_map(|b| &b.lines)
        .flat_map(|l| &l.atoms)
        .filter(|a| a.kind == FIELD)
        .map(|a| (a.id, a.text()))
        .collect()
}

/// The field texts the next pass composes with: unchanged when everything
/// resolved to what was assumed (so page numbers settle in one pass).
pub(crate) fn next_fields(
    doc: &Document,
    snapshot: &LayoutSnapshot,
    current: &BTreeMap<InlineId, String>,
) -> BTreeMap<InlineId, String> {
    let (resolved, _) = resolve_fields(doc, snapshot);
    if resolved == assumed_fields(snapshot) {
        current.clone()
    } else {
        resolved
    }
}

/// Reports orphaned markers and records (37). Read-only.
pub(crate) fn audit(doc: &Document, diagnostics: &mut Vec<crate::Diagnostic>) {
    let audit = doc.inline_audit();
    for (node, count) in audit.orphan_markers {
        diagnostics.push(crate::Diagnostic::new(
            Severity::Warning,
            codes::INLINE_ORPHAN_MARKER,
            crate::Subject::Node(node),
            format!("{count} object marker(s) without a readable record hidden"),
        ));
    }
    if !audit.orphan_records.is_empty() {
        diagnostics.push(crate::Diagnostic::new(
            Severity::Info,
            codes::INLINE_ORPHAN_RECORD,
            crate::Subject::Document,
            format!(
                "{} inline object record(s) without a marker ignored",
                audit.orphan_records.len()
            ),
        ));
    }
}

/// An object of a prepared paragraph. Kinds that don't depend on
/// pagination are measured once, at preparation.
#[derive(Clone, Debug)]
pub(crate) struct PreparedAtom {
    pub id: InlineId,
    pub at: usize,
    pub object: InlineObject,
    pub fixed: Option<(AtomBox, AtomPaint)>,
}

/// The frame a paragraph is prepared for, as objects see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AtomFrame {
    pub inline: Length,
    pub block: Length,
    pub vertical: bool,
}

/// The surrounding face's ascent and descent at the item holding `at`.
fn text_metrics(engine: &Engine, item: Option<&Item>, em: Length) -> (Length, Length) {
    item.and_then(|item| engine.fonts.get(&item.face).ok())
        .map_or((em.mul_ratio(4, 5), em.mul_ratio(1, 5)), |face| {
            let m = face.metrics();
            (
                Length::from_font_units(m.ascent, em, m.units_per_em),
                Length::from_font_units(m.descent, em, m.units_per_em),
            )
        })
}

fn item_at(items: &[Item], at: usize) -> Option<&Item> {
    items
        .get(items.partition_point(|i| i.range.end <= at))
        .filter(|i| i.range.contains(&at))
}

/// The environment to measure an object at byte `at` in.
pub(crate) fn env<'a>(
    engine: &'a Engine,
    items: &'a [Item],
    at: usize,
    em: Length,
    frame: AtomFrame,
    field_text: Option<String>,
) -> MeasureEnv<'a> {
    let item = item_at(items, at);
    let em = item.map_or(em, |i| i.size);
    let (text_ascent, text_descent) = text_metrics(engine, item, em);
    MeasureEnv {
        engine,
        item,
        em,
        text_ascent,
        text_descent,
        frame_inline: frame.inline,
        frame_block: frame.block,
        vertical: frame.vertical,
        field_text,
    }
}

fn report(
    notes: Vec<(Severity, Code, String)>,
    subject: &crate::Subject,
    at: usize,
    out: &mut Vec<crate::Diagnostic>,
) {
    for (severity, code, message) in notes {
        let mut d = crate::Diagnostic::new(severity, code, subject.clone(), message);
        d.bytes = Some(at..at + reprise_text::OBJECT.len_utf8());
        out.push(d);
    }
}

/// Reads a paragraph's objects and measures those that don't depend on
/// pagination.
pub(crate) fn prepare_atoms(
    engine: &Engine,
    node: NodeId,
    entries: Vec<reprise_doc::InlineEntry>,
    items: &[Item],
    em: Length,
    frame: AtomFrame,
    out: &mut Vec<crate::Diagnostic>,
) -> Vec<PreparedAtom> {
    let subject = crate::Subject::Node(node);
    entries
        .into_iter()
        .map(|e| {
            let fixed = (!paginated(&e.object)).then(|| {
                let measured = measure(&e.object, &env(engine, items, e.at, em, frame, None));
                report(measured.notes, &subject, e.at, out);
                (measured.size, measured.paint)
            });
            PreparedAtom {
                id: e.id,
                at: e.at,
                object: e.object,
                fixed,
            }
        })
        .collect()
}

/// An object's box and paint for one composition.
#[derive(Clone, Debug)]
pub(crate) struct Sized {
    pub id: InlineId,
    pub kind: String,
    pub size: AtomBox,
    pub paint: AtomPaint,
}

/// Measures every object from byte `start` on for one composition.
#[allow(clippy::too_many_arguments)]
pub(crate) fn size_atoms(
    engine: &Engine,
    atoms: &[PreparedAtom],
    items: &[Item],
    em: Length,
    frame: AtomFrame,
    fields: &FieldEnv<'_>,
    start: usize,
    subject: &crate::Subject,
    out: &mut Vec<crate::Diagnostic>,
) -> BTreeMap<usize, Sized> {
    // Fields that resolve to the same text in the same face share one
    // shaping: 50,000 page numbers on a page are one request.
    type Key = (String, Option<FaceId>, Length);
    type CachedMeasure = (AtomBox, AtomPaint, Vec<(Severity, Code, String)>);
    let mut shaped: BTreeMap<Key, CachedMeasure> = BTreeMap::new();
    let mut sized = BTreeMap::new();
    for atom in atoms.iter().filter(|a| a.at >= start) {
        let (size, paint) = match &atom.fixed {
            Some((size, paint)) => (*size, paint.clone()),
            None => {
                let text = fields.text(atom.id, &atom.object);
                let env = env(engine, items, atom.at, em, frame, Some(text.clone()));
                let key = (text, env.item.map(|i| i.face.clone()), env.em);
                let (size, paint, notes) = shaped
                    .entry(key)
                    .or_insert_with(|| {
                        let m = measure(&atom.object, &env);
                        (m.size, m.paint, m.notes)
                    })
                    .clone();
                report(notes, subject, atom.at, out);
                (size, paint)
            }
        };
        sized.insert(
            atom.at,
            Sized {
                id: atom.id,
                kind: atom.object.kind.clone(),
                size,
                paint,
            },
        );
    }
    sized
}

/// Turns each object's glyphs into one advance cell of its box's advance.
pub(crate) fn substitute(runs: &mut [reprise_shape::ShapedRun], sized: &BTreeMap<usize, Sized>) {
    if sized.is_empty() {
        return;
    }
    for run in runs {
        let mut done = std::collections::BTreeSet::new();
        for glyph in &mut run.glyphs {
            let at = glyph.cluster as usize;
            let Some(atom) = sized.get(&at) else {
                continue;
            };
            if done.insert(at) {
                glyph.id = reprise_compose::ATOM_GLYPH;
                glyph.advance = atom.size.advance;
            } else {
                glyph.id = reprise_compose::INVISIBLE_GLYPH;
                glyph.advance = Length::ZERO;
            }
            glyph.x_offset = Length::ZERO;
            glyph.y_offset = Length::ZERO;
            glyph.unsafe_to_break = false;
            glyph.unsafe_to_concat = false;
        }
    }
}

/// Reshaping that keeps objects as advance cells.
pub(crate) struct AtomReshape<'a> {
    pub inner: &'a dyn reprise_shape::Reshape,
    pub sized: &'a BTreeMap<usize, Sized>,
}

impl reprise_shape::Reshape for AtomReshape<'_> {
    fn reshape(&self, range: std::ops::Range<usize>) -> Vec<reprise_shape::ShapedRun> {
        let mut runs = self.inner.reshape(range);
        substitute(&mut runs, self.sized);
        runs
    }
}

/// Reports fields a finished pass couldn't resolve.
pub(crate) fn field_notes(doc: &Document, snapshot: &mut LayoutSnapshot) {
    let (_, failed) = resolve_fields(doc, snapshot);
    for (node, why) in failed {
        snapshot.diagnostics.push(crate::Diagnostic::new(
            Severity::Warning,
            codes::FIELD_TARGET,
            crate::Subject::Node(node),
            why,
        ));
    }
}
