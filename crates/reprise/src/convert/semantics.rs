//! Owned semantic DTOs and bounded boundary validation.
use super::*;

pub(crate) fn role_out(role: reprise_doc::Role) -> BlockRole {
    use reprise_doc::Role as R;
    match role {
        R::Body => BlockRole::Body,
        R::Heading(1) => BlockRole::Heading1,
        R::Heading(2) => BlockRole::Heading2,
        R::Heading(3) => BlockRole::Heading3,
        R::Heading(4) => BlockRole::Heading4,
        R::Heading(5) => BlockRole::Heading5,
        R::Heading(_) => BlockRole::Heading6,
        R::Title => BlockRole::Title,
        R::Subtitle => BlockRole::Subtitle,
        R::Quote => BlockRole::Quote,
        R::Code => BlockRole::Code,
        R::Verse => BlockRole::Verse,
    }
}
fn role_in(role: BlockRole) -> reprise_doc::Role {
    use reprise_doc::Role as R;
    match role {
        BlockRole::Body => R::Body,
        BlockRole::Heading1 => R::Heading(1),
        BlockRole::Heading2 => R::Heading(2),
        BlockRole::Heading3 => R::Heading(3),
        BlockRole::Heading4 => R::Heading(4),
        BlockRole::Heading5 => R::Heading(5),
        BlockRole::Heading6 => R::Heading(6),
        BlockRole::Title => R::Title,
        BlockRole::Subtitle => R::Subtitle,
        BlockRole::Quote => R::Quote,
        BlockRole::Code => R::Code,
        BlockRole::Verse => R::Verse,
    }
}
fn list_id(raw: &str) -> Result<reprise_doc::ListId> {
    if raw.len() > 40 {
        return Err(Error::InvalidId(raw.into()));
    }
    let id = reprise_doc::ListId::parse(raw).ok_or_else(|| Error::InvalidId(raw.into()))?;
    if id.to_string() != raw {
        return Err(Error::InvalidId(raw.into()));
    }
    Ok(id)
}
fn list_ref(r: &ListRef) -> Result<reprise_edit::ListRef> {
    Ok(match r {
        ListRef::Existing { id } => reprise_edit::ListRef::Existing(list_id(id)?),
        ListRef::New { index } => reprise_edit::ListRef::New(*index as usize),
    })
}
fn counter(raw: &str) -> Result<i64> {
    if raw.len() > 20 {
        return Err(Error::Invalid("list counter".into()));
    }
    let n = raw
        .parse::<i64>()
        .map_err(|_| Error::Invalid("list counter".into()))?;
    if n.to_string() != raw {
        return Err(Error::Invalid("noncanonical list counter".into()));
    }
    Ok(n)
}
fn definition(d: &ListDefinition) -> Result<reprise_doc::ListDefinition> {
    if d.levels.len() > 9 {
        return Err(Error::Limit("list levels".into()));
    }
    let mut out = reprise_doc::ListDefinition {
        base: d.base.as_deref().map(list_id).transpose()?,
        ..Default::default()
    };
    for l in &d.levels {
        if l.level > 8 || out.levels.contains_key(&l.level) {
            return Err(Error::Invalid("duplicate or invalid list level".into()));
        }
        out.levels.insert(
            l.level,
            reprise_doc::LevelDef {
                format: l.format.as_deref().map(reprise_doc::NumberFormat::parse),
                text: l.text.clone(),
                start: l.start.as_deref().map(counter).transpose()?,
                indent: l.indent.map(Length),
                hanging: l.hanging.map(Length),
                marker: l.marker.as_ref().map(text_style).transpose()?,
            },
        );
    }
    out.validate().map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(out)
}
fn marker_out(s: reprise_doc::formatting::TextStyle) -> TextStyle {
    TextStyle {
        families: s.families,
        size: s.size.map(|l| l.0),
        language: s.language,
        features: s.features.map(|fs| {
            fs.into_iter()
                .map(|f| TextFeature {
                    tag: String::from_utf8_lossy(&f.tag).into_owned(),
                    value: f.value,
                })
                .collect()
        }),
        weight: s.weight,
        slant: s.slant.map(slant_out),
        color: s.color,
        decoration: Decoration {
            underline: s.decoration.underline,
            strike: s.decoration.strike,
        },
        reset: s.reset,
        clear: Vec::new(),
    }
}
pub(crate) fn list_definition_out(d: reprise_doc::ListDefinition) -> ListDefinition {
    ListDefinition {
        base: d.base.map(|id| id.to_string()),
        levels: d
            .levels
            .into_iter()
            .map(|(level, l)| ListLevel {
                level,
                format: l.format.map(|f| f.token().into()),
                text: l.text,
                start: l.start.map(|n| n.to_string()),
                indent: l.indent.map(|v| v.0),
                hanging: l.hanging.map(|v| v.0),
                marker: l.marker.map(marker_out),
            })
            .collect(),
    }
}
pub(crate) fn list_item_out(i: &reprise_doc::ListItem) -> ListItem {
    ListItem {
        id: i.list.to_string(),
        level: i.level,
        value: i.value.to_string(),
        restart: i.restart,
        marker: i.marker.clone(),
    }
}
pub(super) fn command(c: &Command) -> Result<reprise_edit::Command> {
    use reprise_edit::Command as C;
    Ok(match c {
        Command::SetRole { node, role } => C::SetRole {
            node: id(node)?,
            role: role_in(*role),
        },
        Command::CreateList { definition: d } => C::CreateList {
            definition: definition(d)?,
        },
        Command::UpdateList {
            list,
            definition: d,
        } => C::UpdateList {
            list: list_id(list)?,
            definition: definition(d)?,
        },
        Command::DeleteList { list } => C::DeleteList {
            list: list_ref(list)?,
        },
        Command::SetListItem { node, list, level } => C::SetListItem {
            node: id(node)?,
            list: list_ref(list)?,
            level: *level,
        },
        Command::RemoveListItem { node } => C::RemoveListItem { node: id(node)? },
        Command::SetListLevel { node, level } => C::SetListLevel {
            node: id(node)?,
            level: *level,
        },
        Command::IndentListItem { node, by } => C::IndentListItem {
            node: id(node)?,
            by: *by,
        },
        Command::RestartList { node, value } => C::RestartList {
            node: id(node)?,
            value: value.as_deref().map(counter).transpose()?,
        },
        Command::ContinueList { node } => C::ContinueList { node: id(node)? },
        Command::Enter { node, at } => C::Enter {
            node: id(node)?,
            at: *at as usize,
        },
        Command::BackspaceAtStart { node } => C::BackspaceAtStart { node: id(node)? },
        _ => return Err(Error::Invalid("not a semantic command".into())),
    })
}
