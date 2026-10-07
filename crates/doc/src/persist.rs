//! Opaque persistence boundary: no Loro type crosses into the file-format crate.

use loro::{EncodedBlobMode, ExportMode};

use crate::{DocError, Document};

/// History retention is an explicit caller choice. Shallow snapshots retain
/// state and the current frontier, but cannot resolve older snapshot targets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PersistenceMode {
    #[default]
    History,
    Shallow,
}

/// Bound the encoded document and declared operation history before import.
pub const MAX_PERSIST_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PERSIST_OPS: u64 = 4_000_000;
pub const MAX_PERSIST_EXPANDED_BYTES: usize = 128 * 1024 * 1024;

impl Document {
    /// Export a snapshot. An encoding failure returns an empty (invalid) blob;
    /// callers needing the reason should use `try_export`, as reprise-format does.
    pub fn export(&self, mode: PersistenceMode) -> Vec<u8> {
        self.try_export(mode).unwrap_or_default()
    }

    /// Fallible snapshot export, including documents whose history was already
    /// compacted. No timestamp is introduced by persistence.
    pub fn try_export(&self, mode: PersistenceMode) -> Result<Vec<u8>, DocError> {
        self.commit();
        let frontiers = self.doc.state_frontiers();
        let mode = match mode {
            PersistenceMode::History => ExportMode::Snapshot,
            PersistenceMode::Shallow => ExportMode::shallow_snapshot(&frontiers),
        };
        let bytes = self
            .doc
            .export(mode)
            .map_err(|e| DocError::Export(e.to_string()))?;
        validate_snapshot(&bytes)?;
        Ok(bytes)
    }

    /// Import a self-contained snapshot, choosing the editing peer explicitly.
    /// Updates with missing dependencies are not document files.
    pub fn import(bytes: &[u8], peer: u64) -> Result<Document, DocError> {
        validate_snapshot(bytes)?;
        let doc = Document::new(peer)?;
        let status = doc.doc.import(bytes)?;
        if status.pending.is_some() {
            return Err(DocError::Store("snapshot has missing dependencies".into()));
        }
        doc.doc.set_peer_id(peer)?;
        doc.doc.get_tree("content").enable_fractional_index(0);
        Ok(doc)
    }
}

fn validate_snapshot(bytes: &[u8]) -> Result<(), DocError> {
    validate_snapshot_mode(bytes, false).map(|_| ())
}

/// The snapshot checks, returning the snapshot's sorted version vector.
/// `history` refuses shallow snapshots, which can't serve every peer.
pub(crate) fn validate_snapshot_mode(
    bytes: &[u8],
    history: bool,
) -> Result<Vec<(u64, i32)>, DocError> {
    if bytes.len() > MAX_PERSIST_BYTES {
        return Err(DocError::Store("snapshot exceeds byte limit".into()));
    }
    preflight_snapshot(bytes)?;
    let meta = loro::LoroDoc::decode_import_blob_meta(bytes, true)?;
    let accepted = match meta.mode {
        EncodedBlobMode::Snapshot => true,
        EncodedBlobMode::ShallowSnapshot => !history,
        _ => false,
    };
    if !accepted {
        return Err(DocError::Store("expected a current Loro snapshot".into()));
    }
    let mut ops = 0_u64;
    for counter in meta.partial_end_vv.values() {
        let counter = u64::try_from(*counter)
            .map_err(|_| DocError::Store("negative history counter".into()))?;
        ops = ops.saturating_add(counter);
    }
    if ops > MAX_PERSIST_OPS || u64::from(meta.change_num) > MAX_PERSIST_OPS {
        return Err(DocError::Store("snapshot exceeds operation limit".into()));
    }
    let mut end: Vec<_> = meta
        .partial_end_vv
        .iter()
        .map(|(peer, counter)| (*peer, *counter))
        .filter(|&(_, counter)| counter > 0)
        .collect();
    end.sort_unstable();
    Ok(end)
}

fn invalid() -> DocError {
    DocError::Store("invalid or oversized snapshot encoding".into())
}

