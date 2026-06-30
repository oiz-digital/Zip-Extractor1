//! `ZbxDbTrieAdapter` — persistent backing for `zbx_trie::TrieDB`
//! over `zbx_storage::ZbxDb` (S33-state-root W3b).
//!
//! # Why this exists
//!
//! W2 + W3a delivered Yellow-Paper §4.1-conformant state-root computation
//! over an in-memory `MemoryTrieDB`. That works perfectly for greenfield
//! and full-overwrite blocks but has a documented honest limitation: for
//! accounts with un-cached pre-existing storage slots, the recomputed
//! storage_root diverges from canonical because the unread slots are
//! silently treated as absent.
//!
//! `ZbxDbTrieAdapter` closes this by storing trie-internal nodes
//! persistently in a dedicated `Column::TrieNodes` RocksDB column family.
//! Combined with `MutableTrie::from_root(account.storage_root, db)`, it
//! lets `compute_state_root_with_db` reload the existing trie and apply
//! only the dirty slot deltas instead of rebuilding from scratch.
//!
//! # Concurrency model
//!
//! `TrieDB::insert` requires `&mut self`, but `ZbxDb` is wrapped in `Arc`
//! by all real callers. We bridge with a small `Mutex<Vec<...>>` that
//! buffers pending writes and auto-flushes on a fixed batch size. Reads
//! also consult the buffer first (so a node written this block is visible
//! within the same block before flush).
//!
//! # Persistence boundary
//!
//! Writes are flushed to `ZbxDb` via the standard `WriteBatch` API. The
//! adapter does NOT call `write_synced` — durability is delegated to the
//! caller, who decides when to fsync (typically once per finalised block).
//! The block-producer integration is responsible for calling `commit()`
//! before publishing the block header.

use std::sync::{Arc, Mutex};

use zbx_storage::{batch::WriteBatch, schema::Column, ZbxDb};
use zbx_trie::{TrieDB, TrieError};
use zbx_types::H256;

/// Default auto-flush threshold — when the pending buffer reaches this
/// many entries, `insert` triggers an implicit `commit()` so memory
/// pressure stays bounded for very large block-execution windows.
///
/// Tuned for typical Ethereum-style block sizes: a 30M-gas block with
/// ~500 SSTOREs touches at most a few thousand trie nodes, so 1024 is
/// roughly half-block.
const AUTO_FLUSH_THRESHOLD: usize = 1024;

/// Persistent trie-node store backed by `ZbxDb`'s `Column::TrieNodes`.
///
/// Cheap to clone (`Arc<ZbxDb>` + `Arc<Mutex<...>>`); cheap to share
/// across threads. Each clone shares the same pending buffer, so any
/// thread can flush.
pub struct ZbxDbTrieAdapter {
    db: Arc<ZbxDb>,
    /// Pending unflushed writes. Shared across all clones of this
    /// adapter so concurrent inserts batch coherently.
    pending: Arc<Mutex<Vec<(H256, Vec<u8>)>>>,
}

impl ZbxDbTrieAdapter {
    /// Create a new adapter over the given persistent database.
    pub fn new(db: Arc<ZbxDb>) -> Self {
        Self {
            db,
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Drain the pending buffer and write everything to `Column::TrieNodes`
    /// in a single atomic batch. Idempotent when the buffer is empty.
    ///
    /// Callers SHOULD invoke this at well-defined boundaries (typically
    /// after `state_root()` returns and before block publication) so the
    /// canonical chain tip and the trie nodes that justify it land
    /// together on durable storage.
    ///
    /// # Concurrency contract (architect-flagged 2026-05-02)
    ///
    /// We intentionally hold the `pending` mutex for the **entire**
    /// commit window, including the underlying RocksDB write. An earlier
    /// design dropped the lock before the disk write to allow parallel
    /// inserts during flush — but that opened a visibility gap: a
    /// concurrent `get(hash)` between drain and disk-write returned
    /// `None` even though the value would shortly become readable from
    /// disk, surfacing as a spurious `MissingNode` to a peer reader.
    ///
    /// Holding the lock through the write makes commits serial w.r.t.
    /// inserts/reads on this adapter clone family, but commits happen at
    /// most once per block (a few μs of disk I/O), so the throughput
    /// impact is negligible. Correctness wins over throughput here.
    pub fn commit(&self) -> Result<(), TrieError> {
        let mut p = self
            .pending
            .lock()
            .map_err(|e| TrieError::MissingNode(format!("adapter mutex poisoned: {e}")))?;
        if p.is_empty() {
            return Ok(());
        }
        // Task #15: hold the pruner coordination read-guard across the
        // batched trie-node write so the pruner's `lock.write()` blocks
        // until this commit lands. `None` when no pruner is wired
        // (tests / standalone tools) — adapter behaves as before.
        let _pruner_guard = self.db.acquire_commit_read_guard();
        let mut batch = WriteBatch::new();
        for (h, v) in p.iter() {
            batch.put(Column::TrieNodes, h.0.to_vec(), v.clone());
        }
        // Hold the lock through the write so concurrent readers always
        // either see the value in `pending` (before write succeeds) or on
        // disk (after write succeeds). Never both-missing.
        self.db
            .write(batch)
            .map_err(|e| TrieError::MissingNode(format!("trie-node write failed: {e}")))?;
        // Write succeeded: now safe to drain.
        p.clear();
        Ok(())
    }

    /// Number of unflushed entries in the pending buffer (for tests +
    /// metrics).
    pub fn pending_len(&self) -> usize {
        self.pending.lock().map(|p| p.len()).unwrap_or(0)
    }

    /// Direct access to the underlying database (for callers that need to
    /// open multiple adapters or perform other ZbxDb ops alongside).
    pub fn db(&self) -> Arc<ZbxDb> {
        Arc::clone(&self.db)
    }
}

impl Clone for ZbxDbTrieAdapter {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            pending: Arc::clone(&self.pending),
        }
    }
}

impl TrieDB for ZbxDbTrieAdapter {
    fn get(&self, hash: &H256) -> Result<Option<Vec<u8>>, TrieError> {
        // Pending buffer wins over disk so within-block writes are visible
        // to the same-block reads that compute the new root.
        {
            let p = self
                .pending
                .lock()
                .map_err(|e| TrieError::MissingNode(format!("adapter mutex poisoned: {e}")))?;
            for (h, v) in p.iter() {
                if h == hash {
                    return Ok(Some(v.clone()));
                }
            }
        }
        self.db
            .get_trie_node(hash)
            .map_err(|e| TrieError::MissingNode(format!("trie-node read failed: {e}")))
    }

    fn insert(&mut self, hash: H256, value: Vec<u8>) -> Result<(), TrieError> {
        let pending_len = {
            let mut p = self
                .pending
                .lock()
                .map_err(|e| TrieError::MissingNode(format!("adapter mutex poisoned: {e}")))?;
            p.push((hash, value));
            p.len()
        };
        if pending_len >= AUTO_FLUSH_THRESHOLD {
            self.commit()?;
        }
        Ok(())
    }

    fn contains(&self, hash: &H256) -> Result<bool, TrieError> {
        self.get(hash).map(|v| v.is_some())
    }
}
