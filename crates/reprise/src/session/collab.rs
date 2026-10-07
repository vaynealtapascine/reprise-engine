//! Collaboration methods of [`DocumentSession`]: sync packets (format 2),
//! the anchored local selection, tracked undo/redo and presence. See
//! docs/collaboration.md.
use super::DocumentSession;
use crate::convert as cv;
use crate::*;
use reprise_doc::sync::{self, Features, PacketKind};
use std::collections::BTreeMap;

/// Presence payload version inside the awareness bytes.
const PRESENCE_VERSION: u32 = 1;
const MAX_META_ENTRIES: usize = 16;
const MAX_META_KEY: usize = 64;
const MAX_META_VALUE: usize = 1024;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PresenceWire {
    presence: u32,
    selection: Option<StableSelection>,
    meta: BTreeMap<String, String>,
}

fn clocks(vector: &[(u64, i32)]) -> Vec<Clock> {
    vector
        .iter()
        .map(|(p, c)| Clock {
            peer: p.to_string(),
            counter: *c,
        })
        .collect()
}

fn vector(clocks: &[Clock]) -> Result<Vec<(u64, i32)>> {
    if clocks.len() > sync::MAX_VECTOR_ENTRIES {
        return Err(Error::Limit("sync vector".into()));
    }
    let mut out = clocks
        .iter()
        .map(|c| Ok((cv::peer(&c.peer)?, c.counter)))
        .collect::<Result<Vec<_>>>()?;
    out.sort_unstable();
    sync::check_vector(&out).map_err(Error::from)?;
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || s.len() > 2 * reprise_edit::MAX_ANCHOR_BYTES {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i + 2)
                .filter(|h| {
                    h.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        })
        .collect()
}

fn affinity_in(a: Affinity) -> reprise_edit::Affinity {
    match a {
        Affinity::Upstream => reprise_edit::Affinity::Upstream,
        Affinity::Downstream => reprise_edit::Affinity::Downstream,
    }
}

fn affinity_out(a: reprise_edit::Affinity) -> Affinity {
    match a {
        reprise_edit::Affinity::Upstream => Affinity::Upstream,
        reprise_edit::Affinity::Downstream => Affinity::Downstream,
    }
}

fn stable_out(s: &reprise_edit::StableCaret) -> StableCaret {
    StableCaret {
        node: s.node.to_string(),
        affinity: affinity_out(s.affinity),
        anchor: hex(&s.anchor),
    }
}

fn stable_in(s: &StableCaret) -> Option<reprise_edit::StableCaret> {
    Some(reprise_edit::StableCaret {
        node: reprise_doc::NodeId::parse(&s.node)?,
        anchor: unhex(&s.anchor)?,
        affinity: affinity_in(s.affinity),
    })
}

fn selection_in(s: &StableSelection) -> Option<reprise_edit::StableSelection> {
    Some(reprise_edit::StableSelection {
        anchor: stable_in(&s.anchor)?,
        focus: stable_in(&s.focus)?,
    })
}

fn selection_out(s: &reprise_edit::StableSelection) -> StableSelection {
    StableSelection {
        anchor: stable_out(&s.anchor),
        focus: stable_out(&s.focus),
    }
}

fn changes(report: reprise_doc::ChangeReport) -> Changes {
    Changes {
        blocks: report.blocks.iter().map(ToString::to_string).collect(),
        structure: report.structure,
        styles: report.styles,
        relations: report.relations.iter().map(ToString::to_string).collect(),
        ranges: report.ranges.iter().map(ToString::to_string).collect(),
        other: report.other,
    }
}

impl From<sync::SyncError> for Error {
    fn from(e: sync::SyncError) -> Self {
        Self::note(e.note())
    }
}

impl DocumentSession {
    /// A sync packet: a delta of everything `since` lacks, or a full snapshot.
    pub fn sync_export(&self, request: &Payload<SyncRequest>) -> Result<Payload<SyncPacket>> {
        let r = validate(request)?;
        let bytes = match &r.since {
            Some(since) => self.doc().export_delta(&vector(since)?)?,
            None => self.doc().export_snapshot_packet()?,
        };
        let (header, _) = sync::read_header(&bytes)?;
        Ok(Payload::new(SyncPacket {
            document_id: self.document_id.clone(),
            from_peer: self.peer_id.clone(),
            format: u32::from(header.format),
            features: header.features.names(),
            kind: match header.kind {
                PacketKind::Delta => SyncKind::Delta,
                PacketKind::Snapshot => SyncKind::Snapshot,
            },
            since: clocks(&header.since),
            vector: clocks(&header.until),
            content: Bytes { bytes },
        }))
    }

