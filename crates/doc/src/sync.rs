//! Sync packets, format 2 (29, 34): delta updates since a version vector, and
//! a full snapshot for the first join, in a versioned frame that names its
//! features. See `docs/collaboration.md` for the protocol and trust model.
//!
//! A remote packet is checked for shape and cost before the store sees it,
//! never for meaning: whatever the CRDT can represent is accepted, and
//! readers normalise it deterministically (37).
//!
//! Deltas carry Loro's JSON change schema, not its binary update blocks. The
//! binary decoder sizes vectors from untrusted counts before checking them,
//! so a few bytes could ask for gigabytes; JSON expands at most linearly and
//! every count is checked here first.

use std::collections::{BTreeMap, BTreeSet};

use loro::{
    Container, ContainerID, ContainerType, JsonOpContent, JsonSchema, JsonTextOp, JsonTreeOp,
    TreeID, VersionVector,
};
use reprise_diag::{Code, Note, Severity};

use crate::changes::ChangeReport;
use crate::persist::{MAX_PERSIST_OPS, validate_snapshot_mode};
use crate::{DocError, Document};

/// The packet format this engine writes and reads. Format 1 is the facade's
/// self-contained `SyncUpdate` snapshot, which stays readable there.
pub const SYNC_FORMAT: u16 = 2;
/// Largest packet body.
pub const MAX_PACKET_BYTES: usize = 32 * 1024 * 1024;
/// Most changes in one delta.
pub const MAX_PACKET_CHANGES: usize = 1_000_000;
/// Longest single operation, in atoms (characters, list items, deletions).
pub const MAX_PACKET_OP_LEN: u64 = 16 * 1024 * 1024;
/// Most entries in a version vector.
pub const MAX_VECTOR_ENTRIES: usize = 4096;
/// Counters and Lamport timestamps stay below this, so that later local
/// commits can never overflow them.
pub const MAX_PACKET_COUNTER: i64 = 1 << 30;

const MAGIC: &[u8; 4] = b"RSYN";
const RESERVED_PEER: u64 = u64::MAX;

/// A sorted version vector: `(peer, counter)` with distinct peers.
pub type Vector = Vec<(u64, i32)>;

/// Feature bits a packet requires its receiver to understand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Features(pub u64);

impl Features {
    /// The delta body is Loro's JSON change schema, version 1.
    pub const DELTA_JSON: Features = Features(1);
    /// The snapshot body is a Loro 1.16 history snapshot.
    pub const SNAPSHOT_LORO_1_16: Features = Features(1 << 1);
    /// Everything this engine reads.
    pub const SUPPORTED: Features = Features(1 | (1 << 1));

    const NAMES: [(Features, &'static str); 2] = [
        (Features::DELTA_JSON, "delta-json"),
        (Features::SNAPSHOT_LORO_1_16, "snapshot-loro-1.16"),
    ];

    /// The names of the known bits that are set, in bit order. Unknown bits
    /// are written as `bit-N`.
    pub fn names(self) -> Vec<String> {
        (0..64)
            .filter(|bit| self.0 & (1 << bit) != 0)
            .map(|bit| {
                Features::NAMES
                    .iter()
                    .find(|(f, _)| f.0 == 1 << bit)
                    .map_or_else(|| format!("bit-{bit}"), |(_, n)| (*n).to_owned())
            })
            .collect()
    }

    /// The bits named, or the first name that is unknown.
    pub fn from_names(names: &[String]) -> Result<Features, String> {
        let mut bits = 0;
        for name in names {
            let known = Features::NAMES.iter().find(|(_, n)| n == name);
            let bit = match known {
                Some((f, _)) => f.0,
                None => name
                    .strip_prefix("bit-")
                    .and_then(|b| b.parse::<u32>().ok())
                    .filter(|b| *b < 64 && b.to_string() == name[4..])
                    .map(|b| 1_u64 << b)
                    .ok_or_else(|| name.clone())?,
            };
            bits |= bit;
        }
        Ok(Features(bits))
    }
}

/// What a packet's body is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PacketKind {
    /// Everything the sender has beyond `since`.
    Delta,
    /// The sender's whole history, for a first join.
    Snapshot,
}

