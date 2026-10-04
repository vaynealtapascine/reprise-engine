//! Explicit block precedence (33). Multiple relations form a partial order.
use crate::relation::{CopyCrossing, CopyInside, CopyPolicy, OnTargetDeleted, Ownership, RoleSpec};
use crate::{NodeId, Relation, RelationSchema, SchemaId, Target, TargetClass};
use std::borrow::Cow;
pub const READING_ORDER: SchemaId = SchemaId::new("reprise.reading-order");
/// Read `before` in its entirety before `after`. Missing endpoints have no
/// effect. Cycles and conflicts are repaired by layout, never stored back.
pub fn before(before: NodeId, after: NodeId) -> Relation {
    Relation::new(READING_ORDER)
        .target("before", Target::Node(before))
        .target("after", Target::Node(after))
}
pub fn schema() -> RelationSchema {
    RelationSchema {
        id: READING_ORDER,
        version: 1,
        ownership: Ownership::Independent,
        roles: ["before", "after"]
            .into_iter()
            .map(|name| RoleSpec {
                name: Cow::Borrowed(name),
                accepts: Cow::Borrowed(&[TargetClass::Node]),
                min: 1,
                max: Some(1),
            })
            .collect(),
        params: Vec::new(),
        on_target_deleted: OnTargetDeleted::KeepMissing,
        on_copy: CopyPolicy {
            inside: CopyInside::Duplicate,
            crossing: CopyCrossing::KeepOutside,
        },
    }
}
