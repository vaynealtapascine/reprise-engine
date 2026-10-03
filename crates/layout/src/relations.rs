//! Pass 2: relations. Each annotation follows its target line, which is only
//! known after the flow pass (26).

use reprise_doc::{Document, RangeState, RelationKind, Target};
use reprise_geom::{Length, Point};

use crate::flow::{Laid, place};
use crate::{Diagnostic, Engine, LayoutSnapshot, RelationLayout, RelationStatus};

/// Places each annotation beside its target line; if it would overlap the
/// previous annotation it is pushed down.
pub(crate) fn run(
    engine: &Engine,
    doc: &Document,
    snapshot: &mut LayoutSnapshot,
    mut annotations: Vec<Laid>,
) {
    let page = engine.page;
    let margin_x = page.margin_left + page.column_width + page.gutter;
    let mut next_free = Length::ZERO;
    for (id, relation) in doc.relations() {
        let RelationKind::Follow = relation.kind;
        let Target::LineContaining { range } = &relation.target;
        let mut result = RelationLayout {
            id: id.clone(),
            kind: relation.kind,
            source: relation.source,
            status: RelationStatus::Missing,
            target_line: None,
        };
        let (status, node, bytes) = match doc.resolve_range(range) {
            RangeState::Valid { node, bytes } => (RelationStatus::Valid, node, bytes),
            RangeState::Rebound { node, bytes } => (RelationStatus::Rebound, node, bytes),
            RangeState::Missing { .. } => {
                snapshot.diagnostics.push(Diagnostic {
                    node: Some(relation.source),
                    message: format!(
                        "relation {}: target range {} is gone; source not placed",
                        id.0, range.0
                    ),
                });
                snapshot.relations.push(result);
                continue;
            }
        };
        let line = snapshot.line_containing(node, bytes.start);
        let Some(pos) = annotations
            .iter()
            .position(|a| a.block.node == relation.source)
        else {
            snapshot.diagnostics.push(Diagnostic {
                node: Some(relation.source),
                message: format!(
                    "relation {}: source is not an annotation, or is already placed",
                    id.0
                ),
            });
            snapshot.relations.push(result);
            continue;
        };
        let Some(line) = line else {
            snapshot.diagnostics.push(Diagnostic {
                node: Some(node),
                message: format!("relation {}: no line contains byte {}", id.0, bytes.start),
            });
            snapshot.relations.push(result);
            continue;
        };
        let target = &snapshot
            .block(node)
            .expect("line_containing found it")
            .lines[line];
        let want = target.rect.origin.y;
        let at = if want < next_free {
            snapshot.diagnostics.push(Diagnostic {
                node: Some(relation.source),
                message: format!(
                    "relation {}: pushed down to avoid the previous annotation",
                    id.0
                ),
            });
            next_free
        } else {
            want
        };
        let laid = annotations.remove(pos);
        next_free = at + laid.height + page.annotation_spacing;
        snapshot.blocks.push(place(laid, Point::new(margin_x, at)));
        result.status = status;
        result.target_line = Some((node, line));
        snapshot.relations.push(result);
    }
    for left in annotations {
        snapshot.diagnostics.push(Diagnostic {
            node: Some(left.block.node),
            message: "annotation has no relation placing it; not drawn".into(),
        });
    }
}