/// A packet's frame, readable without decoding its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacketHeader {
    pub format: u16,
    pub features: Features,
    pub kind: PacketKind,
    /// What the sender assumed the receiver had, clamped to `until`.
    pub since: Vector,
    /// The sender's version vector when it exported the packet.
    pub until: Vector,
}

/// Why a packet was refused. Nothing from a refused packet is applied.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    /// The packet uses a format this engine doesn't read.
    #[error("unsupported sync packet format {0}")]
    Format(u16),
    /// The packet needs features this engine doesn't have.
    #[error("unsupported sync packet features {0:#x}")]
    Feature(u64),
    /// The packet is malformed, or its frame misdescribes its content.
    #[error("invalid sync packet: {0}")]
    Invalid(String),
    /// The packet exceeds a bound.
    #[error("sync packet limit exceeded: {0}")]
    Limit(&'static str),
    /// The packet depends on operations this replica lacks. Ask the sender
    /// for a delta since `have`.
    #[error("sync packet depends on missing operations")]
    Missing { have: Vector },
    /// The packet carries operations under this replica's own peer that it
    /// doesn't have: a peer is impersonating this replica.
    #[error("sync packet carries unseen operations under the receiver's own peer")]
    LocalPeer,
    /// The store refused a packet that passed every check.
    #[error("the document store refused the packet: {0}")]
    Store(String),
}

pub mod codes {
    //! Sync refusal codes. All are `Error`: the packet was left out.
    use reprise_diag::Code;
    pub const FORMAT: Code = Code::new("sync.format");
    pub const FEATURE: Code = Code::new("sync.feature");
    pub const INVALID: Code = Code::new("sync.invalid");
    pub const LIMIT: Code = Code::new("sync.limit");
    pub const MISSING: Code = Code::new("sync.missing");
    pub const LOCAL_PEER: Code = Code::new("sync.local-peer");
    pub const STORE: Code = Code::new("sync.store");
}

impl SyncError {
    pub fn code(&self) -> Code {
        match self {
            SyncError::Format(_) => codes::FORMAT,
            SyncError::Feature(_) => codes::FEATURE,
            SyncError::Invalid(_) => codes::INVALID,
            SyncError::Limit(_) => codes::LIMIT,
            SyncError::Missing { .. } => codes::MISSING,
            SyncError::LocalPeer => codes::LOCAL_PEER,
            SyncError::Store(_) => codes::STORE,
        }
    }

    pub fn note(&self) -> Note {
        Note::new(Severity::Error, self.code(), self.to_string())
    }
}

fn invalid(why: &str) -> SyncError {
    SyncError::Invalid(why.to_owned())
}

/// Checks a vector's shape: sorted distinct peers, non-negative counters,
/// bounded length, no reserved peer.
pub fn check_vector(vector: &[(u64, i32)]) -> Result<(), SyncError> {
    if vector.len() > MAX_VECTOR_ENTRIES {
        return Err(SyncError::Limit("version vector entries"));
    }
    for pair in vector.windows(2) {
        if let [a, b] = pair
            && a.0 >= b.0
        {
            return Err(invalid("vector peers are not sorted and distinct"));
        }
    }
    for &(peer, counter) in vector {
        if peer == RESERVED_PEER {
            return Err(invalid("vector uses the reserved peer"));
        }
        if counter < 0 || i64::from(counter) > MAX_PACKET_COUNTER {
            return Err(invalid("vector counter out of range"));
        }
    }
    Ok(())
}

fn to_vv(vector: &[(u64, i32)]) -> VersionVector {
    let mut vv = VersionVector::new();
    for &(peer, counter) in vector {
        vv.insert(peer, counter);
    }
    vv
}

