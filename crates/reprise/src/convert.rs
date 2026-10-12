use crate::*;
use reprise_geom::Length;
pub(crate) fn peer(s: &str) -> Result<u64> {
    if s.len() > 20 {
        return Err(Error::InvalidId(s.into()));
    }
    let p = s.parse::<u64>().map_err(|_| Error::InvalidId(s.into()))?;
    if p == u64::MAX || p.to_string() != s {
        return Err(Error::InvalidId(s.into()));
    }
    Ok(p)
}
pub(crate) fn document_id(s: &str) -> Result<reprise_format::DocumentId> {
    if s.len() != 32
        || !s
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(Error::InvalidId(s.into()));
    }
    let mut bytes = [0; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        let start = i * 2;
        let chunk = s
            .get(start..start + 2)
            .ok_or_else(|| Error::InvalidId(s.into()))?;
        *b = u8::from_str_radix(chunk, 16).map_err(|_| Error::InvalidId(s.into()))?;
    }
    Ok(reprise_format::DocumentId(bytes))
}
pub(crate) fn id(s: &str) -> Result<reprise_doc::NodeId> {
    // A flow paragraph carries both host and marker IDs.
    if s.len() > 80 {
        return Err(Error::InvalidId(s.into()));
    }
    let n = reprise_doc::NodeId::parse(s).ok_or_else(|| Error::InvalidId(s.into()))?;
    if n.to_string() != s {
        return Err(Error::InvalidId(s.into()));
    }
    Ok(n)
}
pub(crate) fn style(s: &Style) -> Result<reprise_doc::Style> {
    let mut out = reprise_doc::Style::default();
    if let Some(f) = &s.families {
        if f.len() > 64 || f.iter().any(|s| s.len() > 1024) {
            return Err(Error::Limit("font families".into()));
        }
        out.families = Some(f.clone());
    }
    for (property, text) in [
        (reprise_doc::Property::Size, &s.size),
        (reprise_doc::Property::LineHeight, &s.line_height),
        (reprise_doc::Property::IndentStart, &s.indent_start),
        (reprise_doc::Property::IndentEnd, &s.indent_end),
        (reprise_doc::Property::IndentFirst, &s.indent_first),
        (reprise_doc::Property::SpaceBefore, &s.space_before),
        (reprise_doc::Property::SpaceAfter, &s.space_after),
    ] {
        if let Some(text) = text {
            out = out
                .with_expr(property, text)
                .map_err(|e| Error::Invalid(e.to_string()))?;
        }
    }
    if s.weight.is_some_and(|w| !(1..=1000).contains(&w)) {
        return Err(Error::Invalid("weight must be in 1..1000".into()));
    }
    out.alignment = s.alignment.map(alignment);
    out.tabs = s.tabs.as_ref().map(tabs).transpose()?;
    out.weight = s.weight;
    out.slant = s.slant.map(slant);
    out.decoration = decoration(s.decoration);
    out.color = s.color;
    for n in [s.widows, s.orphans].into_iter().flatten() {
        if !(1..=reprise_doc::MAX_LINE_KEEP).contains(&n) {
            return Err(Error::Invalid("widows and orphans must be in 1..32".into()));
        }
    }
    out.keep_with_next = s.keep_with_next;
    out.keep_together = s.keep_together;
    out.widows = s.widows;
    out.orphans = s.orphans;
    out.background = s.background;
    Ok(out)
}

/// A used paragraph style as the facade reports it.
pub(crate) fn resolved_style(s: &reprise_doc::ComputedStyle) -> ResolvedStyle {
    ResolvedStyle {
        family: s.family.clone(),
        families: s.families.clone(),
        size: s.size.0,
        line_height: s.line_height.0,
        alignment: s.alignment.map(alignment_out),
        tabs: s.tabs.as_ref().map(tabs_out),
        indent_start: s.indent_start.0,
        indent_end: s.indent_end.0,
        indent_first: s.indent_first.0,
        space_before: s.space_before.0,
        space_after: s.space_after.0,
        keep_with_next: s.keep_with_next,
        keep_together: s.keep_together,
        widows: u8::try_from(s.widows()).unwrap_or(1),
        orphans: u8::try_from(s.orphans()).unwrap_or(1),
        weight: s.weight,
        slant: s.slant.map(slant_out),
        decoration: Decoration {
            underline: s.decoration.underline,
            strike: s.decoration.strike,
        },
        color: s.color,
        background: s.background,
    }
}

/// A document style as the facade's DTO: the authored values, lengths in the
/// expression syntax. A legacy single family is reported as a one-family chain.
pub(crate) fn style_out(s: &reprise_doc::Style) -> Style {
    use reprise_doc::Property as P;
    let text = |p: P| s.get(p).map(|v| v.describe());
    Style {
        alignment: s.alignment.map(alignment_out),
        tabs: s.tabs.as_ref().map(tabs_out),
        weight: s.weight,
        slant: s.slant.map(slant_out),
        decoration: Decoration {
            underline: s.decoration.underline,
            strike: s.decoration.strike,
        },
        color: s.color,
        families: s
            .families
            .clone()
            .or_else(|| s.family.clone().map(|f| vec![f])),
        size: text(P::Size),
        line_height: text(P::LineHeight),
        indent_start: text(P::IndentStart),
        indent_end: text(P::IndentEnd),
        indent_first: text(P::IndentFirst),
        space_before: text(P::SpaceBefore),
        space_after: text(P::SpaceAfter),
        keep_with_next: s.keep_with_next,
        keep_together: s.keep_together,
        widows: s.widows,
        orphans: s.orphans,
        background: s.background,
    }
}

