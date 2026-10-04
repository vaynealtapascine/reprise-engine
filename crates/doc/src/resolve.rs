//! Resolving targets against the document (13, 14, 15).
//!
//! This is everything about a target that the document alone can say: nodes,
//! ranges, structural queries, snapshot references, and the anchors of
//! layout queries. Layout answers the rest (which line, which frame) from
//! what this returns.
//!
//! Resolution is a pure function of the document's current state. In
//! particular, [`OnTargetDeleted`] policies are evaluated here, from
//! tombstones and succession links, and never by editing the document when
//! something is deleted. See [`OnTargetDeleted`] for why that is the
//! collaboration-safe choice.

use std::ops::Range;

use serde::Serialize;

use crate::history::{HistoryCache, SnapshotContent, SnapshotState, VersionError};
use crate::relation::{
    OnTargetDeleted, QueryAnchor, Relation, RelationSchema, SchemaRegistry, StructuralQuery, Target,
};
use crate::structure::Succession;
use crate::{Document, NodeId, RangeId, RangeState, RelationId};

/// How a target stands (15). Ordered by how bad it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Binding {
    Valid,
    /// Rebound to the nearest surviving position or successor.
    Rebound,
    /// Several candidates are equally good (see [`Found`]).
    Ambiguous,
    /// Nothing to resolve to.
    Missing,
    /// The schema's policy was to delete the relation, and it has been.
    /// Every target of such a relation has this binding.
    Deleted,
}

/// What a target resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Nothing,
    /// One block. For an `Ambiguous` binding, see [`Found::Nodes`].
    Node(NodeId),
    /// Several blocks: the matches of a multi-valued query, or, for an
    /// `Ambiguous` binding, the candidates in document order.
    Nodes(Vec<NodeId>),
    /// A range's bytes now.
    Range {
        node: NodeId,
        bytes: Range<usize>,
    },
    Snapshot(SnapshotContent),
}

/// What was deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gone {
    Node(NodeId),
    /// A range that is itself deleted, whose block is deleted, or whose
    /// text is gone.
    Range(RangeId),
}

/// Why a target has the binding it has. Layout turns this into diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Live,
    /// The range lost part of its text and kept the nearest position.
    RangeRebound,
    /// The node (or the anchor of a query) was deleted, and `from` named its
    /// successors.
    Succeeded {
        from: NodeId,
    },
    /// The target is deleted and was not rebound.
    Deleted(Gone),
    /// A query matched nothing.
    NoMatch,
    /// The snapshot's version is fine but the subject didn't exist then.
    NotThere,
    /// The snapshot's version can't be read.
    Unavailable(VersionError),
    /// Succession went on for too many generations (37).
    RebindLimit,
    /// The relation's policy deleted it because of another of its targets.
    RelationDeleted,
}

/// A target's resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub binding: Binding,
    pub found: Found,
    pub cause: Cause,
}

impl Outcome {
    fn valid(found: Found) -> Outcome {
        Outcome {
            binding: Binding::Valid,
            found,
            cause: Cause::Live,
        }
    }

    fn missing(cause: Cause) -> Outcome {
        Outcome {
            binding: Binding::Missing,
            found: Found::Nothing,
            cause,
        }
    }

    /// Whether the target was deleted (and not rebound).
    pub fn is_deleted(&self) -> bool {
        matches!(self.cause, Cause::Deleted(_))
    }
}

/// One target of a relation, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub role: String,
    pub target: Target,
    pub outcome: Outcome,
}

/// A relation's targets, resolved against its schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRelation {
    /// The schema's policy is [`OnTargetDeleted::Delete`] and a target is
    /// deleted: the relation is no longer in effect. Every target then has
    /// [`Binding::Deleted`].
    pub deleted: bool,
    /// In role order, then target order.
    pub targets: Vec<ResolvedTarget>,
}

impl Document {
    /// Resolves one target under a deletion policy.
    ///
    /// Layout queries resolve to their anchor: the range's bytes, or the
    /// node. Layout turns that into lines and frames.
    pub fn resolve_target(
        &self,
        target: &Target,
        policy: OnTargetDeleted,
        history: &mut HistoryCache,
    ) -> Outcome {
        match target {
            Target::Node(n) => self.resolve_node(*n, policy),
            Target::Range(r) => self.resolve_range_target(*r),
            Target::Layout(q) => match q.anchor() {
                QueryAnchor::Range(r) => self.resolve_range_target(r),
                QueryAnchor::Node(n) => self.resolve_node(n, policy),
            },
            Target::Structural(q) => self.resolve_structural(q, policy),
            Target::Snapshot(s) => match history.resolve(self, s) {
                SnapshotState::Found(c) => Outcome::valid(Found::Snapshot(c)),
                SnapshotState::NotThere => Outcome::missing(Cause::NotThere),
                SnapshotState::Unavailable(e) => Outcome::missing(Cause::Unavailable(e)),
            },
        }
    }