fn lookup(vector: &[(u64, i32)], peer: u64) -> i32 {
    vector
        .binary_search_by_key(&peer, |&(p, _)| p)
        .ok()
        .and_then(|i| vector.get(i))
        .map_or(0, |&(_, c)| c)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SyncError> {
        let head = self.0.get(..n).ok_or_else(|| invalid("truncated frame"))?;
        self.0 = self.0.get(n..).unwrap_or_default();
        Ok(head)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], SyncError> {
        self.take(N)?
            .try_into()
            .map_err(|_| invalid("truncated frame"))
    }
    fn vector(&mut self) -> Result<Vector, SyncError> {
        let n = usize::from(u16::from_le_bytes(self.array()?));
        if n > MAX_VECTOR_ENTRIES {
            return Err(SyncError::Limit("version vector entries"));
        }
        let mut vector = Vec::with_capacity(n);
        for _ in 0..n {
            let peer = u64::from_le_bytes(self.array()?);
            let counter = i32::from_le_bytes(self.array()?);
            vector.push((peer, counter));
        }
        check_vector(&vector)?;
        Ok(vector)
    }
}

fn write_vector(out: &mut Vec<u8>, vector: &[(u64, i32)]) -> Result<(), SyncError> {
    let n = u16::try_from(vector.len()).map_err(|_| SyncError::Limit("version vector entries"))?;
    out.extend_from_slice(&n.to_le_bytes());
    for &(peer, counter) in vector {
        out.extend_from_slice(&peer.to_le_bytes());
        out.extend_from_slice(&counter.to_le_bytes());
    }
    Ok(())
}

/// Reads a packet's frame and returns it with the body. The format and
/// features are checked before anything else, so a newer packet is refused
/// with [`SyncError::Format`] or [`SyncError::Feature`], never misread.
pub fn read_header(bytes: &[u8]) -> Result<(PacketHeader, &[u8]), SyncError> {
    let mut r = Reader(bytes);
    if r.take(4)? != MAGIC {
        return Err(invalid("not a sync packet"));
    }
    let format = u16::from_le_bytes(r.array()?);
    if format != SYNC_FORMAT {
        return Err(SyncError::Format(format));
    }
    let features = Features(u64::from_le_bytes(r.array()?));
    let unknown = features.0 & !Features::SUPPORTED.0;
    if unknown != 0 {
        return Err(SyncError::Feature(unknown));
    }
    let [kind] = r.array()?;
    let kind = match kind {
        0 => PacketKind::Delta,
        1 => PacketKind::Snapshot,
        _ => return Err(invalid("unknown packet kind")),
    };
    let needed = match kind {
        PacketKind::Delta => Features::DELTA_JSON,
        PacketKind::Snapshot => Features::SNAPSHOT_LORO_1_16,
    };
    if features != needed {
        return Err(invalid("packet features don't match its kind"));
    }
    let since = r.vector()?;
    let until = r.vector()?;
    let len = u32::from_le_bytes(r.array()?);
    let len = usize::try_from(len).map_err(|_| SyncError::Limit("packet bytes"))?;
    if len > MAX_PACKET_BYTES {
        return Err(SyncError::Limit("packet bytes"));
    }
    let body = r.take(len)?;
    if !r.0.is_empty() {
        return Err(invalid("trailing bytes after the packet body"));
    }
    if kind == PacketKind::Snapshot && !since.is_empty() {
        return Err(invalid("a snapshot packet has no since vector"));
    }
    for &(peer, counter) in &since {
        if counter > lookup(&until, peer) {
            return Err(invalid("since is not within until"));
        }
    }
    let header = PacketHeader {
        format,
        features,
        kind,
        since,
        until,
    };
    Ok((header, body))
}

fn write_packet(header: &PacketHeader, body: &[u8]) -> Result<Vec<u8>, SyncError> {
    if body.len() > MAX_PACKET_BYTES {
        return Err(SyncError::Limit("packet bytes"));
    }
    let mut out = Vec::with_capacity(body.len().saturating_add(64));
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&header.format.to_le_bytes());
    out.extend_from_slice(&header.features.0.to_le_bytes());
    out.push(match header.kind {
        PacketKind::Delta => 0,
        PacketKind::Snapshot => 1,
    });
    write_vector(&mut out, &header.since)?;
    write_vector(&mut out, &header.until)?;
    let len = u32::try_from(body.len()).map_err(|_| SyncError::Limit("packet bytes"))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(body);
    Ok(out)
}

/// One change's span, as the preflight sees it.
struct Span {
    peer: u64,
    start: i64,
    end: i64,
}

impl Document {
    fn peer(&self) -> u64 {
        self.doc.peer_id()
    }

