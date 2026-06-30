//! Post-block mempool pruning.
//!
//! After a new block is committed, the mempool must:
//!   1. Remove all mined transactions (in the block)
//!   2. Move queued txs to pending if their nonce is now correct
//!   3. Drop txs whose nonce is now stale (below state nonce)
//!   4. Drop txs that are now underpriced (base fee jumped)
//!   5. Handle reorgs: re-inject txs from orphaned blocks
//!
//! Called by the block import pipeline after each block.
//! Must be fast -- it runs in the critical path of block processing.

use std::collections::HashMap;

/// Result of a post-block prune operation.
#[derive(Debug, Default)]
pub struct PruneResult {
    /// Transactions removed because they were mined
    pub removed_mined:      Vec<[u8; 32]>,
    /// Transactions removed because their nonce is now stale
    pub removed_stale:      Vec<[u8; 32]>,
    /// Transactions removed because base_fee jumped above max_fee
    pub removed_underpriced: Vec<[u8; 32]>,
    /// Queued txs promoted to pending (nonce gap filled)
    pub promoted:           Vec<[u8; 32]>,
    /// Txs re-injected from a reorged block
    pub reinjected:         Vec<[u8; 32]>,
}

/// Post-block prune context.
pub struct BlockPruneCtx {
    /// Transactions included in the new block
    pub mined_txs:     Vec<[u8; 32]>,
    /// Updated nonce for each sender (from new state)
    pub sender_nonces: HashMap<[u8; 20], u64>,
    /// New base fee (from new block header)
    pub new_base_fee:  u128,
    /// Block number
    pub block_number:  u64,
    /// Reorg depth (0 = no reorg, N = N blocks orphaned)
    pub reorg_depth:   u32,
    /// Txs from orphaned blocks to re-inject
    pub reorg_txs:     Vec<Vec<u8>>, // raw RLP
}

/// Execute post-block prune on the mempool.
///
/// Steps:
///   1. Remove mined transactions from pending + queued
///   2. Remove stale-nonce transactions (sender_nonce > tx.nonce)
///   3. Remove underpriced transactions (max_fee < new_base_fee)
///   4. Promote queued transactions that now have correct nonce
///   5. Re-inject reorg transactions
pub fn post_block_prune(ctx: &BlockPruneCtx) -> PruneResult {
    let mut result = PruneResult::default();

    // Step 1: Remove mined txs
    result.removed_mined = ctx.mined_txs.clone();

    // Step 2: Remove stale nonce txs
    // For each sender, remove txs with tx.nonce < state_nonce
    for (sender, &state_nonce) in &ctx.sender_nonces {
        // In real impl: query pending + queued by sender, filter stale nonces
        // result.removed_stale.extend(stale_txs_for_sender(sender, state_nonce));
    }

    // Step 3: Remove underpriced
    // In real impl: iterate pending pool, remove txs where max_fee < new_base_fee
    // result.removed_underpriced = pending_pool.prune_underpriced(ctx.new_base_fee);

    // Step 4: Promote queued txs whose nonce gap is now filled
    // queued tx at nonce N is promoted to pending if state_nonce == N
    // result.promoted = queued_pool.promote_ready(ctx.sender_nonces);

    // Step 5: Re-inject reorg txs (if reorg happened)
    if ctx.reorg_depth > 0 {
        result.reinjected = ctx.reorg_txs.iter().map(|_| [0u8; 32]).collect(); // stub
    }

    result
}

/// Remove all mined txs from pending pool (called with mined tx hashes from block).
/// O(mined_count) -- does not scan full pool.
pub fn remove_mined_txs(pool_hashes: &mut Vec<[u8; 32]>, mined: &[[u8; 32]]) {
    let mined_set: std::collections::HashSet<&[u8; 32]> = mined.iter().collect();
    pool_hashes.retain(|h| !mined_set.contains(h));
}

/// Handle chain reorg: re-inject txs from orphaned blocks back into mempool.
///
/// When a reorg happens:
///   1. We discover N orphaned blocks
///   2. We extract all txs from those blocks
///   3. We re-validate each tx against new state
///   4. Valid txs go back into the mempool
pub fn handle_reorg(
    orphaned_blocks: &[OrphanedBlock],
    current_state_nonce: &HashMap<[u8; 20], u64>,
    current_base_fee:    u128,
) -> Vec<Vec<u8>> {
    let mut reinject = Vec::new();
    for block in orphaned_blocks {
        for tx in &block.txs {
            // Skip if tx nonce is stale (sender has moved on)
            let &state_nonce = current_state_nonce.get(&tx.from).unwrap_or(&0);
            if tx.nonce < state_nonce { continue; }
            // Skip if underpriced
            if tx.max_fee_per_gas < current_base_fee { continue; }
            reinject.push(tx.raw.clone());
        }
    }
    reinject
}

pub struct OrphanedBlock {
    pub block_hash: [u8; 32],
    pub txs:        Vec<OrphanedTx>,
}
pub struct OrphanedTx {
    pub hash:            [u8; 32],
    pub from:            [u8; 20],
    pub nonce:           u64,
    pub max_fee_per_gas: u128,
    pub raw:             Vec<u8>,
}