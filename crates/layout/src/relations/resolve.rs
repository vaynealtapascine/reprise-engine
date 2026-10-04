//! Shared target resolution for relations (13, 14, 15): any [`Target`] to a
//! [`TargetLayout`] with a status and a diagnostic.
//!
//! The document resolves what it can alone (nodes, ranges, structural
//! queries, snapshot references, deletion policies, rebinding). Layout adds
//! what only it knows: which line, frame or page a layout query answers.
//! Everything a relation's behaviour needs to know about its targets comes
//! out of [`Resolver::resolve`], so every schema reports missing, rebound and
//! ambiguous targets the same way.
//!
//! # Rules
//!
//! -   **Never choose silently.** A role that takes one target and gets
//!     several candidates is `Ambiguous`, with every candidate in
//!     `TargetLayout::resolved` (`Nodes` or `Lines`), and a
//!     `relation.ambiguous` warning. A schema's behaviour must not apply an
//!     `Ambiguous` target. There is no preference among a query's matches;
//!     the same goes for several successors in the same generation.
//! -   **Severity follows the role.** A target that is gone, matches nothing
//!     or can't be read is an `Error` in a role that needs a target
//!     (`min > 0`), because something is left out; in an optional role it is
//!     a `Warning` when a named target is gone and `Info` when a query
//!     simply matches nothing.
//! -   `Rebound` is `Info`: the schema asked for rebinding.
//! -   A policy-deleted relation is `Info` (`relation.target-deleted`): the
//!     output is what the schema declared. Its targets have `Deleted` status.
//!
//! `follow` shares this resolver. Its required `line` role retains the
//! existing missing/no-match Error and rebound Info codes. A singleton
//! `LinesIn` identifies one line; multiple matches remain ambiguous.

use reprise_diag::{Code, Severity};
use reprise_doc::{
    Binding, Cause, Document, Found, Gone, HistoryCache, NodeId, Relation, RelationId,
    RelationSchema, ResolvedTarget, Target,
};

use crate::query::LayoutAt;
use crate::{Diagnostic, LayoutSnapshot, RelationStatus, Resolution, Subject, TargetLayout, codes};

/// Resolves relations' targets. One per layout pass: it caches opened
/// document versions, which depend on the document being laid out.
pub(crate) struct Resolver<'a> {
    doc: &'a Document,
    history: HistoryCache,
}

/// A relation's targets, resolved and reported.
pub(crate) struct Resolved {
    /// The schema deletes the relation and a target is deleted (14). Every
    /// target is `Deleted`.
    pub deleted: bool,
    pub targets: Vec<TargetLayout>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Resolved {
    /// What the single target of `role` resolved to, only if a behaviour may
    /// act on it: the role holds exactly one target and it is `Valid` or
    /// `Rebound`. `None` for missing, ambiguous and deleted targets.
    pub fn unique(&self, role: &str) -> Option<&Resolution> {
        let mut in_role = self.targets.iter().filter(|t| t.role == role);
        let target = in_role.next().filter(|_| in_role.next().is_none())?;
        match target.status {
            RelationStatus::Valid | RelationStatus::Rebound => target.resolved.as_ref(),
            _ => None,
        }
    }
}

impl<'a> Resolver<'a> {
    pub fn new(doc: &'a Document) -> Resolver<'a> {
        Resolver {
            doc,
            history: HistoryCache::default(),
        }
    }

    /// Resolves every target of `relation` against `snapshot` and the
    /// document, under the deletion policy of `schema`.
    pub fn resolve(
        &mut self,
        snapshot: &LayoutSnapshot,
        id: RelationId,
        schema: &RelationSchema,
        relation: &Relation,
    ) -> Resolved {
        let resolved = self
            .doc
            .resolve_relation(schema, relation, &mut self.history);
        let mut out = Resolved {
            deleted: resolved.deleted,
            targets: Vec::new(),
            diagnostics: Vec::new(),
        };
        for target in &resolved.targets {
            let spec = schema.roles.iter().find(|r| r.name == target.role);
            let ctx = Context {
                subject: Subject::Relation(id),
                schema,
                min: spec.map_or(0, |r| r.min),
                max: spec.and_then(|r| r.max),
                owner: relation.owner,
            };
            let layout = ctx.target(snapshot, target, &mut out.diagnostics);
            out.targets.push(layout);
        }
        out
    }
}

struct Context<'s> {
    subject: Subject,
    schema: &'s RelationSchema,
    min: u32,
    max: Option<u32>,
    owner: Option<NodeId>,
}

impl Context<'_> {
    /// `Error` if the role needs a target, otherwise `otherwise`.
    fn severity(&self, otherwise: Severity) -> Severity {
        if self.min > 0 {
            Severity::Error
        } else {
            otherwise
        }
    }

