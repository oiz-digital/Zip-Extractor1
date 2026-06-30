//! Snap-sync: download state trie in parallel chunks.

use crate::error::SyncError;
use zbx_types::H256;
use zbx_trie::{MutableTrie, trie::MemoryTrieDB};
use std::collections::{HashMap, BTreeSet};
use tracing::{debug, warn};

/// Size of each state chunk request (number of trie leaf slots).
const CHUNK_SLOTS: usize = 4096;

/// A range of state keys to download in one request.
#[derive(Debug, Clone)]
pub struct StateChunk {
    pub id:         u64,
    pub start_key:  H256,
    pub end_key:    H256,
    pub state_root: H256,
    /// SEC-2026-05-09 Pass-11 — root of the self-contained mini-trie
    /// for this chunk, committed by the snapshot manifest. The
    /// verifier rebuilds a `MutableTrie` from the chunk's leaves
    /// and asserts the computed root equals this value. `None`
    /// during chunk planning before the manifest arrives.
    pub chunk_root: Option<H256>,
}

/// Progress tracker for snap-sync.
pub struct SnapSyncProgress {
    pub total_chunks:      u64,
    pub completed_chunks:  u64,
    pub failed_chunks:     BTreeSet<u64>,
    pub pending:           HashMap<u64, StateChunk>,
}

impl SnapSyncProgress {
    pub fn new(total: u64) -> Self {
        Self {
            total_chunks: total,
            completed_chunks: 0,
            failed_chunks: BTreeSet::new(),
            pending: HashMap::new(),
        }
    }

    pub fn percent_complete(&self) -> f64 {
        if self.total_chunks == 0 { return 100.0; }
        (self.completed_chunks as f64 / self.total_chunks as f64) * 100.0
    }

    pub fn mark_complete(&mut self, chunk_id: u64) {
        self.pending.remove(&chunk_id);
        self.completed_chunks += 1;
        self.failed_chunks.remove(&chunk_id);
        debug!("snap: chunk {} complete ({:.1}%)", chunk_id, self.percent_complete());
    }

    pub fn mark_failed(&mut self, chunk_id: u64) {
        warn!("snap: chunk {} failed, will retry", chunk_id);
        self.failed_chunks.insert(chunk_id);
    }

    pub fn is_complete(&self) -> bool {
        self.completed_chunks >= self.total_chunks && self.failed_chunks.is_empty()
    }
}

/// SEC-2026-05-09 Pass-11 — REAL Merkle verification of a snap chunk.
///
/// Rebuilds an in-memory MPT from `leaf_data`, computes its root, and
/// asserts that root equals `chunk.chunk_root` (the responder-committed
/// mini-trie root from the snapshot manifest).
///
/// **Honest scope:** this is a "self-contained mini-trie per chunk"
/// commitment — each chunk is its own complete MPT whose root the
/// snapshot manifest commits to. This is *not* a full Ethereum-style
/// range proof against the global state root (which would also need
/// the boundary nodes between adjacent chunks); but it IS sufficient
/// to prove that what the responder sent matches what the manifest
/// promised, and the manifest's `state_root` field is independently
/// matched against the pivot block header. A Pass-12 follow-up can
/// add boundary range proofs.
pub fn verify_chunk(chunk: &StateChunk, leaf_data: &[(H256, Vec<u8>)]) -> Result<(), SyncError> {
    if leaf_data.is_empty() {
        return Err(SyncError::ChunkHashMismatch { chunk: chunk.id });
    }
    // Bounds check (cheap fail-fast before the trie rebuild).
    for (key, _) in leaf_data {
        if key < &chunk.start_key || key > &chunk.end_key {
            return Err(SyncError::ChunkHashMismatch { chunk: chunk.id });
        }
    }
    let expected = chunk.chunk_root.ok_or_else(|| {
        SyncError::Interrupted(format!(
            "chunk {} verification requested before manifest arrived",
            chunk.id
        ))
    })?;
    let mut trie = MutableTrie::new(MemoryTrieDB::default());
    for (key, value) in leaf_data {
        trie.insert(key.as_bytes(), value.clone())
            .map_err(|e| SyncError::Interrupted(format!("trie insert: {e}")))?;
    }
    let computed = trie.commit()
        .map_err(|e| SyncError::Interrupted(format!("trie commit: {e}")))?;
    if computed != expected {
        warn!(
            chunk_id = chunk.id,
            ?computed, ?expected,
            "snap chunk Merkle root mismatch"
        );
        return Err(SyncError::ChunkHashMismatch { chunk: chunk.id });
    }
    debug!(chunk_id = chunk.id, ?computed, "snap chunk Merkle-verified");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(b: u8) -> H256 { let mut k = [0u8; 32]; k[31] = b; H256(k) }

    #[test]
    fn verify_chunk_accepts_correct_leaves() {
        // Build the truth root the same way verify_chunk will.
        let leaves: Vec<(H256, Vec<u8>)> = (0..8u8)
            .map(|i| (key(i), vec![i, i, i, i]))
            .collect();
        let mut t = MutableTrie::new(MemoryTrieDB::default());
        for (k, v) in &leaves { t.insert(k.as_bytes(), v.clone()).unwrap(); }
        let truth = t.commit().unwrap();

        let chunk = StateChunk {
            id: 0, start_key: key(0), end_key: key(255),
            state_root: H256::zero(), chunk_root: Some(truth),
        };
        assert!(verify_chunk(&chunk, &leaves).is_ok());
    }

    #[test]
    fn verify_chunk_rejects_tampered_value() {
        let leaves: Vec<(H256, Vec<u8>)> = (0..4u8)
            .map(|i| (key(i), vec![i]))
            .collect();
        let mut t = MutableTrie::new(MemoryTrieDB::default());
        for (k, v) in &leaves { t.insert(k.as_bytes(), v.clone()).unwrap(); }
        let truth = t.commit().unwrap();

        let mut tampered = leaves.clone();
        tampered[2].1 = vec![0xFF];

        let chunk = StateChunk {
            id: 7, start_key: key(0), end_key: key(255),
            state_root: H256::zero(), chunk_root: Some(truth),
        };
        match verify_chunk(&chunk, &tampered) {
            Err(SyncError::ChunkHashMismatch { chunk: 7 }) => {}
            other => panic!("expected ChunkHashMismatch{{7}}, got {other:?}"),
        }
    }

    #[test]
    fn verify_chunk_rejects_out_of_range_key() {
        let chunk = StateChunk {
            id: 1, start_key: key(10), end_key: key(20),
            state_root: H256::zero(), chunk_root: Some(H256::zero()),
        };
        let leaves = vec![(key(5), vec![1])];
        assert!(matches!(
            verify_chunk(&chunk, &leaves),
            Err(SyncError::ChunkHashMismatch { chunk: 1 })
        ));
    }

    #[test]
    fn verify_chunk_rejects_missing_manifest() {
        let chunk = StateChunk {
            id: 2, start_key: key(0), end_key: key(255),
            state_root: H256::zero(), chunk_root: None,
        };
        let leaves = vec![(key(1), vec![1])];
        assert!(matches!(
            verify_chunk(&chunk, &leaves),
            Err(SyncError::Interrupted(_))
        ));
    }
}

