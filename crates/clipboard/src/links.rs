//! Links and comment anchors in the exporters (`docs/annotations.md`).
//!
//! One index per export: which blocks are internal link targets (they get an
//! `id` in HTML), what each link's address is, and for a block the disjoint
//! linked stretches in order. A comment anchor has no representation in the
//! text formats and is reported as not exported.

use std::collections::BTreeSet;
use std::ops::Range;

use reprise_doc::annotation::{AnnotationData, AnnotationState, AnnotationView, LinkTarget};
use reprise_doc::{Document, NodeId, RangeState};

/// The most characters of an HTML `id` made from a node.
const MAX_ID: usize = 64;

/// One linked stretch of one block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stretch {
    pub bytes: Range<usize>,
    /// The address: an external URL, or `#id` of a block in the same export.
    pub href: String,
}

pub(crate) struct LinkIndex<'a> {
    doc: &'a Document,
    view: AnnotationView,
    /// Blocks an internal link goes to, whose `<p>` gets an `id`.
    targets: BTreeSet<NodeId>,
    /// Some link pieces overlapped or could not be written; reported once.
    pub approximated: std::cell::Cell<bool>,
}

/// What the document holds, for the loss report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Presence {
    pub links: bool,
    pub comments: bool,
}

/// The HTML `id` of a block: `r` and the node's counter and peer.
pub(crate) fn target_id(node: NodeId) -> String {
    let mut id: String = format!("r{node}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    id.truncate(MAX_ID);
    id
}

impl<'a> LinkIndex<'a> {
    pub fn new(doc: &'a Document) -> Self {
        let mut index = LinkIndex {
            doc,
            view: doc.annotation_view(),
            targets: BTreeSet::new(),
            approximated: std::cell::Cell::new(false),
        };
        let mut targets = BTreeSet::new();
        for annotation in index.view.iter() {
            if let (AnnotationData::Link(target), AnnotationState::Active { .. }) =
                (&annotation.data, &annotation.state)
                && let Some(node) = index.target_node(target)
            {
                targets.insert(node);
            }
        }
        index.targets = targets;
        index
    }

    pub fn presence(&self) -> Presence {
        let mut out = Presence::default();
        for annotation in self.view.iter() {
            match (&annotation.data, &annotation.state) {
                (_, AnnotationState::Gone) => {}
                (AnnotationData::Link(_), _) => out.links = true,
                (AnnotationData::Comment(_), _) => out.comments = true,
            }
        }
        out
    }

    /// The block an internal link goes to, if it is live.
    fn target_node(&self, target: &LinkTarget) -> Option<NodeId> {
        match target {
            LinkTarget::Url { .. } => None,
            LinkTarget::Node { node } | LinkTarget::Heading { node } => {
                self.doc.is_live(*node).then_some(*node)
            }
            LinkTarget::Range { range } => match self.doc.resolve_range(*range) {
                RangeState::Valid { node, .. } | RangeState::Rebound { node, .. } => Some(node),
                _ => None,
            },
        }
    }

    /// Whether the export must give `node` an `id`.
    pub fn is_target(&self, node: NodeId) -> bool {
        self.targets.contains(&node)
    }

    /// The linked stretches of `node`'s text, disjoint and in order. Where
    /// two links overlap the first keeps the shared text (HTML has no nested
    /// anchors).
    pub fn stretches(&self, node: NodeId, text: &str) -> Vec<Stretch> {
        let mut out: Vec<Stretch> = Vec::new();
        let mut covered = 0usize;
        for (bytes, id) in self.view.pieces(node) {
            let Some(annotation) = self.view.get(*id) else {
                continue;
            };
            let (AnnotationData::Link(target), AnnotationState::Active { .. }) =
                (&annotation.data, &annotation.state)
            else {
                continue;
            };
            let href = match target {
                LinkTarget::Url { url } => url.clone(),
                other => match self.target_node(other) {
                    Some(node) => format!("#{}", target_id(node)),
                    None => {
                        self.approximated.set(true);
                        continue;
                    }
                },
            };
            let start = bytes.start.max(covered);
            let end = bytes.end.min(text.len());
            if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                if bytes.start < covered || start < end {
                    self.approximated.set(true);
                }
                continue;
            }
            if start > bytes.start {
                self.approximated.set(true);
            }
            covered = end;
            out.push(Stretch {
                bytes: start..end,
                href,
            });
        }
        out
    }
}
