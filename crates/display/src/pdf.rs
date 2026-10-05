//! PDF backend. Glyph runs are real PDF text, positioned glyph by glyph at the
//! coordinates layout chose, so the PDF never re-shapes or re-advances them.
//! Each display list becomes one page.
//!
//! [`render`] draws items in list order and writes no structure. The ordered
//! and tagged entry points ([`render_ordered`], [`render_tagged`]) write a
//! tagged PDF aiming at PDF/UA-1 (ISO 14289-1): every glyph run and image is
//! marked content with an MCID reachable from a `StructTreeRoot`, everything
//! else is an artifact, and the catalog and XMP carry the title, language and
//! `pdfuaid:part`. Text is painted in reading order, and every run carries its
//! source text as ActualText, so extraction gives reading-order text with or
//! without the tags.

use std::collections::{BTreeMap, BTreeSet};
use std::num::{NonZeroU16, NonZeroU32};
use std::ops::Range;

use krilla::Document;
use krilla::SerializeSettings;
use krilla::annotation::{Annotation, LinkAnnotation, Target};
use krilla::color::rgb;
use krilla::configure::{Accessibility, ConfigurationBuilder};
use krilla::destination::XyzDestination;
use krilla::error::KrillaError;
use krilla::geom::{PathBuilder, Point};
use krilla::metadata::Metadata;
use krilla::num::NormalizedF32;
use krilla::outline::{Outline, OutlineNode};
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, Stroke};
use krilla::surface::Surface;
use krilla::tagging::{
    Artifact, ArtifactType, ContentTag, Identifier, Node as TagNode, SpanTag, TableHeaderScope,
    Tag, TagGroup, TagId, TagTree,
};
use krilla::text::{Font, GlyphId, KrillaGlyph};
use reprise_diag::Note;
use reprise_font::{FaceId, FontStore};
use reprise_geom::{Length, Matrix};

use crate::{Color, DisplayList, GlyphRun, Item, Path, RenderError, Segment};

pub mod tags;

use tags::{
    CellRole, Child, Content, DEFAULT_LANG, DEFAULT_TITLE, FIGURE_ALT_MISSING, HEADING_LEVEL, Link,
    MAX_DEPTH, Node, REFERENCE_UNLINKED, RUN_SPLIT, Role, STRUCTURE_DEPTH, Scope, Structure,
    UA_NOT_MET,
};

fn pt(l: Length) -> f32 {
    l.to_pt_f32()
}

fn rgb_of(c: Color) -> rgb::Color {
    rgb::Color::new(c.0, c.1, c.2)
}

fn opacity(c: Color) -> NormalizedF32 {
    NormalizedF32::new(c.alpha()).unwrap_or(NormalizedF32::ONE)
}

fn fill(c: Color) -> Fill {
    Fill {
        paint: rgb_of(c).into(),
        opacity: opacity(c),
        rule: Default::default(),
    }
}

fn stroke(c: Color, width: f32) -> Stroke {
    Stroke {
        paint: rgb_of(c).into(),
        width,
        opacity: opacity(c),
        ..Default::default()
    }
}

fn krilla_path(path: &Path) -> Option<krilla::geom::Path> {
    let mut pb = PathBuilder::new();
    for s in &path.0 {
        match *s {
            Segment::Move(p) => pb.move_to(pt(p.x), pt(p.y)),
            Segment::Line(p) => pb.line_to(pt(p.x), pt(p.y)),
            Segment::Quad(c, p) => pb.quad_to(pt(c.x), pt(c.y), pt(p.x), pt(p.y)),
            Segment::Cubic(a, b, p) => {
                pb.cubic_to(pt(a.x), pt(a.y), pt(b.x), pt(b.y), pt(p.x), pt(p.y))
            }
            Segment::Close => pb.close(),
        }
    }
    pb.finish()
}

fn krilla_matrix(m: &Matrix) -> krilla::geom::Transform {
    krilla::geom::Transform::from_row(
        m.xx.to_f32(),
        m.yx.to_f32(),
        m.xy.to_f32(),
        m.yy.to_f32(),
        pt(m.tx),
        pt(m.ty),
    )
}

/// Renders one page per display list. Each page's size in points matches its list.
/// The result has no structure tree; see [`render_ordered`] and [`render_tagged`].
pub fn render(pages: &[DisplayList], fonts: &FontStore) -> Result<Vec<u8>, RenderError> {
    render_with_assets(pages, fonts, &crate::AssetStore::default())
}

pub fn render_with_assets(
    pages: &[DisplayList],
    fonts: &FontStore,
    assets: &crate::AssetStore,
) -> Result<Vec<u8>, RenderError> {
    let mut doc = Document::new();
    let mut krilla_fonts = BTreeMap::new();
    for list in pages {
        let (w, h) = (pt(list.width), pt(list.height));
        let settings = PageSettings::from_wh(w, h).ok_or(RenderError::BadSize(w, h))?;
        let mut page = doc.start_page_with(settings);
        let mut surface = page.surface();
        background(&mut surface, w, h, false);
        let mut pdf = Pdf {
            fonts,
            assets,
            krilla_fonts: &mut krilla_fonts,
            tagged: false,
            artifact: false,
            id: None,
        };
        pdf.items(&mut surface, &list.items)?;
        surface.finish();
        page.finish();
    }
    doc.finish().map_err(|e| RenderError::Pdf(format!("{e:?}")))
}

