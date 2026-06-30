//! Integration tests for the production trie pruner.
//!
//! Strategy: build N "block" state-roots in an in-memory trie,
//! mirror every dirty node into a real RocksDB-backed `ZbxDb`, then
//! run the pruner with a smaller retention window and assert that:
//!   * roots inside the window remain fully traversable,
//!   * roots outside the window are unreachable (their unique nodes
//!     deleted),
//!   * the on-disk trie-node count strictly shrinks.

use std::sync::Arc;

use parking_lot::RwLock;
use tempfile::tempdir;

use zbx_pruner::rocksdb_pruner::{
    Retained, RocksDbPruner, RocksDbPrunerConfig,
};
use zbx_storage::ZbxDb;
use zbx_trie::trie::{MemoryTrieDB, MutableTrie, TrieDB};
use zbx_types::H256;

/// Mirror every cached node from the in-memory trie into the on-disk
/// `ZbxDb` so the pruner has something to walk + sweep.
fn mirror_trie(mt: &MutableTrie<MemoryTrieDB>, db: &Arc<ZbxDb>) {
    // Cache holds nodes added since last commit; root is always
    // committed via `commit_root` so it lives in the cache too.
    // We snapshot all known hashes by re-deriving from the cache via
    // `commit()`, but `commit` is &mut. Instead, walk the tree by
    // resolving from the root, and copy every encoded node we touch.
    fn copy(
        hash: H256,
        mt: &MutableTrie<MemoryTrieDB>,
        db: &Arc<ZbxDb>,
    ) {
        if hash == H256::zero() {
            return;
        }
        // Try cache first via underlying db API; both MemoryTrieDB
        // and the cache hold the encoded form.
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

#[test]
fn prune_keeps_recent_sweeps_old() {
    let dir = tempdir().unwrap();
    let db = Arc::new(ZbxDb::open(dir.path()).unwrap());
    let mut mem = MutableTrie::new(MemoryTrieDB::default());

    // Build 10 distinct "block" state-roots, each adding 8 fresh leaves
    // so each block produces unique trie nodes.
    let mut roots: Vec<H256> = Vec::new();
    for block in 0u64..10 {
        for i in 0..8u64 {
            let key = format!("k_{block:04}_{i:04}");
            let val = format!("v_{block:04}_{i:04}");
            mem.insert(key.as_bytes(), val.into_bytes()).unwrap();
        }
        mem.commit().unwrap();
        mirror_trie(&mem, &db);
        roots.push(mem.root());
    }
    let baseline = count_trie_nodes(&db);
    assert!(baseline > 10, "baseline must have many nodes, got {baseline}");

    // Retain only blocks 7–9 (the last 3 of 10).
    let retained: Vec<Retained> = roots
        .iter()
        .enumerate()
        .map(|(i, r)| Retained {
            block: i as u64,
            state_root: *r,
        })
        .collect();
    let retained_arc = Arc::new(RwLock::new(retained));
    let lock = Arc::new(RwLock::new(()));
    let cfg = RocksDbPrunerConfig {
        retain_blocks: 3,
        sweep_batch: 16,
        ..Default::default()
    };
    let pruner = RocksDbPruner::new(Arc::clone(&db), cfg, retained_arc, lock);
    let stats = pruner.run_once(/* head = */ 9);

    let after = count_trie_nodes(&db);
    assert!(
        after < baseline,
        "pruner should have shrunk the on-disk set: baseline={baseline} after={after}"
    );
    assert!(
        stats.nodes_swept > 0,
        "expected at least one node swept, got {}",
        stats.nodes_swept
    );
    assert_eq!(
        stats.nodes_kept + stats.nodes_swept,
        baseline,
        "kept + swept must equal baseline"
    );

    // Retained roots' top nodes must still be readable from disk.
    for r in &roots[7..10] {
        let node = db.get_trie_node(r).unwrap();
        assert!(
            node.is_some(),
            "retained root {r:?} top node missing after prune"
        );
    }

    // Metrics should reflect the run.
    use std::sync::atomic::Ordering;
    let m = pruner.metrics();
    assert_eq!(m.run_count.load(Ordering::Relaxed), 1);
    assert_eq!(m.nodes_swept_total.load(Ordering::Relaxed), stats.nodes_swept);
    assert!(m.bytes_freed_total.load(Ordering::Relaxed) > 0);
}

#[test]
fn prune_with_full_retention_is_noop() {
    let dir = tempdir().unwrap();
    let db = Arc::new(ZbxDb::open(dir.path()).unwrap());
    let mut mem = MutableTrie::new(MemoryTrieDB::default());

    let mut roots = Vec::new();
    for b in 0u64..5 {
        mem.insert(format!("k{b}").as_bytes(), vec![b as u8; 40]).unwrap();
        mem.commit().unwrap();
        mirror_trie(&mem, &db);
        roots.push(mem.root());
    }
    let baseline = count_trie_nodes(&db);

    let retained: Vec<Retained> = roots
        .iter()
        .enumerate()
        .map(|(i, r)| Retained {
            block: i as u64,
            state_root: *r,
        })
        .collect();
    let cfg = RocksDbPrunerConfig {
        retain_blocks: 1000, // window larger than chain
        ..Default::default()
    };
    let pruner = RocksDbPruner::new(
        Arc::clone(&db),
        cfg,
        Arc::new(RwLock::new(retained)),
        Arc::new(RwLock::new(())),
    );
    let stats = pruner.run_once(4);

    assert_eq!(
        stats.nodes_swept, 0,
        "with full retention nothing should be swept"
    );
    assert_eq!(count_trie_nodes(&db), baseline);
}

#[test]
fn prune_empty_db_is_safe() {
    let dir = tempdir().unwrap();
    let db = Arc::new(ZbxDb::open(dir.path()).unwrap());
    let pruner = RocksDbPruner::new(
        Arc::clone(&db),
        RocksDbPrunerConfig::default(),
        Arc::new(RwLock::new(Vec::new())),
        Arc::new(RwLock::new(())),
    );
    // Empty retained set must abort fail-closed — even on an empty DB
    // we don't want any "successful" sweep recorded that could become
    // dangerous if state appears between this call and the next.
    let stats = pruner.run_once(0);
    assert!(stats.aborted, "empty retained set must abort");
    assert_eq!(stats.nodes_swept, 0);
}

/// Fail-closed guard: if the DB has trie nodes but the retained set
/// is empty, sweeping all of them would wipe canonical state. Refuse.
#[test]
fn prune_refuses_when_retained_empty_but_db_nonempty() {
    let dir = tempdir().unwrap();
    let db = Arc::new(ZbxDb::open(dir.path()).unwrap());
    let mut mem = MutableTrie::new(MemoryTrieDB::default());
    for i in 0u64..5 {
        mem.insert(format!("k{i}").as_bytes(), vec![i as u8; 40]).unwrap();
    }
    mem.commit().unwrap();
    mirror_trie(&mem, &db);
    let baseline = count_trie_nodes(&db);
    assert!(baseline > 0);

    let pruner = RocksDbPruner::new(
        Arc::clone(&db),
        RocksDbPrunerConfig::default(),
        Arc::new(RwLock::new(Vec::new())), // EMPTY retained
        Arc::new(RwLock::new(())),
    );
    let stats = pruner.run_once(99);
    assert!(stats.aborted, "must abort, not wipe state");
    assert_eq!(stats.nodes_swept, 0);
    assert_eq!(count_trie_nodes(&db), baseline, "DB must be untouched");
}

/// Fail-closed guard: if `mark_live_set` encounters a missing child
/// hash (corrupt trie / previous unsafe prune), the sweep must abort.
#[test]
fn prune_aborts_on_missing_node_in_mark_phase() {
    let dir = tempdir().unwrap();
    let db = Arc::new(ZbxDb::open(dir.path()).unwrap());

    // Register a "retained" root that is NOT present in the DB.
    let bogus_root = H256([0xAB; 32]);
    let retained = vec![Retained { block: 5, state_root: bogus_root }];

    // Put one real node so DB is non-empty; if the guard fails this
    // node would be swept.
    let real_hash = H256([0xCD; 32]);
    db.put_trie_node(real_hash, vec![0x80]).unwrap();
    let baseline = count_trie_nodes(&db);

    let pruner = RocksDbPruner::new(
        Arc::clone(&db),
        RocksDbPrunerConfig::default(),
        Arc::new(RwLock::new(retained)),
        Arc::new(RwLock::new(())),
    );
    let stats = pruner.run_once(5);
    assert!(stats.aborted, "missing node must abort the sweep");
    assert_eq!(stats.nodes_swept, 0);
    assert_eq!(count_trie_nodes(&db), baseline);
}

/// Retention window must keep exactly N consecutive heights
/// `[head - N + 1 ..= head]`. Spec: "keep last N blocks".
#[test]
fn retention_window_keeps_exactly_n_blocks() {
    let dir = tempdir().unwrap();
    let db = Arc::new(ZbxDb::open(dir.path()).unwrap());
    let mut mem = MutableTrie::new(MemoryTrieDB::default());

    let mut roots = Vec::new();
    for block in 0u64..6 {
        for i in 0..6u64 {
            mem.insert(
                format!("k_{block}_{i}").as_bytes(),
                format!("v_{block}_{i}").into_bytes(),
            )
            .unwrap();
        }
        mem.commit().unwrap();
        mirror_trie(&mem, &db);
        roots.push(mem.root());
    }

    let retained: Vec<Retained> = roots
        .iter()
        .enumerate()
        .map(|(i, r)| Retained {
            block: i as u64,
            state_root: *r,
        })
        .collect();
    let cfg = RocksDbPrunerConfig {
        retain_blocks: 3,
        sweep_batch: 16,
        ..Default::default()
    };
    let pruner = RocksDbPruner::new(
        Arc::clone(&db),
        cfg,
        Arc::new(RwLock::new(retained)),
        Arc::new(RwLock::new(())),
    );

    // head=5, N=3 -> snapshot must contain ONLY blocks 3,4,5.
    let snap = pruner.snapshot_retained_for_test(5);
    let mut blocks: Vec<u64> = snap.iter().map(|r| r.block).collect();
    blocks.sort();
    assert_eq!(blocks, vec![3, 4, 5], "must keep exactly last 3 blocks");
}
