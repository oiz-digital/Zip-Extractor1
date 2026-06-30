//! ZbxDb: high-level storage API over a RocksDB backend with column families.
//!
//! Production deployments use the on-disk RocksDB backend. The optional
//! `mem` cargo feature swaps in an in-memory HashMap backend for tests.

use crate::{
    batch::WriteBatch,
    error::StorageError,
    schema::{Column, block_number_key, state_key, storage_key},
};
use zbx_types::{
    account::AccountState,
    address::Address,
    block::Block,
    receipt::TransactionReceipt,
    transaction::SignedTransaction,
    H256,
};
use parking_lot::RwLock;
use rocksdb::{ColumnFamilyDescriptor, DB, Options, WriteBatch as RocksWriteBatch, WriteOptions};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};
// Governance proposal persistence (H-4 fix: 2026-06-27)
use serde_json;

/// Latest-block-number metadata key.
const META_LATEST: &[u8] = b"latest_block_number";

/// Persistent RocksDB-backed chain storage.
///
/// All blocks, transactions, receipts, account state, contract storage and
/// contract code live in dedicated column families. Writes go through the
/// atomic `WriteBatch` API.
pub struct ZbxDb {
    db: Arc<DB>,
    path: PathBuf,
    /// Cached latest block number (refreshed on write).
    latest_height: RwLock<Option<u64>>,
    /// Task #15: optional coordination lock shared with the
    /// `zbx_pruner::RocksDbPruner`. When set, every write that
    /// touches `Column::TrieNodes` or advances the chain tip
    /// acquires `lock.read()` for the duration of the write so
    /// the pruner (which takes `lock.write()` while sweeping)
    /// cannot delete a node mid-commit. `None` = no pruner wired
    /// (tests, ad-hoc tools); writes proceed unsynchronised.
    commit_lock: RwLock<Option<Arc<RwLock<()>>>>,
}

impl ZbxDb {
    /// Open (or create) the database at the given path.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let path = path.into();

        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts.set_max_open_files(2048);
        opts.increase_parallelism(num_cpus_or(2) as i32);
        opts.set_keep_log_file_num(10);

        let cfs: Vec<ColumnFamilyDescriptor> = Column::all()
            .iter()
            .map(|c| {
                let mut cf_opts = Options::default();
                cf_opts.create_if_missing(true);
                ColumnFamilyDescriptor::new(c.name(), cf_opts)
            })
            .collect();

        let db = DB::open_cf_descriptors(&opts, &path, cfs)
            .map_err(|e| StorageError::Db(format!("rocksdb open: {e}")))?;
        let db = Arc::new(db);