/// The white page background; an artifact in a tagged PDF.
fn background(surface: &mut Surface<'_>, w: f32, h: f32, tagged: bool) {
    surface.set_fill(Some(fill(Color(255, 255, 255, 255))));
    let rect = krilla::geom::Rect::from_xywh(0.0, 0.0, w, h);
    let path = rect.and_then(|r| {
        let mut pb = PathBuilder::new();
        pb.push_rect(r);
        pb.finish()
    });
    if let Some(path) = path {
        if tagged {
            surface.start_tagged(ContentTag::Artifact(Artifact::new(
                ArtifactType::Background,
                rect,
            )));
        }
        surface.draw_path(&path);
        if tagged {
            surface.end_tagged();
        }
    }
}

/// A glyph-run or image address in an original display list, indexing Group children.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReadingRun {
    pub page: usize,
    pub path: Vec<usize>,
}

/// Emit text in explicit reading order, as a tagged PDF with a flat structure:
/// one `P` per glyph run and one `Figure` per image. [`render_tagged`] takes the
/// real structure (headings, tables, notes).
///
/// The order must address every glyph and image item exactly once. Nested
/// transforms and clips are preserved. Non-text leaves are artifacts and paint
/// first, in original order.
pub fn render_ordered(
    pages: &[DisplayList],
    fonts: &FontStore,
    order: &[ReadingRun],
) -> Result<Vec<u8>, RenderError> {
    render_ordered_with_assets(pages, fonts, &crate::AssetStore::default(), order)
}

pub fn render_ordered_with_assets(
    pages: &[DisplayList],
    fonts: &FontStore,
    assets: &crate::AssetStore,
    order: &[ReadingRun],
) -> Result<Vec<u8>, RenderError> {
    let children = order
        .iter()
        .map(|run| {
            let role = match item_at(pages, run) {
                Some(Item::Image { alt, .. }) => Role::Figure { alt: alt.clone() },
                _ => Role::P,
            };
            Node::new(role, vec![Child::Content(Content::whole(run.clone()))])
        })
        .collect();
    let structure = Structure {
        title: String::new(),
        lang: String::new(),
        children,
    };
    render_tagged(pages, fonts, assets, &structure).map(|t| t.bytes)
}

/// The item a run address names, if it names one.
fn item_at<'a>(pages: &'a [DisplayList], run: &ReadingRun) -> Option<&'a Item> {
    let (first, rest) = run.path.split_first()?;
    let mut item = pages.get(run.page)?.items.get(*first)?;
    for &index in rest {
        let Item::Group { items, .. } = item else {
            return None;
        };
        item = items.get(index)?;
    }
    Some(item)
}

/// A tagged PDF and what the backend reports about it.
pub struct Tagged {
    pub bytes: Vec<u8>,
    pub notes: Vec<Note>,
    /// Whether the file claims PDF/UA-1 (`pdfuaid:part` in its XMP). It does
    /// not when a requirement of the standard could not be met; a
    /// [`tags::UA_NOT_MET`] note then says which.
    pub ua: bool,
}

/// Renders a tagged PDF, aiming at PDF/UA-1, from a structure whose leaves
/// address the pages' glyph and image items in reading order.
///
/// Every leaf must be a distinct glyph or image item, every glyph and image
/// item must be a leaf, and a leaf's page must exist, or this is an error.
/// Pages stay in physical order, but the structure may name leaves of later
/// pages first: the tree carries reading order, painting does not.
///
/// A requirement the file cannot meet (a missing glyph, text with no mapping)
/// does not fail the export: the file stays tagged but drops the PDF/UA-1 claim,
/// and the notes say so.
pub fn render_tagged(
    pages: &[DisplayList],
    fonts: &FontStore,
    assets: &crate::AssetStore,
    structure: &Structure,
) -> Result<Tagged, RenderError> {
    let plan = Plan::new(pages, structure)?;
    let mut notes = plan.notes.clone();
    match build(pages, fonts, assets, structure, &plan, true) {
        Ok((bytes, extra)) => {
            notes.extend(extra);
            Ok(Tagged {
                bytes,
                notes,
                ua: true,
            })
        }
        Err(Failure::Unmet(unmet)) => {
            notes.push(Note::warning(
                UA_NOT_MET,
                format!(
                    "PDF/UA-1 not met ({}); the file is tagged but does not claim conformance",
                    unmet.join(", ")
                ),
            ));
            let (bytes, extra) = build(pages, fonts, assets, structure, &plan, false)
                .map_err(Failure::into_render)?;
            notes.extend(extra);
            Ok(Tagged {
                bytes,
                notes,
                ua: false,
            })
        }
        Err(other) => Err(other.into_render()),
    }
}

