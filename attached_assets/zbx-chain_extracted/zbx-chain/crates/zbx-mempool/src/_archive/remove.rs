//! Transaction removal from the mempool.
//!
//! Transactions are removed from the pool in several scenarios:
//!   1. Included in a block (block import → remove all included txs)
//!   2. Nonce too low (on-chain nonce advanced past tx nonce)
//!   3. Max fee too low (base fee rose above tx's max_fee_per_gas)
//!   4. Expired (in pool > MAX_TX_TTL without being included)
//!   5. Evicted (pool full → lowest-priced tx removed first)
//!
//! # Removal Performance
//! Pool uses a doubly-indexed structure:
//!   - HashMap<TxHash, TxEntry>  → O(1) lookup by hash
//!   - BTreeMap<GasPrice, TxHash> → O(log n) priority ordering
//!
//! remove_transaction() is O(log n) — remove from both indices.

use std::collections::{HashMap, BTreeMap, BTreeSet};
use serde::{Serialize, Deserialize};

/// Maximum time a tx can stay in the mempool: 1 hour
pub const MAX_TX_TTL_SECS: u64 = 3_600;

/// Maximum number of transactions in the pool (DoS protection).
pub const MAX_POOL_SIZE: usize = 100_000;

/// Entry in the tx pool.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxEntry {
    pub hash:          [u8; 32],
    pub sender:        [u8; 20],
    pub nonce:         u64,
    pub max_fee:       u128,
    pub priority_fee:  u128,
    pub gas_limit:     u64,
    pub added_at:      u64,
    pub size_bytes:    usize,
}

/// Reason a transaction was removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RemovalReason {
    /// Included in a block.
    Included { block_hash: [u8; 32], block_number: u64 },
    /// Nonce too low (on-chain nonce overtook this tx).
    StaleNonce { on_chain_nonce: u64, tx_nonce: u64 },
    /// Base fee exceeds tx's max_fee_per_gas.
    UnderPriced { base_fee: u128, max_fee: u128 },
    /// Pool is full — lowest-priced tx evicted.
    Evicted,
    /// Expired after MAX_TX_TTL in pool.
    Expired,
    /// Replaced by higher-fee tx with same nonce.
    Replaced { by_hash: [u8; 32] },
    /// Explicitly cancelled.
    Cancelled,
}

/// Compact, high-performance transaction pool.
pub struct TxPool {
    /// Primary store: tx_hash → TxEntry
    entries:    HashMap<[u8; 32], TxEntry>,
    /// Priority index: (max_fee_per_gas) → BTreeSet<tx_hash>
    by_price:   BTreeMap<u128, BTreeSet<[u8; 32]>>,
    /// Per-sender nonce tracking
    by_sender:  HashMap<[u8; 20], BTreeMap<u64, [u8; 32]>>,
    /// Total gas in pool (for block building estimation)
    total_gas:  u64,
    /// Total size in bytes
    total_size: usize,
}

impl TxPool {
    pub fn new() -> Self {
        Self {
            entries:   HashMap::new(),
            by_price:  BTreeMap::new(),
            by_sender: HashMap::new(),
            total_gas: 0,
            total_size: 0,
        }
    }

    /// Add a transaction to the pool.
    pub fn add_transaction(&mut self, entry: TxEntry) -> Result<(), PoolError> {
        if self.entries.len() >= MAX_POOL_SIZE {
            // Evict lowest-price tx if new tx has higher fee
            if let Some((&lowest_fee, lowest_hashes)) = self.by_price.iter().next() {
                if entry.max_fee > lowest_fee {
                    let evict_hash = *lowest_hashes.iter().next().unwrap();
                    self.remove_transaction(&evict_hash, RemovalReason::Evicted);
                } else {
                    return Err(PoolError::PoolFull);
                }
            }
        }

        let hash = entry.hash;
        self.total_gas  += entry.gas_limit;
        self.total_size += entry.size_bytes;

        self.by_price.entry(entry.max_fee)
            .or_default().insert(hash);
        self.by_sender.entry(entry.sender)
            .or_default().insert(entry.nonce, hash);
        self.entries.insert(hash, entry);

        tracing::debug!(
            hash  = hex::encode(hash),
            pool_size = self.entries.len(),
            "Tx added to pool"
        );
        Ok(())
    }

    /// Remove a single transaction by hash.
    pub fn remove_transaction(
        &mut self,
        hash:   &[u8; 32],
        reason: RemovalReason,
    ) -> Option<TxEntry> {
        let entry = self.entries.remove(hash)?;

        // Remove from price index
        if let Some(hashes) = self.by_price.get_mut(&entry.max_fee) {
            hashes.remove(hash);
            if hashes.is_empty() {
                self.by_price.remove(&entry.max_fee);
            }
        }

        // Remove from sender index
        if let Some(nonces) = self.by_sender.get_mut(&entry.sender) {
            nonces.remove(&entry.nonce);
            if nonces.is_empty() {
                self.by_sender.remove(&entry.sender);
            }
        }

        self.total_gas  = self.total_gas.saturating_sub(entry.gas_limit);
        self.total_size = self.total_size.saturating_sub(entry.size_bytes);

        tracing::debug!(
            hash   = hex::encode(hash),
            reason = format!("{:?}", reason),
            "Tx removed from pool"
        );

        Some(entry)
    }

