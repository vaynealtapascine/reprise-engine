//! Typed range annotations: data attached to a span of text that follows the
//! text as it is edited. See `docs/annotations.md`.
//!
//! An annotation is a persistent range (`ranges.rs`) with one more metadata
//! key, `annot1`. Its ID is the range's ID, its edge behaviour is the range's
//! policy, chosen by its [`Kind`], and its author is the peer that staged it.
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use loro::Frontiers;
use reprise_diag::{Code, Note};
use serde::{Deserialize, Serialize};

use crate::relation::IdMap;
use crate::text::{Affinity, Empty, RangePolicy};
use crate::{DocError, Document, NodeId, RangeId, get_str};

/// An annotation's identity: the ID of the range that carries it.
pub type AnnotationId = RangeId;

/// The `ranges` metadata key holding the versioned envelope.
pub(crate) const KEY: &str = "annot1";

/// A stored annotation that this engine cannot read: a future kind or
/// version, or a malformed or invalid payload. Kept verbatim, never applied.
pub const UNREADABLE: Code = Code::new("annotation.unreadable");
/// More annotations than [`MAX_ANNOTATIONS`]; the excess is not applied.
pub const LIMIT: Code = Code::new("annotation.limit");
/// A link scheme outside the allowlist, such as `javascript:` or `data:`.
pub const LINK_SCHEME: Code = Code::new("annotation.link-scheme");
/// A link that is not an absolute URL this engine accepts.
pub const LINK_INVALID: Code = Code::new("annotation.link-invalid");
/// A link longer than [`MAX_URL_BYTES`].
pub const LINK_LENGTH: Code = Code::new("annotation.link-length");
/// A payload that fails its kind's validation.
pub const PAYLOAD_INVALID: Code = Code::new("annotation.payload-invalid");
/// An annotation over no text.
pub const EMPTY: Code = Code::new("annotation.empty");
/// No live annotation with that ID.
pub const MISSING: Code = Code::new("annotation.missing");

/// The most live annotation records read, in ID order.
pub const MAX_ANNOTATIONS: usize = 65_536;
/// The most annotations one query returns.
pub const MAX_QUERY_RESULTS: usize = 4096;
/// The most paragraph pieces one annotation resolves to.
pub const MAX_PIECES: usize = 4096;
/// The longest accepted URL, in bytes.
pub const MAX_URL_BYTES: usize = 8192;
/// The longest comment thread ID, in bytes.
pub const MAX_THREAD_BYTES: usize = 256;
/// Envelopes are bounded before JSON parsing.
pub(crate) const MAX_ENVELOPE_BYTES: usize = 16 * 1024;

/// What happens to an annotation when all of its text is deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnEmpty {
    /// It collapses to a point that stays queryable, reported as orphaned.
    Orphan,
    /// It is no longer reported; its record stays in the document.
    Disappear,
}

/// What a copy does with an annotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CopyPolicy {
    /// The copy gets a new annotation with the same payload.
    Duplicate,
    /// The copy leaves it out.
    Drop,
}

/// Everything a kind declares. A new kind adds a [`Kind`] variant, an
/// [`AnnotationData`] variant and one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindSpec {
    pub kind: Kind,
    /// The stored kind name.
    pub name: &'static str,
    /// Edge behaviour: whether typing at either end joins the span, and
    /// whether the range survives empty. Must agree with `on_empty`.
    pub policy: RangePolicy,
    pub on_empty: OnEmpty,
    pub copy: CopyPolicy,
}

/// The registry of annotation kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Link,
    Comment,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Link, Kind::Comment];

    pub fn spec(self) -> KindSpec {
        match self {
            // Typing at either edge of a link stays outside it.
            Kind::Link => KindSpec {
                kind: self,
                name: "link",
                policy: RangePolicy::FIXED,
                on_empty: OnEmpty::Disappear,
                copy: CopyPolicy::Duplicate,
            },
            // Typing at the end of commented text joins the comment.
            Kind::Comment => KindSpec {
                kind: self,
                name: "comment",
                policy: RangePolicy {
                    start: Affinity::After,
                    end: Affinity::After,
                    empty: Empty::Keep,
                },
                on_empty: OnEmpty::Orphan,
                copy: CopyPolicy::Drop,
            },
        }
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// Where a link goes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LinkTarget {
    /// An absolute URL with an allowed scheme; see [`validate_url`].
    Url { url: String },
    /// A block of this document.
    Node { node: NodeId },
    /// A heading of this document ("link to heading").
    Heading { node: NodeId },
    /// A persistent position or span of this document.
    Range { range: RangeId },
}