enum Failure {
    /// The PDF/UA-1 validator rejected the file; the requirements it names.
    Unmet(Vec<&'static str>),
    Render(RenderError),
}

impl Failure {
    fn into_render(self) -> RenderError {
        match self {
            Failure::Render(e) => e,
            Failure::Unmet(unmet) => {
                RenderError::Pdf(format!("PDF validation failed: {}", unmet.join(", ")))
            }
        }
    }
}

impl From<RenderError> for Failure {
    fn from(e: RenderError) -> Self {
        Failure::Render(e)
    }
}

fn requirement(error: &krilla::configure::ValidationError) -> &'static str {
    use krilla::configure::ValidationError as V;
    match error {
        V::ContainsNotDefGlyph(..) => "a glyph is missing from its font",
        V::NoCodepointMapping(..) => "a glyph has no text mapping",
        V::InvalidCodepointMapping(..) => "a glyph maps to U+0000, U+FEFF or U+FFFE",
        V::NoDocumentTitle => "no document title",
        V::MissingAltText(..) => "a figure has no alt text",
        V::MissingHeadingTitle => "a heading has no title",
        V::MissingDocumentOutline => "no outline",
        V::MissingAnnotationAltText(..) => "a link has no alt text",
        V::MissingTagging => "no structure tree",
        V::RestrictedLicense(..) => "a font's license restricts embedding",
        _ => "another PDF/UA-1 requirement",
    }
}

/// What a leaf of the structure paints.
struct Leaf {
    /// The item with its enclosing group transforms and clips, ready to paint.
    /// `None` skips painting: an empty split part, or a part kept whole elsewhere.
    item: Option<Item>,
    page: usize,
    /// Painted as an artifact: it sits under a figure without alt text.
    artifact: bool,
}

/// Everything decided before any page is painted.
struct Plan {
    leaves: Vec<Leaf>,
    /// Non-text items per page, wrapped in their groups, in original order.
    artifacts: Vec<Vec<Item>>,
    /// Per node in pre-order: the index of its first leaf, if it has one.
    first_leaf: Vec<Option<usize>>,
    notes: Vec<Note>,
}

type Wrappers = Vec<(Matrix, Option<Path>)>;

fn wrap(item: Item, wrappers: &Wrappers) -> Item {
    let mut wrapped = item;
    for (transform, clip) in wrappers.iter().rev() {
        wrapped = Item::Group {
            transform: *transform,
            clip: clip.clone(),
            items: vec![wrapped],
        };
    }
    wrapped
}

