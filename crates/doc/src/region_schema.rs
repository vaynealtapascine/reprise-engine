//! The relation schemas of the region subsystems (24): floats and notes.
//!
//! Both are owned by the block they place, like `reprise.follow`, and both
//! are *registered schemas*, not new block kinds: a float or a note is an
//! [`Annotation`](crate::BlockKind::Annotation) that a relation places. Layout
//! gives each its behaviour (`reprise-layout`, `floats` and `notes`); here are
//! only the declarations and the parameter vocabularies, so documents, the
//! editing kernel and layout agree on them.

use std::borrow::Cow;

use crate::relation::{
    CopyCrossing, CopyInside, CopyPolicy, OnTargetDeleted, Ownership, ParamKind, ParamSpec,
    RelationSchema, RoleSpec, SchemaId, TargetClass,
};

/// `reprise.float`: the owner block floats to a side or an edge of the frame
/// that holds its anchor, and the text runs around it.
///
/// -   Owned by the block it places.
/// -   Role `anchor`: exactly one range or layout query. The float is
///     placed in the main-flow frame holding the anchor's (last) line.
/// -   Parameter `side` (text, optional): `left`, `right`, `top` or `bottom`.
///     Default `right`.
/// -   Parameter `width` (length, optional): how wide a `left` or `right`
///     float is. Default a third of the frame. `top` and `bottom` floats
///     span the frame.
/// -   Parameter `margin` (length, optional): the gap between the float and
///     the text around it. Default is the engine's float margin.
pub const FLOAT: SchemaId = SchemaId::new("reprise.float");

pub fn float() -> RelationSchema {
    RelationSchema {
        id: FLOAT,
        version: 1,
        ownership: Ownership::Owned,
        roles: vec![RoleSpec {
            name: Cow::Borrowed("anchor"),
            accepts: Cow::Borrowed(&[TargetClass::Range, TargetClass::Layout]),
            min: 1,
            max: Some(1),
        }],
        params: vec![
            ParamSpec {
                name: Cow::Borrowed("side"),
                kind: ParamKind::Text,
                required: false,
            },
            ParamSpec {
                name: Cow::Borrowed("width"),
                kind: ParamKind::Length,
                required: false,
            },
            ParamSpec {
                name: Cow::Borrowed("margin"),
                kind: ParamKind::Length,
                required: false,
            },
        ],
        on_target_deleted: OnTargetDeleted::Rebind,
        on_copy: CopyPolicy {
            inside: CopyInside::Duplicate,
            crossing: CopyCrossing::KeepOutside,
        },
    }
}

/// `reprise.note`: the owner block is a note on its anchor, placed in the
/// notes area of the anchor's page. The anchor may sit in the body or in
/// another note: notes nest.
///
/// -   Owned by the note block.
/// -   Role `anchor`: exactly one range, in a body block or in another note.
/// -   Parameter `placement` (text, optional): `page` (a footnote: the notes
///     area of the anchor's page, the default) or `end` (an endnote: the
///     notes frames after the last page of the body).
pub const NOTE: SchemaId = SchemaId::new("reprise.note");

pub fn note() -> RelationSchema {
    RelationSchema {
        id: NOTE,
        version: 1,
        ownership: Ownership::Owned,
        roles: vec![RoleSpec {
            name: Cow::Borrowed("anchor"),
            accepts: Cow::Borrowed(&[TargetClass::Range]),
            min: 1,
            max: Some(1),
        }],
        params: vec![ParamSpec {
            name: Cow::Borrowed("placement"),
            kind: ParamKind::Text,
            required: false,
        }],
        on_target_deleted: OnTargetDeleted::Rebind,
        on_copy: CopyPolicy {
            inside: CopyInside::Duplicate,
            crossing: CopyCrossing::KeepOutside,
        },
    }
}

/// Which side or edge of its frame a float takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FloatSide {
    Left,
    Right,
    Top,
    Bottom,
}

impl FloatSide {
    pub const fn as_str(self) -> &'static str {
        match self {
            FloatSide::Left => "left",
            FloatSide::Right => "right",
            FloatSide::Top => "top",
            FloatSide::Bottom => "bottom",
        }
    }

    /// `None` for text that isn't a side.
    pub fn parse(text: &str) -> Option<FloatSide> {
        match text {
            "left" => Some(FloatSide::Left),
            "right" => Some(FloatSide::Right),
            "top" => Some(FloatSide::Top),
            "bottom" => Some(FloatSide::Bottom),
            _ => None,
        }
    }
}

/// Where a note goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotePlacement {
    /// The notes area of the page holding the anchor.
    Page,
    /// The notes frames after the body.
    End,
}

impl NotePlacement {
    pub const fn as_str(self) -> &'static str {
        match self {
            NotePlacement::Page => "page",
            NotePlacement::End => "end",
        }
    }

    pub fn parse(text: &str) -> Option<NotePlacement> {
        match text {
            "page" => Some(NotePlacement::Page),
            "end" => Some(NotePlacement::End),
            _ => None,
        }
    }
}