/// SEC-2026-05-09 Pass-11 (architect-review follow-up):
/// CRYPTOGRAPHIC BINDING from chunks to pivot state-root.
///
/// Per-chunk `verify_chunk` only proves "the leaves you sent match
/// the chunk_root the manifest committed to". By itself that is
/// insufficient — a malicious peer can publish a manifest with
/// `state_root = X` (the real pivot state-root, taken from the
/// header) but `chunk_roots = [Y, Z]` where Y and Z are roots of
/// attacker-chosen leaves. Each chunk passes verify_chunk against
/// its own bogus root, and the manifest's `state_root` field equals
/// the pivot header's `state_root`, so the existing checks miss the
/// attack.
///
/// This function closes the bypass: it rebuilds a SINGLE global MPT
/// from the union of all chunks' leaves and asserts the computed
/// root equals the pivot header's `state_root`. Combined with the
/// per-chunk check, this proves end-to-end that what the responder
/// delivered IS the canonical state at the pivot block.
///
/// Cost: O(total leaves) trie inserts at bootstrap time. Acceptable
/// for fast-sync (which already replays the snapshot into local
/// state). For very large states a Pass-12 enhancement can stream
/// chunks into a persistent trie incrementally rather than holding
/// every leaf in memory; the security property is identical.
pub fn verify_global_state_root(
    pivot_state_root: H256,
    all_chunks: &[Vec<(H256, Vec<u8>)>],
) -> Result<(), SyncError> {
    let mut trie = MutableTrie::new(MemoryTrieDB::default());
    let mut total: usize = 0;
    for chunk_leaves in all_chunks {
        for (key, value) in chunk_leaves {
            trie.insert(key.as_bytes(), value.clone())
                .map_err(|e| SyncError::Interrupted(format!("global trie insert: {e}")))?;
            total += 1;
        }
    }
    let computed = trie.commit()
        .map_err(|e| SyncError::Interrupted(format!("global trie commit: {e}")))?;
    if computed != pivot_state_root {
        warn!(
            ?computed, expected = ?pivot_state_root, total_leaves = total,
            "global state-root mismatch — chunks do NOT compose to pivot state_root"
        );
        return Err(SyncError::Interrupted(format!(
            "global state_root mismatch: computed {:?} expected {:?}",
            computed, pivot_state_root
        )));
    }
    debug!(total_leaves = total, ?computed,
           "Pass-11: global state-root binding verified");
    Ok(())
}

/// Divide the full key space [0..2^256) into `n` equal chunks.
pub fn partition_key_space(state_root: H256, n: u64) -> Vec<StateChunk> {
    // Split [0, 2^256) into n equal ranges.
    let mut chunks = Vec::with_capacity(n as usize);
    // Use approximate u64 arithmetic for the chunk boundaries.
    for i in 0..n {
        let start = H256::from_low_u64_be(i * (u64::MAX / n));
        let end   = if i + 1 == n {
            H256::from([0xff; 32])
        } else {
            H256::from_low_u64_be((i + 1) * (u64::MAX / n) - 1)
        };
        chunks.push(StateChunk {
            id: i, start_key: start, end_key: end, state_root,
            chunk_root: None,
        });
    }
    chunks
}