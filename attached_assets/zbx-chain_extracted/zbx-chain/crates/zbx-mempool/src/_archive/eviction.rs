//! Transaction eviction (expired, low-fee, replaced).

use crate::pool::PendingTx;
use tracing::debug;

/// Evict transactions older than this many seconds.
pub const TTL_SECS: u64 = 3600; // 1 hour

/// Evict transactions with a priority fee below this threshold (wei).
pub const MIN_PRIORITY_FEE: u128 = 1_000_000_000; // 1 Gwei

/// Eviction result.
pub enum EvictReason {
    Expired,
    FeeTooLow,
    Replaced,
    NonceConflict,
}

/// Determine whether a transaction should be evicted.
pub fn should_evict(tx: &PendingTx, now_secs: u64, base_fee: u128) -> Option<EvictReason> {
    if now_secs.saturating_sub(tx.received_at) > TTL_SECS {
        debug!(hash = hex::encode(tx.hash), "Evicting expired transaction");
        return Some(EvictReason::Expired);
    }
    if tx.max_fee_per_gas < base_fee {
        debug!(hash = hex::encode(tx.hash), "Evicting underpriced transaction");
        return Some(EvictReason::FeeTooLow);
    }
    None
}