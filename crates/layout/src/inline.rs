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
    /// Which placement of its paragraph this is (see [`FieldInstance`]).
    /// Left out of JSON for content placed once.
    #[serde(skip_serializing_if = "FieldInstance::is_single")]
    pub instance: FieldInstance,
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
    /// The text a composition on its final page can know without any earlier
    /// pass, or `None` to read the plan and guess as usual (sections §10, N6).
    fn direct(&self, _: &InlineObject, _: &FieldEnv<'_>) -> Option<String> {
        None
    }
    /// The text a finished pass resolves, or why it can't.
    fn resolve(&self, object: &InlineObject, at: &FieldContext<'_>) -> Result<String, String>;
    /// The text of a field that couldn't resolve, given the text it was
    /// composed with. Most keep it.
    fn failed(&self, composed: String) -> String {
        composed
    }
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

/// Which placement of an object a resolved field text belongs to.
///
/// An object is one record, but content can be placed more than once: a
/// header or footer shows the same paragraph, with the same objects, on every
/// page, and a repeated table header row shows its cells again on each
/// continuation frame. Each placement is composed separately and may resolve
/// differently (`page-number` is 3 in page 3's footer and 4 in page 4's), so
/// resolved values are keyed by [`FieldKey`], the object and its placement,
/// never by the object alone.
///
/// Non-exhaustive: further placements (a frame chain's frame, with its
/// repetition) are added as variants, so match with a wildcard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum FieldInstance {
    /// The paragraph is placed once, wherever flow puts it: body text, a cell,
    /// a note. Its page is not part of the key, so a value found on one page
    /// can be tried again on another.
    #[default]
    Single,
    /// One page's copy of shared content, by 0-based page index.
    Page(u32),
    /// A running region's content on one page (sections §10): the page, the
    /// slot (so one story in two slots on one page resolves separately; the
    /// variant follows from page and slot) and, for content repeated inside
    /// the instance, an ordinal. In v1 an instance has one frame, so `repeat`
    /// is 0.
    Running {
        page: u32,
        slot: crate::running::SlotId,
        repeat: u32,
    },
}

impl FieldInstance {
    pub fn is_single(&self) -> bool {
        *self == FieldInstance::Single
    }

    /// The instance of content repeated onto 0-based page `page`.
    pub fn of_page(page: usize) -> FieldInstance {
        FieldInstance::Page(u32::try_from(page).unwrap_or(u32::MAX))
    }
}

/// A resolved field text's key: the object and the placement it is resolved
/// for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct FieldKey {
    pub id: InlineId,
    pub instance: FieldInstance,
}

/// The resolved field texts a pass composes with.
pub(crate) type FieldValues = BTreeMap<FieldKey, String>;

/// What a composition read of the field state: the values it found (keyed by
/// object, for one instance) and the page context it guessed with. A running
/// region's memo is valid for another request exactly when these still hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reads {
    pub fields: BTreeMap<FieldKey, Option<String>>,
    /// `(page, pages)` when some field was guessed from the page context.
    pub guessed: Option<(usize, usize)>,
    /// The page label a field was composed with, when one was read directly.
    pub label: Option<String>,
}

/// Where reads are recorded while a composition runs.
pub(crate) type Trace = std::rc::Rc<std::cell::RefCell<Reads>>;

/// The labels of the pages a composition can know.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PageLabels<'a> {
    /// The pages made so far, in order.
    Pages(&'a [crate::PageDescriptor]),
    /// The one page a running region is composed for, before it exists.
    One(usize, &'a str),
}

impl PageLabels<'_> {
    /// The label of 0-based `page`, if it is a page this knows.
    pub fn label(&self, page: usize) -> Option<&str> {
        match self {
            PageLabels::Pages(pages) => pages.get(page).map(|p| p.label.as_str()),
            PageLabels::One(only, label) => (*only == page).then_some(*label),
        }
    }
}

/// What a field may assume while a pass composes: the page being composed,
/// the pages made so far, the placement being composed, and texts a previous
/// pass resolved.
#[derive(Clone, Copy)]
pub(crate) struct FieldEnv<'a> {
    /// 0-based.
    pub page: usize,
    pub pages: usize,
    /// The placement the composed paragraph is: repeated content sets
    /// [`FieldInstance::Page`].
    pub instance: FieldInstance,
    pub values: Option<&'a FieldValues>,
    /// Page labels, and whether `page` is the page the line is finally placed
    /// on (sections §10, N6). A placed composition renders `page-number` from
    /// its page's label directly; an unplaced one (an annotation, composed
    /// before its page is known) reads the previous pass's value.
    pub labels: Option<PageLabels<'a>>,
    pub placed: bool,
    pub trace: Option<&'a std::cell::RefCell<Reads>>,
}

