//! Semantic block roles (06, 33): what a paragraph *is*, never how it looks.
//! See `docs/roles-and-lists.md`.
//!
//! A role is one flat entry, `role1`, in the paragraph's own metadata: the
//! tree node's for a head paragraph or a tree block, the break record's for a
//! break paragraph (`docs/flow.md`). A split copies it with the record, a join
//! keeps the first paragraph's, and undo restores the previous value.

use std::fmt;

use loro::{LoroMap, LoroValue, ValueOrContainer};
use reprise_diag::Note;
use serde::{Deserialize, Serialize};

use crate::{DocError, Document, NodeId};

/// The paragraph metadata key of a role.
pub(crate) const ROLE: &str = "role1";

/// What a paragraph is. Adding a variant: see "Extension points" in
/// `docs/roles-and-lists.md`; every `match` on `Role` is exhaustive, so the
/// compiler names each consumer that must decide.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Ordinary text.
    #[default]
    Body,
    /// A heading, levels 1 to 6.
    Heading(u8),
    /// The document's title.
    Title,
    Subtitle,
    /// A block quotation.
    Quote,
    /// Computer code.
    Code,
    /// A verse paragraph (the extension proof: a role added after the first set).
    Verse,
}

impl Role {
    /// The stored token.
    pub fn token(self) -> String {
        match self {
            Role::Body => "body".into(),
            Role::Heading(n) => format!("heading-{n}"),
            Role::Title => "title".into(),
            Role::Subtitle => "subtitle".into(),
            Role::Quote => "quote".into(),
            Role::Code => "code".into(),
            Role::Verse => "verse".into(),
        }
    }

    /// Reads a stored token. `None` for a token this engine doesn't know,
    /// including headings outside 1 to 6.
    pub fn parse(token: &str) -> Option<Role> {
        Some(match token {
            "body" => Role::Body,
            "title" => Role::Title,
            "subtitle" => Role::Subtitle,
            "quote" => Role::Quote,
            "code" => Role::Code,
            "verse" => Role::Verse,
            _ => {
                let n = token.strip_prefix("heading-")?;
                match n.as_bytes() {
                    [d @ b'1'..=b'6'] => Role::Heading(d - b'0'),
                    _ => return None,
                }
            }
        })
    }

    /// Whether a role is valid to store: headings must be 1 to 6.
    pub fn is_valid(self) -> bool {
        match self {
            Role::Heading(n) => (1..=6).contains(&n),
            _ => true,
        }
    }

    /// The named style an editor pairs with this role (agreed with
    /// `ws/style`'s built-in names). The engine never applies it.
    pub fn default_style(self) -> String {
        match self {
            Role::Body => "Normal".into(),
            Role::Heading(n) => format!("Heading {n}"),
            Role::Title => "Title".into(),
            Role::Subtitle => "Subtitle".into(),
            Role::Quote => "Quote".into(),
            Role::Code => "Code".into(),
            Role::Verse => "Verse".into(),
        }
    }

    /// The level this role has in the outline, if it is in it.
    pub fn outline_level(self) -> Option<u8> {
        match self {
            Role::Heading(n) => Some(n),
            Role::Body | Role::Title | Role::Subtitle | Role::Quote | Role::Code | Role::Verse => {
                None
            }
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.token())
    }
}

impl Serialize for Role {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Role {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Role::parse(&s).ok_or_else(|| serde::de::Error::custom("unknown role"))
    }
}

/// A paragraph's role as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRole {
    /// What layout, export and queries use: body for an unknown token.
    pub role: Role,
    /// The token as written, when this engine can't read it (34).
    pub unknown: Option<String>,
}

impl StoredRole {
    /// The `role.unknown` note for an unreadable token.
    pub fn note(&self) -> Option<Note> {
        self.unknown.as_ref().map(|raw| {
            Note::warning(
                codes::ROLE_UNKNOWN,
                format!("role {raw:?} is not known to this engine; treated as body"),
            )
        })
    }
}

/// One heading of the outline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OutlineEntry {
    pub node: NodeId,
    /// 1 to 6.
    pub level: u8,
    pub text: String,
}

pub(crate) fn read_role(meta: &LoroMap) -> StoredRole {
    match meta.get(ROLE) {
        None => StoredRole {
            role: Role::Body,
            unknown: None,
        },
        Some(ValueOrContainer::Value(LoroValue::String(s))) => match Role::parse(&s) {
            Some(role) => StoredRole {
                role,
                unknown: None,
            },
            None => StoredRole {
                role: Role::Body,
                unknown: Some(s.to_string()),
            },
        },
        Some(other) => StoredRole {
            role: Role::Body,
            unknown: Some(format!("{other:?}")),
        },
    }
}

/// The semantic keys a split copies to the new paragraph (not `restart1`:
/// the new paragraph continues the list it was split from).
pub(crate) const COPIED: [&str; 3] = [ROLE, crate::lists::LIST, crate::lists::LEVEL];

/// Copies the role and list membership entries of `from` into `to`.
pub(crate) fn copy_semantics(from: &LoroMap, to: &LoroMap) -> Result<(), DocError> {
    for key in COPIED {
        if let Some(ValueOrContainer::Value(v)) = from.get(key) {
            to.insert(key, v)?;
        }
    }
    Ok(())
}