impl Plan {
    fn new(pages: &[DisplayList], structure: &Structure) -> Result<Plan, RenderError> {
        let error = |s: &str| RenderError::Pdf(s.into());
        // Index every glyph and image item by address; other leaves are artifacts.
        let mut text: BTreeMap<ReadingRun, (Wrappers, &Item)> = BTreeMap::new();
        let mut artifacts: Vec<Vec<Item>> = vec![Vec::new(); pages.len()];
        for (page, list) in pages.iter().enumerate() {
            let mut stack = Vec::new();
            for (i, item) in list.items.iter().enumerate().rev() {
                stack.push((item, vec![i], Wrappers::new()));
            }
            while let Some((item, path, parents)) = stack.pop() {
                if path.len() > 256 {
                    return Err(error("ordered PDF group nesting exceeds 256"));
                }
                if let Item::Group {
                    transform,
                    clip,
                    items,
                } = item
                {
                    let mut parents = parents;
                    parents.push((*transform, clip.clone()));
                    for (i, item) in items.iter().enumerate().rev() {
                        let mut child = path.clone();
                        child.push(i);
                        stack.push((item, child, parents.clone()));
                    }
                } else if matches!(item, Item::Glyphs(_) | Item::Image { .. }) {
                    text.insert(ReadingRun { page, path }, (parents, item));
                } else if let Some(out) = artifacts.get_mut(page) {
                    out.push(wrap(item.clone(), &parents));
                }
            }
        }

        // Walk the structure in pre-order, without recursion.
        enum Step<'a> {
            Enter(&'a Node, bool),
            Leave(usize, usize),
            Leaf(&'a Content, bool),
        }
        let mut notes = Vec::new();
        let mut visits: Vec<(&Content, bool)> = Vec::new();
        let mut first_leaf: Vec<Option<usize>> = Vec::new();
        let mut stack: Vec<Step> = structure
            .children
            .iter()
            .rev()
            .map(|n| Step::Enter(n, false))
            .collect();
        while let Some(step) = stack.pop() {
            match step {
                Step::Enter(node, artifact) => {
                    let index = first_leaf.len();
                    first_leaf.push(None);
                    let mut artifact = artifact;
                    if let Role::Figure { alt } = &node.role
                        && alt.trim().is_empty()
                    {
                        artifact = true;
                        notes.push(Note::warning(
                            FIGURE_ALT_MISSING,
                            "a figure has no alt text; it is marked as an artifact",
                        ));
                    }
                    if let Role::Heading { level, .. } = &node.role
                        && !(1..=6).contains(level)
                    {
                        notes.push(Note::warning(
                            HEADING_LEVEL,
                            format!("heading level {level} is outside H1 to H6; clamped"),
                        ));
                    }
                    stack.push(Step::Leave(index, visits.len()));
                    for child in node.children.iter().rev() {
                        stack.push(match child {
                            Child::Node(n) => Step::Enter(n, artifact),
                            Child::Content(c) => Step::Leaf(c, artifact),
                        });
                    }
                }
                Step::Leave(index, start) => {
                    if let Some(slot) = first_leaf.get_mut(index) {
                        *slot = (visits.len() > start).then_some(start);
                    }
                }
                Step::Leaf(content, artifact) => visits.push((content, artifact)),
            }
        }

        // Resolve every leaf to the item it paints.
        let mut whole: BTreeSet<&ReadingRun> = BTreeSet::new();
        let mut parts: BTreeMap<&ReadingRun, Vec<(usize, &Range<usize>)>> = BTreeMap::new();
        for (i, (content, _)) in visits.iter().enumerate() {
            if !text.contains_key(&content.run) {
                return Err(error("ordered PDF has a duplicate or missing run address"));
            }
            match &content.part {
                None => {
                    if !whole.insert(&content.run) || parts.contains_key(&content.run) {
                        return Err(error("ordered PDF has a duplicate or missing run address"));
                    }
                }
                Some(range) => {
                    if whole.contains(&content.run) {
                        return Err(error("ordered PDF has a duplicate or missing run address"));
                    }
                    parts.entry(&content.run).or_default().push((i, range));
                }
            }
        }
        let mut split: BTreeMap<usize, Option<GlyphRun>> = BTreeMap::new();
        for (run, uses) in &parts {
            let Some((wrappers_unused, Item::Glyphs(glyphs))) = text.get(*run) else {
                return Err(error("ordered PDF can only split a glyph run"));
            };
            let _ = wrappers_unused;
            let ranges: Vec<&Range<usize>> = uses.iter().map(|(_, r)| *r).collect();
            match split_glyph_run(glyphs, &ranges) {
                Some(pieces) => {
                    for ((i, _), piece) in uses.iter().zip(pieces) {
                        split.insert(*i, Some(piece));
                    }
                }
                None => {
                    notes.push(Note::warning(
                        RUN_SPLIT,
                        "a glyph run could not be split along its parts; it stays whole",
                    ));
                    // Keep the run whole at its first part; the others paint nothing.
                    for (n, (i, _)) in uses.iter().enumerate() {
                        split.insert(*i, (n == 0).then(|| glyphs.clone()));
                    }
                }
            }
        }
        let mut leaves = Vec::with_capacity(visits.len());
        for (i, (content, artifact)) in visits.iter().enumerate() {
            if content.run.page >= pages.len() {
                return Err(error("ordered PDF page does not exist"));
            }
            let Some((wrappers, item)) = text.get(&content.run) else {
                return Err(error("ordered PDF has a duplicate or missing run address"));
            };
            let item = match split.remove(&i) {
                Some(Some(run)) => Some(wrap(Item::Glyphs(run), wrappers)),
                Some(None) => None,
                None => Some(wrap((*item).clone(), wrappers)),
            };
            leaves.push(Leaf {
                item,
                page: content.run.page,
                artifact: *artifact,
            });
        }
        let covered: BTreeSet<&ReadingRun> = visits.iter().map(|(c, _)| &c.run).collect();
        if text.keys().any(|k| !covered.contains(k)) {
            return Err(error("ordered PDF order omits glyph runs"));
        }
        Ok(Plan {
            leaves,
            artifacts,
            first_leaf,
            notes,
        })
    }
}

/// Splits a glyph run among `parts`, which must tile its text. Each glyph goes
/// to the part that holds its whole source range. `None` when that is not possible.
fn split_glyph_run(run: &GlyphRun, parts: &[&Range<usize>]) -> Option<Vec<GlyphRun>> {
    if needs_run_text(run) {
        return None;
    }
    let mut sorted: Vec<&Range<usize>> = parts.to_vec();
    sorted.sort_by_key(|r| (r.start, r.end));
    let mut end = 0;
    for range in &sorted {
        if range.start != end
            || range.start >= range.end
            || run.text.get(range.start..range.end).is_none()
        {
            return None;
        }
        end = range.end;
    }
    if end != run.text.len() {
        return None;
    }
    parts
        .iter()
        .map(|range| {
            let mut glyphs = Vec::new();
            for glyph in &run.glyphs {
                let (start, end) = (glyph.text.start as usize, glyph.text.end as usize);
                if range.start <= start && end <= range.end {
                    let mut glyph = glyph.clone();
                    glyph.text = start.checked_sub(range.start)? as u32
                        ..end.checked_sub(range.start)? as u32;
                    glyphs.push(glyph);
                } else if start < range.end && range.start < end {
                    return None; // straddles a part boundary
                }
            }
            Some(GlyphRun {
                face: run.face.clone(),
                size: run.size,
                color: run.color,
                text: run.text.get(range.start..range.end)?.to_string(),
                glyphs,
                layer: run.layer,
            })
        })
        .collect()
}

/// What the tag tree builder carries while it walks the structure.
struct Tree<'a> {
    ids: &'a [Option<Identifier>],
    links: &'a BTreeMap<usize, Identifier>,
    leaf: usize,
    node: usize,
    note_ids: BTreeSet<String>,
    notes: Vec<Note>,
}

