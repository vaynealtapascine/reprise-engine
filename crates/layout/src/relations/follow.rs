//! `reprise.follow`: a block placed in the margin beside the line it follows.

use reprise_diag::Severity;
use reprise_doc::{Document, LayoutQuery, Param, RangeState, Relation, RelationId, Target};
use reprise_geom::{FrameSpace, Length, PageSpace, Point, Transform};

use crate::flow::{MARGIN, Pending, shift};
use crate::{
    Diagnostic, Engine, LayoutSnapshot, LineRef, RelationLayout, RelationStatus, Resolution,
    Subject, TargetLayout, codes,
};

/// `reprise.follow`: the owner sits in the margin frame, level with the top
/// of its target line. A block that would overlap the previous one is pushed
/// down below it.
pub(super) struct Follow {
    /// The first free position in the margin frame.
    pub(super) next_free: Length,
}

impl Follow {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply(
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