impl<'a> FieldEnv<'a> {
    /// A composition on `page`, placement `instance`, with a previous pass's
    /// `values`.
    pub(crate) fn at(
        page: usize,
        pages: usize,
        instance: FieldInstance,
        values: Option<&'a FieldValues>,
    ) -> Self {
        FieldEnv {
            page,
            pages,
            instance,
            values,
            labels: None,
            placed: false,
            trace: None,
        }
    }
}

impl FieldEnv<'_> {
    /// For compositions whose page isn't known yet (annotations).
    pub(crate) const UNPLACED: FieldEnv<'static> = FieldEnv {
        page: 0,
        pages: 1,
        instance: FieldInstance::Single,
        values: None,
        labels: None,
        placed: false,
        trace: None,
    };

    /// The same environment for the placement `instance`.
    pub(crate) fn placed_as(self, instance: FieldInstance) -> Self {
        FieldEnv { instance, ..self }
    }

    /// What a field on its final page can say without feedback: only where
    /// the page is known to be the one the line lands on (`placed`) and its
    /// label is at hand. An annotation composed before its page is known is
    /// neither, so it keeps reading the previous pass.
    fn direct(&self, object: &InlineObject) -> Option<String> {
        if !self.placed || self.labels.is_none() {
            return None;
        }
        field_function(object)?.direct(object, self)
    }

    /// The text a field composes with.
    pub(crate) fn text(&self, id: InlineId, object: &InlineObject) -> String {
        let key = FieldKey {
            id,
            instance: self.instance,
        };
        if let Some(text) = self.direct(object) {
            if let Some(trace) = self.trace {
                trace.borrow_mut().label = Some(text.clone());
            }
            return text;
        }
        let found = self.values.and_then(|v| v.get(&key));
        if let Some(trace) = self.trace {
            let mut reads = trace.borrow_mut();
            reads.fields.insert(key, found.cloned());
            if found.is_none() {
                reads.guessed = Some((self.page, self.pages));
            }
        }
        if let Some(text) = found {
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
    /// The label of 0-based `page`: its descriptor's when the layout has
    /// sections, else the decimal physical number.
    fn label(&self, page: usize) -> String {
        self.snapshot
            .sections
            .as_ref()
            .and_then(|s| s.pages.get(page))
            .map_or_else(|| (page + 1).to_string(), |p| p.label.clone())
    }

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
    fn direct(&self, _: &InlineObject, env: &FieldEnv<'_>) -> Option<String> {
        env.labels?.label(env.page).map(str::to_owned)
    }
    fn resolve(&self, _: &InlineObject, at: &FieldContext<'_>) -> Result<String, String> {
        Ok(at.label(at.page))
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
            .map(|p| at.label(p - 1))
            .ok_or_else(|| "page-of target is not placed".into())
    }
    /// A target that was resolved and then became unplaced must not keep
    /// showing its old page.
    fn failed(&self, _: String) -> String {
        "?".into()
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

/// The blocks of a finished pass that fields can be in: the flowed and
/// placed blocks, the copies of repeated table header rows, then the blocks of
/// running regions, each page's own.
fn field_blocks(snapshot: &LayoutSnapshot) -> impl Iterator<Item = &crate::BlockLayout> {
    snapshot
        .blocks
        .iter()
        .chain(snapshot.repeated_headers.iter().flat_map(|h| &h.blocks))
        .chain(
            snapshot
                .sections
                .iter()
                .flat_map(|s| &s.running)
                .flat_map(|r| &r.blocks),
        )
}

/// Every field's text as a finished pass places it, with the fields that
/// couldn't resolve (they keep the text they were composed with).
pub(crate) fn resolve_fields(
    doc: &Document,
    snapshot: &LayoutSnapshot,
) -> (FieldValues, Vec<(NodeId, usize, String)>) {
    let mut out = BTreeMap::new();
    let mut failed = Vec::new();
    let mut objects: BTreeMap<NodeId, BTreeMap<InlineId, reprise_doc::InlineEntry>> =
        BTreeMap::new();
    for block in field_blocks(snapshot) {
        if block.lines.iter().all(|l| l.atoms.is_empty()) {
            continue;
        }
        let objects = objects.entry(block.node).or_insert_with(|| {
            doc.inline_objects(block.node)
                .unwrap_or_default()
                .into_iter()
                .map(|e| (e.id, e))
                .collect()
        });
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
                let key = FieldKey {
                    id: atom.id,
                    instance: atom.instance,
                };
                match function.resolve(&entry.object, &at) {
                    Ok(text) => {
                        out.insert(key, text);
                    }
                    Err(why) => {
                        out.insert(key, function.failed(atom.text()));
                        failed.push((block.node, atom.at, why));
                    }
                }
            }
        }
    }
    (out, failed)
}

/// The field texts a pass composed with, read back from its atoms.
pub(crate) fn assumed_fields(snapshot: &LayoutSnapshot) -> FieldValues {
    field_blocks(snapshot)
        .flat_map(|b| &b.lines)
        .flat_map(|l| &l.atoms)
        .filter(|a| a.kind == FIELD)
        .map(|a| {
            (
                FieldKey {
                    id: a.id,
                    instance: a.instance,
                },
                a.text(),
            )
        })
        .collect()
}

/// The field texts the next pass composes with: unchanged when everything
/// resolved to what was assumed (so page numbers settle in one pass).
pub(crate) fn next_fields(
    doc: &Document,
    snapshot: &LayoutSnapshot,
    current: &FieldValues,
) -> FieldValues {
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
    pub instance: FieldInstance,
    pub kind: String,
    pub size: AtomBox,
    pub paint: AtomPaint,
}

impl Sized {
    /// What the object asks of its line above and below the baseline. Text
    /// that is drawn in the surrounding face (a field) asks for nothing: the
    /// line already has the room that text needs, so a page number does not
    /// make its line taller than the line's other words.
    pub(crate) fn line_extent(&self) -> (Length, Length) {
        match self.paint {
            AtomPaint::Text { .. } => (Length::ZERO, Length::ZERO),
            _ => (self.size.ascent, self.size.descent),
        }
    }
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
    // Fields that resolve to the same text in the same style share one
    // shaping: 50,000 page numbers on a page are one request. The style is the
    // whole shaping item but its range (features, language and script change
    // the glyphs), and the function is part of the key because what measuring
    // reports (an unknown function) is cached with the box.
    type Key = (String, Option<&'static str>, String, Length);
    type CachedMeasure = (AtomBox, AtomPaint, Vec<(Severity, Code, String)>);
    let mut shaped: BTreeMap<Key, CachedMeasure> = BTreeMap::new();
    let mut sized = BTreeMap::new();
    for atom in atoms.iter().filter(|a| a.at >= start) {
        let (size, paint) = match &atom.fixed {
            Some((size, paint)) => (*size, paint.clone()),
            None => {
                let text = fields.text(atom.id, &atom.object);
                let env = env(engine, items, atom.at, em, frame, Some(text.clone()));
                let style = env.item.map_or_else(String::new, |item| {
                    format!(
                        "{:?}",
                        Item {
                            range: 0..0,
                            ..item.clone()
                        }
                    )
                });
                let function = field_function(&atom.object).map(|f| f.name());
                let key = (text, function, style, env.em);
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
                instance: fields.instance,
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

/// Reports fields a finished pass couldn't resolve, and drops repeats of what
/// composition reported about an object: a paragraph is composed more than
/// once per pass (a trial for widows and orphans, one fragment per frame), and
/// each composition measures its objects again.
pub(crate) fn field_notes(doc: &Document, snapshot: &mut LayoutSnapshot) {
    let (_, failed) = resolve_fields(doc, snapshot);
    for (node, at, why) in failed {
        let mut d = crate::Diagnostic::new(
            Severity::Warning,
            codes::FIELD_TARGET,
            crate::Subject::Node(node),
            why,
        );
        d.bytes = Some(at..at.saturating_add(reprise_text::OBJECT.len_utf8()));
        snapshot.diagnostics.push(d);
    }
    let mut seen = std::collections::BTreeSet::new();
    snapshot.diagnostics.retain(|d| {
        let about_an_object = d.bytes.is_some()
            && [
                codes::INLINE_KIND,
                codes::IMAGE_SIZE,
                codes::IMAGE_MISSING,
                codes::IMAGE_HEADER,
                codes::IMAGE_LIMIT,
                codes::FIELD_TARGET,
            ]
            .contains(&d.code);
        !about_an_object
            || seen.insert((
                d.code.as_str().to_string(),
                d.subject.clone(),
                d.bytes.clone().map(|b| (b.start, b.end)),
                d.message.clone(),
            ))
    });
}

#[cfg(test)]
#[path = "inline_tests.rs"]
mod tests;
