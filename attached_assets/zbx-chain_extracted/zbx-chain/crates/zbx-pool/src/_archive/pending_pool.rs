//! PendingPool -- ordered set of ready-to-execute transactions.
//!
//! A transaction is "pending" when:
//!   1. Its nonce equals the sender's current on-chain nonce
//!   2. The sender has enough balance to cover gas + value
//!   3. Its gas price >= current base_fee (EIP-1559)
//!
//! Internally stored in a BTreeMap ordered by effective tip (priority fee).
//! Block proposer calls best_transactions() to get the highest-tip txs first.
//!
//! Per-sender limit: MAX_TX_PER_ACCOUNT = 64 pending txs per sender.
//! This prevents a single sender from monopolizing the pool.

use std::collections::{BTreeMap, HashMap, HashSet};

/// Maximum pending transactions per sender account.
pub const MAX_TX_PER_ACCOUNT: usize = 64;

/// Maximum total size of PendingPool.
pub const MAX_PENDING_POOL_SIZE: usize = 10_000;

// ── Priority key (effective tip, descending) ──────────────────────────────────

/// Sort key: (effective_tip DESC, hash ASC) for deterministic ordering.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Debug)]
struct PriorityKey {
    /// Negated tip so that BTreeMap gives highest tip first.
    pub neg_tip: i128,
    pub hash:    [u8; 32],
}

impl PriorityKey {
    fn new(tip: u128, hash: [u8; 32]) -> Self {
        Self { neg_tip: -(tip as i128), hash }
    }
}

// ── PendingPool ───────────────────────────────────────────────────────────────

/// Ordered pool of ready (pending) transactions.
pub struct PendingPool {
    /// Priority-ordered index: PriorityKey -> tx hash
    pub by_priority: BTreeMap<PriorityKey, [u8; 32]>,
    /// tx hash -> PooledTx
    pub txs:         HashMap<[u8; 32], PooledTx>,
    /// sender -> set of tx hashes (for per-account limit + pruning)
    pub by_sender:   HashMap<[u8; 20], Vec<[u8; 32]>>,
    /// Current base fee (updated each block)
    pub base_fee:    u128,
}

#[derive(Debug, Clone)]
pub struct PooledTx {
    pub hash:             [u8; 32],
    pub from:             [u8; 20],
    pub nonce:            u64,
    pub max_fee_per_gas:  u128,   // EIP-1559 fee cap
    pub max_priority_fee: u128,   // EIP-1559 priority fee (tip)
    pub gas_limit:        u64,
    pub value:            u128,
    pub data:             Vec<u8>,
    pub raw:              Vec<u8>, // RLP-encoded tx
    pub received_at:      u64,    // Unix timestamp
}

impl PooledTx {
    /// Effective miner tip = min(max_priority_fee, max_fee_per_gas - base_fee).
    pub fn effective_tip(&self, base_fee: u128) -> u128 {
        self.max_priority_fee.min(self.max_fee_per_gas.saturating_sub(base_fee))
    }
}

impl PendingPool {
    pub fn new(base_fee: u128) -> Self {
        Self {
            by_priority: BTreeMap::new(),
            txs:         HashMap::new(),
            by_sender:   HashMap::new(),
            base_fee,
        }
    }

    /// Add a transaction to the pending pool.
    /// Returns PendingPoolError if per-account limit exceeded or pool full.
    pub fn add(&mut self, tx: PooledTx) -> Result<(), PendingPoolError> {
        // Per-sender limit check
        let sender_txs = self.by_sender.entry(tx.from).or_insert_with(Vec::new);
        if sender_txs.len() >= MAX_TX_PER_ACCOUNT {
            return Err(PendingPoolError::SenderLimitExceeded {
                sender: tx.from,
                limit:  MAX_TX_PER_ACCOUNT,
            });
        }
        // Pool capacity check
        if self.txs.len() >= MAX_PENDING_POOL_SIZE {
            return Err(PendingPoolError::PoolFull);
        }
        let tip = tx.effective_tip(self.base_fee);
        let key = PriorityKey::new(tip, tx.hash);
        self.by_priority.insert(key, tx.hash);
        sender_txs.push(tx.hash);
        self.txs.insert(tx.hash, tx);
        Ok(())
    }

    /// Remove a specific transaction (mined or cancelled).
    pub fn remove(&mut self, hash: &[u8; 32]) -> Option<PooledTx> {
        let tx = self.txs.remove(hash)?;
        let tip = tx.effective_tip(self.base_fee);
        let key = PriorityKey::new(tip, *hash);
        self.by_priority.remove(&key);
        if let Some(sender_txs) = self.by_sender.get_mut(&tx.from) {
            sender_txs.retain(|h| h != hash);
            if sender_txs.is_empty() { self.by_sender.remove(&tx.from); }
        }
        Some(tx)
    }

    /// Iterate transactions ordered by priority (highest tip first).
    /// Used by block proposer to fill a block.
    pub fn best_transactions(&self) -> impl Iterator<Item = &PooledTx> {
        self.by_priority.values().filter_map(|h| self.txs.get(h))
    }

    /// Update base fee (called each new block; re-sorts if needed).
    pub fn update_base_fee(&mut self, new_base_fee: u128) {
        if new_base_fee == self.base_fee { return; }
        // Rebuild priority index with new base fee
        let hashes: Vec<[u8; 32]> = self.txs.keys().copied().collect();
        self.by_priority.clear();
        for hash in hashes {
            if let Some(tx) = self.txs.get(&hash) {
                let tip = tx.effective_tip(new_base_fee);
                self.by_priority.insert(PriorityKey::new(tip, hash), hash);
            }
        }
        self.base_fee = new_base_fee;
    }

    /// Remove all transactions from a sender (e.g. after nonce reset).
    pub fn prune_sender(&mut self, sender: [u8; 20]) {
        if let Some(hashes) = self.by_sender.remove(&sender) {
            for hash in hashes { self.remove(&hash); }
        }
    }

    /// Post-block prune: remove all mined transactions.
    /// Called after a new block is imported with `mined_txs` list.
    pub fn post_block_prune(&mut self, mined_txs: &[[u8; 32]]) {
        for hash in mined_txs { self.remove(hash); }
    }

    /// Remove txs that are now below base_fee (underpriced after base_fee jump).
    pub fn prune_underpriced(&mut self) {
        let underpriced: Vec<[u8; 32]> = self.txs.iter()
            .filter(|(_, tx)| tx.max_fee_per_gas < self.base_fee)
            .map(|(h, _)| *h)
            .collect();
        for hash in underpriced { self.remove(&hash); }
    }

    pub fn len(&self) -> usize { self.txs.len() }
    pub fn is_empty(&self) -> bool { self.txs.is_empty() }
}

#[derive(Debug)]
pub enum PendingPoolError {
    SenderLimitExceeded { sender: [u8; 20], limit: usize },
    PoolFull,
    AlreadyExists,
}