fn tabs_out(t: &reprise_doc::marks::TabStops) -> TabStops {
    TabStops {
        interval: t.interval.0,
        stops: t
            .stops
            .iter()
            .map(|s| TabStop {
                position: s.position.map(|l| l.0),
                alignment: alignment_out(s.alignment),
                leader: s.leader.map(String::from),
            })
            .collect(),
    }
}

/// The stored keys one facade property clears in a paragraph style.
fn style_keys(p: StyleProperty) -> Result<&'static [&'static str]> {
    use StyleProperty as S;
    Ok(match p {
        S::Parent => &["parent"],
        S::Families => &["family", "families"],
        S::Size => &["size"],
        S::LineHeight => &["line-height"],
        S::Alignment => &["alignment"],
        S::Tabs => &["tabs"],
        S::Weight => &["weight"],
        S::Slant => &["slant"],
        S::Underline => &["underline"],
        S::Strike => &["strike"],
        S::Color => &["color"],
        S::IndentStart => &["indent-start"],
        S::IndentEnd => &["indent-end"],
        S::IndentFirst => &["indent-first"],
        S::SpaceBefore => &["space-before"],
        S::SpaceAfter => &["space-after"],
        S::KeepWithNext => &["keep-with-next"],
        S::KeepTogether => &["keep-together"],
        S::Widows => &["widows"],
        S::Orphans => &["orphans"],
        S::Background => &["background"],
        S::Language | S::Features => {
            return Err(Error::Invalid(
                "language and features are character formatting".into(),
            ));
        }
    })
}

/// A paragraph-style patch: the style's supplied fields, and `clear`.
pub(crate) fn style_patch(
    s: &Style,
    parent: Option<&String>,
    clear: &[StyleProperty],
) -> Result<reprise_doc::StylePatch> {
    if clear.len() > 64 {
        return Err(Error::Limit("cleared properties".into()));
    }
    let mut set = style(s)?;
    set.parent = parent.cloned();
    let mut keys = std::collections::BTreeSet::new();
    for p in clear {
        keys.extend(style_keys(*p)?.iter().map(|k| k.to_string()));
    }
    Ok(reprise_doc::StylePatch { set, clear: keys })
}

