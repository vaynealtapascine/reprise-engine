//! Relations (decisions 13, 14 and 15): what a relation targets, and the
//! registered schemas that say what each relation type means.
//!
//! A [`Relation`] is authored data: a schema name, an optional owner, targets
//! grouped by role, and typed parameters. A [`RelationSchema`] declares which
//! roles and parameters a type has, who owns it, and what deleting or copying
//! its targets does to it. Schemas live in a [`SchemaRegistry`], which is
//! engine configuration like the shaping adapter: the built-in types are
//! registered by [`SchemaRegistry::builtin`], and extensions register more.
//!
//! What a relation *does* to layout is not part of the schema. Layout looks
//! the schema up by [`SchemaId`] and applies its own behaviour for it; a
//! relation whose schema layout doesn't know is kept, reported and not
//! applied (34, 37).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{LengthExpr, NodeId, RangeId};

/// A registered relation type's name, namespaced by its owner:
/// `reprise.follow` for built-ins, `<extension>.<name>` for extensions.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaId(Cow<'static, str>);

impl SchemaId {
    pub const fn new(id: &'static str) -> SchemaId {
        SchemaId(Cow::Borrowed(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for SchemaId {
    fn from(s: String) -> Self {
        SchemaId(Cow::Owned(s))
    }
}

impl fmt::Debug for SchemaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for SchemaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The classes of target from decision 13.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetClass {
    /// A node by ID.
    Node,
    /// A persistent range.
    Range,
    /// A query over the content tree, such as "the next stanza".
    Structural,
    /// A query over layout results, re-evaluated after every reflow.
    Layout,
    /// An explicit reference to an earlier layout snapshot.
    Snapshot,
}

/// What a relation points at (13).
///
/// Structural queries and snapshot references are declared target classes
/// but have no variants yet; they are added with the relations workstream.
/// The enum is non-exhaustive so adding them is not a breaking change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Target {
    Node(NodeId),
    Range(RangeId),
    Layout(LayoutQuery),
}

impl Target {
    pub fn class(&self) -> TargetClass {
        match self {
            Target::Node(_) => TargetClass::Node,
            Target::Range(_) => TargetClass::Range,
            Target::Layout(_) => TargetClass::Layout,
        }
    }
}

/// A query over layout results (13). Each query says what zero and several
/// matches mean.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum LayoutQuery {
    /// The visual line containing the start of a range. Never several
    /// matches; zero when the range is missing or its block wasn't laid out.
    LineContaining { range: RangeId },
}

/// A typed parameter value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Param {
    Length(LengthExpr),
    Int(i64),
    Bool(bool),
    Text(String),
}

impl Param {
    pub fn kind(&self) -> ParamKind {
        match self {
            Param::Length(_) => ParamKind::Length,
            Param::Int(_) => ParamKind::Int,
            Param::Bool(_) => ParamKind::Bool,
            Param::Text(_) => ParamKind::Text,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ParamKind {
    Length,
    Int,
    Bool,
    Text,
}

/// One authored relation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub schema: SchemaId,
    /// The node that owns the relation, for schemas with [`Ownership::Owned`].
    pub owner: Option<NodeId>,
    /// Targets by role name. A role may take several targets, in order.
    pub targets: BTreeMap<String, Vec<Target>>,
    pub params: BTreeMap<String, Param>,
}

impl Relation {
    pub fn new(schema: SchemaId) -> Relation {
        Relation {
            schema,
            owner: None,
            targets: BTreeMap::new(),
            params: BTreeMap::new(),
        }
    }

    pub fn owned_by(mut self, owner: NodeId) -> Relation {
        self.owner = Some(owner);
        self
    }

    pub fn target(mut self, role: &str, target: Target) -> Relation {
        self.targets.entry(role.into()).or_default().push(target);
        self
    }

    pub fn param(mut self, name: &str, value: Param) -> Relation {
        self.params.insert(name.into(), value);
        self
    }

    /// The first target in `role`.
    pub fn first(&self, role: &str) -> Option<&Target> {
        self.targets.get(role).and_then(|t| t.first())
    }
}

/// Whether a relation belongs to a node (14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ownership {
    /// The relation belongs to its owner node, and is deleted and copied with it.
    Owned,
    /// The relation exists on its own, between its targets.
    Independent,
}

/// What happens to a relation when one of its targets is deleted (14, 15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnTargetDeleted {
    /// Rebind automatically to the best surviving target, using tombstones and
    /// CRDT identity as evidence, and report the new state (15).
    Rebind,
    /// Keep the relation, reported as missing, until the target comes back
    /// (for example through undo). For schemas that opt out of rebinding.
    KeepMissing,
    /// Delete the relation, leaving a tombstone.
    Delete,
}

/// What copying does to a relation (14, 35).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyPolicy {
    /// When the owner and every target are inside the copied fragment.
    pub inside: CopyInside,
    /// When some targets are outside the copied fragment.
    pub crossing: CopyCrossing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CopyInside {
    /// Copy the relation, remapped to the copied IDs.
    Duplicate,
    /// Leave it out of the copy.
    Drop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CopyCrossing {
    /// Copy it, still pointing at the original targets outside the fragment.
    KeepOutside,
    /// Leave it out of the copy.
    Drop,
}

/// A role a relation fills targets into.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSpec {
    pub name: Cow<'static, str>,
    pub accepts: Cow<'static, [TargetClass]>,
    pub min: u32,
    /// `None` for no upper limit.
    pub max: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamSpec {
    pub name: Cow<'static, str>,
    pub kind: ParamKind,
    pub required: bool,
}

/// A registered relation type (14).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationSchema {
    pub id: SchemaId,
    /// Bumped when the schema's meaning changes; documents record it (34).
    pub version: u32,
    pub ownership: Ownership,
    pub roles: Vec<RoleSpec>,
    pub params: Vec<ParamSpec>,
    pub on_target_deleted: OnTargetDeleted,
    pub on_copy: CopyPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    #[error("no relation schema {0} is registered")]
    Unknown(SchemaId),
    #[error("relation schema {0} is already registered")]
    Duplicate(SchemaId),
    #[error("{schema} relations must have an owner")]
    MissingOwner { schema: SchemaId },
    #[error("{schema} relations are independent and can't have an owner")]
    UnexpectedOwner { schema: SchemaId },
    #[error("{schema} has no role {role:?}")]
    UnknownRole { schema: SchemaId, role: String },
    #[error("role {role:?} of {schema} takes {min}..{max:?} targets, not {count}")]
    Cardinality {
        schema: SchemaId,
        role: String,
        min: u32,
        max: Option<u32>,
        count: usize,
    },
    #[error("role {role:?} of {schema} doesn't accept {class:?} targets")]
    TargetClass {
        schema: SchemaId,
        role: String,
        class: TargetClass,
    },
    #[error("{schema} has no parameter {name:?}")]
    UnknownParam { schema: SchemaId, name: String },
    #[error("parameter {name:?} of {schema} is required")]
    MissingParam { schema: SchemaId, name: String },
    #[error("parameter {name:?} of {schema} must be {expected:?}")]
    ParamKind {
        schema: SchemaId,
        name: String,
        expected: ParamKind,
    },
}

/// The relation schemas an engine knows.
#[derive(Clone, Debug, Default)]
pub struct SchemaRegistry {
    schemas: BTreeMap<SchemaId, RelationSchema>,
}

impl SchemaRegistry {
    /// A registry with every built-in schema.
    pub fn builtin() -> SchemaRegistry {
        let mut registry = SchemaRegistry::default();
        for schema in builtin::all() {
            registry
                .register(schema)
                .expect("built-in schema IDs are distinct");
        }
        registry
    }

