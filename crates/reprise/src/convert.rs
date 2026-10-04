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
    if s.len() > 40 {
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
    ] {
        if let Some(text) = text {
            out = out
                .with_expr(property, text)
                .map_err(|e| Error::Invalid(e.to_string()))?;
        }
    }
    Ok(out)
}
pub(crate) fn kind(k: BlockKind) -> reprise_doc::BlockKind {
    match k {
        BlockKind::Paragraph => reprise_doc::BlockKind::Paragraph,
        BlockKind::Annotation => reprise_doc::BlockKind::Annotation,
    }
}
pub(crate) fn command(c: &Command) -> Result<reprise_edit::Command> {
    use reprise_edit::Command as C;
    Ok(match c {
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
        Command::SetStyle { node, style: s } => C::SetStyleOverride {
            node: id(node)?,
            style: style(s)?,
        },
    })
}
pub(crate) fn applied(a: reprise_edit::Applied, notes: Vec<reprise_diag::Note>) -> Applied {
    Applied {
        blocks: a.blocks.iter().map(ToString::to_string).collect(),
        relations: a.relations.iter().map(ToString::to_string).collect(),
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