    /// Imports a sync packet. A refused packet changes nothing; `sync.missing`
    /// asks the transport for a delta since [`DocumentSession::sync_info`].
    pub fn sync_import(&mut self, request: &Payload<SyncPacket>) -> Result<Payload<SyncReport>> {
        let r = validate(request)?;
        if r.document_id != self.document_id {
            return Err(Error::InvalidId(r.document_id.clone()));
        }
        if r.from_peer == self.peer_id {
            return Err(Error::Invalid(
                "concurrent replicas must have distinct peers".into(),
            ));
        }
        cv::peer(&r.from_peer)?;
        if r.content.bytes.len() > sync::MAX_PACKET_BYTES.saturating_add(1 << 20) {
            return Err(Error::Limit("sync packet bytes".into()));
        }
        // The frame decides; the mirrored fields must agree with it.
        let (header, _) = sync::read_header(&r.content.bytes)?;
        let kind = match header.kind {
            PacketKind::Delta => SyncKind::Delta,
            PacketKind::Snapshot => SyncKind::Snapshot,
        };
        let features = Features::from_names(&r.features)
            .map_err(|name| Error::Invalid(format!("unknown feature name {name}")))?;
        if u32::from(header.format) != r.format
            || features != header.features
            || kind != r.kind
            || vector(&r.since)? != header.since
            || vector(&r.vector)? != header.until
        {
            return Err(Error::Invalid(
                "packet metadata does not describe its frame".into(),
            ));
        }
        let before = self.doc().version_vector();
        let report = self.editor.import_packet(&r.content.bytes)?;
        let changed = self.doc().version_vector() != before;
        if changed {
            self.snapshot = None;
        }
        let (selection, selection_moved) = self.reanchor();
        Ok(Payload::new(SyncReport {
            info: self.sync_info().data,
            changed,
            changes: changes(report),
            selection,
            selection_moved,
        }))
    }

    /// Anchors the host's current selection, so that `sync_import`,
    /// `undo_report` and `redo_report` return it transformed. `None` clears it.
    pub fn set_selection(
        &mut self,
        request: &Payload<Option<Selection>>,
    ) -> Result<Payload<Option<StableSelection>>> {
        let stable = match validate(request)? {
            Some(s) => Some(self.anchor(s)?),
            None => None,
        };
        let out = stable.as_ref().map(selection_out);
        self.selection = stable;
        Ok(Payload::new(out))
    }

    /// The anchored local selection resolved in the current document.
    pub fn local_selection(&self) -> Payload<Option<Selection>> {
        Payload::new(
            self.selection
                .as_ref()
                .and_then(|s| s.resolve(self.doc()))
                .map(|(s, _)| selection_dto(s)),
        )
    }

    /// Anchors a selection without keeping it.
    pub fn anchor_selection(
        &self,
        request: &Payload<Selection>,
    ) -> Result<Payload<StableSelection>> {
        Ok(Payload::new(selection_out(
            &self.anchor(validate(request)?)?,
        )))
    }

    /// Resolves a stable selection now. Malformed anchors are `bindings.invalid`;
    /// deleted text or blocks take the documented fallback; `None` means the
    /// document has no caret block.
    pub fn resolve_selection(
        &self,
        request: &Payload<StableSelection>,
    ) -> Result<Payload<Option<Selection>>> {
        let stable = selection_in(validate(request)?)
            .ok_or_else(|| Error::Invalid("malformed stable selection".into()))?;
        Ok(Payload::new(
            stable.resolve(self.doc()).map(|(s, _)| selection_dto(s)),
        ))
    }

    /// [`DocumentSession::undo`], reporting what changed and the local selection.
    pub fn undo_report(&mut self) -> Result<Payload<EditReport>> {
        let (changed, report) = self.editor.undo_report()?;
        Ok(Payload::new(self.edit_report(changed, report)))
    }

    /// [`DocumentSession::redo`], reporting what changed and the local selection.
    pub fn redo_report(&mut self) -> Result<Payload<EditReport>> {
        let (changed, report) = self.editor.redo_report()?;
        Ok(Payload::new(self.edit_report(changed, report)))
    }

