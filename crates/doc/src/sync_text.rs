//! Text operation preflight for deltas: every text position must be inside
//! the text as it was at the operation's causal version.
//!
//! Loro 1.16 asserts or unwraps when a text insertion or deletion points past
//! the end of its text, in both its linear and its tracker import paths, so a
//! hostile packet could abort the process. Checking needs the text's length
//! at each change's dependencies:
//!
//! -   Changes are processed in Lamport order, which real peers make causal;
//!     a change whose dependencies come later is refused.
//! -   Along a linear chain (each change depends exactly on the state before
//!     it) lengths are simulated from the live state, with no copying. That is
//!     the common case: a peer typing on top of what this replica has.
//! -   A change with other dependencies is checked against a **shadow**
//!     replica checked out at them. Checking out the live document would
//!     clear its undo history. The shadow is built on first need and kept up
//!     to date incrementally, and dropped when a packet is refused.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use loro::{
    Container, ContainerID, ExportMode, Frontiers, ID, JsonOpContent, JsonSchema, JsonTextOp,
    LoroDoc,
};

use crate::Document;
use crate::sync::SyncError;

type PeerOf<'a> = &'a dyn Fn(u64) -> Result<u64, SyncError>;
type Ids = BTreeSet<(u64, i32)>;

fn invalid(why: &str) -> SyncError {
    SyncError::Invalid(why.to_owned())
}

fn text_len(doc: &LoroDoc, cid: &ContainerID) -> i64 {
    match doc.get_container(cid.clone()) {
        Some(Container::Text(t)) => i64::try_from(t.len_unicode()).unwrap_or(i64::MAX),
        _ => 0,
    }
}

fn frontiers(ids: &Ids) -> Frontiers {
    ids.iter()
        .map(|&(peer, counter)| ID::new(peer, counter))
        .collect()
}

impl Document {
    pub(crate) fn preflight_texts(
        &self,
        json: &JsonSchema,
        peer_of: PeerOf<'_>,
    ) -> Result<(), SyncError> {
        let Ok(mut slot) = self.shadow.lock() else {
            return Err(SyncError::Store("shadow replica lock poisoned".into()));
        };
        let mut touched = false;
        let result = self.check_texts(json, peer_of, &mut slot, &mut touched);
        // The shadow may hold part of a refused packet, which a peer could
        // later resend with different content under the same IDs: rebuild it.
        // It stays checked out where it is otherwise, so the next checkout
        // moves only as far as the next packet's dependencies.
        if result.is_err() && touched {
            *slot = None;
        }
        result
    }

    fn check_texts(
        &self,
        json: &JsonSchema,
        peer_of: PeerOf<'_>,
        slot: &mut Option<LoroDoc>,
        touched: &mut bool,
    ) -> Result<(), SyncError> {
        let mut order = Vec::with_capacity(json.changes.len());
        for (i, change) in json.changes.iter().enumerate() {
            order.push((
                change.lamport,
                peer_of(change.id.peer)?,
                change.id.counter,
                i,
            ));
        }
        order.sort_unstable();
        let local = self.doc.oplog_vv();
        let known_local =
            |peer: u64, counter: i32| counter < local.get(&peer).copied().unwrap_or(0);
        let mut processed: BTreeMap<u64, i32> = BTreeMap::new();
        let mut frontier: Ids = self
            .doc
            .oplog_frontiers()
            .iter()
            .map(|id| (id.peer, id.counter))
            .collect();
        // Lengths at `frontier`, for lookup only; `None` when they can't be simulated.
        let mut lengths: Option<HashMap<ContainerID, i64>> = Some(HashMap::new());
        let mut shadow_active = false;
        let mut done: Vec<usize> = Vec::new();

        for (_, peer, counter, i) in order {
            let Some(change) = json.changes.get(i) else {
                continue;
            };
            let known = |p: u64, c: i32, processed: &BTreeMap<u64, i32>| {
                known_local(p, c) || processed.get(&p).is_some_and(|&end| c < end)
            };
            if counter > 0 && !known(peer, counter - 1, &processed) {
                return Err(invalid(
                    "change precedes its own predecessor in Lamport order",
                ));
            }
            let mut deps = Ids::new();
            for dep in &change.deps {
                let p = peer_of(dep.peer)?;
                if !known(p, dep.counter, &processed) {
                    return Err(invalid("change precedes its dependencies in Lamport order"));
                }
                deps.insert((p, dep.counter));
            }
            let has_text = change
                .ops
                .iter()
                .any(|op| matches!(op.content, JsonOpContent::Text(_)));
            if deps != frontier || lengths.is_none() {
                if has_text {
                    *touched |= !done.is_empty();
                    let shadow = self.shadow_for(slot, json, &done, shadow_active)?;
                    shadow_active = true;
                    shadow
                        .checkout(&frontiers(&deps))
                        .map_err(|_| invalid("dependencies are not in the history"))?;
                    lengths = Some(HashMap::new());
                } else {
                    lengths = None;
                }
            }
            if let Some(cache) = lengths.as_mut() {
                let source = if shadow_active { slot.as_ref() } else { None };
                for op in &change.ops {
                    let JsonOpContent::Text(text_op) = &op.content else {
                        continue;
                    };
                    let cid = match &op.container {
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
                    };
                    let len = cache.entry(cid.clone()).or_insert_with(|| match source {
                        Some(shadow) => text_len(shadow, &cid),
                        None => text_len(&self.doc, &cid),
                    });
                    apply(text_op, len)?;
                }
            }
            let end = change
                .ops
                .last()
                .map(|op| i64::from(op.counter) + i64::try_from(op.content.op_len()).unwrap_or(0))
                .and_then(|end| i32::try_from(end).ok())
                .ok_or_else(|| invalid("change counters out of range"))?;
            processed.insert(peer, end);
            frontier = Ids::from([(peer, end - 1)]);
            done.push(i);
            if shadow_active && let Some(shadow) = slot.as_ref() {
                *touched = true;
                import_one(shadow, json, i)?;
            }
        }
        Ok(())
    }

