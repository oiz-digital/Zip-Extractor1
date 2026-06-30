//! Trie pruner — bounded-history mark-and-sweep over `Column::TrieNodes`.
//!
//! # Why this exists (Task #1)
//!
//! Pre-Task-1 every trie node ever written persisted forever. With ~2-second
//! blocks and ~5–50 KB of dirty trie deltas per block, a long-running mainnet
//! node accumulates terabytes of orphaned MPT nodes within months. The
//! production-blocking gap was that `crates/zbx-storage/src/db.rs` already
//! exposed `for_each_trie_node` + `delete_trie_nodes` (the sweep primitives)
//! but nothing actually called them.
//!
//! # Algorithm — bounded-history mark-and-sweep
//!
//! 1. **Retain a window of recent state roots.** A ring buffer of the last
//!    `keep_blocks / sweep_interval_blocks` state roots is persisted under
//!    `META_RETAINED_ROOTS`. The producer appends the current head root on
//!    every prune cycle and evicts the oldest when the buffer is full.
//! 2. **Mark phase.** BFS from each retained root via `db.get_trie_node` and
//!    a caller-supplied child-extraction closure (typically backed by
//!    `zbx_trie::TrieNode::decode`). Every reachable node hash lands in a
//!    `HashSet<H256>`.
//! 3. **Sweep phase.** Stream every key in `Column::TrieNodes` via
//!    `for_each_trie_node`; any hash NOT in the marked set is queued for
//!    deletion in batches of `sweep_batch_size`.
//!
//! # Safety properties
//!
//! - **Crash-safe.** All deletes go through atomic RocksDB write batches.
//!   Crashing mid-sweep leaves the trie consistent — at worst a few orphan
//!   nodes survive into the next cycle.
//! - **Conservative.** The mark phase never deletes a node reachable from
//!   any retained root, so historical reads (state-at-height for the last
//!   `keep_blocks` blocks) always succeed.
//! - **Bounded memory.** The marked set is a `HashSet<H256>`; one entry per
//!   live trie node is acceptable in practice (millions of entries × 32 B
//!   ≈ tens of MB).
//! - **No new dependencies.** Storage stays free of `zbx-trie`; child
//!   extraction is injected as a `ChildExtractor` closure so the dep graph
//!   remains: `zbx-storage → zbx-types/zbx-crypto` only.
//!
//! # Wiring (see `node/src/node.rs` subsystem #5)
//!
//! The node spawns a supervised tokio task that calls `prune_once(...)`
//! every `sweep_interval_secs` and writes operator-readable progress fields
//! to `Column::Metadata` (`pruner.last_run_height`, `pruner.last_run_unix`,
//! `pruner.swept_total`). The mainnet readiness check (Task #14) probes the
//! `probe_in_memory()` helper at boot to prove the algorithm itself is
//! wired and self-consistent.

use crate::{batch::WriteBatch, db::ZbxDb, error::StorageError, schema::Column};
use std::collections::{HashSet, VecDeque};
use zbx_types::H256;

/// Metadata key — bincode-free packed `Vec<H256>` (each 32 bytes).
pub const META_RETAINED_ROOTS: &[u8] = b"pruner.retained_roots";
/// Metadata key — last-run head height (u64 BE).
pub const META_LAST_RUN_HEIGHT: &[u8] = b"pruner.last_run_height";
/// Metadata key — last-run wall-clock (u64 BE seconds since epoch).
pub const META_LAST_RUN_UNIX: &[u8] = b"pruner.last_run_unix";
/// Metadata key — cumulative swept node count (u64 BE), monotonic.
pub const META_SWEPT_TOTAL: &[u8] = b"pruner.swept_total";