impl Tree<'_> {
    fn leaf_nodes(&mut self, node: &Node) -> Vec<TagNode> {
        let mut out = Vec::new();
        let mut stack: Vec<&Node> = vec![node];
        // Iterative pre-order over a subtree that is flattened into one group.
        let mut pending: Vec<std::slice::Iter<'_, Child>> = Vec::new();
        self.node += 1;
        pending.push(node.children.iter());
        stack.clear();
        while let Some(children) = pending.last_mut() {
            match children.next() {
                Some(Child::Content(_)) => {
                    if let Some(Some(id)) = self.ids.get(self.leaf) {
                        out.push(TagNode::Leaf(*id));
                    }
                    self.leaf += 1;
                }
                Some(Child::Node(n)) => {
                    self.node += 1;
                    pending.push(n.children.iter());
                }
                None => {
                    pending.pop();
                }
            }
        }
        out
    }

    fn node(&mut self, node: &Node, depth: usize) -> Option<TagNode> {
        if depth >= MAX_DEPTH {
            // Too deep: keep the content, drop the nesting.
            self.notes.push(Note::warning(
                STRUCTURE_DEPTH,
                format!("structure nested deeper than {MAX_DEPTH}; flattened"),
            ));
            let mut leaves = self.leaf_nodes(node);
            return match leaves.len() {
                0 => None,
                _ => {
                    let mut group = TagGroup::new(Tag::P);
                    group.children.append(&mut leaves);
                    Some(group.into())
                }
            };
        }
        let index = self.node;
        self.node += 1;
        let mut children = Vec::new();
        for child in &node.children {
            match child {
                Child::Node(n) => {
                    if let Some(c) = self.node(n, depth + 1) {
                        children.push(c);
                    }
                }
                Child::Content(_) => {
                    if let Some(Some(id)) = self.ids.get(self.leaf) {
                        children.push(TagNode::Leaf(*id));
                    }
                    self.leaf += 1;
                }
            }
        }
        if children.is_empty() {
            return None;
        }
        let mut group = match &node.role {
            Role::P => TagGroup::new(Tag::P),
            Role::Heading { level, title } => TagGroup::new(Tag::Hn(
                NonZeroU16::new(u16::from((*level).clamp(1, 6))).unwrap_or(NonZeroU16::MIN),
                Some(title.clone()),
            )),
            Role::Figure { alt } => TagGroup::new(Tag::Figure(Some(alt.clone()))),
            Role::Note { id } => {
                let id = id
                    .as_ref()
                    .filter(|id| !id.is_empty() && self.note_ids.insert((*id).clone()))
                    .map(|id| TagId::from(id.bytes()));
                TagGroup::new(Tag::Note.with_id(id))
            }
            Role::Reference { .. } => {
                if let Some(link) = self.links.get(&index) {
                    // Reference > Link > (content, annotation).
                    let mut inner = TagGroup::new(Tag::Link);
                    inner.children = children;
                    inner.children.push(TagNode::Leaf(*link));
                    children = vec![inner.into()];
                }
                TagGroup::new(Tag::Reference)
            }
            Role::Div => TagGroup::new(Tag::Div),
            Role::Table => TagGroup::new(Tag::Table),
            Role::Row => TagGroup::new(Tag::TR),
            Role::Cell(cell) => cell_group(cell),
        };
        group.children = children;
        Some(group.into())
    }
}

fn cell_group(cell: &CellRole) -> TagGroup {
    let row_span = NonZeroU32::new(cell.row_span).filter(|n| n.get() > 1);
    let col_span = NonZeroU32::new(cell.col_span).filter(|n| n.get() > 1);
    match cell.header {
        Some(scope) => {
            let scope = match scope {
                Scope::Row => TableHeaderScope::Row,
                Scope::Column => TableHeaderScope::Column,
                Scope::Both => TableHeaderScope::Both,
            };
            TagGroup::new(
                Tag::TH(scope)
                    .with_row_span(row_span)
                    .with_col_span(col_span),
            )
        }
        None => TagGroup::new(Tag::TD.with_row_span(row_span).with_col_span(col_span)),
    }
}

/// The nodes of a structure in pre-order, with the data the later passes need.
fn preorder(structure: &Structure) -> Vec<&Node> {
    let mut out = Vec::new();
    let mut stack: Vec<&Node> = structure.children.iter().rev().collect();
    while let Some(node) = stack.pop() {
        out.push(node);
        for child in node.children.iter().rev() {
            if let Child::Node(n) = child {
                stack.push(n);
            }
        }
    }
    out
}

