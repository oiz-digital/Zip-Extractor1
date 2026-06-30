//! Production trie pruner over `ZbxDb`'s `Column::TrieNodes`.
//!
//! Mark-and-sweep: walk every retained block's `state_root` to build
//! the live-set, then iterate `Column::TrieNodes` and delete keys not
//! in the live-set. We use an exact `HashSet<H256>` rather than a
//! Bloom filter — Bloom false positives keep dead nodes alive (safe
//! but wasteful), but the spec's intent is correctness, and a small
//! per-node cost (~32 B in the set) is acceptable for the 60 s
//! background cadence.
//!
//! When walking an account-trie leaf we also recurse into the
//! account's `storage_root` (index 2 of the RLP-encoded account) so
//! per-account storage tries are pruned alongside their account.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use zbx_rlp::Rlp;
use zbx_storage::ZbxDb;
use zbx_trie::node::{NodeRef, TrieNode};
use zbx_types::H256;

/// Coordination handle shared with the executor. The executor takes
/// the read lock while committing a block; the pruner takes the
/// write lock while sweeping. This lets multiple block reads / RPC
/// queries proceed in parallel with each other but prevents the
/// pruner from deleting a node the executor is mid-flight referencing.
///
/// # Wiring contract (HONEST GAP)
///
/// This crate ships the lock primitive but does **not** itself wire
/// the read-side acquisition into `ZbxDb::commit_block` or
/// `ZbxDbTrieAdapter::commit`. Until node startup (deferred to the
/// `node/src/main.rs` integration task) clones this `PrunerLock` into
/// the block-producer + adapter and wraps every trie-node write in
/// `let _g = lock.read();`, the lock provides no protection — the
/// pruner's `write()` will simply succeed unopposed during a concurrent
/// block commit, with the documented race that a freshly-written node
/// referenced by a still-finalising state-root could be swept before
/// the producer publishes the header. The race window is small (a few
/// μs of disk I/O between `put_trie_node` and `put_block`) and the
/// pruner runs at 60 s cadence, so the failure mode is rare but real.
/// The integration MUST land before any production deployment turns
/// pruning on.
pub type PrunerLock = Arc<RwLock<()>>;

/// Configuration for the background pruner.
#[derive(Debug, Clone)]
pub struct RocksDbPrunerConfig {
    /// Keep this many recent state-roots fully traversable.
    /// Default 128 matches Geth `--gcmode=full`.
    pub retain_blocks: u64,
    /// How often the background loop runs.
    pub interval: Duration,
    /// Sweep batch size — keys per RocksDB write.
    pub sweep_batch: usize,
}

impl Default for RocksDbPrunerConfig {
    fn default() -> Self {
        Self {
            retain_blocks: 128,
            interval: Duration::from_secs(60),
            sweep_batch: 1000,
        }
    }
}

/// A single (block, state_root) checkpoint the pruner must keep
/// reachable.
#[derive(Debug, Clone, Copy)]
pub struct Retained {
    pub block: u64,
    pub state_root: H256,
}

/// Stats for one prune run.
#[derive(Debug, Default, Clone)]
pub struct RunStats {
    pub nodes_swept: u64,
    pub nodes_kept: u64,
    pub bytes_freed: u64,
    pub elapsed: Duration,
    /// True when sweep was aborted by a fail-closed safety guard
    /// (mark-phase error, empty retained set on non-empty DB, etc).
    /// When set, swept/bytes_freed are zero.
    pub aborted: bool,
}

/// Fail-closed safety errors from the mark phase. The sweep is
/// skipped entirely on any of these — better to leak disk than to
/// delete still-reachable state.
#[derive(Debug)]
enum MarkError {
    /// `get_trie_node` returned an I/O error.
    DbRead(String),
    /// `TrieNode::decode` rejected the bytes for a hash already in
    /// the live-set walk.
    DecodeFailed(String),
    /// A hash referenced by a parent node was not present in the DB.
    /// This means the trie is corrupt or a previous prune was unsafe;
    /// either way the live-set is incomplete and we must not sweep.
    MissingNode(H256),
}

/// Prometheus-shaped counters/gauges. Wired into `zbx-metrics` by the
/// node startup; kept dependency-free here so the pruner can be
/// unit-tested in isolation.
#[derive(Debug, Default)]
pub struct PrunerMetrics {
    pub nodes_swept_total: std::sync::atomic::AtomicU64,
    pub bytes_freed_total: std::sync::atomic::AtomicU64,
    pub last_run_duration_ms: std::sync::atomic::AtomicU64,
    pub run_count: std::sync::atomic::AtomicU64,
    /// Count of nodes that were targeted for deletion but whose
    /// `delete_trie_nodes` batch returned an error. Surfaces silent
    /// disk pressure from RocksDB write failures.
    pub delete_failures_total: std::sync::atomic::AtomicU64,
}

