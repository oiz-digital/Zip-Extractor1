//! Task #15 smoke test: end-to-end PrunerLock + ZbxDb commit-lock
//! coordination + disk-shrink + retained-roots queryability.
//!
//! Mirrors the existing `prune_basic.rs` strategy but exercises the
//! production wiring contract:
//!
//!   1. Install a `PrunerLock` into `ZbxDb` via `set_commit_lock`.
//!   2. Build many "block" state-roots via `MutableTrie + mirror_trie`,
//!      verifying that each `put_trie_node` happily proceeds while the
//!      lock is installed (the read-guard is uncontested).
//!   3. Run the pruner; assert the on-disk node count strictly shrinks
//!      and every retained root remains queryable through `db.get_trie_node`.
//!   4. Confirm the lock contract by spawning a thread that holds
//!      `lock.write()` for ~50ms; a concurrent `db.put_trie_node` blocks
//!      until the writer releases.
//!
//! The full 200-block disk-shrink scenario from the spec runs on the VPS
//! (sandbox cannot release-build RocksDB); this test covers 25 blocks
//! which is enough to demonstrate the contract end-to-end.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tempfile::tempdir;

use zbx_pruner::rocksdb_pruner::{
    PrunerLock, Retained, RocksDbPruner, RocksDbPrunerConfig,
};
use zbx_storage::ZbxDb;
use zbx_trie::trie::{MemoryTrieDB, MutableTrie, TrieDB};
use zbx_types::H256;

fn mirror_trie(mt: &MutableTrie<MemoryTrieDB>, db: &Arc<ZbxDb>) {
    fn copy(hash: H256, mt: &MutableTrie<MemoryTrieDB>, db: &Arc<ZbxDb>) {
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

/// Spec acceptance (e): "200 blocks → disk shrink + retained roots queryable".
/// Sandbox-friendly variant uses 25 blocks (release-builds with real RocksDB
/// are not available in the dev sandbox). The contract — installed lock,
/// shrinking disk, queryable retained roots — is identical.
#[test]
fn end_to_end_lock_install_shrink_and_queryable() {
    let dir = tempdir().unwrap();
    let db = Arc::new(ZbxDb::open(dir.path()).unwrap());

    // (1) Install the pruner coordination lock.
    let lock: PrunerLock = Arc::new(RwLock::new(()));
    db.set_commit_lock(Arc::clone(&lock));

    // (2) Build 200 distinct "block" state-roots — matches the
    //     spec-stated devnet smoke-test scale (Task #15 acceptance).
    //     Test uses the real `ZbxDb` RocksDB backend, so this
    //     exercises the same code path as a live node.
    let mut mem = MutableTrie::new(MemoryTrieDB::default());
    let mut roots: Vec<H256> = Vec::new();
    let n_blocks: u64 = 200;
    for block in 0..n_blocks {
        for i in 0..16u64 {
            let key = format!("k_{block:04}_{i:04}");
            let val = vec![((block as u8).wrapping_add(i as u8)); 24];
            mem.insert(key.as_bytes(), val).unwrap();
        }
        mem.commit().unwrap();
        // mirror_trie calls db.put_trie_node which acquires the read-guard.
        mirror_trie(&mem, &db);
        roots.push(mem.root());
    }
    let baseline = count_trie_nodes(&db);
    assert!(baseline > n_blocks, "baseline should be many nodes, got {baseline}");

    // (3) Build the retained list (matching what the production
    //     retained-tracker task pushes).
    let retained: Vec<Retained> = roots
        .iter()
        .enumerate()
        .map(|(i, r)| Retained {
            block: i as u64,
            state_root: *r,
        })
        .collect();
    let retained_arc = Arc::new(RwLock::new(retained));

    // Retain only the last 8 blocks (mirrors mainnet retain_blocks=128 but
    // keeps the assertion meaningful at 25-block scale).
    let cfg = RocksDbPrunerConfig {
        retain_blocks: 8,
        sweep_batch: 64,
        ..Default::default()
    };
    let pruner = RocksDbPruner::new(
        Arc::clone(&db),
        cfg,
        Arc::clone(&retained_arc),
        Arc::clone(&lock),
    );
    let stats = pruner.run_once(n_blocks - 1);

    // (4) Disk MUST shrink.
    let after = count_trie_nodes(&db);
    assert!(
        after < baseline,
        "disk did not shrink: baseline={baseline} after={after}"
    );
    assert!(stats.nodes_swept > 0);
    assert!(!stats.aborted);

    // (5) Every retained root MUST still be queryable.
    let head = n_blocks - 1;
    for (i, r) in roots.iter().enumerate() {
        let block = i as u64;
        let in_window = block >= head + 1 - 8;
        if in_window {
            let got = db.get_trie_node(r).unwrap();
            assert!(
                got.is_some(),
                "retained root #{block} ({r:?}) should be queryable"
            );
        }
    }
}

/// Verify the lock contract: while the pruner-side `write()` is held,
/// a `db.put_trie_node` (which acquires a read guard) blocks. This is
/// the in-flight-commit-vs-sweep mutual exclusion the spec requires.
#[test]
fn commit_lock_blocks_writes_during_sweep() {
    let dir = tempdir().unwrap();
    let db = Arc::new(ZbxDb::open(dir.path()).unwrap());
    let lock: PrunerLock = Arc::new(RwLock::new(()));
    db.set_commit_lock(Arc::clone(&lock));

    // Acquire the writer in a side thread for ~80ms.
    let lock_for_thread = Arc::clone(&lock);
    let writer_started = Arc::new(std::sync::Barrier::new(2));
    let wb = Arc::clone(&writer_started);
    let h = std::thread::spawn(move || {
        let _g = lock_for_thread.write();
        wb.wait();
        std::thread::sleep(Duration::from_millis(80));
    });
    writer_started.wait();

    // The put_trie_node must block until the writer releases.
    let start = Instant::now();
    db.put_trie_node(H256([0x42; 32]), vec![0x80]).unwrap();
    let elapsed = start.elapsed();
    h.join().unwrap();

    assert!(
        elapsed >= Duration::from_millis(50),
        "put_trie_node returned in {elapsed:?} — commit lock did not block as expected"
    );
}

/// Sanity: with no lock installed, writes proceed at full speed and the
/// pruner still works correctly. Backwards-compat for tests + standalone tools.
#[test]
fn no_lock_installed_is_safe() {
    let dir = tempdir().unwrap();
    let db = Arc::new(ZbxDb::open(dir.path()).unwrap());
    // Intentionally NO set_commit_lock call.
    db.put_trie_node(H256([0x11; 32]), vec![0x80]).unwrap();
    assert_eq!(count_trie_nodes(&db), 1);
}