/// The character fields one facade property clears in `format-text`.
fn text_key(p: StyleProperty) -> Result<&'static str> {
    use StyleProperty as S;
    Ok(match p {
        S::Families => "families",
        S::Size => "size",
        S::Language => "language",
        S::Features => "features",
        S::Weight => "weight",
        S::Slant => "slant",
        S::Underline => "underline",
        S::Strike => "strike",
        S::Color => "color",
        _ => {
            return Err(Error::Invalid(
                "format-text clears character properties only".into(),
            ));
        }
    })
}
fn slant(s: FontStyle) -> reprise_doc::TextSlant {
    match s {
        FontStyle::Normal => reprise_doc::TextSlant::Normal,
        FontStyle::Italic => reprise_doc::TextSlant::Italic,
        FontStyle::Oblique => reprise_doc::TextSlant::Oblique,
    }
}
fn slant_out(s: reprise_doc::TextSlant) -> FontStyle {
    match s {
        reprise_doc::TextSlant::Normal => FontStyle::Normal,
        reprise_doc::TextSlant::Italic => FontStyle::Italic,
        reprise_doc::TextSlant::Oblique => FontStyle::Oblique,
    }
}
fn decoration(s: Decoration) -> reprise_doc::Decoration {
    reprise_doc::Decoration {
        underline: s.underline,
        strike: s.strike,
    }
}
pub(crate) fn kind(k: BlockKind) -> reprise_doc::BlockKind {
    match k {
        BlockKind::Paragraph => reprise_doc::BlockKind::Paragraph,
        BlockKind::Annotation => reprise_doc::BlockKind::Annotation,
        BlockKind::Image => reprise_doc::BlockKind::Image,
    }
}
pub(crate) fn command(c: &Command, medium: reprise_doc::Medium) -> Result<reprise_edit::Command> {
    use reprise_edit::Command as C;
    Ok(match c {
        Command::SetPageSetup { setup } => C::SetPageSetup {
            patch: reprise_doc::page_setup::PageSetupPatch {
                width: setup.width.map(Length),
                height: setup.height.map(Length),
                top: setup.top.map(Length),
                right: setup.right.map(Length),
                bottom: setup.bottom.map(Length),
                left: setup.left.map(Length),
            },
            medium,
        },
        Command::SwapPageOrientation => C::SwapPageOrientation { medium },
        Command::InsertLineBreak { node, at } => C::InsertLineBreak {
            node: id(node)?,
            at: *at as usize,
        },
        Command::InsertTab { node, at } => C::InsertTab {
            node: id(node)?,
            at: *at as usize,
        },
        Command::SetAlignment {
            node,
            at,
            alignment: Some(a),
        } => C::SetAlignment {
            node: id(node)?,
            at: at.map(|v| v as usize),
            alignment: alignment(*a),
        },
        Command::SetAlignment {
            node,
            at,
            alignment: None,
        } => C::ClearAlignment {
            node: id(node)?,
            at: at.map(|v| v as usize),
        },
        Command::SetTabStops { node, tabs: t } => C::SetTabStops {
            node: id(node)?,
            tabs: tabs(t)?,
        },
        Command::AddAnchor {
            node,
            at,
            edge,
            target,
            target_at,
            target_edge,
        } => C::AddAnchor {
            node: id(node)?,
            at: *at as usize,
            target: id(target)?,
            target_at: *target_at as usize,
            edge: match edge {
                LineEdge::Start => reprise_doc::marks::LineEdge::Start,
                LineEdge::End => reprise_doc::marks::LineEdge::End,
            },
            target_edge: match target_edge {
                AnchorEdge::Position => reprise_doc::marks::AnchorEdge::Position,
                AnchorEdge::GapStart => reprise_doc::marks::AnchorEdge::GapStart,
                AnchorEdge::GapEnd => reprise_doc::marks::AnchorEdge::GapEnd,
                AnchorEdge::LineStart => reprise_doc::marks::AnchorEdge::LineStart,
                AnchorEdge::LineEnd => reprise_doc::marks::AnchorEdge::LineEnd,
            },
        },
        Command::RemoveAnchor { id: s } => C::RemoveRelation {
            id: relation_id(s)?,
        },
        Command::InsertImage { .. } => {
            return Err(Error::Invalid(
                "image insertion must be its own transaction".into(),
            ));
        }
        Command::InsertBlock {
            block_kind: BlockKind::Image,
            ..
        } => {
            return Err(Error::Invalid(
                "use insert-image with an asset and alt text".into(),
            ));
        }
        Command::AddRelation { relation: r } => C::AddRelation {
            relation: relation(r)?,
        },
        Command::RemoveRelation { id: s } => C::RemoveRelation {
            id: relation_id(s)?,
        },
        Command::InsertText { node, at, text } => C::InsertText {
            node: id(node)?,
            at: *at as usize,
            text: text.clone(),
        },
        Command::DeleteText { node, start, end } => C::DeleteText {
            node: id(node)?,
            range: *start as usize..*end as usize,
        },
        Command::SplitBlock { node, at } => C::SplitBlock {
            node: id(node)?,
            at: *at as usize,
        },
        Command::JoinBlocks { first, second } => C::JoinBlocks {
            first: id(first)?,
            second: id(second)?,
        },
        Command::InsertBlock {
            parent,
            index,
            block_kind,
            text,
            style: s,
        } => C::InsertBlock {
            parent: parent.as_deref().map(id).transpose()?,
            index: *index as usize,
            block: reprise_doc::NewBlock {
                kind: kind(*block_kind),
                style: String::new(),
                overrides: style(s)?,
                text: text.clone(),
            },
        },
        Command::DeleteBlock { node } => C::DeleteBlock { node: id(node)? },
        Command::MoveBlock {
            node,
            parent,
            index,
        } => C::MoveBlock {
            node: id(node)?,
            parent: parent.as_deref().map(id).transpose()?,
            index: *index as usize,
        },
        Command::SetStyle {
            node,
            style: s,
            clear: None,
        } => C::SetStyleOverride {
            node: id(node)?,
            style: style(s)?,
        },
        Command::SetStyle {
            node,
            style: s,
            clear: Some(clear),
        } => C::PatchOverrides {
            node: id(node)?,
            patch: style_patch(s, None, clear)?,
        },
        Command::DefineStyle {
            name,
            parent,
            style: s,
        } => C::DefineStyle {
            name: name.clone(),
            style: reprise_doc::Style {
                parent: parent.clone(),
                ..style(s)?
            },
        },
        Command::UpdateStyle {
            name,
            parent,
            style: s,
            clear,
        } => C::PatchStyle {
            name: name.clone(),
            patch: style_patch(s, parent.as_ref(), clear)?,
        },
        Command::RenameStyle { from, to } => C::RenameStyle {
            from: from.clone(),
            to: to.clone(),
        },
        Command::DeleteStyle { name } => C::DeleteStyle { name: name.clone() },
        Command::ApplyStyle { node, style: name } => C::SetStyleName {
            node: id(node)?,
            name: name.clone().unwrap_or_default(),
        },
        Command::InsertInline { node, at, object } => C::InsertInline {
            node: id(node)?,
            at: *at as usize,
            object: inline_object(object),
        },
        Command::UpdateInline { node, at, patch } => C::UpdateInline {
            node: id(node)?,
            at: *at as usize,
            patch: inline_patch(patch),
        },
        Command::FormatText {
            node,
            start,
            end,
            style,
        } => C::FormatText {
            node: id(node)?,
            range: *start as usize..*end as usize,
            style: text_style(style)?,
        },
        Command::AddAnnotation {
            node,
            start,
            end,
            annotation,
        } => C::AddAnnotation {
            node: id(node)?,
            range: *start as usize..*end as usize,
            data: annotation_data(annotation)?,
        },
        Command::UpdateAnnotation { id: s, annotation } => C::UpdateAnnotation {
            id: range_id(s)?,
            data: annotation_data(annotation)?,
        },
        Command::RemoveAnnotation { id: s } => C::RemoveAnnotation { id: range_id(s)? },
    })
}

