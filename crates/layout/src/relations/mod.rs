//! Pass 2: relations (13, 14, 15). Runs after flow, because layout queries
//! such as `LineContaining` need the flowed lines (26).
//!
//! Each relation is dispatched on its schema. A relation that can't be read,
//! or whose schema this engine doesn't know, is reported and left alone: it
//! stays in the document (34) and the rest of the layout goes on (37).

use reprise_diag::Severity;
use reprise_doc::Document;
use reprise_doc::relation::{Ownership, builtin};
use reprise_geom::Length;

use crate::flow::Pending;
use crate::{Diagnostic, Engine, LayoutSnapshot, RelationLayout, RelationStatus, Subject, codes};

mod follow;

use follow::Follow;

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
