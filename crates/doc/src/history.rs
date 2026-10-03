//! The document as it was (13, 07): the history behind snapshot targets.
//!
//! A [`Revision`] names a version of one document's operation history. A
//! [`DocumentAt`] is that version, read-only. Versions can become
//! unavailable, and callers must expect it:
//!
//! -   the version is not in this document's history ([`VersionError::Unknown`]).
//!     Peers' edits that haven't been merged yet are exactly this, so it can
//!     stop being true.
//! -   history before some point was compacted away
//!     ([`VersionError::Compacted`], see [`Document::compact_history`]).
//! -   the revision is not a frontier at all ([`VersionError::Malformed`]).
//!
//! A revision doesn't name its document. It resolves against whichever
//! document you ask, and two unrelated documents that pin the same peer ID
//! (as fixtures do) can share revisions. Where that matters, the file format
//! (34) should record a document identity next to it.

use std::collections::BTreeMap;
use std::ops::Range;

use loro::{ExportMode, Frontiers, ID, LoroDoc};
use serde::{Deserialize, Serialize};

use crate::relation::{SnapshotOf, SnapshotRef};
use crate::{DocError, Document, NodeId, RangeId, RangeState, Revision};

/// Why a version can't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "kebab-case")]
pub enum VersionError {
    /// Not a version this engine would write: unsorted, repeating a peer, or
    /// with a negative counter.
    #[error("the version is malformed")]
    Malformed,
    /// Not in this document's history, at least not yet.
    #[error("the version is not in this document's history")]
    Unknown,
    /// In the history of this document once, but compacted away.
    #[error("the version was compacted away")]
    Compacted,
}

/// A document as it was at a [`Revision`]. Read-only: it exposes no way to
/// edit.
pub struct DocumentAt {
    doc: Document,
    version: Revision,
}

impl DocumentAt {
    pub fn revision(&self) -> &Revision {
        &self.version
    }

    /// The top-level blocks that existed then, in order.
    pub fn blocks(&self) -> Vec<NodeId> {
        self.doc.blocks()
    }

    /// A block's text then; `None` if it didn't exist.
    pub fn text(&self, node: NodeId) -> Option<String> {
        self.doc.block(node).ok().map(|b| b.text.to_string())
    }

    /// Where a range was then.
    pub fn resolve_range(&self, id: RangeId) -> RangeState {
        self.doc.resolve_range(id)
    }
}

/// The longest text a [`SnapshotContent`] carries. Longer text is cut at a
/// character boundary and flagged.
pub const MAX_SNAPSHOT_TEXT: usize = 4096;

/// Content as a snapshot target found it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotContent {
    pub version: Revision,
    pub node: NodeId,
    /// The bytes of the block's text *then* that the reference covers: the
    /// whole text for a node, the range's bytes for a range.
    pub bytes: Range<usize>,
    /// The covered text as it was then, cut at [`MAX_SNAPSHOT_TEXT`].
    pub text: String,
    pub truncated: bool,
    /// Whether the subject (the node, or the range) resolves now.
    pub exists_now: bool,
    /// Whether the subject resolves now to exactly the same text.
    pub unchanged: bool,
}

/// What a snapshot reference found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotState {
    Found(SnapshotContent),
    /// The version is fine, but the subject didn't exist (or, for a range,
    /// was empty and missing) then.
    NotThere,
    Unavailable(VersionError),
}

/// Versions already opened, so that several references to one version open it
/// once. Opening a version copies the document, so the cache keeps a few and
/// forgets them all when it fills up: it can't change any answer, only how
/// long it takes.
///
/// A cache belongs to one document at one revision. Make a new one when the
/// document changes.
#[derive(Default)]
pub struct HistoryCache {
    views: BTreeMap<Revision, Result<DocumentAt, VersionError>>,
}

impl HistoryCache {
    /// How many versions are kept open at once.
    pub const CAPACITY: usize = 8;

    /// Resolves a snapshot reference against `now`.
    pub fn resolve(&mut self, now: &Document, reference: &SnapshotRef) -> SnapshotState {
        if self.views.len() >= Self::CAPACITY && !self.views.contains_key(&reference.version) {
            self.views.clear();
        }
        let view = self
            .views
            .entry(reference.version.clone())
            .or_insert_with(|| now.at(&reference.version));
        match view {
            Ok(view) => view.resolve(now, reference),
            Err(e) => SnapshotState::Unavailable(*e),
        }
    }
}