/// A link annotation to add: (node index, area, target page, target point, alt).
type PlannedLink = (usize, krilla::geom::Rect, usize, Point, String);

fn build(
    pages: &[DisplayList],
    fonts: &FontStore,
    assets: &crate::AssetStore,
    structure: &Structure,
    plan: &Plan,
    validate: bool,
) -> Result<(Vec<u8>, Vec<Note>), Failure> {
    let mut configuration = ConfigurationBuilder::new();
    if validate {
        configuration = configuration.with_accessibility_validator(Accessibility::UA1);
    }
    let settings = SerializeSettings {
        configuration: configuration
            .finish()
            .map_err(|e| RenderError::Pdf(format!("{e:?}")))?,
        enable_tagging: true,
        ..Default::default()
    };
    let mut doc = Document::new_with(settings);

    let nodes = preorder(structure);
    // Where a link to a keyed node lands: its first leaf's page, and its point.
    let mut targets: BTreeMap<u32, (usize, Point)> = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        let (Some(key), Some(leaf)) = (node.key, plan.first_leaf.get(index).copied().flatten())
        else {
            continue;
        };
        let (Some(at), Some(leaf)) = (node.at, plan.leaves.get(leaf)) else {
            continue;
        };
        targets
            .entry(key)
            .or_insert((leaf.page, Point::from_xy(pt(at.x), pt(at.y))));
    }
    // Link annotations per page, in pre-order: (node index, annotation).
    let mut notes = Vec::new();
    let mut link_plan: Vec<Vec<PlannedLink>> = vec![Vec::new(); pages.len()];
    for (index, node) in nodes.iter().enumerate() {
        let Role::Reference { link } = &node.role else {
            continue;
        };
        let Some(Link { rect, target, alt }) = link else {
            continue;
        };
        let first = plan.first_leaf.get(index).copied().flatten();
        let page = first.and_then(|l| plan.leaves.get(l)).map(|l| l.page);
        let area = krilla::geom::Rect::from_xywh(
            pt(rect.origin.x),
            pt(rect.origin.y),
            pt(rect.width),
            pt(rect.height),
        );
        match (page, area, targets.get(target)) {
            (Some(page), Some(area), Some((to_page, to_point))) => {
                if let Some(list) = link_plan.get_mut(page) {
                    list.push((index, area, *to_page, *to_point, alt.clone()));
                }
            }
            _ => notes.push(Note::info(
                REFERENCE_UNLINKED,
                "a note reference could not be linked to its note",
            )),
        }
    }

    let mut ids: Vec<Option<Identifier>> = vec![None; plan.leaves.len()];
    let mut link_ids: BTreeMap<usize, Identifier> = BTreeMap::new();
    let mut by_page: Vec<Vec<usize>> = vec![Vec::new(); pages.len()];
    for (i, leaf) in plan.leaves.iter().enumerate() {
        if let Some(list) = by_page.get_mut(leaf.page) {
            list.push(i);
        }
    }
    let mut krilla_fonts = BTreeMap::new();
    for (p, list) in pages.iter().enumerate() {
        let (w, h) = (pt(list.width), pt(list.height));
        let settings = PageSettings::from_wh(w, h).ok_or(RenderError::BadSize(w, h))?;
        let mut page = doc.start_page_with(settings);
        let mut surface = page.surface();
        background(&mut surface, w, h, true);
        let mut pdf = Pdf {
            fonts,
            assets,
            krilla_fonts: &mut krilla_fonts,
            tagged: true,
            artifact: true,
            id: None,
        };
        if let Some(items) = plan.artifacts.get(p) {
            pdf.items(&mut surface, items)?;
        }
        for &i in by_page.get(p).map(Vec::as_slice).unwrap_or_default() {
            let Some(leaf) = plan.leaves.get(i) else {
                continue;
            };
            let Some(item) = &leaf.item else {
                continue;
            };
            pdf.artifact = leaf.artifact;
            pdf.id = None;
            pdf.items(&mut surface, std::slice::from_ref(item))?;
            if let (Some(slot), false) = (ids.get_mut(i), leaf.artifact) {
                *slot = pdf.id.take();
            }
        }
        surface.finish();
        for (index, area, to_page, to_point, alt) in link_plan.get(p).into_iter().flatten() {
            let link = LinkAnnotation::new(
                *area,
                Target::Destination(XyzDestination::new(*to_page, *to_point).into()),
            );
            let id = page.add_tagged_annotation(Annotation::new_link(link, Some(alt.clone())));
            link_ids.insert(*index, id);
        }
        page.finish();
    }

    let mut tree = TagTree::new().with_lang(Some(lang_of(structure)));
    let mut builder = Tree {
        ids: &ids,
        links: &link_ids,
        leaf: 0,
        node: 0,
        note_ids: BTreeSet::new(),
        notes: Vec::new(),
    };
    for node in &structure.children {
        if let Some(n) = builder.node(node, 0) {
            tree.push(n);
        }
    }
    notes.append(&mut builder.notes);
    doc.set_tag_tree(tree);
    doc.set_outline(outline(&nodes, plan));
    doc.set_metadata(
        Metadata::new()
            .title(title_of(structure))
            .language(lang_of(structure))
            .creator("reprise-engine".into()),
    );
    match doc.finish() {
        Ok(bytes) => Ok((bytes, notes)),
        Err(KrillaError::Validation(errors)) => {
            let mut unmet: Vec<&'static str> = errors.iter().map(|(e, _)| requirement(e)).collect();
            unmet.sort_unstable();
            unmet.dedup();
            Err(Failure::Unmet(unmet))
        }
        Err(e) => Err(Failure::Render(RenderError::Pdf(format!("{e:?}")))),
    }
}