/// A comment anchor: the thread it belongs to. Thread contents live elsewhere.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentAnchor {
    pub thread: String,
}

/// An annotation's kind and typed payload.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "kebab-case")]
pub enum AnnotationData {
    Link(LinkTarget),
    Comment(CommentAnchor),
}

/// Why an annotation edit was refused. Nothing was written.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AnnotationError {
    #[error("link scheme {0:?} is not allowed")]
    LinkScheme(String),
    #[error("link is not an accepted absolute URL: {0}")]
    LinkInvalid(&'static str),
    #[error("link of {0} bytes is longer than {MAX_URL_BYTES}")]
    LinkLength(usize),
    #[error("invalid annotation payload: {0}")]
    Payload(String),
    #[error("an annotation must cover some text")]
    Empty,
    #[error("the document holds the most annotations it can ({MAX_ANNOTATIONS})")]
    Limit,
    #[error("no live annotation {0}")]
    Missing(AnnotationId),
    /// An unexpected document failure, not a validation result.
    #[error("the document store refused the change: {0}")]
    Store(String),
}

impl AnnotationError {
    /// The stable code; `None` for an unexpected store failure.
    pub fn code(&self) -> Option<Code> {
        Some(match self {
            AnnotationError::LinkScheme(_) => LINK_SCHEME,
            AnnotationError::LinkInvalid(_) => LINK_INVALID,
            AnnotationError::LinkLength(_) => LINK_LENGTH,
            AnnotationError::Payload(_) => PAYLOAD_INVALID,
            AnnotationError::Empty => EMPTY,
            AnnotationError::Limit => LIMIT,
            AnnotationError::Missing(_) => MISSING,
            AnnotationError::Store(_) => return None,
        })
    }
}

impl From<DocError> for AnnotationError {
    fn from(e: DocError) -> Self {
        match e {
            // Offsets and ranges the caller supplied.
            DocError::Text(e) => AnnotationError::Payload(e.to_string()),
            DocError::NoNode(n) => AnnotationError::Payload(format!("no live block {n}")),
            e => AnnotationError::Store(e.to_string()),
        }
    }
}

impl From<crate::text::TextError> for AnnotationError {
    fn from(e: crate::text::TextError) -> Self {
        AnnotationError::Payload(e.to_string())
    }
}

impl From<loro::LoroError> for AnnotationError {
    fn from(e: loro::LoroError) -> Self {
        AnnotationError::Store(e.to_string())
    }
}

/// Checks a link URL: at most [`MAX_URL_BYTES`], no whitespace, control
/// characters or backslashes, an explicit scheme from the allowlist (`http`,
/// `https`, `mailto`, `tel`, in any case), and a host for `http(s)`. Relative
/// URLs are refused: an annotation has no base to resolve them against.
pub fn validate_url(url: &str) -> Result<(), AnnotationError> {
    if url.len() > MAX_URL_BYTES {
        return Err(AnnotationError::LinkLength(url.len()));
    }
    if url.is_empty() {
        return Err(AnnotationError::LinkInvalid("empty"));
    }
    if url
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(AnnotationError::LinkInvalid(
            "whitespace, control characters or backslashes",
        ));
    }
    // Invisible and bidirectional controls make an address read as another.
    if url.chars().any(|c| {
        matches!(
            c,
            '\u{ad}'
                | '\u{34f}'
                | '\u{61c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
        )
    }) {
        return Err(AnnotationError::LinkInvalid(
            "invisible or bidirectional control characters",
        ));
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err(AnnotationError::LinkInvalid("no scheme"));
    };
    let mut chars = scheme.chars();
    let syntactic = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !syntactic {
        return Err(AnnotationError::LinkInvalid("relative or malformed scheme"));
    }
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => {
            let authority = rest
                .strip_prefix("//")
                .ok_or(AnnotationError::LinkInvalid("no authority"))?;
            let host = authority.split(['/', '?', '#']).next().unwrap_or_default();
            let host = host.rsplit('@').next().unwrap_or_default();
            if host.is_empty() || host.starts_with(':') {
                return Err(AnnotationError::LinkInvalid("no host"));
            }
            Ok(())
        }
        "mailto" | "tel" if !rest.is_empty() => Ok(()),
        "mailto" | "tel" => Err(AnnotationError::LinkInvalid("no address")),
        other => Err(AnnotationError::LinkScheme(
            other.chars().take(32).collect(),
        )),
    }
}

impl AnnotationData {
    pub fn kind(&self) -> Kind {
        match self {
            AnnotationData::Link(_) => Kind::Link,
            AnnotationData::Comment(_) => Kind::Comment,
        }
    }

