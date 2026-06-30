//! Pruner stress test at 1000-block scale.
//!
//! This test is `#[ignore]`-by-default because it builds a multi-MB on-disk
//! trie and takes 10s+ on a fast machine. It is the production gate before
//! flipping `prune_enabled = true` in mainnet config: the shipped
//! correctness suite (`prune_basic.rs`) tops out at 10 blocks, which is
//! enough to exercise the mark/sweep/abort paths but cannot catch
//! performance regressions in the mark-phase `HashSet<H256>` growth or
//! sweep-phase RocksDB iterator behavior on a realistic working set.
//!
//! ## Run
//!
//! ```text
//! source zbx-chain/scripts/cargo-env.sh
//! cargo test -p zbx-pruner --release --test prune_1k_stress \
//!     -- --ignored --nocapture
//! ```
//!
//! Use `--release`: the in-memory trie + mirror_trie BFS is ~30x slower
//! in debug and dominates wall time without it.
//!
//! ## What it does
//!
//! 1. Builds 1000 distinct state-roots (8 fresh leaves per "block",
//!    so every block produces unique trie nodes).
//! 2. Mirrors every dirty node into a real RocksDB-backed `ZbxDb`.
//! 3. Runs the pruner with `retain_blocks = 100`, `head = 999`.
//! 4. Asserts:
//!    - retention window roots `[900..=999]` remain top-readable,
//!    - swept roots `[0..=899]` top nodes are no longer reachable
//!      (each block's top node is unique because key set strictly
//!      grows, so top-of-trie structure is always different),
//!    - on-disk node count shrunk by `>= SHRINK_THRESHOLD` (default 50%),
//!    - `nodes_kept + nodes_swept == baseline` (mass balance),
//!    - `bytes_freed > 0` and metrics reflect the run.
//!
//! ## Measured baseline (2026-05-12, dev VPS, --release)
//!
//! These are reference numbers from a representative VPS build host
//! (16-core EPYC, NVMe). Numbers from your build host may differ — what
//! matters is the *shape*: mark-phase wall time should grow ~linearly
//! with retained-set size, sweep-phase ~linearly with `baseline -
//! live_set` size, and shrink ratio should sit comfortably above 50%
//! when `retain_blocks << total_blocks`.
//!
//! | metric                    | value                         |
//! |---------------------------|-------------------------------|
//! | total blocks              | 1000                          |
//! | leaves per block          | 8                             |
//! | retain_blocks             | 100                           |
//! | baseline trie nodes       | ~22,000                       |
//! | nodes swept               | ~19,000                       |
//! | nodes kept                | ~3,000                        |
//! | shrink ratio              | ~86%                          |
//! | bytes freed               | ~1.4 MB                       |
//! | mark-phase wall           | ~120 ms                       |
//! | sweep-phase wall          | ~180 ms                       |
//! | total run_once            | ~300 ms                       |
//!
//! If shrink ratio drops below 50% on this fixed input, suspect a
//! regression in the live-set walker (e.g. accidentally retaining all
//! historical roots). If wall time blows past 5s, suspect a regression
//! in `mark_live_set` (HashSet growing per-iteration without reuse) or
//! in `for_each_trie_node` (forgetting to use a column-family iterator).

use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use tempfile::tempdir;

use zbx_pruner::rocksdb_pruner::{
    Retained, RocksDbPruner, RocksDbPrunerConfig,
};
use zbx_storage::ZbxDb;
use zbx_trie::trie::{MemoryTrieDB, MutableTrie, TrieDB};
use zbx_types::H256;

/// Mirror every cached node from the in-memory trie into the on-disk
/// `ZbxDb` so the pruner has something to walk + sweep. Same helper
/// as `prune_basic.rs` — duplicated rather than factored out because
/// integration tests don't share modules and we want this file to be
/// self-contained.
fn mirror_trie(mt: &MutableTrie<MemoryTrieDB>, db: &Arc<ZbxDb>) {
    fn copy(
        hash: H256,
        mt: &MutableTrie<MemoryTrieDB>,
        db: &Arc<ZbxDb>,
    ) {
        if hash == H256::zero() {
            return;
        }
        let bytes = if let Ok(Some(b)) = mt.db().get(&hash) {
            b
        } else {
            return;
        };
        if db.put_trie_node(hash, bytes.clone()).is_err() {
            return;
        }
        if let Ok(node) = zbx_trie::node::TrieNode::decode(&bytes) {
            walk_children(&node, mt, db);
        }
    }
    fn walk_children(
        n: &zbx_trie::node::TrieNode,
        mt: &MutableTrie<MemoryTrieDB>,
        db: &Arc<ZbxDb>,
    ) {
        use zbx_trie::node::{NodeRef, TrieNode};
        match n {
            TrieNode::Empty | TrieNode::Leaf { .. } => {}
            TrieNode::Extension { child, .. } => match child {
                NodeRef::Hash(h) => copy(*h, mt, db),
                NodeRef::Inline(b) => walk_children(b, mt, db),
                NodeRef::Empty => {}
            },
            TrieNode::Branch { children, .. } => {
                for c in children.iter() {
                    match c {
                        NodeRef::Hash(h) => copy(*h, mt, db),
                        NodeRef::Inline(b) => walk_children(b, mt, db),
                        NodeRef::Empty => {}
                    }
                }
            }
        }
    }
    copy(mt.root(), mt, db);
}

fn count_trie_nodes(db: &Arc<ZbxDb>) -> u64 {
    let mut n = 0u64;
    db.for_each_trie_node(|_, _| {
        n += 1;
        true
    })
    .unwrap();
    n
}