        // Bootstrap latest_height cache from metadata column.
        let latest = match db
            .cf_handle(Column::Metadata.name())
            .and_then(|cf| db.get_cf(&cf, META_LATEST).ok().flatten())
        {
            Some(b) if b.len() == 8 => {
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&b);
                Some(u64::from_be_bytes(buf))
            }
            _ => None,
        };

        info!(path = %path.display(), latest_height = ?latest, "opened RocksDB chain storage");
        Ok(ZbxDb {
            db,
            path,
            latest_height: RwLock::new(latest),
            commit_lock: RwLock::new(None),
        })
    }

    /// Task #15: install the `zbx_pruner::RocksDbPruner` coordination
    /// lock. After this returns, every `commit_block` /
    /// `put_trie_node` / `ZbxDbTrieAdapter::commit` acquires
    /// `lock.read()` for the duration of its write, so the pruner's
    /// `lock.write()` blocks until in-flight commits finish.
    ///
    /// Idempotent — may be called more than once during boot. Safe
    /// to leave unset for tests / standalone tools.
    pub fn set_commit_lock(&self, lock: Arc<RwLock<()>>) {
        *self.commit_lock.write() = Some(lock);
    }

    /// Acquire an owned read-guard on the pruner coordination lock,
    /// or `None` if no lock is installed. The returned guard must be
    /// held for the entire duration of any write that touches
    /// `Column::TrieNodes` or the chain-tip pointer.
    ///
    /// Returns a `parking_lot::ArcRwLockReadGuard` so the guard owns
    /// its `Arc` and can outlive the brief outer borrow on
    /// `self.commit_lock`.
    pub fn acquire_commit_read_guard(
        &self,
    ) -> Option<parking_lot::lock_api::ArcRwLockReadGuard<parking_lot::RawRwLock, ()>> {
        let outer = self.commit_lock.read();
        outer.as_ref().map(|l| Arc::clone(l).read_arc())
    }

    /// Path on disk where the database lives.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    fn cf(&self, col: Column) -> Result<&rocksdb::ColumnFamily, StorageError> {
        self.db
            .cf_handle(col.name())
            .ok_or_else(|| StorageError::Db(format!("missing CF: {}", col.name())))
    }

    /// Atomically apply a write batch (default: no fsync — relies on WAL).
    pub fn write(&self, batch: WriteBatch) -> Result<(), StorageError> {
        self.write_inner(batch, false)
    }

    /// Atomically apply a write batch with fsync — use for canonical chain
    /// state (block headers, latest-height pointer) where loss-of-power must
    /// not roll back the chain tip.
    pub fn write_synced(&self, batch: WriteBatch) -> Result<(), StorageError> {
        self.write_inner(batch, true)
    }

    fn write_inner(&self, batch: WriteBatch, sync: bool) -> Result<(), StorageError> {
        let mut wb = RocksWriteBatch::default();
        for (col, key, value) in &batch.ops {
            let cf = self.cf(*col)?;
            if value.is_empty() {
                // Convention: empty value means delete (matches batch.delete()).
                wb.delete_cf(&cf, key);
            } else {
                wb.put_cf(&cf, key, value);
            }
        }
        let mut opts = WriteOptions::default();
        opts.set_sync(sync);
        self.db
            .write_opt(wb, &opts)
            .map_err(|e| StorageError::Db(format!("rocksdb write: {e}")))
    }

    fn get_raw(&self, col: Column, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        let cf = self.cf(col)?;
        self.db
            .get_cf(&cf, key)
            .map_err(|e| StorageError::Db(format!("rocksdb get: {e}")))
    }

    // -----------------------------------------------------------------------
    // Block operations
    // -----------------------------------------------------------------------

    pub fn put_block(&self, block: &Block) -> Result<(), StorageError> {
        // Task #15: pruner coordination — `put_block` is the canonical
        // tip-advance write path used by `block_producer::execute_and_commit_inner`.
        // Hold the pruner read-guard across the whole batch + tip
        // pointer update so a concurrent sweep cannot interleave
        // between the new block being persisted and the latest-height
        // pointer flipping (would otherwise let the pruner observe an
        // intermediate `head_n` and GC trie nodes the producer is
        // about to anchor). No-op when no commit_lock is installed
        // (genesis loaders, single-binary tests).
        let _guard = self.acquire_commit_read_guard();

        let hash = block.hash();
        let number = block.number();
        let encoded = serde_json::to_vec(block)
            .map_err(|e| StorageError::Encode(e.to_string()))?;
        let mut batch = WriteBatch::new();
        batch.put(Column::Blocks, hash.0.to_vec(), encoded);
        batch.put(
            Column::BlockNumbers,
            block_number_key(number).to_vec(),
            hash.0.to_vec(),
        );
        // Update latest pointer in metadata.
        batch.put(
            Column::Metadata,
            META_LATEST.to_vec(),
            number.to_be_bytes().to_vec(),
        );
        // Canonical chain tip — fsync to survive power loss.
        self.write_synced(batch)?;
        let mut latest = self.latest_height.write();
        if latest.map(|h| number > h).unwrap_or(true) {
            *latest = Some(number);
        }
        Ok(())
    }

    pub fn get_block_by_hash(&self, hash: &H256) -> Result<Option<Block>, StorageError> {
        match self.get_raw(Column::Blocks, &hash.0)? {
            None => Ok(None),
            Some(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| StorageError::Decode(e.to_string())),
        }
    }

    pub fn get_block_by_number(&self, number: u64) -> Result<Option<Block>, StorageError> {
        let key = block_number_key(number);
        match self.get_raw(Column::BlockNumbers, &key)? {
            None => Ok(None),
            Some(hash_bytes) if hash_bytes.len() == 32 => {
                let hash = zbx_types::H256::from_slice(&hash_bytes);
                self.get_block_by_hash(&hash)
            }
            Some(other) => {
                warn!(len = other.len(), "block_number → hash entry has wrong length");
                Ok(None)
            }
        }
    }

    pub fn get_latest_block_number(&self) -> Option<u64> {
        *self.latest_height.read()
    }

    /// Returns the genesis block (height 0), if persisted.
    pub fn genesis(&self) -> Result<Option<Block>, StorageError> {
        self.get_block_by_number(0)
    }

    // -----------------------------------------------------------------------
    // Transaction operations
    // -----------------------------------------------------------------------

    pub fn put_transaction(&self, tx: &SignedTransaction) -> Result<(), StorageError> {
        let encoded = serde_json::to_vec(tx)
            .map_err(|e| StorageError::Encode(e.to_string()))?;
        let mut batch = WriteBatch::new();
        batch.put(Column::Transactions, tx.hash.0.to_vec(), encoded);
        self.write(batch)
    }

    pub fn get_transaction(&self, hash: &H256) -> Result<Option<SignedTransaction>, StorageError> {
        match self.get_raw(Column::Transactions, &hash.0)? {
            None => Ok(None),
            Some(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| StorageError::Decode(e.to_string())),
        }
    }

    // -----------------------------------------------------------------------
    // Receipt operations
    // -----------------------------------------------------------------------

    pub fn put_receipt(&self, receipt: &TransactionReceipt) -> Result<(), StorageError> {
        let encoded = serde_json::to_vec(receipt)
            .map_err(|e| StorageError::Encode(e.to_string()))?;
        let mut batch = WriteBatch::new();
        batch.put(
            Column::Receipts,
            receipt.transaction_hash.0.to_vec(),
            encoded,
        );
        self.write(batch)
    }

    pub fn get_receipt(&self, tx_hash: &H256) -> Result<Option<TransactionReceipt>, StorageError> {
        match self.get_raw(Column::Receipts, &tx_hash.0)? {
            None => Ok(None),
            Some(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| StorageError::Decode(e.to_string())),
        }
    }

    // -----------------------------------------------------------------------
    // Account state operations
    // -----------------------------------------------------------------------

    pub fn get_account(&self, addr: &Address) -> Result<AccountState, StorageError> {
        let key = state_key(addr.as_bytes());
        match self.get_raw(Column::State, &key)? {
            None => Ok(AccountState::default()),
            Some(b) => serde_json::from_slice(&b)
                .map_err(|e| StorageError::Decode(e.to_string())),
        }
    }

    /// Persist an account directly to the `State` column family.
    ///
    /// SEC-2026-05-09 (Pass-6 C4) — DANGER: this writes the on-disk state
    /// without going through `StateDB`/`StateView` and therefore does NOT
    /// update the in-memory MPT root.  If a caller invokes this outside
    /// the genesis or post-execution-commit paths, the on-disk state
    /// will silently desync from the chain's `state_root` — verifying
    /// peers will reject every block this node produces.
    ///
    /// Authorised callers (audited 2026-05-09):
    /// - `node/src/genesis.rs` — pre-genesis allocation seed
    /// - `node/src/block_producer.rs` — post-execution commit (the diff
    ///   was already routed through `StateDB::set_account`, so the MPT
    ///   root computed there matches the bytes written here)
    ///
    /// Any new caller MUST be reviewed against the same invariant: the
    /// account bytes written here must be IDENTICAL to what
    /// `StateDB::commit()` would have produced for the same diff.
    pub fn put_account(&self, addr: &Address, state: &AccountState) -> Result<(), StorageError> {
        let key = state_key(addr.as_bytes());
        let val = serde_json::to_vec(state)
            .map_err(|e| StorageError::Encode(e.to_string()))?;
        let mut batch = WriteBatch::new();
        batch.put(Column::State, key.to_vec(), val);
        // SEC-2026-05-09 Pass-13 (STORAGE-T0-DURABILITY): account state
        // writes MUST fsync. Pre-Pass-13 this used `write` (no fsync) and
        // the chain tip pointer in `put_block` is fsync'd separately; on
        // crash between the two, the latest_block pointer could advance
        // past unsynced state-diffs, leading to "block exists but state
        // missing" inconsistency on restart.
        self.write_synced(batch)
    }

    // -----------------------------------------------------------------------
    // Atomic block commit (SEC-2026-05-09 Pass-13 STORAGE-T0-ATOMICITY)
    // -----------------------------------------------------------------------

    /// Atomically commit a block + its transactions + receipts + the
    /// latest-height pointer in a SINGLE fsync'd write.
    ///
    /// Pre-Pass-13 the producer called `put_block` then `put_transaction`
    /// for each tx then `put_receipt` for each receipt, in 3 separate
    /// non-atomic writes. A crash between any two of those left the chain
    /// tip ahead of its receipts — RPC returned `null` for confirmed-tx
    /// hashes, indexers double-counted on restart, and a sync peer could
    /// observe a block that referenced receipts that did not yet exist.
    /// This single batched write either commits everything or nothing.
    pub fn commit_block(
        &self,
        block: &Block,
        txs: &[SignedTransaction],
        receipts: &[TransactionReceipt],
    ) -> Result<(), StorageError> {
        // Task #15: hold the pruner read-guard across the chain-tip
        // advance so the pruner cannot start sweeping with a head
        // that points to nodes the block-producer just wrote.
        let _pruner_guard = self.acquire_commit_read_guard();
        let hash = block.hash();
        let number = block.number();
        let block_bytes = serde_json::to_vec(block)
            .map_err(|e| StorageError::Encode(e.to_string()))?;

        let mut batch = WriteBatch::new();
        batch.put(Column::Blocks, hash.0.to_vec(), block_bytes);
        batch.put(
            Column::BlockNumbers,
            block_number_key(number).to_vec(),
            hash.0.to_vec(),
        );
        for tx in txs {
            let enc = serde_json::to_vec(tx)
                .map_err(|e| StorageError::Encode(e.to_string()))?;
            batch.put(Column::Transactions, tx.hash.0.to_vec(), enc);
        }
        for r in receipts {
            let enc = serde_json::to_vec(r)
                .map_err(|e| StorageError::Encode(e.to_string()))?;
            batch.put(Column::Receipts, r.transaction_hash.0.to_vec(), enc);
        }
        // Latest pointer LAST in batch (RocksDB applies in insertion
        // order; the pointer becomes visible only after every other
        // write in the same batch is durable).
        batch.put(
            Column::Metadata,
            META_LATEST.to_vec(),
            number.to_be_bytes().to_vec(),
        );
        self.write_synced(batch)?;

        let mut latest = self.latest_height.write();
        if latest.map(|h| number > h).unwrap_or(true) {
            *latest = Some(number);
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Contract storage
    // -----------------------------------------------------------------------

    /// Reads a 32-byte storage slot. Missing slots return all-zero (EVM semantics).
    /// DB errors are propagated to the caller — they MUST NOT be silently masked.
    pub fn get_storage(&self, addr: &Address, slot: &[u8; 32]) -> Result<[u8; 32], StorageError> {
        let key = storage_key(addr.as_bytes(), slot);
        match self.get_raw(Column::Storage, &key)? {
            None => Ok([0u8; 32]),
            Some(b) if b.len() == 32 => {
                let mut out = [0u8; 32];
                out.copy_from_slice(&b);
                Ok(out)
            }
            Some(other) => Err(StorageError::Decode(format!(
                "storage slot has wrong length: {}",
                other.len()
            ))),
        }
    }

    pub fn put_storage(
        &self,
        addr: &Address,
        slot: &[u8; 32],
        value: [u8; 32],
    ) -> Result<(), StorageError> {
        let key = storage_key(addr.as_bytes(), slot);
        let mut batch = WriteBatch::new();
        batch.put(Column::Storage, key.to_vec(), value.to_vec());
        self.write(batch)
    }

    // -----------------------------------------------------------------------
    // Contract code
    // -----------------------------------------------------------------------

    /// Reads contract bytecode by code hash. Missing entries return an empty
    /// `Vec` (matches EVM semantics for `EXTCODECOPY` of an EOA), but DB
    /// errors are propagated to the caller and never masked.
    pub fn get_code(&self, code_hash: &H256) -> Result<Vec<u8>, StorageError> {
        Ok(self.get_raw(Column::Code, &code_hash.0)?.unwrap_or_default())
    }

    pub fn put_code(&self, code_hash: H256, code: Vec<u8>) -> Result<(), StorageError> {
        let mut batch = WriteBatch::new();
        batch.put(Column::Code, code_hash.0.to_vec(), code);
        self.write(batch)
    }

    // -----------------------------------------------------------------------
    // Metadata helpers
    // -----------------------------------------------------------------------

    pub fn get_metadata(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_raw(Column::Metadata, key)
    }

    pub fn put_metadata(&self, key: &[u8], value: Vec<u8>) -> Result<(), StorageError> {
        let mut batch = WriteBatch::new();
        batch.put(Column::Metadata, key.to_vec(), value);
        self.write(batch)
    }

    /// Write multiple `(key, value)` pairs into the `Metadata` CF as a
    /// SINGLE fsynced RocksDB write batch.
    ///
    /// Both keys land atomically (RocksDB write-batch semantics: all or
    /// nothing on crash), and the batch is fsynced (`write_synced`) so
    /// power loss between the call returning and the next operation
    /// cannot lose either entry. This is the right primitive for
    /// metadata pairs that must move together — e.g. the governance
    /// `VersionRegistry` + `ProposalRegistry` couple, where a torn
    /// half-write would let the chain re-execute an already-applied
    /// proposal on restart.
    pub fn put_metadata_batch_synced(
        &self,
        pairs: &[(&[u8], Vec<u8>)],
    ) -> Result<(), StorageError> {
        let mut batch = WriteBatch::new();
        for (k, v) in pairs {
            batch.put(Column::Metadata, k.to_vec(), v.clone());
        }
        self.write_synced(batch)
    }

    // -----------------------------------------------------------------------
    // Trie-node store (S33-state-root W3b)
    // -----------------------------------------------------------------------

    /// Read a Merkle-Patricia Trie node by its keccak256 hash. Returns
    /// `None` when the node is absent (which a `MutableTrie` will treat as
    /// `MissingNode` if it tries to resolve a hash-linked child).
    ///
    /// Used by `zbx_state::ZbxDbTrieAdapter` to back the canonical
    /// world-state and per-account storage tries.
    pub fn get_trie_node(&self, hash: &H256) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_raw(Column::TrieNodes, &hash.0)
    }

    /// Write a single trie node. Prefer batched writes via the
    /// `ZbxDbTrieAdapter::commit()` path during block execution; this
    /// single-node helper exists for tests, recovery tools, and ad-hoc
    /// admin operations.
    pub fn put_trie_node(&self, hash: H256, node: Vec<u8>) -> Result<(), StorageError> {
        // Task #15: pruner read-guard for the trie-node write.
        let _pruner_guard = self.acquire_commit_read_guard();
        let mut batch = WriteBatch::new();
        batch.put(Column::TrieNodes, hash.0.to_vec(), node);
        self.write(batch)
    }

    /// Stream every (hash, byte_len) entry in `Column::TrieNodes`.
    /// The callback returns `false` to stop iteration early. Used by
    /// the background pruner's sweep phase to avoid materialising the
    /// entire key-set in memory.
    pub fn for_each_trie_node<F>(&self, mut f: F) -> Result<(), StorageError>
    where
        F: FnMut(H256, usize) -> bool,
    {
        let cf = self.cf(Column::TrieNodes)?;
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (k, v) = item.map_err(|e| StorageError::Db(format!("trie iter: {e}")))?;
            if k.len() != 32 {
                continue;
            }
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&k);
            if !f(H256(hash), v.len()) {
                break;
            }
        }
        Ok(())
    }

    /// Delete a batch of trie-node keys atomically. Used by the
    /// background pruner's sweep phase.
    pub fn delete_trie_nodes(&self, hashes: &[H256]) -> Result<(), StorageError> {
        if hashes.is_empty() {
            return Ok(());
        }
        let cf = self.cf(Column::TrieNodes)?;
        let mut wb = RocksWriteBatch::default();
        for h in hashes {
            wb.delete_cf(&cf, h.0);
        }
        self.db
            .write(wb)
            .map_err(|e| StorageError::Db(format!("trie delete batch: {e}")))
    }

    // -----------------------------------------------------------------------
    // SEC-2026-05-09 Pass-11 — slashing pipeline persistence
    //
    // The consensus driver may detect remote validator equivocation
    // (vote-level, NOT just our own on_proposal double-vote). Pre-
    // Pass-11 the evidence was only `tracing::error!`'d as
    // "SLASHABLE" and lost on restart. These CRUD helpers back the
    // `EvidenceStore` in `zbx-staking::persistence`, which feeds
    // `SlashingPipeline` which submits to `SlashingRegistryV2` and
    // ultimately burns stake on the active `ValidatorSet`. The
    // bytes-level format is bincode (compact, schema-stable on
    // append-only enum/struct evolution).
    //
    // We deliberately use `write_synced` for slashing writes — even a
    // sub-second-old equivocation evidence loss could shield a
    // misbehaving validator. fsync cost is acceptable because the
    // detector fires at most a few times per epoch in normal
    // operation.
    // -----------------------------------------------------------------------

    /// Store a raw `EquivocationEvidence` blob keyed by its content
    /// hash (32 bytes). Idempotent — duplicate detections of the same
    /// `(round, phase, validator, vote_a, vote_b)` overwrite with the
    /// same payload. Caller is responsible for computing the hash
    /// (see `zbx-staking::persistence::evidence_id`).
    pub fn put_slashing_evidence(
        &self,
        evidence_hash: &[u8; 32],
        bytes: Vec<u8>,
    ) -> Result<(), StorageError> {
        let mut batch = WriteBatch::new();
        batch.put(Column::SlashingEvidence, evidence_hash.to_vec(), bytes);
        self.write_synced(batch)
    }

    pub fn get_slashing_evidence(
        &self,
        evidence_hash: &[u8; 32],
    ) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_raw(Column::SlashingEvidence, evidence_hash)
    }

    /// Iterate every persisted equivocation evidence blob. Used at
    /// node startup to rehydrate the pipeline (re-submit any
    /// evidence that wasn't yet finalized) and by ops tooling.
    pub fn iter_slashing_evidence(
        &self,
    ) -> Result<Vec<([u8; 32], Vec<u8>)>, StorageError> {
        let cf = self.cf(Column::SlashingEvidence)?;
        let mut out = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (k, v) = item.map_err(|e| StorageError::Db(format!("iter: {e}")))?;
            if k.len() == 32 {
                let mut id = [0u8; 32];
                id.copy_from_slice(&k);
                out.push((id, v.into_vec()));
            }
        }
        Ok(out)
    }

    /// Persist a `SlashEvidenceRecord` (post-submit / post-finalize
    /// snapshot). Caller passes the bincode-encoded record; we store
    /// it under the record's 32-byte ID. Synced for the same
    /// reasoning as `put_slashing_evidence`.
    pub fn put_slashing_record(
        &self,
        record_id: &[u8; 32],
        bytes: Vec<u8>,
    ) -> Result<(), StorageError> {
        let mut batch = WriteBatch::new();
        batch.put(Column::SlashingRecords, record_id.to_vec(), bytes);
        self.write_synced(batch)
    }

    pub fn get_slashing_record(
        &self,
        record_id: &[u8; 32],
    ) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_raw(Column::SlashingRecords, record_id)
    }

    /// Iterate every persisted slashing record. Used by the pipeline
    /// at startup to rebuild in-memory `SlashingRegistryV2` state
    /// (records + epoch_slash_count) without rerunning consensus.
    pub fn iter_slashing_records(
        &self,
    ) -> Result<Vec<([u8; 32], Vec<u8>)>, StorageError> {
        let cf = self.cf(Column::SlashingRecords)?;
        let mut out = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (k, v) = item.map_err(|e| StorageError::Db(format!("iter: {e}")))?;
            if k.len() == 32 {
                let mut id = [0u8; 32];
                id.copy_from_slice(&k);
                out.push((id, v.into_vec()));
            }
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------
    // Slashing bond ledger (full-slashing-upgrade).
    //
    // Persists whistleblower deposits AND appeal bonds keyed by
    // `record_id (32) || reporter_addr (20)` = 52 bytes. fsynced for the
    // same loss-of-equivocation reason as `put_slashing_evidence`.
    // Iteration uses a 32-byte prefix scan on `record_id` so a single
    // finalize / overturn pass can collect every bond attached to one
    // slash without scanning the whole CF.
    // -----------------------------------------------------------------------

    /// Compose a bond key: `record_id (32) || reporter (20)` = 52 bytes.
    /// Exposed for callers that need to test a single (record, reporter)
    /// pair without iterating.
    pub fn slashing_bond_key(record_id: &[u8; 32], reporter: &Address) -> [u8; 52] {
        let mut k = [0u8; 52];
        k[..32].copy_from_slice(record_id);
        k[32..].copy_from_slice(reporter.as_bytes());
        k
    }

    pub fn put_slashing_bond(
        &self,
        record_id: &[u8; 32],
        reporter: &Address,
        bytes: Vec<u8>,
    ) -> Result<(), StorageError> {
        let key = Self::slashing_bond_key(record_id, reporter);
        let mut batch = WriteBatch::new();
        batch.put(Column::SlashingBonds, key.to_vec(), bytes);
        self.write_synced(batch)
    }

    pub fn get_slashing_bond(
        &self,
        record_id: &[u8; 32],
        reporter: &Address,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let key = Self::slashing_bond_key(record_id, reporter);
        self.get_raw(Column::SlashingBonds, &key)
    }

    pub fn delete_slashing_bond(
        &self,
        record_id: &[u8; 32],
        reporter: &Address,
    ) -> Result<(), StorageError> {
        let key = Self::slashing_bond_key(record_id, reporter);
        let mut batch = WriteBatch::new();
        batch.delete(Column::SlashingBonds, key.to_vec());
        self.write_synced(batch)
    }

    /// Enumerate every bond attached to one slash record via a 32-byte
    /// prefix scan on `record_id`. Returns `(reporter, bond_bytes)`
    /// pairs in lexicographic reporter order.
    pub fn iter_slashing_bonds_for_record(
        &self,
        record_id: &[u8; 32],
    ) -> Result<Vec<(Address, Vec<u8>)>, StorageError> {
        let cf = self.cf(Column::SlashingBonds)?;
        let mut out = Vec::new();
        let mode = rocksdb::IteratorMode::From(record_id, rocksdb::Direction::Forward);
        let iter = self.db.iterator_cf(&cf, mode);
        for item in iter {
            let (k, v) = item.map_err(|e| StorageError::Db(format!("iter bonds: {e}")))?;
            // Stop as soon as we leave the record_id prefix.
            if k.len() != 52 || &k[..32] != &record_id[..] {
                break;
            }
            let mut addr = [0u8; 20];
            addr.copy_from_slice(&k[32..]);
            out.push((Address(addr), v.into_vec()));
        }
        Ok(out)
    }

    /// Iterate every persisted slashing bond across all records.
    /// Used at startup to rehydrate the in-memory bond state on the
    /// pipeline restart path (and by ops tooling).
    pub fn iter_all_slashing_bonds(
        &self,
    ) -> Result<Vec<([u8; 32], Address, Vec<u8>)>, StorageError> {
        let cf = self.cf(Column::SlashingBonds)?;
        let mut out = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (k, v) = item.map_err(|e| StorageError::Db(format!("iter bonds: {e}")))?;
            if k.len() != 52 { continue; }
            let mut rid = [0u8; 32];
            rid.copy_from_slice(&k[..32]);
            let mut addr = [0u8; 20];
            addr.copy_from_slice(&k[32..]);
            out.push((rid, Address(addr), v.into_vec()));
        }
        Ok(out)
    }

    // Staking-pipeline persistence: Delegations CF (per-delegator stake
    // ledger) and Unbonding CF (21-day maturity queue). All writes go
    // through write_synced to avoid lost-delegation-on-crash.

    fn delegation_key(validator: &Address, delegator: &Address) -> [u8; 40] {
        let mut k = [0u8; 40];
        k[..20].copy_from_slice(validator.as_bytes());
        k[20..].copy_from_slice(delegator.as_bytes());
        k
    }

    fn unbonding_key(unlock_height: u64, delegator: &Address, validator: &Address) -> [u8; 48] {
        let mut k = [0u8; 48];
        k[..8].copy_from_slice(&unlock_height.to_be_bytes());
        k[8..28].copy_from_slice(delegator.as_bytes());
        k[28..].copy_from_slice(validator.as_bytes());
        k
    }

    /// Read the per-delegator delegation amount (0 if absent).
    pub fn get_delegation(
        &self,
        validator: &Address,
        delegator: &Address,
    ) -> Result<u128, StorageError> {
        let key = Self::delegation_key(validator, delegator);
        match self.get_raw(Column::Delegations, &key)? {
            None => Ok(0),
            Some(b) if b.len() == 16 => {
                let mut buf = [0u8; 16];
                buf.copy_from_slice(&b);
                Ok(u128::from_le_bytes(buf))
            }
            Some(other) => Err(StorageError::Decode(format!(
                "delegation entry has wrong length: {}",
                other.len()
            ))),
        }
    }

    /// Persist a per-delegator delegation amount. A value of `0`
    /// deletes the entry (no zero rows in the ledger).
    pub fn put_delegation(
        &self,
        validator: &Address,
        delegator: &Address,
        amount: u128,
    ) -> Result<(), StorageError> {
        let key = Self::delegation_key(validator, delegator);
        let mut batch = WriteBatch::new();
        if amount == 0 {
            batch.delete(Column::Delegations, key.to_vec());
        } else {
            batch.put(Column::Delegations, key.to_vec(), amount.to_le_bytes().to_vec());
        }
        self.write_synced(batch)
    }

    /// Persist a per-epoch `ValidatorSet` snapshot. Caller serialises with
    /// bincode (see `zbx-staking::ValidatorSet`).
    pub fn put_validator_set(
        &self,
        epoch: u64,
        encoded: Vec<u8>,
    ) -> Result<(), StorageError> {
        let mut batch = WriteBatch::new();
        batch.put(Column::ValidatorSets, epoch.to_be_bytes().to_vec(), encoded);
        self.write_synced(batch)
    }

    /// Read a per-epoch `ValidatorSet` snapshot, or `None` if absent.
    pub fn get_validator_set(&self, epoch: u64) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_raw(Column::ValidatorSets, &epoch.to_be_bytes())
    }

    /// Read a single unbonding entry's amount, or `0` if absent.
    /// Used by the staking handler to accumulate same-block repeated
    /// undelegations on a `(unlock_height, delegator, validator)` key.
    pub fn get_unbonding_entry(
        &self,
        unlock_height: u64,
        delegator: &Address,
        validator: &Address,
    ) -> Result<u128, StorageError> {
        let cf = self.cf(Column::Unbonding)?;
        let key = Self::unbonding_key(unlock_height, delegator, validator);
        match self.db.get_cf(&cf, &key) {
            Ok(Some(v)) if v.len() == 16 => {
                let mut amt = [0u8; 16];
                amt.copy_from_slice(&v);
                Ok(u128::from_le_bytes(amt))
            }
            Ok(_) => Ok(0),
            Err(e) => Err(StorageError::Db(format!("get_cf: {e}"))),
        }
    }

    /// Write an unbonding entry. NOTE: pure write — does NOT accumulate.
    /// Callers that may produce same-`(unlock_height, delegator, validator)`
    /// repeats MUST first call `get_unbonding_entry` and pass the sum.
    /// `dispatch_staking_tx` already does this.
    pub fn put_unbonding_entry(
        &self,
        unlock_height: u64,
        delegator: &Address,
        validator: &Address,
        amount: u128,
    ) -> Result<(), StorageError> {
        let key = Self::unbonding_key(unlock_height, delegator, validator);
        let mut batch = WriteBatch::new();
        batch.put(Column::Unbonding, key.to_vec(), amount.to_le_bytes().to_vec());
        self.write_synced(batch)
    }

    /// Iterate every unbonding entry whose `unlock_height <=
    /// current_height` AND whose delegator matches `who`. Returns
    /// `(unlock_height, validator, amount)` triples — caller passes
    /// these back to `delete_unbonding_entry` after crediting the
    /// delegator's balance.
    pub fn iter_matured_unbondings_for(
        &self,
        who: &Address,
        current_height: u64,
    ) -> Result<Vec<(u64, Address, u128)>, StorageError> {
        let cf = self.cf(Column::Unbonding)?;
        let mut out = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (k, v) = item.map_err(|e| StorageError::Db(format!("iter: {e}")))?;
            if k.len() != 48 || v.len() != 16 {
                continue;
            }
            let mut h = [0u8; 8];
            h.copy_from_slice(&k[..8]);
            let unlock_height = u64::from_be_bytes(h);
            // Keys are sorted lexicographically — once we pass the
            // current-height threshold we can stop.
            if unlock_height > current_height {
                break;
            }
            let mut del = [0u8; 20];
            del.copy_from_slice(&k[8..28]);
            if &del != who.as_bytes() {
                continue;
            }
            let mut vd = [0u8; 20];
            vd.copy_from_slice(&k[28..]);
            let mut amt = [0u8; 16];
            amt.copy_from_slice(&v);
            out.push((unlock_height, Address(vd), u128::from_le_bytes(amt)));
        }
        Ok(out)
    }

    /// Delete a previously-iterated unbonding entry. Caller must
    /// pass the same `(unlock_height, delegator, validator)` triple.
    pub fn delete_unbonding_entry(
        &self,
        unlock_height: u64,
        delegator: &Address,
        validator: &Address,
    ) -> Result<(), StorageError> {
        let key = Self::unbonding_key(unlock_height, delegator, validator);
        let mut batch = WriteBatch::new();
        batch.delete(Column::Unbonding, key.to_vec());
        self.write_synced(batch)
    }

    /// Atomically delete a batch of unbonding entries in a single
    /// fsync'd WriteBatch. Used by Withdraw to avoid partial deletes.
    pub fn delete_unbonding_entries(
        &self,
        delegator: &Address,
        entries: &[(u64, Address)],
    ) -> Result<(), StorageError> {
        let mut batch = WriteBatch::new();
        for (h, validator) in entries {
            let key = Self::unbonding_key(*h, delegator, validator);
            batch.delete(Column::Unbonding, key.to_vec());
        }
        self.write_synced(batch)
    }
    /// Atomically apply a deferred staking write-set in a single
    /// fsync'd `WriteBatch`. Called by the block producer AFTER the
    /// reorg pre-commit check passes and AFTER block accounts/trie are
    /// flushed. Combining all delegation puts/deletes and unbonding
    /// puts/deletes into one batch eliminates the consistency window
    /// where a dropped candidate block could leave staking-side state
    /// drift on disk.
    pub fn apply_staking_delta(
        &self,
        delegations: &[(Address, Address, u128)],
        unbonding_puts: &[(u64, Address, Address, u128)],
        unbonding_deletes: &[(u64, Address, Address)],
    ) -> Result<(), StorageError> {
        let mut batch = WriteBatch::new();
        for (validator, delegator, amount) in delegations {
            let key = Self::delegation_key(validator, delegator);
            if *amount == 0 {
                batch.delete(Column::Delegations, key.to_vec());
            } else {
                batch.put(Column::Delegations, key.to_vec(), amount.to_le_bytes().to_vec());
            }
        }
        for (unlock, delegator, validator, amount) in unbonding_puts {
            let key = Self::unbonding_key(*unlock, delegator, validator);
            batch.put(Column::Unbonding, key.to_vec(), amount.to_le_bytes().to_vec());
        }
        for (unlock, delegator, validator) in unbonding_deletes {
            let key = Self::unbonding_key(*unlock, delegator, validator);
            batch.delete(Column::Unbonding, key.to_vec());
        }
        self.write_synced(batch)
    }
    /// Apply slashing burns directly to on-disk account balances in
    /// a single fsync'd `WriteBatch`. Each `(offender, amount_wei)`
    /// is debited from the offender's `AccountState.balance` (saturating
    /// at zero — over-slash is impossible by registry construction but
    /// safer than a panic). Returns the per-offender actually-burned
    /// amount in input order.
    ///
    /// This bridges the staking pipeline's `apply_slash_burn` (which
    /// debits validator-metadata `self_stake`) into the EVM account
    /// state — without it, a slashed validator would still appear to
    /// hold the full pre-slash balance to every `eth_getBalance` /
    /// transfer / contract call.
    pub fn apply_slash_burns(
        &self,
        burns: &[(Address, u128)],
    ) -> Result<Vec<u128>, StorageError> {
        let mut batch = WriteBatch::new();
        let mut actual = Vec::with_capacity(burns.len());
        for (offender, amount_wei) in burns {
            let mut acct = self.get_account(offender)?;
            let bal = acct.balance_u128();
            let burn = (*amount_wei).min(bal);
            acct.set_balance_u128(bal - burn);
            let key = state_key(offender.as_bytes());
            let val = serde_json::to_vec(&acct)
                .map_err(|e| StorageError::Encode(e.to_string()))?;
            batch.put(Column::State, key.to_vec(), val);
            actual.push(burn);
        }
        self.write_synced(batch)?;
        Ok(actual)
    }
    // -----------------------------------------------------------------------
    // Bridge replay-protection: BridgeSpentOps column family
    //
    // MAINNET-BLOCKER fix: the cross-chain bridge `MultisigAuth` previously
    // kept its `spent_operations` set in process memory only. A relayer
    // restart cleared the set, allowing a relay/replay of any previously
    // executed operation hash.
    //
    // These methods back `zbx_bridge::persistence::BridgeSpentOpsStore`.
    // The write path (`put_bridge_spent_op`) uses `write_synced` (fsync)
    // so the hash survives power-loss.  The startup path
    // (`iter_bridge_spent_ops`) rehydrates the in-memory set on node start.
    //
    // Crash-safety invariant:
    //   `put_bridge_spent_op(h)` is called BEFORE `MultisigAuth::mark_spent(h)`
    //   so that even a crash between the two still leaves `h` in the DB.
    //   On restart `iter_bridge_spent_ops` restores `h` to the in-memory set
    //   and the relayer correctly blocks the replay.
    // -----------------------------------------------------------------------

    /// Durably record a bridge execution hash.
    ///
    /// Must be called inside `BridgeRelayer::execute()` BEFORE
    /// `MultisigAuth::mark_spent()` so the record survives a crash
    /// between the two calls.
    ///
    /// Idempotent — writing the same hash twice is a no-op.
    pub fn put_bridge_spent_op(&self, hash: H256) -> Result<(), StorageError> {
        let mut batch = WriteBatch::new();
        // Value is a single sentinel byte (must be non-empty so the WriteBatch
        // layer does not misinterpret it as a delete).
        batch.put(Column::BridgeSpentOps, hash.0.to_vec(), vec![1u8]);
        self.write_synced(batch)
    }

    /// Load every previously recorded bridge execution hash.
    ///
    /// Called once on node startup to rehydrate `MultisigAuth::spent_operations`
    /// and restore replay protection across restarts.
    pub fn iter_bridge_spent_ops(&self) -> Result<Vec<H256>, StorageError> {
        let cf = self.cf(Column::BridgeSpentOps)?;
        let mut out = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (k, _) = item.map_err(|e| StorageError::Db(format!("bridge_spent_ops iter: {e}")))?;
            if k.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&k);
                out.push(H256(arr));
            }
        }
        Ok(out)
    }
    // ── Governance proposals (H-4 fix: 2026-06-27) ──────────────────────────
    //
    // Pre-fix: proposals lived only in `RpcState::governance_proposals`
    // (Arc<RwLock<HashMap<String, Value>>>). A node restart silently discarded
    // every pending proposal; re-submitters had no way to know their IDs were
    // gone and `zbx_getGovernanceProposal` would permanently return `not_found`.
    //
    // Fix: every `zbx_proposeGovernance` call now writes to this CF with fsync
    // BEFORE updating the in-memory map, so the durable source-of-truth is
    // always RocksDB. On node startup `load_all_governance_proposals` rehydrates
    // the in-memory map.  `zbx_get_governance_proposal` reads from the in-memory
    // cache (fast path); the DB is the recovery source.

    /// Persist a governance proposal. Idempotent — re-submitting the same
    /// `proposal_id` (same title+description+type hash) overwrites with the
    /// same JSON, which is a safe no-op.
    ///
    /// Written with `write_synced` (fsync) so a crash between DB write and
    /// in-memory insert never silently drops a submitted proposal.
    pub fn put_governance_proposal(
        &self,
        proposal_id: &str,
        proposal_json: &serde_json::Value,
    ) -> Result<(), StorageError> {
        let json_bytes = serde_json::to_vec(proposal_json)
            .map_err(|e| StorageError::Db(format!("governance proposal encode: {e}")))?;
        let mut batch = WriteBatch::new();
        batch.put(
            Column::GovernanceProposals,
            proposal_id.as_bytes().to_vec(),
            json_bytes,
        );
        self.write_synced(batch)
    }

    /// Retrieve a single governance proposal by `proposal_id`.
    ///
    /// Returns `None` if no proposal with that ID exists.
    pub fn get_governance_proposal(
        &self,
        proposal_id: &str,
    ) -> Result<Option<serde_json::Value>, StorageError> {
        match self.get_raw(Column::GovernanceProposals, proposal_id.as_bytes())? {
            None => Ok(None),
            Some(bytes) => {
                let val: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|e| StorageError::Db(format!("governance proposal decode: {e}")))?;
                Ok(Some(val))
            }
        }
    }

    /// Load ALL governance proposals from RocksDB.
    ///
    /// Called once on node startup to rehydrate `RpcState::governance_proposals`
    /// so the in-memory cache reflects durable on-disk state.
    pub fn load_all_governance_proposals(
        &self,
    ) -> Result<std::collections::HashMap<String, serde_json::Value>, StorageError> {
        let cf = self.cf(Column::GovernanceProposals)?;
        let mut out = std::collections::HashMap::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (k, v) = item
                .map_err(|e| StorageError::Db(format!("governance_proposals iter: {e}")))?;
            let id = String::from_utf8(k.to_vec())
                .map_err(|e| StorageError::Db(format!("governance proposal key utf8: {e}")))?;
            let val: serde_json::Value = serde_json::from_slice(&v)
                .map_err(|e| StorageError::Db(format!("governance proposal decode '{id}': {e}")))?;
            out.insert(id, val);
        }
        Ok(out)
    }
}  // end impl ZbxDb

fn num_cpus_or(default: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(default)
}
