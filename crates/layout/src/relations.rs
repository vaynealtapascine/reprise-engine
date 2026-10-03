//! Pass 2: relations (13, 14, 15). Runs after flow, because layout queries
//! such as `LineContaining` need the flowed lines (26).
//!
//! Each relation is dispatched on its schema. A relation that can't be read,
//! or whose schema this engine doesn't know, is reported and left alone: it
//! stays in the document (34) and the rest of the layout goes on (37).

use reprise_diag::Severity;
use reprise_doc::relation::{Ownership, builtin};
use reprise_doc::{Document, LayoutQuery, Param, RangeState, Relation, RelationId, Target};
use reprise_geom::{FrameSpace, Length, PageSpace, Point, Transform};

use crate::flow::{MARGIN, Pending, shift};
use crate::{
    Diagnostic, Engine, LayoutSnapshot, LineRef, RelationLayout, RelationStatus, Resolution,
    Subject, TargetLayout, codes,
};

pub(crate) fn run(
    engine: &Engine,
    doc: &Document,
    snapshot: &mut LayoutSnapshot,
    mut pending: Vec<Pending>,
) {
    let mut follow = Follow {
        next_free: Length::MIN,
    };
    for (id, relation) in doc.relations() {
        let relation = match relation {
            Ok(r) => r,
            Err(_) => {
                snapshot.diagnostics.push(Diagnostic::new(
                    Severity::Warning,
                    codes::RELATION_UNREADABLE,
                    Subject::Relation(id),
                    "relation can't be read by this engine; kept but not applied",
                ));
                continue;
            }
        };
        let mut result = RelationLayout {
            id,
            schema: relation.schema.clone(),
            owner: relation.owner,
            status: RelationStatus::Valid,
            applied: false,
            targets: Vec::new(),
        };
        let schema = engine.schemas.get(&relation.schema);
        let owned = schema.is_some_and(|s| s.ownership == Ownership::Owned);
        if let Some(owner) = relation.owner.filter(|&o| owned && doc.block(o).is_err()) {
            snapshot.diagnostics.push(Diagnostic::new(
                Severity::Info,
                codes::RELATION_OWNER_DELETED,
                Subject::Relation(id),
                format!("owner {owner} was deleted; the relation went with it"),
            ));
            result.status = RelationStatus::OwnerDeleted;
            snapshot.relations.push(result);
            continue;
        }
        if schema.is_none() {
            snapshot.diagnostics.push(Diagnostic::new(
                Severity::Warning,
                codes::RELATION_UNKNOWN_SCHEMA,
                Subject::Relation(id),
                format!("schema {} is not registered; not applied", relation.schema),
            ));
        } else if relation.schema == builtin::FOLLOW {
            follow.apply(
                engine,
                doc,
                snapshot,
                &mut pending,
                id,
                &relation,
                &mut result,
            );
        } else {
            snapshot.diagnostics.push(Diagnostic::new(
                Severity::Info,
                codes::RELATION_NOT_APPLIED,
                Subject::Relation(id),
                format!("layout has no behaviour for {}", relation.schema),
            ));
        }
        result.status = result
            .targets
            .iter()
            .map(|t| t.status)
            .max()
            .unwrap_or(result.status);
        snapshot.relations.push(result);
    }
    for left in pending {
        snapshot.diagnostics.push(Diagnostic::new(
            Severity::Warning,
            codes::UNPLACED,
            Subject::Node(left.block.node),
            "no relation places this block; not drawn",
        ));
    }
}

/// `reprise.follow`: the owner sits in the margin frame, level with the top
/// of its target line. A block that would overlap the previous one is pushed
/// down below it.
struct Follow {
    /// The first free position in the margin frame.
    next_free: Length,
}

impl Follow {
    #[allow(clippy::too_many_arguments)]
    fn apply(
        &mut self,
        engine: &Engine,
        doc: &Document,
        snapshot: &mut LayoutSnapshot,
        pending: &mut Vec<Pending>,
        id: RelationId,
        relation: &Relation,
        result: &mut RelationLayout,
    ) {
        let subject = Subject::Relation(id);
        let report = |snapshot: &mut LayoutSnapshot, severity, code, message: String| {
            snapshot
                .diagnostics
                .push(Diagnostic::new(severity, code, subject.clone(), message));
        };
        let Some(Target::Layout(LayoutQuery::LineContaining { range })) = relation.first("line")
        else {
            report(
                snapshot,
                Severity::Error,
                codes::RELATION_BAD_TARGET,
                "role `line` must hold a line-containing query".into(),
            );
            return;
        };
        let mut target = TargetLayout {
            role: "line".into(),
            status: RelationStatus::Missing,
            resolved: None,
        };
        let (status, node, bytes) = match doc.resolve_range(*range) {
            RangeState::Valid { node, bytes } => (RelationStatus::Valid, node, bytes),
            RangeState::Rebound { node, bytes } => {
                report(
                    snapshot,
                    Severity::Info,
                    codes::RELATION_REBOUND,
                    format!("target range {range} lost part of its text; rebound"),
                );
                (RelationStatus::Rebound, node, bytes)
            }
            RangeState::Missing { .. } => {
                report(
                    snapshot,
                    Severity::Error,
                    codes::RELATION_MISSING_TARGET,
                    format!("target range {range} is gone; owner not placed"),
                );
                result.targets.push(target);
                return;
            }
        };
        let Some(line) = snapshot.line_containing(node, bytes.start) else {
            report(
                snapshot,
                Severity::Error,
                codes::RELATION_NO_MATCH,
                format!("no line of {node} contains byte {}", bytes.start),
            );
            result.targets.push(target);
            return;
        };
        target.status = status;
        target.resolved = Some(Resolution::Line(line));
        result.targets.push(target);

        let owner = relation.owner;
        let Some(pos) = pending.iter().position(|p| Some(p.block.node) == owner) else {
            report(
                snapshot,
                Severity::Error,
                codes::RELATION_OWNER,
                "owner is not a block a relation can place, or is already placed".into(),
            );
            return;
        };
        let Some(want) = line_top_in(snapshot, line, MARGIN) else {
            return;
        };
        let offset = match relation.params.get("offset") {
            Some(Param::Length(expr)) => expr.resolve(pending[pos].block.style.size),
            _ => Length::ZERO,
        };
        let want = want + offset;
        let at = if want < self.next_free {
            report(
                snapshot,
                Severity::Warning,
                codes::RELATION_PUSHED,
                "pushed down to avoid the previous block".into(),
            );
            self.next_free
        } else {
            want
        };
        let placed = pending.remove(pos);
        self.next_free = at + placed.extent + engine.page.annotation_spacing;
        snapshot.blocks.push(shift(placed.block, at));
        result.applied = true;
    }
}

/// The top of a line's box, on the block axis of another frame.
fn line_top_in(snapshot: &LayoutSnapshot, line: LineRef, frame: usize) -> Option<Length> {
    let l = snapshot.line(line)?;
    let from = snapshot.frame(l.frame)?;
    let to: Transform<PageSpace, FrameSpace> = snapshot.frame(frame)?.to_page.inverse()?;
    let on_page = from.to_page.apply(l.rect.origin);
    Some(to.apply(Point::new(on_page.x, on_page.y)).y)
}