/// The kernel's annotation data for the wire form; ids are parsed here and
/// the payload is validated by the kernel.
pub(crate) fn annotation_data(
    p: &AnnotationPayload,
) -> Result<reprise_doc::annotation::AnnotationData> {
    use reprise_doc::annotation::{AnnotationData, CommentAnchor, LinkTarget};
    Ok(match p {
        AnnotationPayload::Link { target } => AnnotationData::Link(match target {
            LinkDestination::Url { url } => LinkTarget::Url { url: url.clone() },
            LinkDestination::Node { node } => LinkTarget::Node { node: id(node)? },
            LinkDestination::Heading { node } => LinkTarget::Heading { node: id(node)? },
            LinkDestination::Range { range } => LinkTarget::Range {
                range: range_id(range)?,
            },
        }),
        AnnotationPayload::Comment { thread } => AnnotationData::Comment(CommentAnchor {
            thread: thread.clone(),
        }),
    })
}

fn link_destination(t: &reprise_doc::annotation::LinkTarget) -> LinkDestination {
    use reprise_doc::annotation::LinkTarget;
    match t {
        LinkTarget::Url { url } => LinkDestination::Url { url: url.clone() },
        LinkTarget::Node { node } => LinkDestination::Node {
            node: node.to_string(),
        },
        LinkTarget::Heading { node } => LinkDestination::Heading {
            node: node.to_string(),
        },
        LinkTarget::Range { range } => LinkDestination::Range {
            range: range.to_string(),
        },
    }
}

pub(crate) fn annotation_payload(d: &reprise_doc::annotation::AnnotationData) -> AnnotationPayload {
    use reprise_doc::annotation::AnnotationData;
    match d {
        AnnotationData::Link(t) => AnnotationPayload::Link {
            target: link_destination(t),
        },
        AnnotationData::Comment(c) => AnnotationPayload::Comment {
            thread: c.thread.clone(),
        },
    }
}

fn clamp_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The spans and status of an annotation.
fn annotation_where(
    state: &reprise_doc::annotation::AnnotationState,
) -> Option<(AnnotationStatus, Vec<AnnotationSpan>)> {
    use reprise_doc::annotation::AnnotationState;
    Some(match state {
        AnnotationState::Active { pieces, .. } => (
            AnnotationStatus::Active,
            pieces
                .iter()
                .map(|p| AnnotationSpan {
                    node: p.node.to_string(),
                    start: clamp_u32(p.bytes.start),
                    end: clamp_u32(p.bytes.end),
                })
                .collect(),
        ),
        AnnotationState::Orphaned { node, at } => (
            AnnotationStatus::Orphaned,
            node.iter()
                .map(|n| AnnotationSpan {
                    node: n.to_string(),
                    start: clamp_u32(*at),
                    end: clamp_u32(*at),
                })
                .collect(),
        ),
        // A link whose text is all gone is not reported.
        AnnotationState::Gone => return None,
    })
}

pub(crate) fn annotation_out(a: &reprise_doc::annotation::Annotation) -> Option<Annotation> {
    let (status, spans) = annotation_where(&a.state)?;
    Some(Annotation {
        id: a.id.to_string(),
        author: a.author.to_string(),
        status,
        spans,
        payload: annotation_payload(&a.data),
    })
}

pub(crate) fn annotation_brief(a: &reprise_doc::annotation::Annotation) -> Option<AnnotationBrief> {
    use reprise_doc::annotation::{AnnotationData, Kind};
    let (status, spans) = annotation_where(&a.state)?;
    Some(AnnotationBrief {
        id: a.id.to_string(),
        kind: match a.kind() {
            Kind::Link => AnnotationKind::Link,
            Kind::Comment => AnnotationKind::Comment,
        },
        status,
        spans,
        thread: match &a.data {
            AnnotationData::Comment(c) => Some(c.thread.clone()),
            AnnotationData::Link(_) => None,
        },
    })
}

pub(crate) fn annotation_kind(k: AnnotationKind) -> reprise_doc::annotation::Kind {
    match k {
        AnnotationKind::Link => reprise_doc::annotation::Kind::Link,
        AnnotationKind::Comment => reprise_doc::annotation::Kind::Comment,
    }
}

pub(crate) fn annotation_list(l: reprise_doc::annotation::AnnotationList) -> AnnotationList {
    AnnotationList {
        annotations: l.annotations.iter().filter_map(annotation_out).collect(),
        truncated: l.truncated,
    }
}

pub(crate) fn span_rect(r: &reprise_layout::span::SpanRect) -> SpanRect {
    let point = |p: reprise_geom::Point<reprise_geom::PageSpace>| Point { x: p.x.0, y: p.y.0 };
    SpanRect {
        page: clamp_u32(r.page),
        frame: clamp_u32(r.frame),
        node: r.line.node.to_string(),
        line: clamp_u32(r.line.line),
        frame_rect: rect(r.frame_rect),
        bounds: rect(r.bounds),
        corners: r.corners.map(point),
        repeated: r.repeated,
    }
}

pub(crate) fn annotation_geometry(
    id: &reprise_doc::annotation::AnnotationId,
    g: &reprise_layout::span::SpanGeometry,
) -> AnnotationGeometry {
    AnnotationGeometry {
        id: id.to_string(),
        rects: g.rects.iter().map(span_rect).collect(),
        top: g.top.map(|t| SpanTop {
            page: clamp_u32(t.page),
            y: t.y.0,
        }),
        truncated: g.truncated,
        unplaced: clamp_u32(g.unplaced),
    }
}