fn lang_of(structure: &Structure) -> String {
    match structure.lang.trim() {
        "" => DEFAULT_LANG.into(),
        lang => lang.into(),
    }
}

fn title_of(structure: &Structure) -> String {
    match structure.title.trim() {
        "" => DEFAULT_TITLE.into(),
        title => title.into(),
    }
}

/// The headings as a nested outline: a heading sits under the nearest earlier
/// heading of a lower level.
fn outline(nodes: &[&Node], plan: &Plan) -> Outline {
    let mut root = Outline::new();
    let mut open: Vec<(u8, OutlineNode)> = Vec::new();
    let close = |open: &mut Vec<(u8, OutlineNode)>, root: &mut Outline, down_to: u8| {
        while let Some((level, _)) = open.last() {
            if *level < down_to {
                break;
            }
            if let Some((_, done)) = open.pop() {
                match open.last_mut() {
                    Some((_, parent)) => parent.push_child(done),
                    None => root.push_child(done),
                }
            }
        }
    };
    for (index, node) in nodes.iter().enumerate() {
        let Role::Heading { level, title } = &node.role else {
            continue;
        };
        let Some(leaf) = plan
            .first_leaf
            .get(index)
            .copied()
            .flatten()
            .and_then(|l| plan.leaves.get(l))
        else {
            continue;
        };
        if title.trim().is_empty() {
            continue;
        }
        let level = (*level).clamp(1, 6);
        let at = node.at.map_or(Point::from_xy(0.0, 0.0), |p| {
            Point::from_xy(pt(p.x), pt(p.y))
        });
        close(&mut open, &mut root, level);
        open.push((
            level,
            OutlineNode::new(title.clone(), XyzDestination::new(leaf.page, at)),
        ));
    }
    close(&mut open, &mut root, 0);
    root
}

struct Pdf<'a> {
    fonts: &'a FontStore,
    assets: &'a crate::AssetStore,
    krilla_fonts: &'a mut BTreeMap<FaceId, Font>,
    /// Marks every leaf as content or artifact and records its marked-content id.
    tagged: bool,
    /// The leaf being painted is an artifact. Only read when `tagged`.
    artifact: bool,
    /// The marked-content id of the content leaf just painted.
    id: Option<Identifier>,
}