    fn resolve_node(&self, n: NodeId, policy: OnTargetDeleted) -> Outcome {
        if self.is_live(n) {
            Outcome::valid(Found::Node(n))
        } else {
            self.rebind_node(n, policy)
        }
    }

    /// `n` is deleted. Under `Rebind`, look for what replaced it.
    fn rebind_node(&self, n: NodeId, policy: OnTargetDeleted) -> Outcome {
        if policy != OnTargetDeleted::Rebind {
            return Outcome::missing(Cause::Deleted(Gone::Node(n)));
        }
        let cause = Cause::Succeeded { from: n };
        match self.succession(n) {
            Succession::Live(mut live) if live.len() == 1 => Outcome {
                binding: Binding::Rebound,
                found: Found::Node(live.remove(0)),
                cause,
            },
            Succession::Live(live) => Outcome {
                binding: Binding::Ambiguous,
                found: Found::Nodes(live),
                cause,
            },
            Succession::None => Outcome::missing(Cause::Deleted(Gone::Node(n))),
            Succession::LimitReached => Outcome::missing(Cause::RebindLimit),
        }
    }

    fn resolve_range_target(&self, r: RangeId) -> Outcome {
        match self.resolve_range(r) {
            RangeState::Valid { node, bytes } => Outcome::valid(Found::Range { node, bytes }),
            RangeState::Rebound { node, bytes } => Outcome {
                binding: Binding::Rebound,
                found: Found::Range { node, bytes },
                cause: Cause::RangeRebound,
            },
            RangeState::Missing { .. } => Outcome::missing(Cause::Deleted(Gone::Range(r))),
        }
    }

    fn resolve_structural(&self, q: &StructuralQuery, policy: OnTargetDeleted) -> Outcome {
        let rebound = match q.anchor() {
            Some(anchor) if !self.is_live(anchor) => {
                let o = self.rebind_node(anchor, policy);
                match o.found {
                    // Ask the same question of the successor.
                    Found::Node(new) if o.binding == Binding::Rebound => {
                        Some((q.with_anchor(new), o.cause))
                    }
                    // No successor, or several: nothing to ask.
                    _ => return o,
                }
            }
            _ => None,
        };
        let (query, via) = match &rebound {
            Some((query, cause)) => (query, Some(*cause)),
            None => (q, None),
        };
        let matches = self.evaluate(query);
        let found = match matches.first() {
            None => return Outcome::missing(Cause::NoMatch),
            Some(_) if query.is_multi() => Found::Nodes(matches),
            Some(&first) => Found::Node(first),
        };
        match via {
            Some(cause) => Outcome {
                binding: Binding::Rebound,
                found,
                cause,
            },
            None => Outcome::valid(found),
        }
    }

    /// Resolves every target of a relation and applies the schema's deletion
    /// policy (14, 15).
    pub fn resolve_relation(
        &self,
        schema: &RelationSchema,
        relation: &Relation,
        history: &mut HistoryCache,
    ) -> ResolvedRelation {
        let mut targets: Vec<ResolvedTarget> = relation
            .targets
            .iter()
            .flat_map(|(role, ts)| ts.iter().map(move |t| (role, t)))
            .map(|(role, target)| ResolvedTarget {
                role: role.clone(),
                target: target.clone(),
                outcome: self.resolve_target(target, schema.on_target_deleted, history),
            })
            .collect();
        let deleted = schema.on_target_deleted == OnTargetDeleted::Delete
            && targets.iter().any(|t| t.outcome.is_deleted());
        if deleted {
            for t in &mut targets {
                t.outcome.binding = Binding::Deleted;
                t.outcome.found = Found::Nothing;
                if !t.outcome.is_deleted() {
                    t.outcome.cause = Cause::RelationDeleted;
                }
            }
        }
        ResolvedRelation { deleted, targets }
    }

    /// The relations whose schema deletes them and that have a deleted
    /// target now: what an editing command may really delete, in ID order.
    ///
    /// This is a read. Whether to delete them is the command's decision: a
    /// relation that is left in place is still reported as no longer in
    /// effect, and costs nothing. Relations with unknown schemas, and ones
    /// that can't be read, are never listed.
    pub fn dead_relations(&self, schemas: &SchemaRegistry) -> Vec<RelationId> {
        let mut history = HistoryCache::default();
        self.relations()
            .into_iter()
            .filter_map(|(id, relation)| {
                let relation = relation.ok()?;
                let schema = schemas.get(&relation.schema)?;
                if schema.on_target_deleted != OnTargetDeleted::Delete {
                    return None;
                }
                // Snapshot targets are never deleted, and opening a version
                // is the one expensive resolution, so skip those.
                let gone = relation
                    .targets
                    .values()
                    .flatten()
                    .filter(|t| !matches!(t, Target::Snapshot(_)))
                    .any(|t| {
                        self.resolve_target(t, schema.on_target_deleted, &mut history)
                            .is_deleted()
                    });
                gone.then_some(id)
            })
            .collect()
    }
}