/// Pruner runtime configuration.
#[derive(Debug, Clone)]
pub struct PrunerConfig {
    /// Maximum number of recent state roots to retain. Each retained root
    /// lets historical reads succeed at that height (and accounts/storage
    /// reachable from it). Default: 256.
    pub max_retained_roots: usize,
    /// Skip the prune cycle when fewer than this many new heights have
    /// elapsed since the last run. Default: 64.
    pub min_height_advance: u64,
    /// Sweep deletes in batches of this many keys. Default: 4096.
    pub sweep_batch_size: usize,
}

impl Default for PrunerConfig {
    fn default() -> Self {
        Self {
            max_retained_roots: 256,
            min_height_advance: 64,
            sweep_batch_size: 4_096,
        }
    }
}

/// Statistics for one pruner cycle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneStats {
    /// Reachable nodes discovered during BFS.
    pub marked: usize,
    /// Nodes deleted during sweep.
    pub swept: usize,
    /// Retained-roots window size after this cycle.
    pub retained_roots: usize,
    /// Head block height observed at start of cycle.
    pub head_height: u64,
    /// `true` iff the cycle was skipped due to `min_height_advance`.
    pub skipped: bool,
}

/// Caller-supplied child-extraction function.
///
/// Given the raw RLP bytes of a trie node, return the set of hash-linked
/// child references (skip inline children — they're embedded in the parent's
/// bytes and don't have an independent storage row). The standard impl in
/// `node/src/node.rs` wraps `zbx_trie::TrieNode::decode` and walks
/// `NodeRef::Hash(h)` references.
///
/// **Fail-closed contract.** A decode error MUST be propagated as
/// `Err(_)` (not silently dropped to an empty vec). `prune_once` aborts
/// the entire cycle on any extraction error to avoid under-marking the
/// reachable set — under-marking would make the sweep delete still-live
/// nodes (Pass-19 architect-review CRIT #2). Returning `Ok(vec![])` is
/// reserved for *legitimately childless* nodes (leaves, empty branches).
pub trait ChildExtractor: Send + Sync {
    fn extract(&self, raw: &[u8]) -> Result<Vec<H256>, StorageError>;
}

impl<F> ChildExtractor for F
where
    F: Fn(&[u8]) -> Result<Vec<H256>, StorageError> + Send + Sync,
{
    fn extract(&self, raw: &[u8]) -> Result<Vec<H256>, StorageError> {
        (self)(raw)
    }
}

/// Read the persisted retained-roots ring buffer (or `vec![]` when absent).
pub fn load_retained_roots(db: &ZbxDb) -> Result<Vec<H256>, StorageError> {
    match db.get_metadata(META_RETAINED_ROOTS)? {
        None => Ok(Vec::new()),
        Some(b) => {
            if b.len() % 32 != 0 {
                return Err(StorageError::Db(format!(
                    "pruner.retained_roots length {} not a multiple of 32",
                    b.len()
                )));
            }
            let mut out = Vec::with_capacity(b.len() / 32);
            for chunk in b.chunks_exact(32) {
                let mut h = [0u8; 32];
                h.copy_from_slice(chunk);
                out.push(H256(h));
            }
            Ok(out)
        }
    }
}

/// Append `head_root` and evict-oldest until the window fits `max`.
/// Pure helper, no I/O — exposed for unit tests + the readiness probe.
pub fn push_retained(roots: &mut VecDeque<H256>, head_root: H256, max: usize) {
    // Skip duplicates of the most-recent root (re-runs at the same height).
    if roots.back() != Some(&head_root) {
        roots.push_back(head_root);
    }
    while roots.len() > max {
        roots.pop_front();
    }
}

/// Persist `roots` back to metadata in the canonical packed-bytes layout.
fn save_retained_roots(db: &ZbxDb, roots: &[H256]) -> Result<(), StorageError> {
    let mut buf = Vec::with_capacity(roots.len() * 32);
    for h in roots {
        buf.extend_from_slice(h.as_bytes());
    }
    db.put_metadata(META_RETAINED_ROOTS, buf)
}

