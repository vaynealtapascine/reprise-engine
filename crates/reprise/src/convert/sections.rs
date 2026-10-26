//! Section, story and region commands at the facade boundary (sections §5, §17).
//! Identifiers are parsed here and every property is checked for being a
//! scalar before the kernel sees it; the kernel validates the rest.
use super::*;
use reprise_doc::sections::{BreakKind as DocBreak, SectionId, SectionProperties};
use serde_json::{Map, Value};

/// The most properties one command carries: a section has under thirty known
/// keys and a bounded number of unknown ones are kept.
const MAX_PROPERTIES: usize = 64;
/// The most blocks one `create-story` makes.
const MAX_STORY_BLOCKS: usize = 4096;

fn section(raw: &str) -> Result<SectionId> {
    if raw.len() > 80 {
        return Err(Error::InvalidId(raw.into()));
    }
    SectionId::parse(raw).ok_or_else(|| Error::InvalidId(raw.into()))
}

fn properties(patch: &Option<Map<String, Value>>) -> Result<SectionProperties> {
    let Some(map) = patch else {
        return Ok(SectionProperties::new());
    };
    if map.len() > MAX_PROPERTIES {
        return Err(Error::Limit("section properties".into()));
    }
    let mut out = SectionProperties::new();
    for (key, value) in map {
        let scalar = matches!(value, Value::String(_) | Value::Bool(_)) || value.as_i64().is_some();
        if !scalar {
            return Err(Error::Invalid(format!(
                "{key}: a section property is a string, a boolean or an integer"
            )));
        }
        out.insert(key.clone(), value.clone());
    }
    Ok(out)
}

fn kind(k: BreakKind) -> DocBreak {
    match k {
        BreakKind::Section => DocBreak::Section,
        BreakKind::Page => DocBreak::Page,
        BreakKind::Column => DocBreak::Column,
    }
}

pub(crate) fn command(c: &Command) -> Result<Option<reprise_edit::Command>> {
    use reprise_edit::Command as C;
    Ok(Some(match c {
        Command::InsertBreak {
            at,
            break_kind,
            patch,
        } => C::InsertBreak {
            at: match at {
                BreakPosition::Position { node, offset } => reprise_edit::BreakAt::Position {
                    node: id(node)?,
                    offset: *offset as usize,
                },
                BreakPosition::Before { node } => reprise_edit::BreakAt::Before(id(node)?),
            },
            kind: kind(*break_kind),
            patch: properties(patch)?,
        },
        Command::RemoveBreak { block } => C::RemoveBreak { block: id(block)? },
        Command::SetSection {
            section: s,
            patch,
            clear,
        } => {
            let clear = clear.clone().unwrap_or_default();
            if clear.len() > MAX_PROPERTIES {
                return Err(Error::Limit("section properties".into()));
            }
            C::SetSection {
                section: section(s)?,
                patch: properties(patch)?,
                clear,
            }
        }
        Command::CreateStory { blocks } => {
            if blocks.len() > MAX_STORY_BLOCKS {
                return Err(Error::Limit("story blocks".into()));
            }
            C::CreateStory {
                blocks: blocks
                    .iter()
                    .map(|b| {
                        reprise_doc::NewBlock::new(
                            super::kind(b.block_kind),
                            b.style.as_deref().unwrap_or_default(),
                            &b.text,
                        )
                    })
                    .collect(),
            }
        }
        Command::SetRegion {
            section: s,
            slot,
            variant,
            binding,
        } => C::SetRegion {
            section: section(s)?,
            slot: slot.clone(),
            variant: variant.clone(),
            binding: match binding {
                RegionBinding::Inherit => reprise_edit::RegionBinding::Inherit,
                RegionBinding::None => reprise_edit::RegionBinding::None,
                RegionBinding::Story { story } => reprise_edit::RegionBinding::Story(id(story)?),
            },
        },
        Command::CreateRegion {
            section: s,
            slot,
            variant,
            from,
        } => C::CreateRegion {
            section: section(s)?,
            slot: slot.clone(),
            variant: variant.clone(),
            from: match from {
                StoryFrom::Empty => reprise_edit::StoryFrom::Empty,
                StoryFrom::CopyOf { story } => reprise_edit::StoryFrom::CopyOf(id(story)?),
            },
        },
        Command::LinkRegion {
            section: s,
            slot,
            variant,
        } => C::SetRegion {
            section: section(s)?,
            slot: slot.clone(),
            variant: variant.clone(),
            binding: reprise_edit::RegionBinding::Inherit,
        },
        Command::DeleteStory { story } => C::DeleteStory { story: id(story)? },
        _ => return Ok(None),
    }))
}