impl Document {
    /// A live block's role, with the raw token when it is unknown.
    pub fn stored_role(&self, id: NodeId) -> Result<StoredRole, DocError> {
        self.block(id)?;
        Ok(read_role(&self.meta_of(id)?))
    }

    /// A live block's role: body when none is set or the stored one is unknown.
    pub fn role(&self, id: NodeId) -> Result<Role, DocError> {
        Ok(self.stored_role(id)?.role)
    }

    /// Sets a block's role. Does not commit.
    pub fn set_role(&self, id: NodeId, role: Role) -> Result<(), DocError> {
        if !role.is_valid() {
            return Err(DocError::Malformed(id, "heading levels are 1 to 6"));
        }
        self.block(id)?;
        self.meta_of(id)?.insert(ROLE, role.token())?;
        Ok(())
    }

    /// Copies `from`'s role and list membership onto `to` (for splits and
    /// moves that copy a paragraph). Does not commit.
    pub fn copy_semantics_to(&self, from: NodeId, to: NodeId) -> Result<(), DocError> {
        let source = self.meta_of(from)?;
        copy_semantics(&source, &self.meta_of(to)?)
    }

    /// Every live heading in document order, with its level and text, for a
    /// table of contents and navigation (16). Unknown roles are not headings.
    pub fn outline(&self) -> Vec<OutlineEntry> {
        self.order(crate::Domain::Body)
            .into_iter()
            .filter_map(|node| {
                let level = self.role(node).ok()?.outline_level()?;
                let text = self.block(node).ok()?.text.to_string();
                Some(OutlineEntry { node, level, text })
            })
            .collect()
    }

    /// Whether any paragraph ever had a role or list membership, or any list
    /// definition exists, including deleted ones whose history is retained.
    /// The package declares a required feature bit for it (34).
    pub fn has_block_semantics(&self) -> bool {
        use loro::Container;
        if self.has_root(crate::lists::LISTS) && !self.tree(crate::lists::LISTS).nodes().is_empty()
        {
            return true;
        }
        let keys = [
            ROLE,
            crate::lists::LIST,
            crate::lists::LEVEL,
            crate::lists::RESTART,
        ];
        let marked = |meta: &LoroMap| keys.iter().any(|k| meta.get(k).is_some());
        let mut visited = 0usize;
        if self.has_root(crate::flow::BREAKS)
            && let LoroValue::Map(records) = self.break_records().get_value()
        {
            for value in records.values() {
                visited = visited.saturating_add(1);
                if visited > 1 << 22 {
                    return true;
                }
                let LoroValue::Container(id) = value else {
                    continue;
                };
                if let Some(Container::Map(m)) = self.doc.get_container(id.clone())
                    && marked(&m)
                {
                    return true;
                }
            }
        }
        {
            let tree = self.tree("content");
            for id in tree.nodes() {
                visited = visited.saturating_add(1);
                if visited > 1 << 22 {
                    return true;
                }
                if let Ok(meta) = tree.get_meta(id)
                    && marked(&meta)
                {
                    return true;
                }
            }
        }
        false
    }
}

/// The role and list diagnostic codes (37). Every one is a `Warning`: the
/// paragraph is always laid out, but not as the stored data asked.
pub mod codes {
    use reprise_diag::Code;

    /// A stored role this engine can't read; treated as body.
    pub const ROLE_UNKNOWN: Code = Code::new("role.unknown");
    /// An item names a missing, deleted or unreadable list definition, or a
    /// definition's base is missing; the item is a plain paragraph.
    pub const LIST_DEFINITION_MISSING: Code = Code::new("list.definition-missing");
    /// A stored property of a list definition can't be read; inherited or
    /// default used.
    pub const LIST_DEFINITION_UNREADABLE: Code = Code::new("list.definition-unreadable");
    /// A definition's `base` chain loops or is longer than 16 links.
    pub const LIST_DEFINITION_CYCLE: Code = Code::new("list.definition-cycle");
    /// A stored level outside 0 to 8; clamped.
    pub const LIST_LEVEL_CLAMPED: Code = Code::new("list.level-clamped");
    /// A stored numbering format this engine can't read; rendered as decimal.
    pub const LIST_FORMAT_UNKNOWN: Code = Code::new("list.format-unknown");
    /// A counter its format can't show (roman outside 1 to 3,999, alphabetic
    /// below 1); rendered as decimal.
    pub const LIST_COUNTER_RANGE: Code = Code::new("list.counter-range");
    /// A marker template longer than the limit; cut.
    pub const LIST_LIMIT: Code = Code::new("list.limit");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_round_trip_and_reject_bad_levels() {
        for role in [
            Role::Body,
            Role::Heading(1),
            Role::Heading(6),
            Role::Title,
            Role::Subtitle,
            Role::Quote,
            Role::Code,
            Role::Verse,
        ] {
            assert_eq!(Role::parse(&role.token()), Some(role));
            assert!(role.is_valid());
        }
        for bad in [
            "heading-0",
            "heading-7",
            "heading-12",
            "heading-",
            "Heading-1",
            "",
        ] {
            assert_eq!(Role::parse(bad), None, "{bad}");
        }
        assert!(!Role::Heading(0).is_valid());
        assert!(!Role::Heading(7).is_valid());
        assert_eq!(Role::Heading(2).default_style(), "Heading 2");
        assert_eq!(Role::Heading(3).outline_level(), Some(3));
        assert_eq!(Role::Title.outline_level(), None);
    }
}
