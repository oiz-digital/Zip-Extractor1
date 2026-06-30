//! Durable storage backend for bridge replay-protection.
//!
//! ## MAINNET-BLOCKER (OUT1) — what this module fixes
//!
//! `MultisigAuth` previously kept `spent_operations` (the set of already-
//! executed bridge operation hashes) only in process memory.  A relayer restart
//! silently cleared the set, allowing any previously-executed `msg_hash` to be
//! replayed — triggering a second mint/unlock on the destination chain
//! (double-spend).
//!
//! ## Architecture
//!
//! ```text
//!  BridgeRelayer::execute()
//!       │
//!       ├─ 1. auth.is_spent(&msg_hash)          ← fast in-memory check
//!       ├─ 2. auth.verify_threshold(...)          ← ECDSA crypto
//!       ├─ 3. store.persist_one(msg_hash)         ← fsync to RocksDB  ← HERE
//!       └─ 4. auth.mark_spent(msg_hash)           ← in-memory set
//!
//! Node startup:
//!       store.load_all()  →  relayer.load_spent_ops(hashes)
//! ```
//!
//! The critical ordering in step 3 before step 4 guarantees crash-safety:
//! if the process dies between 3 and 4, the hash is in the DB but not in
//! memory.  On the next startup, `load_all()` rehydrates the in-memory set
//! from the DB, restoring replay protection.
//!
//! ## Node wiring (add to node startup, BEFORE accepting bridge traffic)
//!
//! ```rust,ignore
//! use zbx_bridge::persistence::BridgeSpentOpsStore;
//! use std::sync::Arc;
//!
//! // Construct the store with a shared DB handle.
//! let store = Arc::new(BridgeSpentOpsStore::new(Arc::clone(&db)));
//!
//! // Attach to the relayer — loads all persisted hashes into memory.
//! relayer.attach_storage(store)?;
//!
//! // Now start accepting bridge transactions.
//! ```

use std::sync::Arc;
use zbx_types::H256;
use zbx_storage::ZbxDb;
use tracing::info;

/// Trait abstracting the persistence backend for bridge spent-operations.
///
/// The production implementation is [`BridgeSpentOpsStore`] (backed by
/// RocksDB).  Tests can substitute a no-op or in-memory implementation.
pub trait SpentOpsStore: Send + Sync {
    /// Durably record `hash` as spent.
    ///
    /// Must be called BEFORE `MultisigAuth::mark_spent()` in the execution
    /// path.  Implementations must use a synced (fsync) write — a buffered
    /// write is not acceptable because loss-of-power before the OS flushes
    /// would lose the record and allow a replay.
    fn persist_one(&self, hash: H256) -> Result<(), String>;

    /// Return all hashes persisted so far.
    ///
    /// Called once on node startup to rehydrate the in-memory
    /// `MultisigAuth::spent_operations` set.
    fn load_all(&self) -> Result<Vec<H256>, String>;
}

/// RocksDB-backed implementation of [`SpentOpsStore`].
///
/// Uses `ZbxDb::put_bridge_spent_op` (fsync write to `Column::BridgeSpentOps`)
/// and `ZbxDb::iter_bridge_spent_ops` (full column-family scan on startup).
pub struct BridgeSpentOpsStore {
    db: Arc<ZbxDb>,
}

impl BridgeSpentOpsStore {
    /// Construct with a shared `ZbxDb` handle.
    ///
    /// The `Arc` is cloned internally — the store does not take exclusive
    /// ownership, so the caller can keep using the same `db` for other
    /// column families.
    pub fn new(db: Arc<ZbxDb>) -> Self {
        Self { db }
    }
}

impl SpentOpsStore for BridgeSpentOpsStore {
    /// Write `hash` to `Column::BridgeSpentOps` with fsync.
    ///
    /// The write is idempotent — persisting the same hash twice is a no-op.
    fn persist_one(&self, hash: H256) -> Result<(), String> {
        self.db
            .put_bridge_spent_op(hash)
            .map_err(|e| format!("bridge_spent_ops write failed: {e}"))
    }

    /// Scan every entry in `Column::BridgeSpentOps` and return the hashes.
    ///
    /// Typical startup load is O(n) in the number of executed bridge
    /// operations — at most a few thousand entries for a heavily-used bridge.
    /// The result is loaded into `MultisigAuth::spent_operations` once and
    /// thereafter only the fast in-memory path is used.
    fn load_all(&self) -> Result<Vec<H256>, String> {
        let hashes = self.db
            .iter_bridge_spent_ops()
            .map_err(|e| format!("bridge_spent_ops load failed: {e}"))?;
        info!(
            count = hashes.len(),
            "bridge: loaded {} spent-op hashes from persistent storage",
            hashes.len()
        );
        Ok(hashes)
    }
}

// ── In-memory no-op store for tests ──────────────────────────────────────────

/// A no-op [`SpentOpsStore`] that satisfies the interface without requiring
/// a RocksDB instance.  Used in unit tests that do not need persistence.
///
/// WARNING: This implementation discards all writes immediately. Never use
/// in production — it is functionally equivalent to the pre-fix in-memory-only
/// behaviour.
pub struct MemSpentOpsStore {
    inner: parking_lot::Mutex<Vec<H256>>,
}

impl Default for MemSpentOpsStore {
    fn default() -> Self {
        Self { inner: parking_lot::Mutex::new(Vec::new()) }
    }
}

impl SpentOpsStore for MemSpentOpsStore {
    fn persist_one(&self, hash: H256) -> Result<(), String> {
        let mut v = self.inner.lock();
        if !v.contains(&hash) {
            v.push(hash);
        }
        Ok(())
    }

    fn load_all(&self) -> Result<Vec<H256>, String> {
        Ok(self.inner.lock().clone())
    }
}