impl Pdf<'_> {
    fn begin(&mut self, surface: &mut Surface<'_>, span: SpanTag<'_>) {
        let tag = if self.tagged && self.artifact {
            ContentTag::Artifact(Artifact::with_kind(ArtifactType::Other))
        } else {
            ContentTag::Span(span)
        };
        let id = surface.start_tagged(tag);
        if self.tagged && !self.artifact {
            self.id = Some(id);
        }
    }

    fn items(&mut self, surface: &mut Surface<'_>, items: &[Item]) -> Result<(), RenderError> {
        for item in items {
            match item {
                Item::Glyphs(run) => self.glyphs(surface, run)?,
                Item::Image {
                    asset, rect, alt, ..
                } => {
                    self.begin(
                        surface,
                        SpanTag {
                            actual_text: Some(alt),
                            ..SpanTag::empty()
                        },
                    );
                    if rect.width <= Length::ZERO || rect.height <= Length::ZERO {
                        // Keep semantic text for an authored zero-size image.
                        let marker = reprise_geom::Rect::new(rect.origin, Length(1), Length(1));
                        if let Some(path) = krilla_path(&Path::rect(marker)) {
                            surface.set_fill(Some(fill(Color(0, 0, 0, 0))));
                            surface.set_stroke(None);
                            surface.draw_path(&path);
                        }
                        surface.end_tagged();
                        continue;
                    }
                    if let Some((w, h, rgba)) =
                        self.assets.get(asset).and_then(crate::image_pixels::decode)
                    {
                        if let Some(size) =
                            krilla::geom::Size::from_wh(pt(rect.width), pt(rect.height))
                        {
                            surface.push_transform(&krilla::geom::Transform::from_translate(
                                pt(rect.origin.x),
                                pt(rect.origin.y),
                            ));
                            surface.draw_image(krilla::image::Image::from_rgba8(rgba, w, h), size);
                            surface.pop();
                        }
                    } else if let Some(path) = krilla_path(&Path::rect(*rect)) {
                        surface.set_fill(Some(fill(Color(221, 221, 221, 255))));
                        surface.set_stroke(None);
                        surface.draw_path(&path);
                    }
                    surface.end_tagged();
                }
                Item::Path {
                    path,
                    fill: fill_color,
                    stroke: stroke_style,
                    ..
                } => {
                    let Some(path) = krilla_path(path) else {
                        continue;
                    };
                    surface.set_fill(fill_color.map(fill));
                    surface.set_stroke(stroke_style.map(|s| stroke(s.color, pt(s.width))));
                    if self.tagged {
                        surface.start_tagged(ContentTag::Artifact(Artifact::with_kind(
                            ArtifactType::Layout,
                        )));
                    }
                    surface.draw_path(&path);
                    if self.tagged {
                        surface.end_tagged();
                    }
                }
                Item::Group {
                    transform,
                    clip,
                    items,
                } => {
                    surface.push_transform(&krilla_matrix(transform));
                    let clip = clip.as_ref().and_then(krilla_path);
                    if let Some(path) = &clip {
                        surface.push_clip_path(path, &FillRule::NonZero);
                    }
                    self.items(surface, items)?;
                    if clip.is_some() {
                        surface.pop();
                    }
                    surface.pop();
                }
            }
        }
        Ok(())
    }

    fn glyphs(&mut self, surface: &mut Surface<'_>, run: &GlyphRun) -> Result<(), RenderError> {
        if run.glyphs.is_empty() || run.size <= Length::ZERO {
            return Ok(());
        }
        let font = match self.krilla_fonts.get(&run.face) {
            Some(f) => f.clone(),
            None => {
                // A face from a collection (TTC/OTC) is one of several in the
                // same bytes; krilla must open the same one shaping used.
                let face = self.fonts.get(&run.face)?;
                let font = Font::new(face.data().to_vec().into(), face.declaration().face_index)
                    .ok_or_else(|| RenderError::Pdf("krilla could not read the font".into()))?;
                self.krilla_fonts.insert(run.face.clone(), font.clone());
                font
            }
        };
        surface.set_fill(Some(fill(run.color)));
        surface.set_stroke(None);
        // krilla must see all glyphs in a cluster together to emit ActualText
        // rather than duplicate the source once per glyph. Zero advances and
        // offsets from the origin keep the layout's absolute glyph positions.
        let size = pt(run.size);
        let fallback = needs_run_text(run);
        let glyphs: Vec<_> = run
            .glyphs
            .iter()
            .map(|g| {
                let range = if fallback {
                    0..run.text.len()
                } else {
                    g.text.start as usize..g.text.end as usize
                };
                KrillaGlyph::new(
                    GlyphId::new(g.id),
                    0.0,
                    pt(g.x) / size,
                    -pt(g.y) / size,
                    0.0,
                    range,
                    None,
                )
            })
            .collect();
        if self.tagged {
            self.begin(
                surface,
                SpanTag {
                    actual_text: Some(&run.text),
                    ..SpanTag::empty()
                },
            );
        }
        surface.draw_glyphs(
            Point::from_xy(0.0, 0.0),
            &glyphs,
            font,
            &run.text,
            size,
            false,
        );
        if self.tagged {
            surface.end_tagged();
        }
        Ok(())
    }
}

/// A valid LTR mapping partitions all source bytes, with repeated ranges
/// allowed for a cluster. Everything else (RTL, holes, overlaps, empty or
/// invalid UTF-8 ranges) falls back to one source span for the entire run.
/// A single glyph can express that span through ToUnicode; multiple glyphs
/// make krilla emit ActualText. Neither case guesses missing characters.
fn needs_run_text(run: &GlyphRun) -> bool {
    let mut end = 0;
    let mut previous = None;
    for glyph in &run.glyphs {
        let range = glyph.text.start as usize..glyph.text.end as usize;
        if range.is_empty() || run.text.get(range.clone()).is_none() {
            return true;
        }
        if previous.as_ref() != Some(&range) {
            if range.start != end {
                return true;
            }
            end = range.end;
            previous = Some(range);
        }
    }
    end != run.text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample;

    #[test]
    fn renders_one_page_per_list() {
        let (list, fonts) = sample::list();
        let bytes = render(&[list.clone(), list], &fonts).expect("renders");
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(
            bytes.len() > 1000,
            "embeds the font subset: {} bytes",
            bytes.len()
        );
        assert!(
            bytes.windows(5).any(|w| w == b"/Font"),
            "text is drawn with a font, not outlines"
        );
        let pages = bytes
            .windows(10)
            .filter(|w| w == b"/Type /Pag" || w == b"/Type/Page")
            .count();
        assert!(pages >= 2, "two pages: {pages}");
    }

    #[test]
    fn no_pages_is_still_a_pdf() {
        let (_, fonts) = sample::list();
        let bytes = render(&[], &fonts).expect("renders");
        assert!(bytes.starts_with(b"%PDF-"));
    }
}
