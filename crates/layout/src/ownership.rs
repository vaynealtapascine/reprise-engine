//! Which relations name a block their owner, as the flow learns it: the one
//! door through which composition reads that, in the body and in a running
//! story.
//!
//! The flow treats three kinds of block by what owns them: an annotation a
//! float or note owns is left to the region pass, an image likewise, and an
//! image a `follow` relation places is composed and left for that relation.
//! Composing a table's cell reads the same thing of the annotations in it. None
//! of that is in a block's own inputs, so whatever memoises a composition has
//! to key it, and a read that escaped its key has been a defect three times
//! (the keep chain's classification, the continuation's owner, the cell
//! annotation of a running table).
//!
//! So there is one reader of ownership, [`Ownership`], and what it answers is
//! decided by where the flow runs:
//!
//! -   the **body** is laid out afresh by every layout, and nothing it composes
//!     outlives the layout that read the relations, so it reads them when its
//!     flow begins ([`Classes::read`]) and keeps the answer for that flow;
//! -   a **running story** is composed once and reused by every page that shows
//!     it, by what its digest holds. Its flow does not read the relations at
//!     all: it is answered by the digest ([`Digest::class_of`]), which read
//!     them once, when it was made, for every annotation and image it holds,
//!     and which is the key. A change of ownership is a change of the key.
//!
//! The classes of the document (`Classes`) and the answers (`Ownership`) have
//! no public way to be made from the relations except these two, and nothing
//! outside this module holds a set of owners, so a new read of ownership in
//! composition has to be a call of [`Ownership::class`], which is keyed in the
//! one place that matters. The region pass does not compose with ownership; it
//! reads the relations itself, and `tests/relation_reads.rs` fails when any
//! other file starts to.

use std::collections::BTreeSet;
use std::rc::Rc;

use reprise_doc::relation::builtin;
use reprise_doc::{Document, Domain, NodeId};

use crate::Engine;
use crate::running::compose::Digest;

/// What the relations of a document make of a block, as far as the flow is
/// concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    /// No float, note or follow relation names it owner.
    Free,
    /// A follow relation places it (and no float or note owns it).
    Follower,
    /// A float or note owns it: the region pass places it.
    Owner,
}

/// The classes of every block of a document, read from its relations.
#[derive(Debug, Default)]
pub(crate) struct Classes {
    owners: BTreeSet<NodeId>,
    followers: BTreeSet<NodeId>,
}

impl Classes {
    /// One pass over the relations. Only the body's flow and a story's digest
    /// call this.
    pub(crate) fn read(engine: &Engine, doc: &Document) -> Self {
        let mut classes = Self::default();
        for (_, relation) in doc.relations() {
            let Ok(relation) = relation else { continue };
            let Some(owner) = relation.owner else {
                continue;
            };
            if crate::regions::is_region(&relation)
                && engine.schemas.get(&relation.schema).is_some()
            {
                classes.owners.insert(owner);
            }
            if relation.schema == builtin::FOLLOW {
                classes.followers.insert(owner);
            }
        }
        classes
    }

    pub(crate) fn class(&self, node: NodeId) -> Class {
        if self.owners.contains(&node) {
            Class::Owner
        } else if self.followers.contains(&node) {
            Class::Follower
        } else {
            Class::Free
        }
    }
}

/// What a flow is told of ownership. See the module comment.
#[derive(Clone)]
pub(crate) struct Ownership(Source);

#[derive(Clone)]
enum Source {
    /// The body: read from the document's relations when the flow began.
    Document(Rc<Classes>),
    /// A running story: what its digest keyed.
    Keyed(Rc<Digest>),
    /// A story flow with no digest to answer it: no block is owned. Nothing
    /// composes a story without one, and what it would make is no memo's.
    Unkeyed,
}

impl Ownership {
    /// The ownership a flow over `domain` starts with: the document's for the
    /// body, none yet for a story, which is answered by its digest once it is
    /// scoped to it ([`Ownership::keyed`]).
    pub(crate) fn of(engine: &Engine, doc: &Document, domain: Domain) -> Self {
        Self(match domain {
            Domain::Body => Source::Document(Rc::new(Classes::read(engine, doc))),
            _ => Source::Unkeyed,
        })
    }

    /// The ownership of the flow that composes the story `digest` was made
    /// for: nothing but what the digest holds.
    pub(crate) fn keyed(digest: Rc<Digest>) -> Self {
        Self(Source::Keyed(digest))
    }

    /// How the relations classify `node`.
    pub(crate) fn class(&self, node: NodeId) -> Class {
        match &self.0 {
            Source::Document(classes) => classes.class(node),
            Source::Keyed(digest) => digest.class_of(node),
            Source::Unkeyed => Class::Free,
        }
    }

    /// Whether a float or note owns `node`.
    pub(crate) fn owns(&self, node: NodeId) -> bool {
        self.class(node) == Class::Owner
    }
}