    /// A delta packet: everything this replica has that `since` lacks.
    /// `since` is usually the receiver's [`Document::version_vector`].
    pub fn export_delta(&self, since: &[(u64, i32)]) -> Result<Vec<u8>, SyncError> {
        check_vector(since)?;
        let until = self.version_vector();
        check_vector(&until)?;
        let clamped: Vector = until
            .iter()
            .map(|&(peer, counter)| (peer, lookup(since, peer).min(counter)))
            .filter(|&(_, counter)| counter > 0)
            .collect();
        let json = self
            .doc
            .export_json_updates(&to_vv(&clamped), &to_vv(&until));
        let body = serde_json::to_vec(&json).map_err(|e| SyncError::Store(e.to_string()))?;
        let header = PacketHeader {
            format: SYNC_FORMAT,
            features: Features::DELTA_JSON,
            kind: PacketKind::Delta,
            since: clamped,
            until,
        };
        write_packet(&header, &body)
    }

    /// A snapshot packet: this replica's whole history, for a first join.
    pub fn export_snapshot_packet(&self) -> Result<Vec<u8>, SyncError> {
        let body = self
            .try_export(crate::PersistenceMode::History)
            .map_err(|e| SyncError::Store(e.to_string()))?;
        let until = self.version_vector();
        check_vector(&until)?;
        let header = PacketHeader {
            format: SYNC_FORMAT,
            features: Features::SNAPSHOT_LORO_1_16,
            kind: PacketKind::Snapshot,
            since: Vec::new(),
            until,
        };
        write_packet(&header, &body)
    }

    /// Imports a packet from another replica and reports what changed. A
    /// refused packet changes nothing. Importing a packet again changes
    /// nothing and reports nothing.
    pub fn import_packet(&self, bytes: &[u8]) -> Result<ChangeReport, SyncError> {
        let (header, body) = read_header(bytes)?;
        self.commit();
        match header.kind {
            PacketKind::Delta => self.import_delta(&header, body),
            PacketKind::Snapshot => self.import_snapshot(&header, body),
        }
    }

    fn import_delta(&self, header: &PacketHeader, body: &[u8]) -> Result<ChangeReport, SyncError> {
        let json: JsonSchema =
            serde_json::from_slice(body).map_err(|e| SyncError::Invalid(e.to_string()))?;
        self.preflight_delta(header, &json)?;
        let (status, report) = self.tracked(|| self.doc.import_json_updates(json));
        let status = status.map_err(|e| SyncError::Store(e.to_string()))?;
        if status.pending.is_some() {
            return Err(SyncError::Missing {
                have: self.version_vector(),
            });
        }
        Ok(report)
    }

    fn import_snapshot(
        &self,
        header: &PacketHeader,
        body: &[u8],
    ) -> Result<ChangeReport, SyncError> {
        let end = validate_snapshot_mode(body, true).map_err(|e| match e {
            DocError::Store(why) => SyncError::Invalid(why),
            other => SyncError::Invalid(other.to_string()),
        })?;
        if end != header.until {
            return Err(invalid("until does not describe the snapshot"));
        }
        let local = self.version_vector();
        let own = self.peer();
        if lookup(&header.until, own) > lookup(&local, own) {
            return Err(SyncError::LocalPeer);
        }
        let (status, report) = self.tracked(|| self.doc.import(body));
        let status = status.map_err(|e| SyncError::Store(e.to_string()))?;
        if status.pending.is_some() {
            return Err(SyncError::Missing {
                have: self.version_vector(),
            });
        }
        Ok(report)
    }