    pub fn register(&mut self, schema: RelationSchema) -> Result<(), SchemaError> {
        if self.schemas.contains_key(&schema.id) {
            return Err(SchemaError::Duplicate(schema.id));
        }
        self.schemas.insert(schema.id.clone(), schema);
        Ok(())
    }

    pub fn get(&self, id: &SchemaId) -> Option<&RelationSchema> {
        self.schemas.get(id)
    }

    /// Schemas in ID order.
    pub fn iter(&self) -> impl Iterator<Item = &RelationSchema> {
        self.schemas.values()
    }

    /// Checks a relation against its schema: ownership, roles, target classes,
    /// cardinality and parameters.
    pub fn validate(&self, relation: &Relation) -> Result<(), SchemaError> {
        let id = &relation.schema;
        let schema = self
            .get(id)
            .ok_or_else(|| SchemaError::Unknown(id.clone()))?;
        match (schema.ownership, relation.owner) {
            (Ownership::Owned, None) => {
                return Err(SchemaError::MissingOwner { schema: id.clone() });
            }
            (Ownership::Independent, Some(_)) => {
                return Err(SchemaError::UnexpectedOwner { schema: id.clone() });
            }
            _ => {}
        }
        if let Some(role) = relation
            .targets
            .keys()
            .find(|r| !schema.roles.iter().any(|s| s.name == r.as_str()))
        {
            return Err(SchemaError::UnknownRole {
                schema: id.clone(),
                role: role.clone(),
            });
        }
        for spec in &schema.roles {
            let targets = relation
                .targets
                .get(spec.name.as_ref())
                .map_or(&[][..], Vec::as_slice);
            let count = targets.len();
            if count < spec.min as usize || spec.max.is_some_and(|m| count > m as usize) {
                return Err(SchemaError::Cardinality {
                    schema: id.clone(),
                    role: spec.name.to_string(),
                    min: spec.min,
                    max: spec.max,
                    count,
                });
            }
            if let Some(t) = targets.iter().find(|t| !spec.accepts.contains(&t.class())) {
                return Err(SchemaError::TargetClass {
                    schema: id.clone(),
                    role: spec.name.to_string(),
                    class: t.class(),
                });
            }
        }
        for (name, value) in &relation.params {
            let spec = schema
                .params
                .iter()
                .find(|p| p.name == name.as_str())
                .ok_or_else(|| SchemaError::UnknownParam {
                    schema: id.clone(),
                    name: name.clone(),
                })?;
            if value.kind() != spec.kind {
                return Err(SchemaError::ParamKind {
                    schema: id.clone(),
                    name: name.clone(),
                    expected: spec.kind,
                });
            }
        }
        if let Some(missing) = schema
            .params
            .iter()
            .find(|p| p.required && !relation.params.contains_key(p.name.as_ref()))
        {
            return Err(SchemaError::MissingParam {
                schema: id.clone(),
                name: missing.name.to_string(),
            });
        }
        Ok(())
    }
}

/// The built-in relation schemas (14). Alignment, spacing, order, grouping,
/// breaks and references are added with the relations workstream.
pub mod builtin {
    use std::borrow::Cow;

    use super::*;

    /// `reprise.follow`: the owner block sits beside its target line, level
    /// with the line's top, and moves with it when the text reflows.
    ///
    /// -   Owned by the block it places.
    /// -   Role `line`: exactly one layout query.
    /// -   Parameter `offset` (length, optional): moves the block down from the
    ///     line's top, or up when negative.
    pub const FOLLOW: SchemaId = SchemaId::new("reprise.follow");

    pub fn follow() -> RelationSchema {
        RelationSchema {
            id: FOLLOW,
            version: 1,
            ownership: Ownership::Owned,
            roles: vec![RoleSpec {
                name: Cow::Borrowed("line"),
                accepts: Cow::Borrowed(&[TargetClass::Layout]),
                min: 1,
                max: Some(1),
            }],
            params: vec![ParamSpec {
                name: Cow::Borrowed("offset"),
                kind: ParamKind::Length,
                required: false,
            }],
            on_target_deleted: OnTargetDeleted::Rebind,
            on_copy: CopyPolicy {
                inside: CopyInside::Duplicate,
                crossing: CopyCrossing::KeepOutside,
            },
        }
    }

    pub fn all() -> Vec<RelationSchema> {
        vec![follow()]
    }
}