    /// Awareness bytes carrying this peer's presence.
    pub fn presence(&self, request: &Payload<Presence>) -> Result<Payload<Awareness>> {
        let r = validate(request)?;
        if r.meta.len() > MAX_META_ENTRIES
            || r.meta
                .iter()
                .any(|(k, v)| k.len() > MAX_META_KEY || v.len() > MAX_META_VALUE)
        {
            return Err(Error::Limit("presence metadata".into()));
        }
        let selection = match &r.selection {
            Some(s) => Some(selection_out(&self.anchor(s)?)),
            None => None,
        };
        let bytes = serde_json::to_vec(&PresenceWire {
            presence: PRESENCE_VERSION,
            selection,
            meta: r.meta.clone(),
        })
        .map_err(|e| Error::Invalid(e.to_string()))?;
        self.awareness(&bytes)
    }

    /// Resolves another peer's presence on the current layout. Stale or garbage
    /// presence degrades to an empty view; only an unsupported envelope
    /// version is an error.
    pub fn resolve_presence(&self, request: &Payload<Awareness>) -> Result<Payload<PresenceView>> {
        let r = validate(request)?;
        let mut view = PresenceView {
            peer_id: r.peer_id.clone(),
            ..PresenceView::default()
        };
        if r.document_id != self.document_id
            || cv::peer(&r.peer_id).is_err()
            || r.content.bytes.len() > MAX_AWARENESS_BYTES
        {
            return Ok(Payload::new(view));
        }
        let Ok(wire) = serde_json::from_slice::<PresenceWire>(&r.content.bytes) else {
            return Ok(Payload::new(view));
        };
        if wire.presence != PRESENCE_VERSION
            || wire.meta.len() > MAX_META_ENTRIES
            || wire
                .meta
                .iter()
                .any(|(k, v)| k.len() > MAX_META_KEY || v.len() > MAX_META_VALUE)
        {
            return Ok(Payload::new(view));
        }
        view.meta = wire.meta;
        let Some((selection, _)) = wire
            .selection
            .as_ref()
            .and_then(selection_in)
            .and_then(|s| s.resolve(self.doc()))
        else {
            return Ok(Payload::new(view));
        };
        view.selection = Some(selection_dto(selection));
        if let Ok(nav) = self.navigator() {
            let focus = nav.normalize(selection.focus);
            view.caret = focus.and_then(|c| nav.caret_rect(c)).map(|r| PageRect {
                page: r.page as u32,
                rect: cv::rect(r.rect),
            });
            if let (Some(anchor), Some(focus)) = (nav.normalize(selection.anchor), focus) {
                let normalized = reprise_edit::Selection { anchor, focus };
                view.rects = nav
                    .selection_rects(&normalized)
                    .into_iter()
                    .map(|r| PageRect {
                        page: r.page as u32,
                        rect: cv::rect(r.rect),
                    })
                    .collect();
            }
        }
        Ok(Payload::new(view))
    }

    fn anchor(&self, s: &Selection) -> Result<reprise_edit::StableSelection> {
        let anchor = self.anchor_caret(&s.anchor)?;
        let focus = self.anchor_caret(&s.focus)?;
        Ok(reprise_edit::StableSelection { anchor, focus })
    }

    fn anchor_caret(&self, c: &Caret) -> Result<reprise_edit::StableCaret> {
        let caret = cv::caret(c)?;
        reprise_edit::StableCaret::anchor(self.doc(), caret).map_err(|e| match e {
            reprise_edit::StableError::NoBlock(n) => Error::InvalidId(n.to_string()),
            reprise_edit::StableError::BadOffset(_) => {
                Error::Invalid("caret is not at a UTF-8 boundary in its block".into())
            }
        })
    }

    /// Resolves the local selection and re-anchors it where it moved, so that
    /// a fallback position is kept rather than recomputed from old anchors.
    fn reanchor(&mut self) -> (Option<Selection>, bool) {
        let Some(stable) = &self.selection else {
            return (None, false);
        };
        let Some((selection, moved)) = stable.resolve(self.doc()) else {
            return (None, true);
        };
        if moved && let Ok(fresh) = reprise_edit::StableSelection::anchor(self.doc(), &selection) {
            self.selection = Some(fresh);
        }
        (Some(selection_dto(selection)), moved)
    }

    fn edit_report(&mut self, changed: bool, report: reprise_doc::ChangeReport) -> EditReport {
        if changed {
            self.snapshot = None;
        }
        let (selection, selection_moved) = self.reanchor();
        EditReport {
            changed,
            changes: changes(report),
            selection,
            selection_moved,
        }
    }
}

fn selection_dto(s: reprise_edit::Selection) -> Selection {
    Selection {
        anchor: cv::caret_out(s.anchor),
        focus: cv::caret_out(s.focus),
    }
}