/// No allocation or decompression: bound Loro 1.16's fast-snapshot KV tables
/// before its metadata reader (which otherwise decompresses without a budget).
/// Unknown encodings fail closed. Authored schemas remain Loro's responsibility.
fn preflight_snapshot(bytes: &[u8]) -> Result<(), DocError> {
    if bytes.get(..4) != Some(b"loro") || bytes.get(20..22) != Some([0, 3].as_slice()) {
        return Err(invalid());
    }
    let mut input = Input(bytes.get(22..).ok_or_else(invalid)?);
    let mut expanded = 0_usize;
    for _ in 0..3 {
        let len = input.u32()?;
        let table = input.take(len)?;
        if !table.is_empty() && table != b"E" {
            preflight_table(table, &mut expanded)?;
        }
    }
    if !input.0.is_empty() {
        return Err(invalid());
    }
    Ok(())
}

struct Input<'a>(&'a [u8]);

impl<'a> Input<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], DocError> {
        let head = self.0.get(..len).ok_or_else(invalid)?;
        self.0 = self.0.get(len..).ok_or_else(invalid)?;
        Ok(head)
    }
    fn byte(&mut self) -> Result<u8, DocError> {
        self.take(1)?.first().copied().ok_or_else(invalid)
    }
    fn u16(&mut self) -> Result<usize, DocError> {
        Ok(usize::from(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| invalid())?,
        )))
    }
    fn u32(&mut self) -> Result<usize, DocError> {
        usize::try_from(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| invalid())?,
        ))
        .map_err(|_| invalid())
    }
}

fn charge(expanded: &mut usize, amount: usize) -> Result<(), DocError> {
    *expanded = expanded.checked_add(amount).ok_or_else(invalid)?;
    if *expanded > MAX_PERSIST_EXPANDED_BYTES {
        return Err(invalid());
    }
    Ok(())
}

fn preflight_table(table: &[u8], expanded: &mut usize) -> Result<(), DocError> {
    if table.get(..5) != Some(b"LORO\0") {
        return Err(invalid());
    }
    let end = table.len().checked_sub(4).ok_or_else(invalid)?;
    let mut footer = Input(table.get(end..).ok_or_else(invalid)?);
    let offset = footer.u32()?;
    if offset < 5 || offset >= end {
        return Err(invalid());
    }
    let mut meta = Input(table.get(offset..end).ok_or_else(invalid)?);
    let count = meta.u32()?;
    if count == 0 || count > 65_536 || count > meta.0.len() / 7 {
        return Err(invalid());
    }
    let mut previous: Option<(usize, u8)> = None;
    for _ in 0..count {
        let start = meta.u32()?;
        if let Some((prev, codec)) = previous {
            preflight_block(table, prev, start, codec, expanded)?;
        }
        let len = meta.u16()?;
        meta.take(len)?;
        let flags = meta.byte()?;
        if flags & 0x80 == 0 {
            let len = meta.u16()?;
            meta.take(len)?;
        }
        previous = Some((start, flags & 0x7f));
    }
    if let Some((prev, codec)) = previous {
        preflight_block(table, prev, offset, codec, expanded)?;
    }
    meta.take(4)?; // checksum; Loro checks its value.
    if !meta.0.is_empty() {
        return Err(invalid());
    }
    Ok(())
}

fn preflight_block(
    table: &[u8],
    start: usize,
    end: usize,
    codec: u8,
    expanded: &mut usize,
) -> Result<(), DocError> {
    if start < 5 || end <= start {
        return Err(invalid());
    }
    let payload_end = end.checked_sub(4).ok_or_else(invalid)?;
    let bytes = table.get(start..payload_end).ok_or_else(invalid)?;
    match codec {
        0 => charge(expanded, bytes.len()),
        1 => preflight_lz4(bytes, expanded),
        _ => Err(invalid()),
    }
}