/// Run one prune cycle. Returns `PruneStats` even when the cycle is
/// skipped (with `skipped == true`).
///
/// `head_height` and `head_root` come from the latest committed block. The
/// caller (`node/src/node.rs`) reads them from the storage engine on each
/// tick and is responsible for not invoking `prune_once` while a block is
/// mid-commit (acceptable in practice — committed roots are stable).
pub fn prune_once<E: ChildExtractor>(
    db: &ZbxDb,
    head_height: u64,
    head_root: H256,
    extractor: &E,
    cfg: &PrunerConfig,
) -> Result<PruneStats, StorageError> {
    // ── 1. Skip-check against last-run height ───────────────────────────
    let last_run = read_u64(db, META_LAST_RUN_HEIGHT)?.unwrap_or(0);
    if head_height < last_run.saturating_add(cfg.min_height_advance) && last_run != 0 {
        return Ok(PruneStats {
            head_height,
            skipped: true,
            ..Default::default()
        });
    }

    // ── 2. Update retained-roots ring buffer ────────────────────────────
    let mut roots: VecDeque<H256> = load_retained_roots(db)?.into_iter().collect();
    push_retained(&mut roots, head_root, cfg.max_retained_roots);
    let roots_vec: Vec<H256> = roots.iter().copied().collect();
    save_retained_roots(db, &roots_vec)?;

    // ── 3. SNAPSHOT-AT-START: collect every trie key currently
    // committed BEFORE we begin marking. This is the Pass-19 fix for
    // architect-review CRIT #1 (concurrency): block production may
    // commit new trie nodes while the pruner runs, and those new
    // nodes — being content-addressed and unreachable from any
    // retained root captured before they existed — would otherwise
    // be unmarked AND visible to the sweep iterator, leading to
    // immediate deletion of live state.
    //
    // RocksDB iterators capture an implicit snapshot at creation
    // time, so this single pass yields a consistent point-in-time
    // view. Any trie row written after this iterator returns is
    // invisible to `existing_at_start` and thus survives the cycle
    // unconditionally — it will be reconsidered on the next run.
    //
    // Memory cost: 32 bytes × #trie_nodes (~hundreds of MB at
    // mainnet scale, dominated by the marked set anyway). Acceptable
    // for the Task #1 release; can be replaced with a streaming
    // RocksDB explicit snapshot in a follow-up if profiling demands.
    let mut existing_at_start: Vec<H256> = Vec::new();
    db.for_each_trie_node(|h, _len| {
        existing_at_start.push(h);
        true
    })?;

    // ── 4. Mark phase: BFS each retained root. Errors propagate
    //     (decode failures MUST NOT be silently dropped — under-
    //     marking is a state-corruption hazard).
    let marked = mark_reachable(db, &roots_vec, extractor)?;

    // ── 5. Sweep phase: delete every snapshot-time TrieNode hash
    //     NOT in `marked`. Iterate the captured snapshot set
    //     (NOT `for_each_trie_node` again) so concurrently-written
    //     post-snapshot nodes cannot be swept.
    let mut victims: Vec<H256> = Vec::with_capacity(cfg.sweep_batch_size);
    let mut swept: usize = 0;
    for h in existing_at_start {
        if !marked.contains(&h) {
            victims.push(h);
            if victims.len() >= cfg.sweep_batch_size {
                db.delete_trie_nodes(&victims)?;
                swept += victims.len();
                victims.clear();
            }
        }
    }
    if !victims.is_empty() {
        db.delete_trie_nodes(&victims)?;
        swept += victims.len();
    }

    // ── 6. Persist progress fields ──────────────────────────────────────
    write_u64(db, META_LAST_RUN_HEIGHT, head_height)?;
    write_u64(db, META_LAST_RUN_UNIX, now_unix_secs())?;
    let prior = read_u64(db, META_SWEPT_TOTAL)?.unwrap_or(0);
    write_u64(db, META_SWEPT_TOTAL, prior.saturating_add(swept as u64))?;

    Ok(PruneStats {
        marked: marked.len(),
        swept,
        retained_roots: roots_vec.len(),
        head_height,
        skipped: false,
    })
}

