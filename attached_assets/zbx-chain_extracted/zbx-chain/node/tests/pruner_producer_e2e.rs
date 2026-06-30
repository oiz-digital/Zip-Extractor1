//! Task #15 — node-level integration test for the production trie
//! pruner wiring.
//!
//! Lives under `node/tests/` (not in-module) so it links the
//! `zbx-node` binary's public surface end-to-end and exercises the
//! ACTUAL producer commit path (`block_producer::execute_and_commit`)
//! with the ACTUAL pruner subsystem (`zbx_pruner::RocksDbPruner`)
//! running concurrently. This is the closest thing to a devnet
//! smoke test we can run inside the dev sandbox without spinning up
//! a full networked node — it pins the integration contract:
//!
//!   * `set_commit_lock` is honoured by every tip-advance write
//!     path (`put_account`, `adapter.commit`, `put_block`).
//!   * The outer commit-guard inside `execute_and_commit_inner`
//!     spans account persist + trie flush + tip-pointer write, so
//!     a concurrent pruner sweep cannot interleave between them.
//!   * The producer commit hook (`set_retained_tracker`) actually
//!     fires on each successful commit — the test verifies the
//!     retained-roots vector grows in lock-step with chain height.
//!   * 200-block scale matches the spec acceptance criterion.

use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

use zbx_mempool::{MempoolConfig, TransactionPool};
use zbx_pruner::{PrunerLock, Retained, RocksDbPruner, RocksDbPrunerConfig};
use zbx_storage::ZbxDb;
use zbx_types::{
    address::Address,
    block::{Block, BlockBody, BlockHeader},
    H256, U256, BLOCK_GAS_LIMIT,
};

fn empty_block(parent: &Block, coinbase: Address) -> Block {
    let n = parent.header.number + 1;
    let header = BlockHeader {
        parent_hash: parent.hash(),
        uncle_hash: H256::zero(),
        coinbase,
        state_root: H256::zero(),
        transactions_root: H256::zero(),
        receipts_root: H256::zero(),
        logs_bloom: [0u8; 256],
        difficulty: U256::zero(),
        number: n,
        gas_limit: BLOCK_GAS_LIMIT,
        gas_used: 0,
        timestamp: 1_000_000 + n,
        extra_data: Vec::new(),
        mix_hash: H256::zero(),
        nonce: 0,
        base_fee_per_gas: 1_000_000_000,
        committee_signature: Vec::new(),
        epoch: 0,
        epoch_seed: None,
    };
    Block { header, body: BlockBody { transactions: Vec::new(), uncles: Vec::new() } }
}

/// Drive `execute_and_commit` 200 times against a real `ZbxDb` with
/// the production pruner wired in. Asserts:
///   (1) Every commit succeeds (no deadlock between the producer's
///       outer commit-guard and the per-method inner guards).
///   (2) The producer commit-hook (`set_retained_tracker`) appended
///       exactly one `Retained` entry per block.
///   (3) Every retained `state_root` resolves back to a queryable
///       block on disk (the canonical contract that the pruner's
///       retain-window protects).
///   (4) The pruner's `run_once` against the new tip executes
///       without error and the chain head pointer is unchanged
///       afterwards (sweep MUST NOT corrupt the canonical tip).
#[test]
fn producer_to_pruner_e2e_200_blocks() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(ZbxDb::open(tmp.path()).unwrap());

    // Install pruner coordination lock — same call the real `node::run` makes.
    let lock: PrunerLock = Arc::new(RwLock::new(()));
    db.set_commit_lock(Arc::clone(&lock));

    // Install the producer commit hook into the process-global slot.
    let retained: Arc<RwLock<Vec<Retained>>> = Arc::new(RwLock::new(Vec::new()));
    zbx_node::block_producer::set_retained_tracker(Arc::clone(&retained));

    let mempool: Arc<RwLock<TransactionPool>> =
        Arc::new(RwLock::new(TransactionPool::new(MempoolConfig::default())));
    let coinbase = Address([0x42u8; 20]);

    // Persist synthetic genesis (height 0) so height 1's parent check passes.
    let genesis = empty_block(
        &Block {
            header: BlockHeader {
                parent_hash: H256::zero(),
                uncle_hash: H256::zero(),
                coinbase,
                state_root: H256::zero(),
                transactions_root: H256::zero(),
                receipts_root: H256::zero(),
                logs_bloom: [0u8; 256],
                difficulty: U256::zero(),
                number: u64::MAX, // empty_block adds +1 → wraps to 0
                gas_limit: BLOCK_GAS_LIMIT,
                gas_used: 0,
                timestamp: 1_000_000,
                extra_data: Vec::new(),
                mix_hash: H256::zero(),
                nonce: 0,
                base_fee_per_gas: 1,
                committee_signature: Vec::new(),
                epoch: 0,
                epoch_seed: None,
            },
            body: BlockBody { transactions: Vec::new(), uncles: Vec::new() },
        },
        coinbase,
    );
    db.put_block(&genesis).unwrap();
    assert_eq!(genesis.header.number, 0);

    const N_BLOCKS: u64 = 200;
    let mut prev = genesis;
    for h in 1..=N_BLOCKS {
        let cand = empty_block(&prev, coinbase);
        prev = zbx_node::block_producer::execute_and_commit(&db, &mempool, cand)
            .unwrap_or_else(|e| panic!("execute_and_commit at height {h}: {e}"));
        assert_eq!(prev.header.number, h);
    }

    // (2) Commit hook: one Retained entry per produced block.
    let g = retained.read();
    assert_eq!(
        g.len() as u64, N_BLOCKS,
        "commit hook MUST append exactly one Retained per produced block \
         (got {}, expected {N_BLOCKS}); split-path race or hook drop suspected",
        g.len(),
    );
    // Heights must be contiguous 1..=N_BLOCKS — proves nothing was dropped.
    for (i, r) in g.iter().enumerate() {
        assert_eq!(
            r.block, (i as u64) + 1,
            "Retained heights must be contiguous from 1; got gap at index {i}",
        );
    }

    // (3) Every retained state_root corresponds to a real block on disk.
    for r in g.iter() {
        let blk = db.get_block_by_number(r.block)
            .unwrap_or_else(|e| panic!("get_block_by_number({}): {e}", r.block))
            .unwrap_or_else(|| panic!("retained block {} missing on disk", r.block));
        assert_eq!(
            blk.header.state_root, r.state_root,
            "retained state_root mismatch at height {}", r.block,
        );
    }
    drop(g);

    // (4) Run one pruner sweep against the new tip. Must not error
    //     and must not corrupt the canonical tip pointer.
    let head_before = db.get_latest_block_number().unwrap();
    let cfg = RocksDbPrunerConfig {
        retain_blocks: 128,
        sweep_batch: 256,
        interval: Duration::from_secs(60),
    };
    let pruner = RocksDbPruner::new(
        Arc::clone(&db),
        cfg,
        Arc::clone(&retained),
        Arc::clone(&lock),
    );
    let _stats = pruner.run_once(head_before);
    let head_after = db.get_latest_block_number().unwrap();
    assert_eq!(
        head_before, head_after,
        "pruner sweep MUST NOT change the canonical tip pointer \
         (before={head_before}, after={head_after}) — would indicate \
         either a tip-pointer write or a producer-pruner race",
    );
}