    /// The payload checks that need no document: URL and thread ID syntax.
    pub fn validate(&self) -> Result<(), AnnotationError> {
        match self {
            AnnotationData::Link(LinkTarget::Url { url }) => validate_url(url),
            AnnotationData::Link(_) => Ok(()),
            AnnotationData::Comment(CommentAnchor { thread }) => {
                let ok = !thread.is_empty()
                    && thread.len() <= MAX_THREAD_BYTES
                    && thread.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-')
                    });
                if ok {
                    Ok(())
                } else {
                    Err(AnnotationError::Payload(format!(
                        "a thread ID is 1 to {MAX_THREAD_BYTES} bytes of [A-Za-z0-9._:-]"
                    )))
                }
            }
        }
    }

    /// The same data for a pasted copy: internal targets follow the paste's
    /// ID map. A target outside the copy stays only when `same_namespace`
    /// (the source and target document are the same); otherwise `None`.
    pub fn remapped(&self, ids: &IdMap, same_namespace: bool) -> Option<AnnotationData> {
        let AnnotationData::Link(target) = self else {
            return Some(self.clone());
        };
        let node = |n: &NodeId| match ids.nodes.get(n) {
            Some(&new) => Some(new),
            None => same_namespace.then_some(*n),
        };
        let target = match target {
            LinkTarget::Url { .. } => target.clone(),
            LinkTarget::Node { node: n } => LinkTarget::Node { node: node(n)? },
            LinkTarget::Heading { node: n } => LinkTarget::Heading { node: node(n)? },
            LinkTarget::Range { range } => LinkTarget::Range {
                range: match ids.ranges.get(range) {
                    Some(&new) => new,
                    None if same_namespace => *range,
                    None => return None,
                },
            },
        };
        Some(AnnotationData::Link(target))
    }

    /// The stored envelope.
    pub(crate) fn envelope(&self) -> Result<String, AnnotationError> {
        let mut value =
            serde_json::to_value(self).map_err(|e| AnnotationError::Store(e.to_string()))?;
        if let serde_json::Value::Object(map) = &mut value {
            map.insert("version".into(), 1.into());
        }
        serde_json::to_string(&value).map_err(|e| AnnotationError::Store(e.to_string()))
    }

    /// Reads a stored envelope: `Err` names why it can't be used.
    pub(crate) fn read(raw: &str) -> Result<AnnotationData, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Envelope {
            version: u32,
            kind: String,
            payload: serde_json::Value,
        }
        if raw.len() > MAX_ENVELOPE_BYTES {
            return Err("the envelope exceeds its byte limit".into());
        }
        let envelope: Envelope =
            serde_json::from_str(raw).map_err(|_| "the envelope is malformed".to_string())?;
        if envelope.version != 1 {
            return Err(format!("unsupported version {}", envelope.version));
        }
        let Some(kind) = Kind::parse(&envelope.kind) else {
            return Err("an unknown kind".to_string());
        };
        let data: AnnotationData = serde_json::from_value(serde_json::json!({
            "kind": kind.name(),
            "payload": envelope.payload,
        }))
        .map_err(|_| format!("a malformed {} payload", kind.name()))?;
        data.validate().map_err(|e| e.to_string())?;
        Ok(data)
    }
}

/// One paragraph's part of an annotation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Piece {
    pub node: NodeId,
    pub bytes: Range<usize>,
}

/// Where an annotation is now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum AnnotationState {
    /// Over text: one nonempty piece per paragraph, in flow order. `rebound`
    /// when an end's character was deleted and the end moved.
    Active { pieces: Vec<Piece>, rebound: bool },
    /// All of its text was deleted and its kind orphans (comments): a point,
    /// or no place at all when its block is gone.
    Orphaned { node: Option<NodeId>, at: usize },
    /// All of its text was deleted and its kind disappears (links). The
    /// record remains, so undo and concurrent payload edits still apply.
    Gone,
}

/// A readable live annotation, as of one revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Annotation {
    pub id: AnnotationId,
    pub data: AnnotationData,
    /// The peer that created it.
    pub author: u64,
    pub state: AnnotationState,
}

impl Annotation {
    pub fn kind(&self) -> Kind {
        self.data.kind()
    }

    pub fn is_orphaned(&self) -> bool {
        matches!(self.state, AnnotationState::Orphaned { .. })
    }
}

/// A bounded query result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AnnotationList {
    pub annotations: Vec<Annotation>,
    /// More matched than [`MAX_QUERY_RESULTS`].
    pub truncated: bool,
}