    /// Every check on a delta before the store sees it. See
    /// `docs/collaboration.md`, "Delta preflight".
    fn preflight_delta(&self, header: &PacketHeader, json: &JsonSchema) -> Result<(), SyncError> {
        if json.schema_version != 1 {
            return Err(invalid("unsupported JSON change schema"));
        }
        if json.changes.len() > MAX_PACKET_CHANGES {
            return Err(SyncError::Limit("changes per packet"));
        }
        if json
            .peers
            .as_ref()
            .is_some_and(|p| p.len() > MAX_VECTOR_ENTRIES)
        {
            return Err(SyncError::Limit("peers per packet"));
        }
        let peer_of = |p: u64| -> Result<u64, SyncError> {
            let peer = match &json.peers {
                Some(table) => usize::try_from(p)
                    .ok()
                    .and_then(|i| table.get(i))
                    .copied()
                    .ok_or_else(|| invalid("peer index out of range"))?,
                None => p,
            };
            if peer == RESERVED_PEER {
                return Err(invalid("change uses the reserved peer"));
            }
            Ok(peer)
        };
        let mut spans = Vec::with_capacity(json.changes.len());
        let mut deps = Vec::new();
        let mut atoms = 0_u64;
        for change in &json.changes {
            let peer = peer_of(change.id.peer)?;
            let start = i64::from(change.id.counter);
            if start < 0 {
                return Err(invalid("negative change counter"));
            }
            if change.ops.is_empty() {
                return Err(invalid("change without operations"));
            }
            let mut end = start;
            for op in &change.ops {
                if i64::from(op.counter) != end {
                    return Err(invalid("operation counters are not contiguous"));
                }
                check_op(&op.content)?;
                let len = op_len(&op.content);
                if len == 0 || len > MAX_PACKET_OP_LEN {
                    return Err(SyncError::Limit("operation length"));
                }
                atoms = atoms.saturating_add(len);
                end = end.saturating_add(i64::try_from(len).unwrap_or(i64::MAX));
                if end > MAX_PACKET_COUNTER {
                    return Err(SyncError::Limit("operation counters"));
                }
            }
            let lamport_end = i64::from(change.lamport).saturating_add(end - start);
            if lamport_end > MAX_PACKET_COUNTER {
                return Err(SyncError::Limit("Lamport timestamps"));
            }
            for dep in &change.deps {
                if dep.counter < 0 {
                    return Err(invalid("negative dependency counter"));
                }
                deps.push((peer_of(dep.peer)?, i64::from(dep.counter)));
            }
            spans.push(Span { peer, start, end });
        }
        if atoms > MAX_PERSIST_OPS {
            return Err(SyncError::Limit("operations per packet"));
        }
        // The content must cover exactly [since, until) for every peer.
        let mut by_peer: BTreeMap<u64, Vec<(i64, i64)>> = BTreeMap::new();
        for s in &spans {
            by_peer.entry(s.peer).or_default().push((s.start, s.end));
        }
        for (peer, list) in &mut by_peer {
            list.sort_unstable();
            let mut at = i64::from(lookup(&header.since, *peer));
            for &(start, end) in list.iter() {
                if start != at {
                    return Err(invalid("changes do not match the packet's vectors"));
                }
                at = end;
            }
            if at != i64::from(lookup(&header.until, *peer)) {
                return Err(invalid("changes do not match the packet's vectors"));
            }
        }
        for &(peer, counter) in &header.until {
            if counter > lookup(&header.since, peer) && !by_peer.contains_key(&peer) {
                return Err(invalid("changes do not match the packet's vectors"));
            }
        }
        // Causal completeness against this replica.
        let local = self.version_vector();
        let own = self.peer();
        for peer in by_peer.keys() {
            if lookup(&header.since, *peer) > lookup(&local, *peer) {
                return Err(SyncError::Missing { have: local });
            }
            if *peer == own && lookup(&header.until, own) > lookup(&local, own) {
                return Err(SyncError::LocalPeer);
            }
        }
        for (peer, counter) in deps {
            let mut known = i64::from(lookup(&local, peer));
            if by_peer.contains_key(&peer) {
                known = known.max(i64::from(lookup(&header.until, peer)));
            }
            if counter >= known {
                return Err(SyncError::Missing { have: local });
            }
        }
        self.preflight_trees(json, &peer_of)?;
        self.preflight_texts(json, &peer_of)
    }