impl PrunerMetrics {
    fn record(&self, s: &RunStats) {
        use std::sync::atomic::Ordering;
        self.nodes_swept_total
            .fetch_add(s.nodes_swept, Ordering::Relaxed);
        self.bytes_freed_total
            .fetch_add(s.bytes_freed, Ordering::Relaxed);
        self.last_run_duration_ms
            .store(s.elapsed.as_millis() as u64, Ordering::Relaxed);
        self.run_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Production pruner driver.
pub struct RocksDbPruner {
    db: Arc<ZbxDb>,
    config: RocksDbPrunerConfig,
    /// Live retained set, updated by the chain head as new blocks
    /// finalise. Pruner only reads this.
    retained: Arc<RwLock<Vec<Retained>>>,
    lock: PrunerLock,
    metrics: Arc<PrunerMetrics>,
}

impl RocksDbPruner {
    pub fn new(
        db: Arc<ZbxDb>,
        config: RocksDbPrunerConfig,
        retained: Arc<RwLock<Vec<Retained>>>,
        lock: PrunerLock,
    ) -> Self {
        Self {
            db,
            config,
            retained,
            lock,
            metrics: Arc::new(PrunerMetrics::default()),
        }
    }

    pub fn metrics(&self) -> Arc<PrunerMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Test-only accessor for `snapshot_retained` so integration tests
    /// can verify the retention-window arithmetic without running a
    /// full prune cycle.
    #[doc(hidden)]
    pub fn snapshot_retained_for_test(&self, head: u64) -> Vec<Retained> {
        self.snapshot_retained(head)
    }

    /// Snapshot the retained checkpoints inside the retention window.
    ///
    /// "Keep last N" is interpreted as exactly N consecutive heights
    /// `[head - N + 1 ..= head]`. With `head=9, N=3` this returns
    /// blocks 7,8,9 (3 blocks, not 4).
    fn snapshot_retained(&self, head: u64) -> Vec<Retained> {
        let r = self.retained.read();
        let cutoff = head
            .saturating_add(1)
            .saturating_sub(self.config.retain_blocks);
        r.iter()
            .filter(|c| c.block >= cutoff && c.block <= head)
            .copied()
            .collect()
    }

    /// Compute the live-set fail-closed. Returns `Err` on any DB read
    /// or decode error so the caller can abort the sweep.
    pub fn mark_live_set(&self, head: u64) -> Result<HashSet<H256>, String> {
        let snapshot = self.snapshot_retained(head);
        let mut live: HashSet<H256> = HashSet::new();
        for cp in &snapshot {
            self.walk(cp.state_root, &mut live, /* walk_storage_root */ true)
                .map_err(|e| format!("mark walk failed at root {:?}: {:?}", cp.state_root, e))?;
        }
        Ok(live)
    }

    /// Recursively walk a trie root, marking every reachable node hash.
    /// On account-trie leaves, also recurse into the account's
    /// `storage_root` (idx 2 of the RLP account).
    ///
    /// Fail-closed: any DB read error, decode error, or missing-node
    /// link bubbles up to abort the entire prune cycle. We would rather
    /// leak disk than delete a still-reachable node.
    fn walk(
        &self,
        root: H256,
        live: &mut HashSet<H256>,
        walk_storage_root: bool,
    ) -> Result<(), MarkError> {
        if root == H256::zero() || !live.insert(root) {
            return Ok(());
        }
        let bytes = self
            .db
            .get_trie_node(&root)
            .map_err(|e| MarkError::DbRead(e.to_string()))?
            .ok_or(MarkError::MissingNode(root))?;
        let node = TrieNode::decode(&bytes).map_err(|e| MarkError::DecodeFailed(e.to_string()))?;
        match node {
            TrieNode::Empty => {}
            TrieNode::Leaf { value, .. } => {
                if walk_storage_root {
                    if let Some(sr) = account_storage_root(&value) {
                        self.walk(sr, live, /* nested storage trie */ false)?;
                    }
                }
            }
            TrieNode::Extension { child, .. } => {
                self.descend(child, live, walk_storage_root)?;
            }
            TrieNode::Branch { children, value } => {
                for child in children.iter() {
                    self.descend(child.clone(), live, walk_storage_root)?;
                }
                if walk_storage_root {
                    if let Some(v) = value {
                        if let Some(sr) = account_storage_root(&v) {
                            self.walk(sr, live, false)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn descend(
        &self,
        child: NodeRef,
        live: &mut HashSet<H256>,
        walk_storage_root: bool,
    ) -> Result<(), MarkError> {
        match child {
            NodeRef::Hash(h) => self.walk(h, live, walk_storage_root),
            NodeRef::Inline(boxed) => self.walk_inline(*boxed, live, walk_storage_root),
            NodeRef::Empty => Ok(()),
        }
    }

    /// Inline nodes are not stored separately, but their children may
    /// be hash-linked, so we still need to descend.
    fn walk_inline(
        &self,
        node: TrieNode,
        live: &mut HashSet<H256>,
        walk_storage_root: bool,
    ) -> Result<(), MarkError> {
        match node {
            TrieNode::Empty => {}
            TrieNode::Leaf { value, .. } => {
                if walk_storage_root {
                    if let Some(sr) = account_storage_root(&value) {
                        self.walk(sr, live, false)?;
                    }
                }
            }
            TrieNode::Extension { child, .. } => {
                self.descend(child, live, walk_storage_root)?;
            }
            TrieNode::Branch { children, value } => {
                for child in children.iter() {
                    self.descend(child.clone(), live, walk_storage_root)?;
                }
                if walk_storage_root {
                    if let Some(v) = value {
                        if let Some(sr) = account_storage_root(&v) {
                            self.walk(sr, live, false)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Run one mark-and-sweep cycle. Holds the write coordination
    /// lock for the duration of the sweep batches.
    ///
    /// Fail-closed safety guards (any one aborts the sweep with
    /// `aborted = true` and `swept = 0`):
    ///   * the retained-roots list is empty (would imply "delete
    ///     everything" — refused unconditionally),
    ///   * the snapshot inside the retention window is empty
    ///     (head hasn't caught up to any retained checkpoint yet),
    ///   * `mark_live_set` returned an error (DB read failed, decode
    ///     failed, or a referenced child hash is missing).
    pub fn run_once(&self, head: u64) -> RunStats {
        let start = Instant::now();
        let mut stats = RunStats::default();

        if self.retained.read().is_empty() {
            warn!("pruner: retained set empty — refusing to sweep (fail-closed)");
            stats.aborted = true;
            stats.elapsed = start.elapsed();
            self.metrics.record(&stats);
            return stats;
        }
        if self.snapshot_retained(head).is_empty() {
            warn!(
                head,
                "pruner: no retained roots inside window — refusing to sweep (fail-closed)"
            );
            stats.aborted = true;
            stats.elapsed = start.elapsed();
            self.metrics.record(&stats);
            return stats;
        }
        let live = match self.mark_live_set(head) {
            Ok(set) => set,
            Err(e) => {
                warn!(error = %e, "pruner: mark phase failed — refusing to sweep (fail-closed)");
                stats.aborted = true;
                stats.elapsed = start.elapsed();
                self.metrics.record(&stats);
                return stats;
            }
        };
        debug!(head, live = live.len(), "pruner: mark phase complete");

        let _guard = self.lock.write();

        // Sweep accounting: nodes/bytes are credited to `nodes_swept`
        // and `bytes_freed` only AFTER the batched delete returns Ok.
        // Failed batches are tracked separately so metrics never
        // overstate disk reclaimed.
        let mut to_delete: Vec<H256> = Vec::with_capacity(self.config.sweep_batch);
        let mut pending_bytes: u64 = 0;
        let mut nodes_kept: u64 = 0;
        let mut nodes_swept: u64 = 0;
        let mut bytes_freed: u64 = 0;
        let mut delete_failures: u64 = 0;
        let batch_size = self.config.sweep_batch;
        let db = Arc::clone(&self.db);

        let iter_res = self.db.for_each_trie_node(|h, len| {
            if live.contains(&h) {
                nodes_kept += 1;
                return true;
            }
            to_delete.push(h);
            pending_bytes += len as u64;
            if to_delete.len() >= batch_size {
                let drained = std::mem::take(&mut to_delete);
                let drained_bytes = std::mem::take(&mut pending_bytes);
                let count = drained.len() as u64;
                match db.delete_trie_nodes(&drained) {
                    Ok(()) => {
                        nodes_swept += count;
                        bytes_freed += drained_bytes;
                    }
                    Err(e) => {
                        delete_failures += count;
                        warn!(error = %e, count, "pruner: sweep batch delete failed");
                    }
                }
            }
            true
        });
        if let Err(e) = iter_res {
            warn!(error = %e, "pruner: trie-node iterator failed");
        }
        if !to_delete.is_empty() {
            let count = to_delete.len() as u64;
            let drained_bytes = pending_bytes;
            match self.db.delete_trie_nodes(&to_delete) {
                Ok(()) => {
                    nodes_swept += count;
                    bytes_freed += drained_bytes;
                }
                Err(e) => {
                    delete_failures += count;
                    warn!(error = %e, count, "pruner: final sweep batch delete failed");
                }
            }
        }
        if delete_failures > 0 {
            self.metrics
                .delete_failures_total
                .fetch_add(delete_failures, std::sync::atomic::Ordering::Relaxed);
        }
        stats.nodes_kept = nodes_kept;
        stats.nodes_swept = nodes_swept;
        stats.bytes_freed = bytes_freed;
        stats.elapsed = start.elapsed();
        self.metrics.record(&stats);
        info!(
            head,
            swept = stats.nodes_swept,
            kept = stats.nodes_kept,
            bytes = stats.bytes_freed,
            elapsed_ms = stats.elapsed.as_millis() as u64,
            "pruner: sweep complete"
        );
        stats
    }

    /// Spawn the background loop. Returns the join handle so callers
    /// can `.abort()` on shutdown.
    pub fn spawn<F>(self: Arc<Self>, head_provider: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn() -> u64 + Send + 'static,
    {
        let interval = self.config.interval;
        tokio::spawn(async move {
            loop {
                sleep(interval).await;
                let head = head_provider();
                if head == 0 {
                    continue;
                }
                let me = Arc::clone(&self);
                let stats = tokio::task::spawn_blocking(move || me.run_once(head))
                    .await
                    .unwrap_or_default();
                debug!(?stats, "pruner: background tick");
            }
        })
    }
}

/// Decode an account RLP value and return its `storage_root` (the
/// 3rd element of the 4- or 5-tuple). Returns `None` if the value is
/// not a recognisable account encoding (e.g. a raw storage-slot
/// value, which is just a 32-byte string).
fn account_storage_root(value: &[u8]) -> Option<H256> {
    let rlp = Rlp::new(value);
    if !rlp.is_list() {
        return None;
    }
    let count = rlp.item_count().ok()?;
    if count != 4 && count != 5 {
        return None;
    }
    let sr_bytes: Vec<u8> = rlp.val_at(2).ok()?;
    if sr_bytes.len() != 32 {
        return None;
    }
    let mut h = [0u8; 32];
    h.copy_from_slice(&sr_bytes);
    Some(H256(h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn h(byte: u8) -> H256 {
        H256([byte; 32])
    }

    #[test]
    fn account_storage_root_extracts_idx_2() {
        use zbx_rlp::RlpStream;
        let mut s = RlpStream::new();
        s.begin_list(4);
        s.append(&[1u8][..]); // nonce
        s.append(&[2u8][..]); // balance
        s.append(&h(0x33).0[..]); // storage_root
        s.append(&h(0x44).0[..]); // code_hash
        let bytes = s.out();
        assert_eq!(account_storage_root(&bytes), Some(h(0x33)));
    }

    #[test]
    fn account_storage_root_rejects_non_list() {
        assert!(account_storage_root(&[0x80]).is_none());
        assert!(account_storage_root(&[0u8; 32]).is_none());
    }

    #[test]
    fn account_storage_root_rejects_wrong_arity() {
        use zbx_rlp::RlpStream;
        let mut s = RlpStream::new();
        s.begin_list(3);
        s.append(&[1u8][..]);
        s.append(&[2u8][..]);
        s.append(&[3u8][..]);
        assert!(account_storage_root(&s.out()).is_none());
    }

    #[test]
    fn metrics_record_aggregates() {
        let m = PrunerMetrics::default();
        m.record(&RunStats {
            nodes_swept: 10,
            nodes_kept: 5,
            bytes_freed: 1024,
            elapsed: Duration::from_millis(42),
            aborted: false,
        });
        m.record(&RunStats {
            nodes_swept: 3,
            nodes_kept: 1,
            bytes_freed: 256,
            elapsed: Duration::from_millis(7),
            aborted: false,
        });
        assert_eq!(m.nodes_swept_total.load(Ordering::Relaxed), 13);
        assert_eq!(m.bytes_freed_total.load(Ordering::Relaxed), 1280);
        assert_eq!(m.last_run_duration_ms.load(Ordering::Relaxed), 7);
        assert_eq!(m.run_count.load(Ordering::Relaxed), 2);
    }
}