fn preflight_lz4(bytes: &[u8], expanded: &mut usize) -> Result<(), DocError> {
    let mut r = Input(bytes);
    if r.take(4)? != [4, 34, 77, 24] {
        return Err(invalid());
    }
    let flags = r.byte()?;
    let descriptor = r.byte()?;
    if flags >> 6 != 1 || flags & 3 != 0 {
        return Err(invalid());
    }
    let max_block = match (descriptor >> 4) & 7 {
        4 => 64 * 1024,
        5 => 256 * 1024,
        6 => 1024 * 1024,
        7 => 4 * 1024 * 1024,
        _ => return Err(invalid()),
    };
    let declared = if flags & 8 != 0 {
        let length = u64::from_le_bytes(r.take(8)?.try_into().map_err(|_| invalid())?);
        let length = usize::try_from(length).map_err(|_| invalid())?;
        if length > MAX_PERSIST_EXPANDED_BYTES {
            return Err(invalid());
        }
        Some(length)
    } else {
        None
    };
    r.take(1)?; // header checksum, verified by Loro's decoder
    let mut produced = 0_usize;
    loop {
        let word = r.u32()?;
        if word == 0 {
            break;
        }
        let len = word & 0x7fff_ffff;
        if len > max_block {
            return Err(invalid());
        }
        let block = r.take(len)?;
        let amount = if word & 0x8000_0000 != 0 {
            len
        } else {
            lz4_block_size(
                block,
                if flags & 0x20 == 0 { produced } else { 0 },
                max_block,
            )?
        };
        produced = produced.checked_add(amount).ok_or_else(invalid)?;
        charge(expanded, amount)?;
        if flags & 0x10 != 0 {
            r.take(4)?;
        }
    }
    if flags & 4 != 0 {
        r.take(4)?;
    }
    if !r.0.is_empty() || declared.is_some_and(|len| len != produced) {
        return Err(invalid());
    }
    Ok(())
}

fn extended_length(r: &mut Input<'_>, base: usize) -> Result<usize, DocError> {
    let mut len = base;
    if base == 15 {
        loop {
            let byte = r.byte()?;
            len = len.checked_add(usize::from(byte)).ok_or_else(invalid)?;
            if byte != 255 {
                break;
            }
        }
    }
    Ok(len)
}

fn lz4_block_size(bytes: &[u8], history: usize, max_block: usize) -> Result<usize, DocError> {
    let mut r = Input(bytes);
    let mut size = 0_usize;
    while !r.0.is_empty() {
        let token = r.byte()?;
        let literal = extended_length(&mut r, usize::from(token >> 4))?;
        r.take(literal)?;
        size = size.checked_add(literal).ok_or_else(invalid)?;
        if size > max_block {
            return Err(invalid());
        }
        if r.0.is_empty() {
            break;
        }
        let offset = r.u16()?;
        if offset == 0 || offset > size.saturating_add(history) {
            return Err(invalid());
        }
        let matched = extended_length(&mut r, usize::from(token & 15))?
            .checked_add(4)
            .ok_or_else(invalid)?;
        size = size.checked_add(matched).ok_or_else(invalid)?;
        if size > max_block {
            return Err(invalid());
        }
    }
    Ok(size)
}