    fn target(
        &self,
        snapshot: &LayoutSnapshot,
        target: &ResolvedTarget,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> TargetLayout {
        let mut layout = TargetLayout {
            role: target.role.clone(),
            status: RelationStatus::Missing,
            resolved: None,
        };
        let mut say = |severity: Severity, code: Code, message: String| {
            diagnostics.push(Diagnostic::new(
                severity,
                code,
                self.subject.clone(),
                message,
            ));
        };
        let outcome = &target.outcome;
        match outcome.binding {
            Binding::Deleted => {
                layout.status = RelationStatus::Deleted;
                if let Cause::Deleted(gone) = outcome.cause {
                    say(
                        Severity::Info,
                        codes::RELATION_TARGET_DELETED,
                        format!(
                            "{} was deleted; {} deletes the relation with it",
                            describe(gone),
                            self.schema.id
                        ),
                    );
                }
            }
            Binding::Missing => match outcome.cause {
                Cause::Deleted(gone) => say(
                    self.severity(Severity::Warning),
                    codes::RELATION_MISSING_TARGET,
                    format!("{} is gone", describe(gone)),
                ),
                Cause::NoMatch => say(
                    self.severity(Severity::Info),
                    codes::RELATION_NO_MATCH,
                    "the query matched nothing".into(),
                ),
                Cause::NotThere => say(
                    self.severity(Severity::Info),
                    codes::RELATION_NO_MATCH,
                    "the snapshot's subject did not exist at that version".into(),
                ),
                Cause::Unavailable(e) => say(
                    self.severity(Severity::Warning),
                    codes::RELATION_SNAPSHOT_UNAVAILABLE,
                    format!("snapshot unavailable: {e}"),
                ),
                Cause::RebindLimit => say(
                    Severity::Warning,
                    codes::RELATION_REBIND_LIMIT,
                    "rebinding followed too many generations of successors; giving up".into(),
                ),
                _ => {}
            },
            Binding::Ambiguous => {
                layout.status = RelationStatus::Ambiguous;
                if let Found::Nodes(nodes) = &outcome.found {
                    layout.resolved = Some(Resolution::Nodes(nodes.clone()));
                    say(
                        Severity::Warning,
                        codes::RELATION_AMBIGUOUS,
                        format!(
                            "the deleted target has {} equally good successors; none chosen",
                            nodes.len()
                        ),
                    );
                }
            }
            Binding::Valid | Binding::Rebound => {
                match answer(snapshot, &target.target, &outcome.found) {
                    None => say(
                        self.severity(Severity::Info),
                        codes::RELATION_NO_MATCH,
                        "layout has no answer for the query (not laid out, or past the edge)"
                            .into(),
                    ),
                    Some(resolution) => {
                        layout.status = if outcome.binding == Binding::Rebound {
                            RelationStatus::Rebound
                        } else {
                            RelationStatus::Valid
                        };
                        match outcome.cause {
                            Cause::RangeRebound => say(
                                Severity::Info,
                                codes::RELATION_REBOUND,
                                "target range lost part of its text; rebound".into(),
                            ),
                            Cause::Succeeded { from } => say(
                                Severity::Info,
                                codes::RELATION_REBOUND,
                                format!("{from} was deleted; rebound to its successor"),
                            ),
                            _ => {}
                        }
                        let several = match &resolution {
                            Resolution::Nodes(v) => v.len(),
                            Resolution::Lines(v) => v.len(),
                            _ => 1,
                        };
                        if self.max == Some(1) && several > 1 {
                            layout.status = RelationStatus::Ambiguous;
                            say(
                                Severity::Warning,
                                codes::RELATION_AMBIGUOUS,
                                format!(
                                    "the role takes one target and the query matches {several}; none chosen"
                                ),
                            );
                        }
                        // Legacy follow/LineContaining reported only the
                        // placement failure for an owner targeting itself.
                        // Preserve its public diagnostic set when sharing
                        // resolution; other queries retain self-reference info.
                        let legacy_follow = self.schema.id
                            == reprise_doc::relation::builtin::FOLLOW
                            && matches!(
                                target.target,
                                Target::Layout(reprise_doc::LayoutQuery::LineContaining { .. })
                            );
                        if !legacy_follow && self.owner.is_some_and(|o| names(&resolution, o)) {
                            say(
                                Severity::Info,
                                codes::RELATION_SELF_REFERENCE,
                                "the target is the relation's own owner".into(),
                            );
                        }
                        layout.resolved = Some(resolution);
                    }
                }
            }
        }
        layout
    }
}

fn describe(gone: Gone) -> String {
    match gone {
        Gone::Node(n) => format!("node {n}"),
        Gone::Range(r) => format!("range {r}"),
    }
}

/// Whether a resolution is, or is inside, `node`'s blocks or lines. Text
/// ranges don't count: a note pointing into its own text is ordinary.
fn names(resolution: &Resolution, node: NodeId) -> bool {
    match resolution {
        Resolution::Node(n) => *n == node,
        Resolution::Nodes(v) => v.contains(&node),
        Resolution::Line(l) => l.node == node,
        Resolution::Lines(v) => v.iter().any(|l| l.node == node),
        _ => false,
    }
}

/// What a resolved target is, once layout has answered its query.
fn answer(snapshot: &LayoutSnapshot, target: &Target, found: &Found) -> Option<Resolution> {
    if let Target::Layout(query) = target {
        let at = match found {
            Found::Range { node, bytes } => LayoutAt::Range {
                node: *node,
                bytes: bytes.clone(),
            },
            Found::Node(node) => LayoutAt::Node(*node),
            _ => return None,
        };
        return snapshot.answer(query, &at);
    }
    match found {
        Found::Nothing => None,
        Found::Node(n) => Some(Resolution::Node(*n)),
        Found::Nodes(v) => Some(Resolution::Nodes(v.clone())),
        Found::Range { node, bytes } => Some(Resolution::Range {
            node: *node,
            bytes: bytes.clone(),
        }),
        Found::Snapshot(c) => Some(Resolution::Snapshot(c.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_never_selects_ambiguous_deleted_or_multiple_targets() {
        let mut resolved = Resolved {
            deleted: false,
            targets: vec![],
            diagnostics: vec![],
        };
        assert!(resolved.unique("line").is_none());
        for status in [
            RelationStatus::Missing,
            RelationStatus::Ambiguous,
            RelationStatus::Deleted,
        ] {
            resolved.targets = vec![TargetLayout {
                role: "line".into(),
                status,
                resolved: Some(Resolution::Lines(vec![])),
            }];
            assert!(resolved.unique("line").is_none());
        }
        let target = TargetLayout {
            role: "line".into(),
            status: RelationStatus::Valid,
            resolved: Some(Resolution::Lines(vec![])),
        };
        resolved.targets = vec![target.clone()];
        assert!(resolved.unique("line").is_some());
        resolved.targets = vec![target; 4096];
        assert!(resolved.unique("line").is_none());
    }
}