#[derive(Default)]
pub(crate) struct AnnotationCache {
    frontiers: Option<Frontiers>,
    index: Option<Arc<AnnotationIndex>>,
}

/// Every annotation of one revision, read in one pass (see the note).
#[derive(Default)]
pub(crate) struct AnnotationIndex {
    pub(crate) entries: BTreeMap<AnnotationId, Annotation>,
    /// Paragraph-local pieces and orphan points, sorted by `(start, id)`.
    pub(crate) by_node: BTreeMap<NodeId, Vec<(Range<usize>, AnnotationId)>>,
    /// Live records with an envelope, readable or not.
    pub(crate) records: usize,
    pub(crate) notes: Vec<Note>,
}

/// Every annotation of one revision, for readers that make many lookups
/// (layout asking for the links of each block on a page). Cheap to clone and
/// to get: the index is built once per revision and shared.
#[derive(Clone)]
pub struct AnnotationView(Arc<AnnotationIndex>);

/// An annotation's part in one paragraph. For an orphan the span is empty.
pub type NodePieces = [(Range<usize>, AnnotationId)];

impl AnnotationView {
    /// A readable live annotation.
    pub fn get(&self, id: AnnotationId) -> Option<&Annotation> {
        self.0.entries.get(&id)
    }

    /// The annotation parts in `node`, sorted by `(start, end, ID)`, and
    /// orphan points. Empty for a block with none.
    pub fn pieces(&self, node: NodeId) -> &NodePieces {
        self.0.by_node.get(&node).map_or(&[], Vec::as_slice)
    }

    /// Readable live annotations in ID order.
    pub fn iter(&self) -> impl Iterator<Item = &Annotation> {
        self.0.entries.values()
    }

    /// How many readable live annotations there are.
    pub fn len(&self) -> usize {
        self.0.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.entries.is_empty()
    }
}

impl Document {
    /// This revision's annotations (see [`AnnotationView`]).
    pub fn annotation_view(&self) -> AnnotationView {
        AnnotationView(self.annotation_index())
    }

    pub(crate) fn annotation_index(&self) -> Arc<AnnotationIndex> {
        if self.doc.get_pending_txn_len() > 0 {
            return Arc::new(self.build_annotation_index());
        }
        let frontiers = self.doc.state_frontiers();
        let mut cache = self.annotations.lock().unwrap_or_else(|e| e.into_inner());
        if cache.frontiers.as_ref() == Some(&frontiers)
            && let Some(index) = &cache.index
        {
            return index.clone();
        }
        let index = Arc::new(self.build_annotation_index());
        cache.frontiers = Some(frontiers);
        cache.index = Some(index.clone());
        index
    }

    fn build_annotation_index(&self) -> AnnotationIndex {
        let mut index = AnnotationIndex::default();
        if !self.has_ranges_root() {
            return index;
        }
        let tree = self.tree("ranges");
        let mut ids: Vec<_> = tree.nodes();
        ids.sort();
        let mut unreadable = 0usize;
        let mut hosts = BTreeMap::new();
        let mut limited = false;
        for id in ids {
            let Ok(meta) = tree.get_meta(id) else {
                continue;
            };
            if meta.get(KEY).is_none() || !self.live(&tree, id) {
                continue;
            }
            index.records += 1;
            if index.records > MAX_ANNOTATIONS {
                limited = true;
                continue;
            }
            let id = RangeId(id);
            let data = match get_str(&meta, KEY) {
                Some(raw) => AnnotationData::read(&raw),
                None => Err("the envelope is not text".into()),
            };
            let Ok(data) = data else {
                unreadable += 1;
                continue;
            };
            let state = self.annotation_state(&mut hosts, &meta, data.kind(), &mut limited);
            let points: Vec<(NodeId, Range<usize>)> = match &state {
                AnnotationState::Active { pieces, .. } => {
                    pieces.iter().map(|p| (p.node, p.bytes.clone())).collect()
                }
                AnnotationState::Orphaned { node: Some(n), at } => vec![(*n, *at..*at)],
                _ => Vec::new(),
            };
            for (node, bytes) in points {
                index.by_node.entry(node).or_default().push((bytes, id));
            }
            index.entries.insert(
                id,
                Annotation {
                    id,
                    data,
                    author: id.0.peer,
                    state,
                },
            );
        }
        for pieces in index.by_node.values_mut() {
            pieces.sort_by_key(|(bytes, id)| (bytes.start, bytes.end, *id));
        }
        if unreadable > 0 {
            index.notes.push(Note::warning(
                UNREADABLE,
                format!("{unreadable} unreadable annotations retained without application"),
            ));
        }
        if limited {
            index.notes.push(Note::warning(
                LIMIT,
                format!("annotations beyond {MAX_ANNOTATIONS} records or {MAX_PIECES} paragraphs are not applied"),
            ));
        }
        index
    }