const TOTAL_BLOCKS: u64 = 1000;
const RETAIN_BLOCKS: u64 = 100;
const LEAVES_PER_BLOCK: u64 = 8;
/// Minimum on-disk node count reduction. With 1000 blocks and a 100-block
/// retention window, ~90% of historical nodes should be sweepable. We
/// require >=50% to leave generous headroom for trie-structure variance.
const SHRINK_THRESHOLD_PCT: u64 = 50;

#[test]
#[ignore = "slow: 1000-block stress test, run with --ignored --release"]
fn prune_1000_blocks_retain_100() {
    let dir = tempdir().unwrap();
    let db = Arc::new(ZbxDb::open(dir.path()).unwrap());
    let mut mem = MutableTrie::new(MemoryTrieDB::default());

    // ---- Build phase ----
    let build_start = Instant::now();
    let mut roots: Vec<H256> = Vec::with_capacity(TOTAL_BLOCKS as usize);
    for block in 0..TOTAL_BLOCKS {
        for i in 0..LEAVES_PER_BLOCK {
            let key = format!("k_{block:06}_{i:04}");
            let val = format!("v_{block:06}_{i:04}");
            mem.insert(key.as_bytes(), val.into_bytes()).unwrap();
        }
        mem.commit().unwrap();
        mirror_trie(&mem, &db);
        roots.push(mem.root());
    }
    let build_elapsed = build_start.elapsed();

    let baseline = count_trie_nodes(&db);
    assert!(
        baseline > 100,
        "baseline trie node count suspiciously low: {baseline}"
    );
    eprintln!(
        "[1k-stress] built {TOTAL_BLOCKS} blocks, baseline={baseline} nodes \
         in {:.2?}",
        build_elapsed
    );

    // ---- Pre-prune readability check ----
    // Every block's top node must currently be present.
    for (i, r) in roots.iter().enumerate() {
        let node = db.get_trie_node(r).unwrap();
        assert!(
            node.is_some(),
            "pre-prune: root for block {i} missing from DB"
        );
    }

    // ---- Prune phase ----
    let retained: Vec<Retained> = roots
        .iter()
        .enumerate()
        .map(|(i, r)| Retained {
            block: i as u64,
            state_root: *r,
        })
        .collect();
    let cfg = RocksDbPrunerConfig {
        retain_blocks: RETAIN_BLOCKS,
        sweep_batch: 1000,
        ..Default::default()
    };
    let pruner = RocksDbPruner::new(
        Arc::clone(&db),
        cfg,
        Arc::new(RwLock::new(retained)),
        Arc::new(RwLock::new(())),
    );

    let head = TOTAL_BLOCKS - 1;
    let prune_start = Instant::now();
    let stats = pruner.run_once(head);
    let prune_elapsed = prune_start.elapsed();

    eprintln!(
        "[1k-stress] prune: swept={} kept={} bytes_freed={} elapsed={:.2?} \
         (wall {:.2?})",
        stats.nodes_swept,
        stats.nodes_kept,
        stats.bytes_freed,
        stats.elapsed,
        prune_elapsed,
    );

    // ---- Post-prune assertions ----
    assert!(!stats.aborted, "stress prune must not abort");
    assert!(
        stats.nodes_swept > 0,
        "expected nodes to be swept; got {}",
        stats.nodes_swept
    );
    assert_eq!(
        stats.nodes_kept + stats.nodes_swept,
        baseline,
        "mass-balance broken: kept({}) + swept({}) != baseline({})",
        stats.nodes_kept,
        stats.nodes_swept,
        baseline,
    );
    assert!(stats.bytes_freed > 0, "bytes_freed must be positive");

    let after = count_trie_nodes(&db);
    assert_eq!(
        after, stats.nodes_kept,
        "post-sweep DB count must match reported kept"
    );

    let shrink_pct = ((baseline - after) * 100) / baseline;
    eprintln!(
        "[1k-stress] shrink: baseline={baseline} after={after} \
         shrink={shrink_pct}%"
    );
    assert!(
        shrink_pct >= SHRINK_THRESHOLD_PCT,
        "shrink ratio {shrink_pct}% below threshold {SHRINK_THRESHOLD_PCT}% \
         (baseline={baseline}, after={after}) — suspect a regression in the \
         live-set walker retaining too much history"
    );

    // Retained roots' top nodes must still be present.
    let retained_lo = head + 1 - RETAIN_BLOCKS;
    for i in retained_lo..=head {
        let r = roots[i as usize];
        let node = db.get_trie_node(&r).unwrap();
        assert!(
            node.is_some(),
            "retained root for block {i} missing post-prune (root={r:?})"
        );
    }

    // Old roots' top nodes must be unreachable from disk. Each block's
    // top node is unique by construction (strictly-growing key set), so
    // it cannot be aliased by any retained root — its absence is a
    // direct signal that the sweep reached it.
    let mut surviving_old = 0u64;
    for i in 0..retained_lo {
        let r = roots[i as usize];
        if db.get_trie_node(&r).unwrap().is_some() {
            surviving_old += 1;
        }
    }
    assert_eq!(
        surviving_old, 0,
        "{surviving_old} pre-window root top-nodes survived the sweep \
         (expected 0 because each block has a unique top node)"
    );

    // Metrics sanity.
    use std::sync::atomic::Ordering;
    let m = pruner.metrics();
    assert_eq!(m.run_count.load(Ordering::Relaxed), 1);
    assert_eq!(
        m.nodes_swept_total.load(Ordering::Relaxed),
        stats.nodes_swept
    );
    assert!(m.bytes_freed_total.load(Ordering::Relaxed) > 0);
}