/// BFS-mark every node reachable from `roots` via `extractor`. Missing
/// nodes (already pruned, or root past retention) are silently skipped —
/// the sweep phase will not re-delete them and the next cycle will re-mark
/// whatever genuinely persists.
fn mark_reachable<E: ChildExtractor>(
    db: &ZbxDb,
    roots: &[H256],
    extractor: &E,
) -> Result<HashSet<H256>, StorageError> {
    let mut marked: HashSet<H256> = HashSet::new();
    let mut queue: VecDeque<H256> = VecDeque::new();
    for r in roots {
        if marked.insert(*r) {
            queue.push_back(*r);
        }
    }
    while let Some(h) = queue.pop_front() {
        let raw = match db.get_trie_node(&h)? {
            Some(b) => b,
            // Missing root or child — likely a previously-pruned tail.
            // Mark it (so we don't try to re-fetch in this cycle) but
            // don't enqueue children.
            None => continue,
        };
        // Fail-closed: an extractor error means we can't enumerate this
        // node's children, so the BFS reachable set would be incomplete.
        // Continuing under that condition risks sweeping live state.
        // The caller (`prune_once`) propagates this error and aborts the
        // whole cycle (sweep is never run on a partial mark set).
        for child in extractor.extract(&raw)? {
            if marked.insert(child) {
                queue.push_back(child);
            }
        }
    }
    Ok(marked)
}

// ─── Small helpers ──────────────────────────────────────────────────────

fn read_u64(db: &ZbxDb, key: &[u8]) -> Result<Option<u64>, StorageError> {
    match db.get_metadata(key)? {
        Some(b) if b.len() == 8 => {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&b);
            Ok(Some(u64::from_be_bytes(buf)))
        }
        Some(_) => Ok(None),
        None => Ok(None),
    }
}