    /// Tree operations must name nodes that exist, in this replica or created
    /// by the packet itself. Loro's tree state unwraps a move's target, so a
    /// move of an unknown node would abort the process.
    fn preflight_trees(
        &self,
        json: &JsonSchema,
        peer_of: &dyn Fn(u64) -> Result<u64, SyncError>,
    ) -> Result<(), SyncError> {
        let container = |id: &ContainerID| -> Result<ContainerID, SyncError> {
            Ok(match id {
                ContainerID::Normal {
                    peer,
                    counter,
                    container_type,
                } => ContainerID::Normal {
                    peer: peer_of(*peer)?,
                    counter: *counter,
                    container_type: *container_type,
                },
                root => root.clone(),
            })
        };
        let node = |id: &TreeID| -> Result<TreeID, SyncError> {
            Ok(TreeID {
                peer: peer_of(id.peer)?,
                counter: id.counter,
            })
        };
        let mut created = BTreeSet::new();
        for change in &json.changes {
            for op in &change.ops {
                if let JsonOpContent::Tree(JsonTreeOp::Create { target, .. }) = &op.content {
                    created.insert((container(&op.container)?.to_string(), node(target)?));
                }
            }
        }
        for change in &json.changes {
            for op in &change.ops {
                let JsonOpContent::Tree(tree_op) = &op.content else {
                    continue;
                };
                let cid = container(&op.container)?;
                if cid.container_type() != ContainerType::Tree {
                    return Err(invalid("tree operation on a container that is not a tree"));
                }
                let key = cid.to_string();
                let exists = |id: &TreeID| -> Result<bool, SyncError> {
                    let id = node(id)?;
                    Ok(created.contains(&(key.clone(), id)) || self.tree_has(&cid, id))
                };
                let (target, parent) = match tree_op {
                    JsonTreeOp::Create { parent, .. } => (None, parent.as_ref()),
                    JsonTreeOp::Move { target, parent, .. } => (Some(target), parent.as_ref()),
                    JsonTreeOp::Delete { target } => (Some(target), None),
                };
                for id in target.into_iter().chain(parent) {
                    if !exists(id)? {
                        return Err(invalid("tree operation names a node that does not exist"));
                    }
                }
            }
        }
        Ok(())
    }

    fn tree_has(&self, cid: &ContainerID, id: TreeID) -> bool {
        let tree = match cid {
            ContainerID::Root { name, .. } => self.doc.get_tree(name.as_str()),
            normal => match self.doc.get_container(normal.clone()) {
                Some(Container::Tree(tree)) => tree,
                _ => return false,
            },
        };
        tree.is_node_deleted(&id).is_ok()
    }
}

/// The operation vocabulary of `delta-json`: what the engine itself writes.
/// Map writes, text insertions and deletions, and tree operations. Loro's
/// text marks, lists, movable lists and counters are never written by the
/// engine, and its state code unwraps on inconsistent marks, so they are
/// refused; an engine that needs them must declare a new feature bit.
///
/// Every position and length is in `0..2^30`: Loro converts some of them
/// to `i32`, where `u32::MAX` would turn negative.
fn check_op(content: &JsonOpContent) -> Result<(), SyncError> {
    let ok = |n: i64| (0..MAX_PACKET_COUNTER).contains(&n);
    let in_range = match content {
        JsonOpContent::Text(JsonTextOp::Insert { pos, .. }) => ok(i64::from(*pos)),
        JsonOpContent::Text(JsonTextOp::Delete { pos, len, .. }) => {
            let (pos, len) = (i64::from(*pos), i64::from(*len));
            ok(pos) && len != 0 && ok(len.abs()) && ok(pos + len.abs())
        }
        JsonOpContent::Map(_) | JsonOpContent::Tree(_) => true,
        JsonOpContent::Text(JsonTextOp::Mark { .. } | JsonTextOp::MarkEnd)
        | JsonOpContent::List(_)
        | JsonOpContent::MovableList(_)
        | JsonOpContent::Future(_) => {
            return Err(invalid("operation kind is not part of delta-json"));
        }
    };
    if in_range {
        Ok(())
    } else {
        Err(SyncError::Limit("operation positions"))
    }
}

/// An operation's length in atoms, without trusting a sign.
fn op_len(content: &JsonOpContent) -> u64 {
    u64::try_from(content.op_len()).unwrap_or(u64::MAX)
}

#[cfg(test)]
pub(crate) mod tests_support {
    pub(crate) fn write(header: &super::PacketHeader, body: &[u8]) -> Vec<u8> {
        super::write_packet(header, body).unwrap()
    }
}