impl Document {
    /// Sorted version vector, opaque to transport implementations.
    pub fn version_vector(&self) -> Vec<(u64, i32)> {
        self.commit();
        let mut vector: Vec<_> = self
            .doc
            .oplog_vv()
            .iter()
            .map(|(peer, counter)| (*peer, *counter))
            .collect();
        vector.sort_unstable();
        vector
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_lz4_rejects_bombs_and_counts_literals_and_matches() {
        let bomb = [
            vec![4, 34, 77, 24, 0x68, 0x70],
            u64::MAX.to_le_bytes().to_vec(),
            vec![0, 0, 0, 0, 0],
        ]
        .concat();
        assert!(preflight_lz4(&bomb, &mut 0).is_err());
        assert_eq!(
            lz4_block_size(&[0x10, b'a', 1, 0, 0x10, b'z'], 0, 65536).unwrap(),
            6
        );
        assert!(lz4_block_size(&[0, 0, 0], 0, 65536).is_err());
        let mut expanded = MAX_PERSIST_EXPANDED_BYTES;
        assert!(charge(&mut expanded, 1).is_err());
    }

    #[test]
    fn lz4_bomb_without_a_declared_length_is_also_bounded() {
        // Each compressed block expands to 4 MiB by repeating its first byte.
        // 33 blocks exceed the total budget, using only about 550 KiB as input.
        let max_block = 4 * 1024 * 1024;
        let mut block = vec![0x1f, b'a', 1, 0];
        let extension = max_block - 1 - 4 - 15;
        block.extend(std::iter::repeat_n(255, extension / 255));
        block.push((extension % 255) as u8);
        assert_eq!(lz4_block_size(&block, 0, max_block).unwrap(), max_block);
        let mut frame = vec![4, 34, 77, 24, 0x60, 0x70, 0];
        for _ in 0..33 {
            frame.extend_from_slice(&(block.len() as u32).to_le_bytes());
            frame.extend_from_slice(&block);
        }
        frame.extend_from_slice(&0_u32.to_le_bytes());
        assert!(preflight_lz4(&frame, &mut 0).is_err());
    }

    #[test]
    fn declared_inner_bomb_is_rejected_at_the_document_import_boundary() {
        let bomb = [
            vec![4, 34, 77, 24, 0x68, 0x70],
            u64::MAX.to_le_bytes().to_vec(),
            vec![0, 0, 0, 0, 0],
        ]
        .concat();
        let mut table = b"LORO\0".to_vec();
        table.extend_from_slice(&bomb);
        table.extend_from_slice(&0_u32.to_le_bytes()); // block checksum
        let offset = table.len() as u32;
        table.extend_from_slice(&1_u32.to_le_bytes()); // one block
        table.extend_from_slice(&5_u32.to_le_bytes());
        table.extend_from_slice(&1_u16.to_le_bytes());
        table.push(b'k');
        table.push(0x81); // large block, LZ4
        table.extend_from_slice(&0_u32.to_le_bytes()); // metadata checksum
        table.extend_from_slice(&offset.to_le_bytes());
        let mut snapshot = b"loro".to_vec();
        snapshot.extend_from_slice(&[0; 16]);
        snapshot.extend_from_slice(&[0, 3]);
        snapshot.extend_from_slice(&(table.len() as u32).to_le_bytes());
        snapshot.extend_from_slice(&table);
        snapshot.extend_from_slice(&[0; 8]); // empty state and shallow tables
        assert!(matches!(
            Document::import(&snapshot, 2),
            Err(DocError::Store(_))
        ));
    }

    #[test]
    fn snapshot_and_every_truncation_are_checked_without_unwinding() {
        let doc = Document::new(1).unwrap();
        doc.append_block(crate::BlockKind::Paragraph, "body", &"a".repeat(100_000))
            .unwrap();
        let bytes = doc.export(PersistenceMode::History);
        preflight_snapshot(&bytes).unwrap();
        assert_eq!(
            Document::import(&bytes, 2).unwrap().revision(),
            doc.revision()
        );
        for end in 0..bytes.len() {
            assert!(Document::import(&bytes[..end], 2).is_err());
        }
    }

    /// Orchestrator review: the preflight reads Loro 1.16's private snapshot
    /// encoding and fails closed on anything else. Bumping Loro must be a
    /// deliberate step that rechecks it, so pin the version here, as
    /// `reprise-shape` pins harfrust.
    #[test]
    fn preflight_is_pinned_to_the_loro_version_it_reads() {
        let lock = include_str!("../../../Cargo.lock").replace(
            "
", "
",
        );
        assert!(
            lock.contains(
                "name = \"loro\"
version = \"1.16.2\""
            ),
            "Loro changed: recheck preflight_snapshot against its encoding, then update this pin"
        );
    }

    /// Orchestrator review: blobs Loro itself produces but that aren't
    /// documents (updates, updates from another peer, a snapshot followed by
    /// trailing bytes, two snapshots back to back) are rejected, never
    /// imported half-way or panicked on.
    #[test]
    fn loro_blobs_that_are_not_documents_are_rejected() {
        use crate::BlockKind;
        let doc = Document::new(1).unwrap();
        doc.append_block(BlockKind::Paragraph, "body", "words")
            .unwrap();
        doc.commit();
        let updates = doc.doc.export(ExportMode::all_updates()).unwrap();
        assert!(
            Document::import(&updates, 1).is_err(),
            "updates are not a document"
        );
        let other = doc.fork(2).unwrap();
        other
            .append_block(BlockKind::Paragraph, "body", "more")
            .unwrap();
        other.commit();
        let delta = other
            .doc
            .export(ExportMode::updates(&doc.doc.oplog_vv()))
            .unwrap();
        assert!(
            Document::import(&delta, 1).is_err(),
            "a delta is not a document"
        );
        let snapshot = doc.export(PersistenceMode::History);
        let mut trailing = snapshot.clone();
        trailing.extend_from_slice(b" trailing");
        assert!(Document::import(&trailing, 1).is_err(), "trailing bytes");
        let mut twice = snapshot.clone();
        twice.extend_from_slice(&snapshot);
        assert!(Document::import(&twice, 1).is_err(), "two snapshots");
        assert!(
            Document::import(&snapshot, 1).is_ok(),
            "the snapshot itself opens"
        );
    }
}