pub(crate) fn page_link(l: &reprise_layout::span::PageLink) -> PageLink {
    PageLink {
        id: l.id.to_string(),
        target: link_destination(&l.target),
        rects: l.rects.iter().map(span_rect).collect(),
    }
}
fn text_style(s: &TextStyle) -> Result<reprise_doc::formatting::TextStyle> {
    let features = s
        .features
        .as_ref()
        .map(|features| {
            features
                .iter()
                .map(|f| {
                    let tag: [u8; 4] = f.tag.as_bytes().try_into().map_err(|_| {
                        Error::Invalid(
                            "OpenType feature tag must contain four ASCII characters".into(),
                        )
                    })?;
                    Ok(reprise_doc::formatting::TextFeature {
                        tag,
                        value: f.value,
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    Ok(reprise_doc::formatting::TextStyle {
        families: s.families.clone(),
        size: s.size.map(Length),
        language: s.language.clone(),
        features,
        weight: s.weight,
        slant: s.slant.map(slant),
        decoration: decoration(s.decoration),
        color: s.color,
        reset: s.reset,
        clear: s
            .clear
            .iter()
            .map(|p| text_key(*p).map(String::from))
            .collect::<Result<_>>()?,
    })
}
pub(crate) fn text_runs(f: reprise_doc::formatting::TextFormats) -> Option<Vec<TextRun>> {
    f.has_formatting.then(|| {
        f.runs
            .into_iter()
            .map(|run| TextRun {
                start: u32::try_from(run.bytes.start).unwrap_or(u32::MAX),
                end: u32::try_from(run.bytes.end).unwrap_or(u32::MAX),
                style: TextStyle {
                    families: run.style.families,
                    size: run.style.size.map(|l| l.0),
                    language: run.style.language,
                    features: run.style.features.map(|fs| {
                        fs.into_iter()
                            .map(|f| TextFeature {
                                tag: String::from_utf8_lossy(&f.tag).into_owned(),
                                value: f.value,
                            })
                            .collect()
                    }),
                    weight: run.style.weight,
                    slant: run.style.slant.map(slant_out),
                    decoration: Decoration {
                        underline: run.style.decoration.underline,
                        strike: run.style.decoration.strike,
                    },
                    color: run.style.color,
                    reset: false,
                    clear: Vec::new(),
                },
            })
            .collect()
    })
}
pub(crate) fn applied(a: reprise_edit::Applied, notes: Vec<reprise_diag::Note>) -> Applied {
    Applied {
        blocks: a.blocks.iter().map(ToString::to_string).collect(),
        relations: a.relations.iter().map(ToString::to_string).collect(),
        annotations: a.annotations.iter().map(ToString::to_string).collect(),
        effects: a
            .effects
            .into_iter()
            .filter_map(|e| {
                use reprise_edit::Effect as E;
                Some(match e {
                    E::Text {
                        node,
                        at,
                        removed,
                        inserted,
                    } => Effect::Text {
                        node: node.to_string(),
                        at: at as u32,
                        removed: removed as u32,
                        inserted: inserted as u32,
                    },
                    E::Split { node, at, new } => Effect::Split {
                        node: node.to_string(),
                        at: at as u32,
                        new: new.to_string(),
                    },
                    E::Join { first, second, at } => Effect::Join {
                        first: first.to_string(),
                        second: second.to_string(),
                        at: at as u32,
                    },
                    E::Deleted(node) => Effect::Deleted {
                        node: node.to_string(),
                    },
                    E::Moved { node, new } => Effect::Moved {
                        node: node.to_string(),
                        new: new.to_string(),
                    },
                    _ => return None,
                })
            })
            .collect(),
        diagnostics: notes.into_iter().map(crate::error::diagnostic).collect(),
    }
}
pub(crate) fn caret(c: &Caret) -> Result<reprise_edit::Caret> {
    Ok(reprise_edit::Caret {
        node: id(&c.node)?,
        offset: c.offset as usize,
        affinity: match c.affinity {
            Affinity::Upstream => reprise_edit::Affinity::Upstream,
            Affinity::Downstream => reprise_edit::Affinity::Downstream,
        },
    })
}
pub(crate) fn caret_out(c: reprise_edit::Caret) -> Caret {
    Caret {
        node: c.node.to_string(),
        offset: c.offset as u32,
        affinity: match c.affinity {
            reprise_edit::Affinity::Upstream => Affinity::Upstream,
            reprise_edit::Affinity::Downstream => Affinity::Downstream,
        },
    }
}
pub(crate) fn rect<S: reprise_geom::Space>(r: reprise_geom::Rect<S>) -> Rect {
    Rect {
        x: r.origin.x.0,
        y: r.origin.y.0,
        width: r.width.0,
        height: r.height.0,
    }
}
pub(crate) fn revision(r: &reprise_doc::Revision) -> Vec<Clock> {
    r.0.iter()
        .map(|(peer, counter)| Clock {
            peer: peer.to_string(),
            counter: *counter,
        })
        .collect()
}
pub(crate) fn pass(p: reprise_layout::incremental::Pass) -> Pass {
    use reprise_layout::incremental::Pass as P;
    match p {
        P::Flow => Pass::Flow,
        P::RegionFeedback => Pass::RegionFeedback,
        P::Relations => Pass::Relations,
        P::ReadingOrder => Pass::ReadingOrder,
        P::Complete => Pass::Complete,
    }
}
pub(crate) fn movement(m: Movement) -> reprise_edit::Movement {
    use reprise_edit::Movement as M;
    match m {
        Movement::NextGrapheme => M::NextGrapheme,
        Movement::PreviousGrapheme => M::PreviousGrapheme,
        Movement::NextWord => M::NextWord,
        Movement::PreviousWord => M::PreviousWord,
        Movement::VisualRight => M::VisualRight,
        Movement::VisualLeft => M::VisualLeft,
        Movement::InlineForward => M::InlineForward,
        Movement::InlineBackward => M::InlineBackward,
        Movement::LineUp => M::LineUp,
        Movement::LineDown => M::LineDown,
        Movement::LineStart => M::LineStart,
        Movement::LineEnd => M::LineEnd,
        Movement::LineLeftmost => M::LineLeftmost,
        Movement::LineRightmost => M::LineRightmost,
        Movement::LineInlineStart => M::LineInlineStart,
        Movement::LineInlineEnd => M::LineInlineEnd,
        Movement::BlockStart => M::BlockStart,
        Movement::BlockEnd => M::BlockEnd,
        Movement::DocumentStart => M::DocumentStart,
        Movement::DocumentEnd => M::DocumentEnd,
    }
}
pub(crate) fn dimension(d: Dimension) -> reprise_doc::expr::Dim {
    use reprise_doc::expr::Dim as D;
    match d {
        Dimension::Length => D::Length,
        Dimension::Number => D::Number,
        Dimension::Percentage => D::Percentage,
        Dimension::Ratio => D::Ratio,
    }
}
pub(crate) fn capability(c: Capability) -> reprise_plugin::Capability {
    match c {
        Capability::ReadText => reprise_plugin::Capability::ReadText,
        Capability::InsertText => reprise_plugin::Capability::InsertText,
    }
}
pub(crate) fn display(list: reprise_display::DisplayList) -> Result<DisplayList> {
    // The frozen display schema is pinned by the parity test.
    let bytes = serde_json::to_vec(&list).map_err(|e| Error::Invalid(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(e.to_string()))
}
pub(crate) fn layout_diagnostic(d: &reprise_layout::Diagnostic) -> Diagnostic {
    Diagnostic {
        code: d.code.to_string(),
        severity: crate::error::severity(d.severity),
        message: d.message.clone(),
        subject: Some(match &d.subject {
            reprise_layout::Subject::Document => "document".into(),
            reprise_layout::Subject::Node(n) => format!("node:{n}"),
            reprise_layout::Subject::Range(n) => format!("range:{n}"),
            reprise_layout::Subject::Relation(n) => format!("relation:{n}"),
        }),
        start: d.bytes.as_ref().map(|r| r.start as u32),
        end: d.bytes.as_ref().map(|r| r.end as u32),
    }
}
pub(crate) fn length(n: i32) -> Length {
    Length(n)
}

fn relation_id(s: &str) -> Result<reprise_doc::RelationId> {
    if s.len() > 40 {
        return Err(Error::InvalidId(s.into()));
    }
    let id = reprise_doc::RelationId::parse(s).ok_or_else(|| Error::InvalidId(s.into()))?;
    if id.to_string() != s {
        return Err(Error::InvalidId(s.into()));
    }
    Ok(id)
}
pub(crate) fn range_id_of(s: &str) -> Result<reprise_doc::RangeId> {
    range_id(s)
}
fn range_id(s: &str) -> Result<reprise_doc::RangeId> {
    if s.len() > 40 {
        return Err(Error::InvalidId(s.into()));
    }
    let id = reprise_doc::RangeId::parse(s).ok_or_else(|| Error::InvalidId(s.into()))?;
    if id.to_string() != s {
        return Err(Error::InvalidId(s.into()));
    }
    Ok(id)
}
fn transcode<A: serde::Serialize, B: serde::de::DeserializeOwned>(a: &A) -> Result<B> {
    let bytes = serde_json::to_vec(a).map_err(|e| Error::Invalid(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(e.to_string()))
}
pub(crate) fn relation(r: &Relation) -> Result<reprise_doc::Relation> {
    if r.schema.len() > 1024 || r.targets.len() > 256 || r.params.len() > 256 {
        return Err(Error::Limit("relation roles/params".into()));
    }
    let mut out = reprise_doc::Relation::new(r.schema.clone().into());
    out.owner = r.owner.as_deref().map(id).transpose()?;
    let mut count = 0usize;
    for (role, targets) in &r.targets {
        if role.len() > 1024 {
            return Err(Error::Limit("relation role name".into()));
        }
        count = count.saturating_add(targets.len());
        if count > 4096 {
            return Err(Error::Limit("relation targets".into()));
        }
        for target in targets {
            let target = match target {
                Target::Node(n) => reprise_doc::Target::Node(id(n)?),
                Target::Range(n) => reprise_doc::Target::Range(range_id(n)?),
                Target::Structural(q) => reprise_doc::Target::Structural(transcode(q)?),
                Target::Layout(q) => reprise_doc::Target::Layout(transcode(q)?),
                Target::Snapshot(s) => {
                    if s.revision.len() > 4096 {
                        return Err(Error::Limit("snapshot frontier".into()));
                    }
                    let mut clocks = Vec::new();
                    for clock in &s.revision {
                        if clock.counter < 0 {
                            return Err(Error::Invalid("snapshot counter".into()));
                        }
                        clocks.push((peer(&clock.peer)?, clock.counter));
                    }
                    if clocks.windows(2).any(|p| p[0] >= p[1]) {
                        return Err(Error::Invalid("snapshot frontier order".into()));
                    }
                    reprise_doc::Target::Snapshot(reprise_doc::SnapshotRef {
                        version: reprise_doc::Revision(clocks),
                        of: match &s.of {
                            SnapshotSubject::Node(n) => reprise_doc::SnapshotOf::Node(id(n)?),
                            SnapshotSubject::Range(n) => {
                                reprise_doc::SnapshotOf::Range(range_id(n)?)
                            }
                        },
                    })
                }
            };
            out = out.target(role, target);
        }
    }
    for (name, p) in &r.params {
        if name.len() > 1024 {
            return Err(Error::Limit("relation parameter name".into()));
        }
        let p = match p {
            Param::Length(RelativeLength::Pt(n)) => {
                reprise_doc::Param::Length(reprise_doc::LengthExpr::Pt(Length(*n)))
            }
            Param::Length(RelativeLength::Em(n)) => {
                reprise_doc::Param::Length(reprise_doc::LengthExpr::Em(*n))
            }
            Param::Int(n) => {
                if n.len() > 20 {
                    return Err(Error::Invalid("integer parameter".into()));
                }
                let v = n
                    .parse::<i64>()
                    .map_err(|_| Error::Invalid("integer parameter".into()))?;
                if v.to_string() != *n {
                    return Err(Error::Invalid("integer parameter".into()));
                }
                reprise_doc::Param::Int(v)
            }
            Param::Bool(b) => reprise_doc::Param::Bool(*b),
            Param::Text(t) => {
                if t.len() > 1024 * 1024 {
                    return Err(Error::Limit("relation text parameter".into()));
                }
                reprise_doc::Param::Text(t.clone())
            }
        };
        out = out.param(name, p);
    }
    Ok(out)
}
pub(crate) fn schema(s: &RelationSchema) -> Result<reprise_doc::RelationSchema> {
    if s.id.len() > 1024
        || s.roles.len() > 256
        || s.params.len() > 256
        || s.roles
            .iter()
            .any(|r| r.name.len() > 1024 || r.accepts.len() > 5)
        || s.params.iter().any(|p| p.name.len() > 1024)
    {
        return Err(Error::Limit("relation schema".into()));
    }
    transcode(s)
}

pub(crate) fn exported(result: reprise_clipboard::ExportResult) -> Exported {
    Exported {
        content: Bytes {
            bytes: result.bytes,
        },
        losses: result
            .losses
            .features
            .into_iter()
            .map(|loss| Loss {
                code: loss.code.to_string(),
                disposition: match loss.disposition {
                    reprise_clipboard::Disposition::Preserved => Disposition::Preserved,
                    reprise_clipboard::Disposition::Approximated => Disposition::Approximated,
                    reprise_clipboard::Disposition::Dropped => Disposition::Dropped,
                },
                detail: loss.detail,
            })
            .collect(),
        diagnostics: result
            .losses
            .notes
            .into_iter()
            .map(crate::error::diagnostic)
            .collect(),
    }
}

pub(crate) fn alignment(a: Alignment) -> reprise_doc::marks::Alignment {
    match a {
        Alignment::Start => reprise_doc::marks::Alignment::Start,
        Alignment::Centre => reprise_doc::marks::Alignment::Centre,
        Alignment::End => reprise_doc::marks::Alignment::End,
        Alignment::Justify => reprise_doc::marks::Alignment::Justify,
    }
}
fn alignment_out(a: reprise_doc::marks::Alignment) -> Alignment {
    match a {
        reprise_doc::marks::Alignment::Start => Alignment::Start,
        reprise_doc::marks::Alignment::Centre => Alignment::Centre,
        reprise_doc::marks::Alignment::End => Alignment::End,
        reprise_doc::marks::Alignment::Justify => Alignment::Justify,
    }
}
pub(crate) fn tabs(t: &TabStops) -> Result<reprise_doc::marks::TabStops> {
    if t.stops.len() > reprise_doc::marks::MAX_TAB_STOPS {
        return Err(Error::Limit("tab stops".into()));
    }
    let stops = t
        .stops
        .iter()
        .map(|s| {
            let leader = s
                .leader
                .as_ref()
                .map(|raw| {
                    let mut chars = raw.chars();
                    let c = chars
                        .next()
                        .ok_or_else(|| Error::Invalid("empty tab leader".into()))?;
                    if chars.next().is_some() {
                        return Err(Error::Invalid("tab leader must be one character".into()));
                    }
                    Ok(c)
                })
                .transpose()?;
            Ok(reprise_doc::marks::TabStop {
                position: s.position.map(Length),
                alignment: alignment(s.alignment),
                leader,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let out = reprise_doc::marks::TabStops {
        interval: Length(t.interval),
        stops,
    };
    out.validate().map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(out)
}
pub(crate) fn mark(m: reprise_layout::marks::Mark) -> Mark {
    Mark {
        kind: match m.kind {
            reprise_layout::marks::MarkKind::ParagraphEnd => MarkKind::ParagraphEnd,
            reprise_layout::marks::MarkKind::LineBreak => MarkKind::LineBreak,
            reprise_layout::marks::MarkKind::Gap => MarkKind::Gap,
            reprise_layout::marks::MarkKind::Alignment => MarkKind::Alignment,
            reprise_layout::marks::MarkKind::Anchor => MarkKind::Anchor,
        },
        node: m.node.to_string(),
        offset: u32::try_from(m.offset).unwrap_or(u32::MAX),
        line: u32::try_from(m.line).unwrap_or(u32::MAX),
        from: Point {
            x: m.from.x.0,
            y: m.from.y.0,
        },
        to: m.to.map(|p| Point { x: p.x.0, y: p.y.0 }),
        target_page: m.target_page.map(|p| u32::try_from(p).unwrap_or(u32::MAX)),
        alignment: m.alignment.map(alignment_out),
        relation: m.relation.map(|id| id.to_string()),
        state: m.state.map(|s| match s {
            reprise_layout::RelationStatus::Valid => RelationState::Valid,
            reprise_layout::RelationStatus::Rebound => RelationState::Rebound,
            reprise_layout::RelationStatus::Ambiguous => RelationState::Ambiguous,
            reprise_layout::RelationStatus::Missing => RelationState::Missing,
            reprise_layout::RelationStatus::OwnerDeleted => RelationState::OwnerDeleted,
            reprise_layout::RelationStatus::Deleted => RelationState::Deleted,
        }),
        applied: m.applied,
        edge: m.edge.map(|e| match e {
            reprise_doc::marks::LineEdge::Start => LineEdge::Start,
            reprise_doc::marks::LineEdge::End => LineEdge::End,
        }),
        target_node: m.target_node.map(|n| n.to_string()),
        target_offset: m
            .target_offset
            .map(|o| u32::try_from(o).unwrap_or(u32::MAX)),
        target_edge: m.target_edge.map(|e| match e {
            reprise_doc::marks::AnchorEdge::Position => AnchorEdge::Position,
            reprise_doc::marks::AnchorEdge::GapStart => AnchorEdge::GapStart,
            reprise_doc::marks::AnchorEdge::GapEnd => AnchorEdge::GapEnd,
            reprise_doc::marks::AnchorEdge::LineStart => AnchorEdge::LineStart,
            reprise_doc::marks::AnchorEdge::LineEnd => AnchorEdge::LineEnd,
        }),
        diagnostic: m.diagnostic.map(|c| c.to_string()),
    }
}

pub(crate) fn font_declaration(r: &FontDeclaration) -> reprise_font::FontDeclaration {
    reprise_font::FontDeclaration {
        family: r.family.clone(),
        descriptors: reprise_font::Descriptors {
            weight: r.weight,
            stretch: r.stretch,
            style: match r.style {
                FontStyle::Normal => reprise_font::FontStyle::Normal,
                FontStyle::Italic => reprise_font::FontStyle::Italic,
                FontStyle::Oblique => reprise_font::FontStyle::Oblique,
            },
        },
        face_index: r.face_index,
    }
}
pub(crate) fn font_settings(r: &FontSettings) -> reprise_font::FontSettings {
    reprise_font::FontSettings {
        generics: r
            .generics
            .iter()
            .map(|(g, name)| {
                let g = match g {
                    GenericFamily::Serif => reprise_font::GenericFamily::Serif,
                    GenericFamily::SansSerif => reprise_font::GenericFamily::SansSerif,
                    GenericFamily::Monospace => reprise_font::GenericFamily::Monospace,
                    GenericFamily::Script => reprise_font::GenericFamily::Script,
                };
                (g, name.clone())
            })
            .collect(),
        fallbacks: r.fallbacks.clone(),
        synthetic_oblique: r.synthetic_oblique,
    }
}
pub(crate) fn inline_align(a: InlineAlign) -> reprise_doc::InlineAlign {
    match a {
        InlineAlign::Baseline => reprise_doc::InlineAlign::Baseline,
        InlineAlign::Middle => reprise_doc::InlineAlign::Middle,
        InlineAlign::TextTop => reprise_doc::InlineAlign::TextTop,
        InlineAlign::TextBottom => reprise_doc::InlineAlign::TextBottom,
    }
}
fn inline_align_out(a: reprise_doc::InlineAlign) -> InlineAlign {
    match a {
        reprise_doc::InlineAlign::Baseline => InlineAlign::Baseline,
        reprise_doc::InlineAlign::Middle => InlineAlign::Middle,
        reprise_doc::InlineAlign::TextTop => InlineAlign::TextTop,
        reprise_doc::InlineAlign::TextBottom => InlineAlign::TextBottom,
    }
}
pub(crate) fn inline_object(o: &InlineObject) -> reprise_doc::InlineObject {
    reprise_doc::InlineObject {
        kind: o.kind.clone(),
        align: inline_align(o.align),
        params: o.params.clone(),
    }
}
pub(crate) fn inline_patch(p: &InlinePatch) -> reprise_doc::InlinePatch {
    reprise_doc::InlinePatch {
        align: p.align.map(inline_align),
        params: p.params.clone(),
    }
}
pub(crate) fn inline_entry(e: reprise_doc::InlineEntry) -> InlineEntry {
    InlineEntry {
        id: e.id.to_string(),
        at: u32::try_from(e.at).unwrap_or(u32::MAX),
        object: InlineObject {
            kind: e.object.kind,
            align: inline_align_out(e.object.align),
            params: e.object.params,
        },
    }
}