    /// The shadow replica, holding everything the live document has plus the
    /// packet changes already checked.
    fn shadow_for<'s>(
        &self,
        slot: &'s mut Option<LoroDoc>,
        json: &JsonSchema,
        done: &[usize],
        active: bool,
    ) -> Result<&'s LoroDoc, SyncError> {
        if slot.is_none() {
            self.commit();
            let shadow = self.doc.fork();
            *slot = Some(shadow);
        }
        let Some(shadow) = slot.as_ref() else {
            return Err(SyncError::Store("no shadow replica".into()));
        };
        if !active {
            // Imports reach its history even while it is checked out.
            let missing = self
                .doc
                .export(ExportMode::updates(&shadow.oplog_vv()))
                .map_err(|e| SyncError::Store(e.to_string()))?;
            shadow
                .import(&missing)
                .map_err(|e| SyncError::Store(e.to_string()))?;
            for &i in done {
                import_one(shadow, json, i)?;
            }
        }
        Ok(shadow)
    }
}

fn import_one(shadow: &LoroDoc, json: &JsonSchema, i: usize) -> Result<(), SyncError> {
    let Some(change) = json.changes.get(i) else {
        return Ok(());
    };
    let one = JsonSchema {
        schema_version: json.schema_version,
        start_version: Frontiers::default(),
        peers: json.peers.clone(),
        changes: vec![change.clone()],
    };
    shadow
        .import_json_updates(one)
        .map(|_| ())
        .map_err(|e| SyncError::Store(e.to_string()))
}

/// Checks one text operation against the text's length and applies its
/// effect on that length.
fn apply(op: &JsonTextOp, len: &mut i64) -> Result<(), SyncError> {
    match op {
        JsonTextOp::Insert { pos, text } => {
            if i64::from(*pos) > *len {
                return Err(invalid("text insertion past the end of its text"));
            }
            *len = len.saturating_add(i64::try_from(text.chars().count()).unwrap_or(i64::MAX));
        }
        JsonTextOp::Delete {
            pos, len: signed, ..
        } => {
            let (pos, signed) = (i64::from(*pos), i64::from(*signed));
            // Loro's signed spans: forwards from `pos`, or backwards ending at it.
            let (start, end) = if signed > 0 {
                (pos, pos + signed)
            } else {
                (pos + 1 + signed, pos + 1)
            };
            if start < 0 || end > *len || start >= end {
                return Err(invalid("text deletion outside its text"));
            }
            *len -= end - start;
        }
        JsonTextOp::Mark { .. } | JsonTextOp::MarkEnd => {
            return Err(invalid("operation kind is not part of delta-json"));
        }
    }
    Ok(())
}