    /// Where one stored anchor of a range made in `node` is now: its
    /// paragraph, the byte offset there, and whether its character is live.
    /// Equivalent to [`Document::locate`], but reads each host's text and
    /// flow once per index build rather than once per anchor.
    fn locate_indexed(
        &self,
        hosts: &mut BTreeMap<loro::TreeID, HostContext>,
        node: NodeId,
        anchor: &crate::text::Anchor,
    ) -> Option<(NodeId, usize, bool)> {
        use crate::text::Resolved;
        let host = hosts.entry(node.node).or_insert_with(|| {
            let flow = self.flow(node.node);
            let intervals = flow.as_ref().map_or_else(Vec::new, |f| {
                f.paras
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| f.is_live(p.id))
                    .map(|(i, p)| (p.region.end.1, i))
                    .collect()
            });
            HostContext {
                flow,
                intervals,
                text: self.host_text(node.node).map(crate::text::Text::from_loro),
                live: self.live(&self.tree("content"), node.node),
            }
        });
        let (Some(flow), Some(text)) = (&host.flow, &host.text) else {
            let (node, r) = self.locate(node, anchor)?;
            return Some((node, r.offset(), matches!(r, Resolved::Live(_))));
        };
        if !host.live {
            return None;
        }
        let resolved = text.resolve_shared(anchor).ok()?;
        let u = resolved.offset();
        // Lower bound on inclusive ends preserves the first paragraph at a
        // shared boundary, including zero-length/hidden paragraph regions.
        let i = interval_at(&host.intervals, u, &mut || {})?;
        let para = flow.paras.get(i)?;
        if u < para.region.start.1 {
            return None;
        }
        let byte = |u: usize| {
            text.loro().convert_pos(
                u,
                loro::cursor::PosType::Unicode,
                loro::cursor::PosType::Bytes,
            )
        };
        // The paragraph-local offset counts only its visible segments; a
        // position in a hidden gap belongs to the start of the next segment.
        let mut at = 0usize;
        for seg in &para.segs {
            if u < seg.chars.start {
                break;
            }
            if u <= seg.chars.end {
                at += byte(u)?.checked_sub(seg.bytes.start)?;
                return Some((para.id, at, matches!(resolved, Resolved::Live(_))));
            }
            at += seg.bytes.len();
        }
        Some((para.id, at, matches!(resolved, Resolved::Live(_))))
    }

    /// Resolves one annotation's range into paragraph pieces.
    fn annotation_state(
        &self,
        hosts: &mut BTreeMap<loro::TreeID, HostContext>,
        meta: &loro::LoroMap,
        kind: Kind,
        limited: &mut bool,
    ) -> AnnotationState {
        let empty = |node: Option<NodeId>, at: usize| match kind.spec().on_empty {
            OnEmpty::Orphan => AnnotationState::Orphaned { node, at },
            OnEmpty::Disappear => AnnotationState::Gone,
        };
        let anchor = |key| {
            crate::ranges::stored_anchor(meta.get(key))
                .and_then(|b| crate::text::Anchor::decode(&b))
        };
        let Some(node) = get_str(meta, "node").and_then(|s| NodeId::parse(&s)) else {
            return empty(None, 0);
        };
        let (Some(start), Some(end)) = (anchor("start"), anchor("end")) else {
            return empty(None, 0);
        };
        let (Some((start_node, start, live_start)), Some((end_node, end, live_end))) = (
            self.locate_indexed(hosts, node, &start),
            self.locate_indexed(
                hosts,
                get_str(meta, "end-node1")
                    .and_then(|s| NodeId::parse(&s))
                    .unwrap_or(node),
                &end,
            ),
        ) else {
            return empty(self.is_live(node).then_some(node), 0);
        };
        let crossed = self.range_node_cmp(start_node, end_node).is_gt()
            || (start_node == end_node && start > end);
        let (end_node, end) = if crossed {
            (start_node, start)
        } else {
            (end_node, end)
        };
        let mut pieces = Vec::new();
        if start_node == end_node {
            if start < end {
                pieces.push(Piece {
                    node: start_node,
                    bytes: start..end,
                });
            }
        } else if start_node.node != end_node.node {
            let order = self.document_order();
            let mut inside = false;
            for node in order {
                if node == start_node {
                    inside = true;
                }
                if !inside {
                    continue;
                }
                if pieces.len() >= MAX_PIECES {
                    *limited = true;
                    break;
                }
                if let Ok(block) = self.block(node) {
                    let a = if node == start_node { start } else { 0 };
                    let b = if node == end_node {
                        end
                    } else {
                        block.text.len()
                    };
                    if a < b {
                        pieces.push(Piece { node, bytes: a..b });
                    }
                }
                if node == end_node {
                    break;
                }
            }
        } else if let Some(flow) = hosts.get(&start_node.node).and_then(|h| h.flow.clone()) {
            let Some(first) = flow.index_of(start_node) else {
                return empty(Some(start_node), start);
            };
            for para in flow.paras.iter().skip(first) {
                if pieces.len() >= MAX_PIECES {
                    *limited = true;
                    break;
                }
                let len: usize = para.segs.iter().map(|s| s.bytes.len()).sum();
                let bytes = if para.id == start_node {
                    start..len
                } else if para.id == end_node {
                    0..end
                } else {
                    0..len
                };
                if !bytes.is_empty() {
                    pieces.push(Piece {
                        node: para.id,
                        bytes,
                    });
                }
                if para.id == end_node {
                    break;
                }
            }
        }
        if pieces.is_empty() {
            return empty(Some(start_node), start);
        }
        AnnotationState::Active {
            pieces,
            rebound: !(live_start && live_end),
        }
    }

    /// Notes about annotations this engine could not apply.
    pub fn annotation_notes(&self) -> Vec<Note> {
        self.annotation_index().notes.clone()
    }

    /// A live, readable annotation.
    pub fn annotation(&self, id: AnnotationId) -> Option<Annotation> {
        self.annotation_index().entries.get(&id).cloned()
    }

    /// Annotations over byte `at` of `node` (`start <= at < end`), and orphans
    /// at `at`, in `(start, ID)` order.
    pub fn annotations_at(&self, node: NodeId, at: usize) -> AnnotationList {
        self.annotations_where(node, |bytes| {
            (bytes.start <= at && at < bytes.end) || (bytes.is_empty() && bytes.start == at)
        })
    }

    /// Annotations with a piece overlapping `bytes` of `node`, and orphans in
    /// it, in `(start, ID)` order. An empty `bytes` is a position.
    pub fn annotations_in(&self, node: NodeId, bytes: Range<usize>) -> AnnotationList {
        if bytes.is_empty() {
            return self.annotations_at(node, bytes.start);
        }
        self.annotations_where(node, |piece| {
            if piece.is_empty() {
                bytes.start <= piece.start && piece.start <= bytes.end
            } else {
                piece.start < bytes.end && bytes.start < piece.end
            }
        })
    }

    fn annotations_where(
        &self,
        node: NodeId,
        matches: impl Fn(&Range<usize>) -> bool,
    ) -> AnnotationList {
        let index = self.annotation_index();
        let mut out = AnnotationList::default();
        for (bytes, id) in index.by_node.get(&node).into_iter().flatten() {
            if !matches(bytes) {
                continue;
            }
            if out.annotations.len() >= MAX_QUERY_RESULTS {
                out.truncated = true;
                break;
            }
            if let Some(a) = index.entries.get(id) {
                out.annotations.push(a.clone());
            }
        }
        out
    }

    /// All annotations of a kind that are not gone, in ID order.
    pub fn annotations_of_kind(&self, kind: Kind) -> AnnotationList {
        let index = self.annotation_index();
        let mut out = AnnotationList::default();
        for a in index.entries.values() {
            if a.kind() != kind || a.state == AnnotationState::Gone {
                continue;
            }
            if out.annotations.len() >= MAX_QUERY_RESULTS {
                out.truncated = true;
                break;
            }
            out.annotations.push(a.clone());
        }
        out
    }

    /// Whether any range carries an annotation or a restore hint, including
    /// tombstoned and unreadable ones: packages must not claim compatibility
    /// with readers that would show them as bare ranges.
    pub fn has_annotations(&self) -> bool {
        if !self.has_ranges_root() {
            return false;
        }
        let tree = self.tree("ranges");
        tree.nodes().into_iter().any(|id| {
            tree.get_meta(id).is_ok_and(|m| {
                m.get(KEY).is_some() || m.get(crate::annotation_restore::HINTS).is_some()
            })
        })
    }

    /// Live annotation records, readable or not, for capacity checks.
    pub fn annotation_count(&self) -> usize {
        self.annotation_index().records
    }

    /// Checks data that names this document: an internal target must be live.
    fn check_target(&self, data: &AnnotationData) -> Result<(), AnnotationError> {
        data.validate()?;
        match data {
            AnnotationData::Link(LinkTarget::Node { node } | LinkTarget::Heading { node }) => {
                if !self.is_live(*node) {
                    return Err(AnnotationError::Payload(format!(
                        "link target {node} is not a live block"
                    )));
                }
            }
            AnnotationData::Link(LinkTarget::Range { range }) => {
                if matches!(
                    self.resolve_range(*range),
                    crate::RangeState::Missing { .. }
                ) {
                    return Err(AnnotationError::Payload(format!(
                        "link target {range} is not a live range"
                    )));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Checks an annotation could be added over `bytes` of `node`, without
    /// writing anything. `pending` counts additions not yet written.
    pub fn check_annotation(
        &self,
        node: NodeId,
        bytes: Range<usize>,
        data: &AnnotationData,
        pending: usize,
    ) -> Result<(), AnnotationError> {
        if bytes.is_empty() {
            return Err(AnnotationError::Empty);
        }
        self.block(node)?.text.slice(bytes)?;
        self.check_annotation_data(data, pending)
    }

    /// The span-independent checks: payload, internal targets and capacity.
    /// `pending` counts additions not yet written.
    pub fn check_annotation_data(
        &self,
        data: &AnnotationData,
        pending: usize,
    ) -> Result<(), AnnotationError> {
        self.check_target(data)?;
        if self.annotation_count().saturating_add(pending) >= MAX_ANNOTATIONS {
            return Err(AnnotationError::Limit);
        }
        Ok(())
    }

    /// Checks an annotation can be removed: a live record, readable or not.
    pub fn check_annotation_removal(&self, id: AnnotationId) -> Result<(), AnnotationError> {
        self.annotation_record(id).map(|_| ())
    }

    /// Adds an annotation over `bytes` of `node`, with its kind's policy.
    /// Undoable; it does not end the editing step. The range and its
    /// envelope are staged, so undo and redo keep its ID and payload.
    pub fn add_annotation(
        &self,
        node: NodeId,
        bytes: Range<usize>,
        data: &AnnotationData,
    ) -> Result<AnnotationId, AnnotationError> {
        self.check_annotation(node, bytes.clone(), data, 0)?;
        self.write_annotation(node, bytes, data)
    }

    /// [`Document::add_annotation`] for a caller that has already checked
    /// capacity with [`Document::check_annotation_data`] (the editing kernel
    /// plans a whole transaction first). It still checks the span and the
    /// target, which are cheap, but not the annotation count: counting means
    /// reading the index, which a transaction in progress forces to be rebuilt
    /// for every addition.
    pub fn add_annotation_prechecked(
        &self,
        node: NodeId,
        bytes: Range<usize>,
        data: &AnnotationData,
    ) -> Result<AnnotationId, AnnotationError> {
        if bytes.is_empty() {
            return Err(AnnotationError::Empty);
        }
        self.block(node)?.text.slice(bytes.clone())?;
        self.check_target(data)?;
        self.write_annotation(node, bytes, data)
    }

    fn write_annotation(
        &self,
        node: NodeId,
        bytes: Range<usize>,
        data: &AnnotationData,
    ) -> Result<AnnotationId, AnnotationError> {
        let raw = data.envelope()?;
        let id = self.stage_fragment_range(node, bytes, data.kind().spec().policy)?;
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        self.tree("ranges").get_meta(id.0)?.insert(KEY, raw)?;
        self.doc.commit();
        self.activate_fragment_range(id)?;
        Ok(id)
    }

    /// Writes the data of a staged range (a paste), outside the undo history
    /// like the range itself. The range stays inactive until its paste
    /// activates it.
    pub fn stage_annotation_data(
        &self,
        id: AnnotationId,
        data: &AnnotationData,
    ) -> Result<(), AnnotationError> {
        data.validate()?;
        let raw = data.envelope()?;
        let tree = self.tree("ranges");
        if !tree.contains(id.0) {
            return Err(AnnotationError::Missing(id));
        }
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        tree.get_meta(id.0)?.insert(KEY, raw)?;
        self.doc.commit();
        Ok(())
    }

    /// The live record of an annotation, readable or not.
    fn annotation_record(&self, id: AnnotationId) -> Result<loro::LoroMap, AnnotationError> {
        let tree = self.tree("ranges");
        if !self.has_ranges_root() || !self.live(&tree, id.0) {
            return Err(AnnotationError::Missing(id));
        }
        let meta = tree.get_meta(id.0)?;
        if meta.get(KEY).is_none() {
            return Err(AnnotationError::Missing(id));
        }
        Ok(meta)
    }

    /// Checks a payload update without writing it.
    pub fn check_annotation_update(
        &self,
        id: AnnotationId,
        data: &AnnotationData,
    ) -> Result<(), AnnotationError> {
        let meta = self.annotation_record(id)?;
        let current = get_str(&meta, KEY)
            .ok_or_else(|| AnnotationError::Payload("the stored annotation is unreadable".into()))
            .and_then(|raw| {
                AnnotationData::read(&raw).map_err(|why| {
                    AnnotationError::Payload(format!("the stored annotation is unreadable: {why}"))
                })
            })?;
        if current.kind() != data.kind() {
            return Err(AnnotationError::Payload(
                "an annotation's kind cannot change".into(),
            ));
        }
        self.check_target(data)
    }

    /// Replaces an annotation's payload. Undoable; does not commit. An
    /// unreadable record (a newer engine's) is never overwritten.
    pub fn update_annotation(
        &self,
        id: AnnotationId,
        data: &AnnotationData,
    ) -> Result<(), AnnotationError> {
        self.check_annotation_update(id, data)?;
        let raw = data.envelope()?;
        self.annotation_record(id)?.insert(KEY, raw)?;
        Ok(())
    }

    /// Removes an annotation, readable or not. Undoable; does not commit.
    /// The ID is never reused (07).
    pub fn remove_annotation(&self, id: AnnotationId) -> Result<(), AnnotationError> {
        self.annotation_record(id)?.insert("deleted", true)?;
        Ok(())
    }

    /// Annotations wholly inside `bytes` of `source`, with slice-local
    /// offsets, before a copying edit moves that text (a split by copy, a
    /// cross-host join, a move by copy). See [`Document::move_annotations`].
    pub(crate) fn annotations_to_move(
        &self,
        source: NodeId,
        bytes: Range<usize>,
    ) -> Vec<(AnnotationId, Range<usize>, RangePolicy)> {
        let index = self.annotation_index();
        let mut out = Vec::new();
        for (piece, id) in index.by_node.get(&source).into_iter().flatten() {
            let Some(a) = index.entries.get(id) else {
                continue;
            };
            let AnnotationState::Active { pieces, .. } = &a.state else {
                continue;
            };
            if pieces.len() != 1 || piece.start < bytes.start || piece.end > bytes.end {
                continue;
            }
            if let Ok(Some(policy)) = self.range_policy(*id) {
                out.push((
                    *id,
                    piece.start - bytes.start..piece.end - bytes.start,
                    policy,
                ));
            }
        }
        out
    }

    /// Re-anchors annotations collected by `annotations_to_move` onto the
    /// copied text at `at` of `target`, keeping their IDs. Undoable.
    pub(crate) fn move_annotations(
        &self,
        target: NodeId,
        at: usize,
        moves: Vec<(AnnotationId, Range<usize>, RangePolicy)>,
    ) -> Result<(), DocError> {
        for (id, bytes, policy) in moves {
            self.reanchor_fragment_range(
                id,
                target,
                at.saturating_add(bytes.start)..at.saturating_add(bytes.end),
                policy,
            )?;
        }
        Ok(())
    }
}

/// One host's text and flow, read once per index build.
pub(crate) struct HostContext {
    flow: Option<Arc<crate::flow::HostFlow>>,
    /// Inclusive scalar end and paragraph index, in flow order.
    intervals: Vec<(usize, usize)>,
    text: Option<crate::text::Text>,
    live: bool,
}

/// Ordered endpoint lookup. `charge` makes the comparison bound testable without
/// depending on scheduler, build mode or fixture-construction time.
fn interval_at(intervals: &[(usize, usize)], u: usize, charge: &mut impl FnMut()) -> Option<usize> {
    let at = intervals.partition_point(|(end, _)| {
        charge();
        *end < u
    });
    intervals.get(at).map(|(_, i)| *i)
}

#[cfg(test)]
#[test]
fn interval_lookup_scales_logarithmically_and_keeps_boundary_order() {
    for paragraphs in [1usize, 100, 1000, 4096] {
        let intervals: Vec<_> = (0..paragraphs).map(|i| (i * 2 + 2, i)).collect();
        let mut work = 0usize;
        for _ in 0..500 {
            assert_eq!(
                interval_at(&intervals, paragraphs * 2 - 1, &mut || work += 1),
                Some(paragraphs - 1)
            );
        }
        assert!(work <= 500 * (paragraphs.ilog2() as usize + 2));
    }
    let intervals = [(2, 0), (2, 1), (4, 2)];
    assert_eq!(interval_at(&intervals, 2, &mut || {}), Some(0));
    assert_eq!(interval_at(&intervals, 3, &mut || {}), Some(2));
}