impl DocumentAt {
    fn resolve(&self, now: &Document, reference: &SnapshotRef) -> SnapshotState {
        // The subject then: its node, the bytes covered, and the text.
        let (node, bytes) = match reference.of {
            SnapshotOf::Node(node) => match self.doc.block(node) {
                Ok(block) => (node, 0..block.text.len()),
                Err(_) => return SnapshotState::NotThere,
            },
            SnapshotOf::Range(range) => match self.doc.resolve_range(range) {
                RangeState::Valid { node, bytes } | RangeState::Rebound { node, bytes } => {
                    (node, bytes)
                }
                RangeState::Missing { .. } => return SnapshotState::NotThere,
            },
        };
        let Some(then) = self
            .doc
            .block(node)
            .ok()
            .and_then(|b| b.text.slice(bytes.clone()).ok())
        else {
            return SnapshotState::NotThere;
        };
        // The subject now.
        let current = match reference.of {
            SnapshotOf::Node(node) => now.block(node).ok().map(|b| b.text.to_string()),
            SnapshotOf::Range(range) => match now.resolve_range(range) {
                RangeState::Valid { node, bytes } | RangeState::Rebound { node, bytes } => {
                    now.block(node).ok().and_then(|b| b.text.slice(bytes).ok())
                }
                RangeState::Missing { .. } => None,
            },
        };
        let (text, truncated) = truncate(then.clone());
        SnapshotState::Found(SnapshotContent {
            version: reference.version.clone(),
            node,
            bytes,
            text,
            truncated,
            exists_now: current.is_some(),
            unchanged: current.is_some_and(|c| c == then),
        })
    }
}

fn truncate(mut text: String) -> (String, bool) {
    if text.len() <= MAX_SNAPSHOT_TEXT {
        return (text, false);
    }
    let mut end = MAX_SNAPSHOT_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
}

impl Document {
    /// Checks a revision and turns it into a Loro frontier. Loro panics on
    /// frontiers it doesn't expect, so nothing unchecked reaches it: a
    /// revision must be sorted by peer with each peer once, and every ID in
    /// it must be in the history.
    fn frontiers_of(&self, version: &Revision) -> Result<Frontiers, VersionError> {
        let ids = &version.0;
        if ids.windows(2).any(|w| w[0].0 >= w[1].0) || ids.iter().any(|&(_, c)| c < 0) {
            return Err(VersionError::Malformed);
        }
        let known = self.doc.oplog_vv();
        if ids
            .iter()
            .any(|&(peer, counter)| known.get(&peer).is_none_or(|&end| counter >= end))
        {
            return Err(VersionError::Unknown);
        }
        Ok(match ids.as_slice() {
            [] => Frontiers::default(),
            [(peer, counter)] => Frontiers::from(ID::new(*peer, *counter)),
            many => many.iter().map(|&(p, c)| ID::new(p, c)).collect(),
        })
    }

    /// The document as it was at `version`. The empty revision is the empty
    /// document before anything was written.
    pub fn at(&self, version: &Revision) -> Result<DocumentAt, VersionError> {
        self.commit();
        let frontiers = self.frontiers_of(version)?;
        let doc = if self.doc.is_shallow() {
            // Loro can't open a past version of a compacted document
            // (`fork_at` is not implemented for them), so only the present
            // is available. Anything older is, truthfully, compacted away.
            if *version != self.revision() {
                return Err(VersionError::Compacted);
            }
            self.doc.fork()
        } else {
            // The version is in the history, so a failure can only be that
            // the history before it is gone.
            self.doc
                .fork_at(&frontiers)
                .map_err(|_| VersionError::Compacted)?
        };
        Ok(DocumentAt {
            doc: Document { doc },
            version: version.clone(),
        })
    }

    /// Resolves a snapshot reference. Opens the version afresh each time; use
    /// a [`HistoryCache`] to resolve several.
    pub fn resolve_snapshot(&self, reference: &SnapshotRef) -> SnapshotState {
        HistoryCache::default().resolve(self, reference)
    }

    /// A copy of this document that has dropped all history before `since`
    /// (07): the compaction tombstones will need eventually. The copy has the
    /// same content, IDs, tombstones and relations, and merges with replicas
    /// that are not older than `since`. Every version
    /// older than the copy's present becomes [`VersionError::Compacted`] in it.
    /// That is stricter than the history it keeps: Loro can't yet open past
    /// versions of a compacted document, only its present one. A snapshot
    /// target in a compacted document therefore resolves only if it names
    /// the document's current revision, and otherwise reports
    /// `relation.snapshot-unavailable`. `since` marks how much history the
    /// copy keeps for merging with replicas.
    ///
    /// `peer` is the copy's peer ID. Retire the original; keeping both
    /// editing under one peer ID corrupts the history.
    pub fn compact_history(&self, since: &Revision, peer: u64) -> Result<Document, DocError> {
        self.commit();
        let frontiers = self
            .frontiers_of(since)
            .map_err(|e| DocError::Export(e.to_string()))?;
        let bytes = self
            .doc
            .export(ExportMode::shallow_snapshot(&frontiers))
            .map_err(|e| DocError::Export(e.to_string()))?;
        let doc = LoroDoc::new();
        doc.set_peer_id(peer)?;
        doc.import(&bytes)?;
        doc.get_tree("content").enable_fractional_index(0);
        Ok(Document { doc })
    }
}
