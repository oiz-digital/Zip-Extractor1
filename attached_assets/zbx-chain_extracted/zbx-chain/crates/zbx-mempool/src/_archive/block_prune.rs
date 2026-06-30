//! Post-block mempool pruning — remove mined transactions, promote queued.

use std::collections::{HashMap, HashSet};
use crate::types::{TxHash, Address};
use crate::mempool::{PendingPool, QueuedPool, PendingTransaction, QueuedTransaction};

/// Prune result
#[derive(Debug, Clone, Default)]
pub struct PruneResult {
    pub removed_mined: usize,
    pub removed_stale: usize,
    pub promoted: usize,
    pub demoted: usize,
    pub total_pending_after: usize,
    pub total_queued_after: usize,
}

/// Post-block pruner
pub struct BlockPruner {
    pub config: PruneConfig,
}

#[derive(Debug, Clone)]
pub struct PruneConfig {
    /// Evict pending txs older than this (seconds)
    pub max_pending_age_secs: u64,
    /// Evict queued txs older than this (seconds)
    pub max_queued_age_secs: u64,
    /// Max nonce gap allowed in queued pool
    pub max_nonce_gap: u64,
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            max_pending_age_secs: 3600,
            max_queued_age_secs: 7200,
            max_nonce_gap: 16,
        }
    }
}

impl BlockPruner {
    pub fn new(config: PruneConfig) -> Self { Self { config } }

    /// Run after a block is imported
    pub fn prune_after_block(
        &self,
        pending: &mut PendingPool,
        queued: &mut QueuedPool,
        mined_hashes: &HashSet<TxHash>,
        new_nonces: &HashMap<Address, u64>, // latest nonce per sender after block
    ) -> PruneResult {
        let mut result = PruneResult::default();

        // 1. Remove mined transactions from pending
        for hash in mined_hashes {
            if pending.remove_by_hash(*hash).is_some() {
                result.removed_mined += 1;
            }
            queued.remove_by_hash(*hash);
        }

        // 2. Remove stale transactions (nonce < current state nonce)
        for (sender, &new_nonce) in new_nonces {
            // Remove pending txs with old nonces
            let stale_pending = pending.remove_below_nonce(*sender, new_nonce);
            result.removed_stale += stale_pending;

            // Remove queued txs with old nonces
            let stale_queued = queued.remove_below_nonce(*sender, new_nonce);
            result.removed_stale += stale_queued;

            // Promote queued -> pending (if nonce gap is now filled)
            let promoted = self.try_promote(pending, queued, *sender, new_nonce);
            result.promoted += promoted;
        }

        // 3. Evict expired transactions
        let now = std::time::Instant::now();
        let max_pending_age = std::time::Duration::from_secs(self.config.max_pending_age_secs);
        let max_queued_age = std::time::Duration::from_secs(self.config.max_queued_age_secs);
        result.removed_stale += pending.evict_expired(now, max_pending_age);
        result.removed_stale += queued.evict_expired(now, max_queued_age);

        // 4. Enforce nonce gap in queued pool
        for (sender, &new_nonce) in new_nonces {
            let removed = queued.enforce_nonce_gap(*sender, new_nonce, self.config.max_nonce_gap);
            result.removed_stale += removed;
        }

        result.total_pending_after = pending.len();
        result.total_queued_after = queued.len();

        tracing::debug!(
            mined = result.removed_mined,
            stale = result.removed_stale,
            promoted = result.promoted,
            pending = result.total_pending_after,
            queued = result.total_queued_after,
            "Pool pruned after block"
        );

        result
    }

    /// Try to promote queued transactions to pending (fill nonce gap)
    fn try_promote(
        &self,
        pending: &mut PendingPool,
        queued: &mut QueuedPool,
        sender: Address,
        current_nonce: u64,
    ) -> usize {
        let mut promoted = 0;
        let mut next_nonce = current_nonce;
        loop {
            if let Some(tx) = queued.remove_exact(sender, next_nonce) {
                pending.add(PendingTransaction {
                    tx: tx.tx.clone(),
                    added_at: tx.added_at,
                    is_local: tx.is_local,
                });
                promoted += 1;
                next_nonce += 1;
            } else {
                break;
            }
        }
        promoted
    }
}