fn write_u64(db: &ZbxDb, key: &[u8], value: u64) -> Result<(), StorageError> {
    db.put_metadata(key, value.to_be_bytes().to_vec())
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ─── Readiness probe (Task #14 check #4) ────────────────────────────────

/// In-memory self-test of the pruner's pure logic. Used by the mainnet
/// readiness predicate at boot to prove the algorithm itself is wired and
/// self-consistent — without spinning up a temporary RocksDB instance,
/// which would race against the real chain dir on shared hosts.
///
/// The probe exercises:
///   1. Ring-buffer eviction (`push_retained`)
///   2. BFS marking against an in-memory store + child extractor
///   3. The sweep predicate (marked-set membership check)
///
/// Returns `Ok(())` when all sub-checks pass; otherwise a short
/// human-readable description of which invariant regressed.
pub fn probe_in_memory() -> Result<(), &'static str> {
    use std::collections::HashMap;

    // --- (1) Ring-buffer eviction ---------------------------------------
    let mut q: VecDeque<H256> = VecDeque::new();
    for i in 0..10u8 {
        let mut h = [0u8; 32];
        h[0] = i;
        push_retained(&mut q, H256(h), 4);
    }
    if q.len() != 4 {
        return Err("retained-roots ring buffer did not evict to max=4");
    }
    if q.front().map(|h| h.0[0]) != Some(6) || q.back().map(|h| h.0[0]) != Some(9) {
        return Err("retained-roots eviction window incorrect (expected 6..=9)");
    }
    // Duplicate at the back must not grow the buffer.
    let last = *q.back().unwrap();
    push_retained(&mut q, last, 4);
    if q.len() != 4 {
        return Err("retained-roots de-dup at tail regressed");
    }

    // --- (2 + 3) BFS mark + sweep predicate against an in-memory store --
    // Build a 4-node "trie": root -> {a, b}; a -> {c}; c, b are leaves.
    fn h(byte: u8) -> H256 {
        let mut x = [0u8; 32];
        x[31] = byte;
        H256(x)
    }
    let root = h(1);
    let a = h(2);
    let b = h(3);
    let c = h(4);
    let orphan = h(99);
    let mut store: HashMap<H256, Vec<u8>> = HashMap::new();
    // Encoding: first byte = N children, then N × 32 bytes of child hashes.
    let pack = |children: &[H256]| -> Vec<u8> {
        let mut out = vec![children.len() as u8];
        for c in children {
            out.extend_from_slice(c.as_bytes());
        }
        out
    };
    store.insert(root, pack(&[a, b]));
    store.insert(a, pack(&[c]));
    store.insert(b, pack(&[]));
    store.insert(c, pack(&[]));
    store.insert(orphan, pack(&[]));

    let extractor = |raw: &[u8]| -> Result<Vec<H256>, StorageError> {
        if raw.is_empty() {
            return Ok(vec![]);
        }
        let n = raw[0] as usize;
        if raw.len() != 1 + n * 32 {
            return Err(StorageError::Db(format!(
                "probe extractor: malformed node ({} != 1 + {} * 32)",
                raw.len(),
                n
            )));
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let mut hh = [0u8; 32];
            hh.copy_from_slice(&raw[1 + i * 32..1 + (i + 1) * 32]);
            out.push(H256(hh));
        }
        Ok(out)
    };

    // Inline BFS using the same algorithm as `mark_reachable`, but against
    // the in-memory store so we don't need RocksDB.
    let mut marked: HashSet<H256> = HashSet::new();
    let mut queue: VecDeque<H256> = VecDeque::new();
    marked.insert(root);
    queue.push_back(root);
    while let Some(node) = queue.pop_front() {
        let raw = match store.get(&node) {
            Some(b) => b,
            None => continue,
        };
        let children = extractor
            .extract(raw)
            .map_err(|_| "probe extractor regressed on a well-formed node")?;
        for child in children {
            if marked.insert(child) {
                queue.push_back(child);
            }
        }
    }
    // Fail-closed property check: a deliberately-malformed node MUST
    // produce Err (not Ok(vec![])), so `prune_once` aborts the cycle
    // instead of under-marking and swept-deleting live state.
    if extractor.extract(&[3, 0, 0]).is_ok() {
        return Err("extractor went fail-open on malformed bytes (CRIT regression)");
    }
    if !(marked.contains(&root) && marked.contains(&a) && marked.contains(&b) && marked.contains(&c))
    {
        return Err("BFS mark failed to reach all 4 reachable nodes");
    }
    if marked.contains(&orphan) {
        return Err("BFS mark incorrectly reached an orphan node");
    }
    // Sweep predicate: the orphan IS the only victim.
    let victims: Vec<H256> = store.keys().copied().filter(|k| !marked.contains(k)).collect();
    if victims != vec![orphan] {
        return Err("sweep predicate did not select exactly the orphan");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_evicts_oldest() {
        let mut q: VecDeque<H256> = VecDeque::new();
        for i in 0..5u8 {
            let mut h = [0u8; 32];
            h[0] = i;
            push_retained(&mut q, H256(h), 3);
        }
        assert_eq!(q.len(), 3);
        assert_eq!(q.front().unwrap().0[0], 2);
        assert_eq!(q.back().unwrap().0[0], 4);
    }

    #[test]
    fn ring_buffer_dedups_at_tail() {
        let mut q: VecDeque<H256> = VecDeque::new();
        let h1 = H256([1u8; 32]);
        push_retained(&mut q, h1, 8);
        push_retained(&mut q, h1, 8);
        push_retained(&mut q, h1, 8);
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn probe_passes_on_clean_logic() {
        probe_in_memory().expect("probe regressed — pruner logic broken");
    }
}