    /// Remove all transactions included in a block.
    pub fn remove_included(&mut self, tx_hashes: &[[u8; 32]], block_number: u64) -> usize {
        let mut removed = 0;
        for hash in tx_hashes {
            let reason = RemovalReason::Included {
                block_hash:   *hash,
                block_number,
            };
            if self.remove_transaction(hash, reason).is_some() {
                removed += 1;
            }
        }
        tracing::info!(removed = removed, block = block_number, "Block txs removed from pool");
        removed
    }

    /// Evict expired transactions older than MAX_TX_TTL.
    pub fn evict_expired(&mut self, now: u64) -> usize {
        let expired: Vec<[u8; 32]> = self.entries.values()
            .filter(|e| now.saturating_sub(e.added_at) > MAX_TX_TTL_SECS)
            .map(|e| e.hash)
            .collect();
        let count = expired.len();
        for hash in &expired {
            self.remove_transaction(hash, RemovalReason::Expired);
        }
        if count > 0 {
            tracing::info!(count = count, "Expired txs evicted from pool");
        }
        count
    }

    /// Remove all txs from a sender with nonce < on_chain_nonce (stale).
    pub fn remove_stale_nonces(&mut self, sender: [u8; 20], on_chain_nonce: u64) -> usize {
        let stale_hashes: Vec<([u8; 32], u64)> = self.by_sender
            .get(&sender)
            .map(|nonces| {
                nonces.iter()
                    .filter(|(&n, _)| n < on_chain_nonce)
                    .map(|(&n, &h)| (h, n))
                    .collect()
            })
            .unwrap_or_default();

        let count = stale_hashes.len();
        for (hash, stale_nonce) in stale_hashes {
            self.remove_transaction(&hash, RemovalReason::StaleNonce {
                on_chain_nonce,
                tx_nonce: stale_nonce,
            });
        }
        count
    }

    /// Pool status.
    pub fn len(&self) -> usize     { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn total_gas(&self) -> u64 { self.total_gas }

    /// Get a transaction by hash.
    pub fn get(&self, hash: &[u8; 32]) -> Option<&TxEntry> { self.entries.get(hash) }
}

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("pool is full (max {MAX_POOL_SIZE} transactions)")]
    PoolFull,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(hash: u8, sender: u8, nonce: u64, max_fee: u128) -> TxEntry {
        TxEntry {
            hash:         [hash; 32],
            sender:       [sender; 20],
            nonce,
            max_fee,
            priority_fee: 0,
            gas_limit:    21_000,
            added_at:     1000,
            size_bytes:   200,
        }
    }

    #[test]
    fn add_and_get() {
        let mut pool = TxPool::new();
        pool.add_transaction(make_entry(0x01, 0xAA, 0, 2_000_000_000)).unwrap();
        assert_eq!(pool.len(), 1);
        assert!(pool.get(&[0x01; 32]).is_some());
    }

    #[test]
    fn remove_transaction_cleans_all_indices() {
        let mut pool = TxPool::new();
        pool.add_transaction(make_entry(0x01, 0xAA, 0, 2_000_000_000)).unwrap();
        pool.remove_transaction(&[0x01; 32], RemovalReason::Cancelled);
        assert!(pool.is_empty());
        assert!(pool.by_price.is_empty());
        assert!(pool.by_sender.is_empty());
    }

    #[test]
    fn remove_included_block_txs() {
        let mut pool = TxPool::new();
        pool.add_transaction(make_entry(0x01, 0xAA, 0, 2_000_000_000)).unwrap();
        pool.add_transaction(make_entry(0x02, 0xBB, 0, 3_000_000_000)).unwrap();
        let removed = pool.remove_included(&[[0x01; 32], [0x02; 32]], 100);
        assert_eq!(removed, 2);
        assert!(pool.is_empty());
    }

    #[test]
    fn evict_expired_removes_old_txs() {
        let mut pool = TxPool::new();
        pool.add_transaction(make_entry(0x01, 0xAA, 0, 1_000_000_000)).unwrap();
        // now = added_at + TTL + 1
        let evicted = pool.evict_expired(1000 + MAX_TX_TTL_SECS + 1);
        assert_eq!(evicted, 1);
        assert!(pool.is_empty());
    }

    #[test]
    fn stale_nonce_removal() {
        let mut pool = TxPool::new();
        pool.add_transaction(make_entry(0x01, 0xAA, 0, 1_000_000_000)).unwrap();
        pool.add_transaction(make_entry(0x02, 0xAA, 1, 1_000_000_000)).unwrap();
        // On-chain nonce advances to 1 → nonce 0 is stale
        let removed = pool.remove_stale_nonces([0xAA; 20], 1);
        assert_eq!(removed, 1);
        assert_eq!(pool.len(), 1); // nonce 1 still valid
    }
}