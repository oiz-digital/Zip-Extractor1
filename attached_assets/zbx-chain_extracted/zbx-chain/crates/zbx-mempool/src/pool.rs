//! Transaction pool implementation.

use crate::{error::MempoolError, nonce_tracker::NonceTracker};
use std::collections::{BTreeMap, HashMap};
use zbx_types::{
    address::Address,
    block::BlockHeader,
    transaction::SignedTransaction,
    H256, BLOCK_GAS_LIMIT,
};
use tracing::{debug, info, warn};

/// SEC-2026-05-09 (R2): per-tx wei cost = `value + gas_limit * max_fee`.
/// Saturating arithmetic so a tx with bogus oversized fields can't panic.
fn slot_cost(tx: &SignedTransaction) -> u128 {
    let max_cost = (tx.tx.max_fee_per_gas as u128).saturating_mul(tx.tx.gas_limit as u128);
    let v = if tx.tx.value.bits() > 128 { u128::MAX } else { tx.tx.value.low_u128() };
    max_cost.saturating_add(v)
}

/// Configuration for the mempool.
#[derive(Debug, Clone)]
pub struct MempoolConfig {
    pub max_pending: usize,
    pub max_queued: usize,
    pub min_gas_tip: u64,
    pub max_tx_size: usize,
    /// SEC-2026-05-09 (R2): hard cap on combined pending+queued slots a
    /// single sender may occupy. Prevents one address from monopolising the
    /// pool with thousands of cheap future-nonce txs.
    pub max_slots_per_sender: usize,
    /// SEC-2026-05-09 Pass-13 (mempool T1-NONCE-GAP): hard ceiling on
    /// `tx.nonce - sender_on_chain_nonce`. A future-nonce tx beyond this
    /// gap can never be promoted (gap will never close in normal
    /// operation) and is memory-DoS bait. 256 mirrors geth's default
    /// `--txpool.queue` window.
    pub max_nonce_gap: u64,
}

impl Default for MempoolConfig {
    fn default() -> Self {
        MempoolConfig {
            max_pending: 5_000,
            max_queued: 2_000,
            min_gas_tip: 1_000_000, // 0.001 Gwei minimum tip
            max_tx_size: 128 * 1024, // 128 KB
            max_slots_per_sender: 64,
            max_nonce_gap: 256,
        }
    }
}

/// Pending transaction slot with priority key.
#[derive(Debug, Clone)]
struct PendingSlot {
    tx: SignedTransaction,
    priority: u64, // effective tip in wei/gas
}

/// The main transaction pool.
pub struct TransactionPool {
    config: MempoolConfig,
    /// Pending txs: (sender, nonce) → slot
    pending: HashMap<(Address, u64), PendingSlot>,
    /// Queued txs (future nonce): (sender, nonce) → tx
    queued: HashMap<(Address, u64), SignedTransaction>,
    /// All known tx hashes → (sender, nonce) for dedup
    known: HashMap<H256, (Address, u64)>,
    nonce_tracker: NonceTracker,
    /// Current network base fee (EIP-1559).
    base_fee: u64,
}

impl TransactionPool {
    pub fn new(config: MempoolConfig) -> Self {
        TransactionPool {
            config,
            pending: HashMap::new(),
            queued: HashMap::new(),
            known: HashMap::new(),
            nonce_tracker: NonceTracker::new(),
            base_fee: 1_000_000_000, // 1 Gwei default
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn queued_count(&self) -> usize {
        self.queued.len()
    }

    pub fn total_count(&self) -> usize {
        self.pending.len() + self.queued.len()
    }

    /// SEC-2026-05-09 (R2): count combined pending+queued slots for a sender.
    fn sender_slot_count(&self, addr: &Address) -> usize {
        self.pending.keys().filter(|(a, _)| a == addr).count()
            + self.queued.keys().filter(|(a, _)| a == addr).count()
    }

    /// SEC-2026-05-09 (R2): sum of `value + gas_limit*max_fee` for every
    /// currently pooled tx from `addr`. Used to enforce that the sender
    /// can actually afford the cumulative cost of all in-flight txs, not
    /// just each individual one.
    fn sender_cumulative_cost(&self, addr: &Address) -> u128 {
        let cost_of = |tx: &SignedTransaction| -> u128 {
            let max_cost = (tx.tx.max_fee_per_gas as u128) * (tx.tx.gas_limit as u128);
            let v = if tx.tx.value.bits() > 128 { u128::MAX } else { tx.tx.value.low_u128() };
            max_cost.saturating_add(v)
        };
        let mut total: u128 = 0;
        for ((a, _), slot) in &self.pending {
            if a == addr {
                total = total.saturating_add(cost_of(&slot.tx));
            }
        }
        for ((a, _), tx) in &self.queued {
            if a == addr {
                total = total.saturating_add(cost_of(tx));
            }
        }
        total
    }

    /// Update the base fee from the latest block header.
    pub fn update_base_fee(&mut self, header: &BlockHeader) {
        self.base_fee = header.base_fee_per_gas;
    }

    /// Add a new transaction to the pool.
    pub fn add_transaction(
        &mut self,
        tx: SignedTransaction,
        sender_balance: u128,
        sender_on_chain_nonce: u64,
    ) -> Result<H256, MempoolError> {
        let hash = tx.hash;

        // Dedup check
        if self.known.contains_key(&hash) {
            return Err(MempoolError::AlreadyKnown(hex::encode(hash)));
        }

        // SEC-2026-05-09 Pass-15 (CRIT-04): cryptographic signature
        // verification at admission. Pre-fix `add_transaction` checked
        // balance / nonce / fee / intrinsic gas but NEVER recovered
        // the ECDSA signer from the signature, so any caller could
        // submit a `SignedTransaction` with an arbitrary `tx.from`
        // address, an arbitrary signature, and any properly-formed
        // payload — the pool would happily accept it and forward to
        // every peer. Forged-tx flooding for any high-balance address
        // (chain-wide gossip amplification, mempool DoS, plus
        // "phantom-tx" UX corruption in explorers) was trivial.
        // Now: verify_hash() ensures the cached hash matches the
        // payload, then recover_signer() recovers the ECDSA signer
        // from `tx.signing_hash()` and asserts it matches `tx.from`.
        if !tx.verify_hash() {
            return Err(MempoolError::InvalidSignature(
                "cached tx hash does not match recomputed hash".into(),
            ));
        }
        let signing_hash = tx.tx.signing_hash();
        // Bridge the two `Signature` types — zbx_types::Signature is the
        // wire-level form on the tx, zbx_crypto::Signature is the
        // recovery form expected by `recover_signer`. Same byte layout.
        let crypto_sig = zbx_crypto::Signature::from_bytes(&tx.sig.to_bytes())
            .map_err(|e| MempoolError::InvalidSignature(format!("sig parse: {e}")))?;
        let recovered = zbx_crypto::secp256k1::recover_signer(&signing_hash, &crypto_sig)
            .map_err(|e| MempoolError::InvalidSignature(format!("recover failed: {e}")))?;
        if recovered != tx.from {
            return Err(MempoolError::InvalidSignature(format!(
                "signer mismatch: recovered {:?} != tx.from {:?}",
                recovered, tx.from
            )));
        }

        // Basic validation
        if tx.tx.gas_limit > BLOCK_GAS_LIMIT {
            return Err(MempoolError::GasLimitTooHigh {
                gas_limit: tx.tx.gas_limit,
                block_limit: BLOCK_GAS_LIMIT,
            });
        }

        let tip = tx.effective_gas_price(self.base_fee)
            .saturating_sub(self.base_fee);
        if tip < self.config.min_gas_tip {
            return Err(MempoolError::FeeTooLow {
                tip,
                min: self.config.min_gas_tip,
            });
        }

        // SEC-2026-05-09 Pass-12 (mempool H1): intrinsic gas precheck.
        // Without this, a tx with `gas_limit < 21000` (or `< 53000` for
        // CREATE) would be admitted, then immediately fail at execution
        // with no fee paid — a free DoS slot that returns to the pool on
        // every restart. Same bound the EVM applies, enforced one layer up.
        let intrinsic = tx.tx.intrinsic_gas();
        if tx.tx.gas_limit < intrinsic {
            return Err(MempoolError::IntrinsicGasTooLow {
                addr: tx.from,
                gas_limit: tx.tx.gas_limit,
                intrinsic,
            });
        }

        // Cost check: value + gas_limit * max_fee
        let max_cost = (tx.tx.max_fee_per_gas as u128) * (tx.tx.gas_limit as u128);
        // Saturate U256 → u128: any overflow already exceeds total balance.
        let value_u128 = if tx.tx.value.bits() > 128 { u128::MAX } else { tx.tx.value.low_u128() };
        let total_cost = max_cost.saturating_add(value_u128);
        if total_cost > sender_balance {
            return Err(MempoolError::InsufficientBalance {
                balance: sender_balance,
                cost: total_cost,
            });
        }

        let addr = tx.from;
        let nonce = tx.tx.nonce;

        self.nonce_tracker.set_on_chain(addr, sender_on_chain_nonce);
        let expected = self.nonce_tracker.next_nonce(&addr);

        if nonce < sender_on_chain_nonce {
            return Err(MempoolError::NonceTooLow {
                addr,
                expected: sender_on_chain_nonce,
                got: nonce,
            });
        }

        // SEC-2026-05-09 Pass-13 (mempool T1-NONCE-GAP): reject txs whose
        // nonce sits more than `max_nonce_gap` slots ahead of the
        // sender's on-chain nonce. Without this cap, a sender could submit
        // a tx with nonce = u64::MAX which would never be promoted but
        // would still occupy a queued slot + the cumulative-balance
        // reservation budget for the lifetime of the pool.
        let gap = nonce.saturating_sub(sender_on_chain_nonce);
        if gap > self.config.max_nonce_gap {
            return Err(MempoolError::NonceGapTooLarge {
                addr,
                nonce,
                on_chain: sender_on_chain_nonce,
                max_gap: self.config.max_nonce_gap,
            });
        }

        let key = (addr, nonce);

        // SEC-2026-05-09 (R2): per-sender slot cap. Replacement (same key)
        // does not count as a new slot — the old one will be evicted.
        let is_replacement =
            self.pending.contains_key(&key) || self.queued.contains_key(&key);
        if !is_replacement {
            let used = self.sender_slot_count(&addr);
            if used >= self.config.max_slots_per_sender {
                return Err(MempoolError::TooManySlotsPerSender {
                    addr,
                    slots: used + 1,
                    max: self.config.max_slots_per_sender,
                });
            }
        }

        // SEC-2026-05-09 (R2): cumulative balance reservation. The cost of
        // the candidate tx PLUS every other currently-pooled tx from this
        // sender must fit inside the on-chain balance. Without this check a
        // sender could submit 1000 txs that each individually pass the
        // per-tx affordability check but in aggregate exceed their balance,
        // forcing the block builder to silently drop most of them.
        let new_cost = total_cost; // already computed above
        let mut other_reserved = self.sender_cumulative_cost(&addr);
        // If this is a replacement, subtract the old slot's cost so we
        // don't double-count.
        if let Some(slot) = self.pending.get(&key) {
            other_reserved =
                other_reserved.saturating_sub(slot_cost(&slot.tx));
        } else if let Some(old_tx) = self.queued.get(&key) {
            other_reserved = other_reserved.saturating_sub(slot_cost(old_tx));
        }
        let projected = other_reserved.saturating_add(new_cost);
        if projected > sender_balance {
            return Err(MempoolError::CumulativeBalanceExceeded {
                addr,
                reserved: projected,
                balance: sender_balance,
            });
        }

        // SEC-2026-05-09 Pass-12 (mempool C1): replacement gas-price floor.
        // A replacement (same sender + same nonce) must bump the effective
        // tip by at least 12.5% (geth/erigon parity). Without this an
        // attacker can repeatedly replace their own pending tx with one
        // that's identical or only marginally cheaper, churning pool slots
        // and starving honest users — for free, since unrelayed
        // replacements never pay gas.
        if let Some(slot) = self.pending.get(&key) {
            // required = ceil(old_tip * 1125 / 1000)
            let required = slot.priority.saturating_mul(1125) / 1000 + 1;
            if tip < required {
                return Err(MempoolError::ReplacementUnderpriced {
                    addr, nonce, new_tip: tip, required,
                });
            }
        } else if let Some(old_tx) = self.queued.get(&key) {
            let old_tip = old_tx.effective_gas_price(self.base_fee)
                .saturating_sub(self.base_fee);
            let required = old_tip.saturating_mul(1125) / 1000 + 1;
            if tip < required {
                return Err(MempoolError::ReplacementUnderpriced {
                    addr, nonce, new_tip: tip, required,
                });
            }
        }

        // SEC-2026-05-09 (R1): if this is a replacement, evict the old
        // hash from `known` BEFORE inserting the new one — otherwise the
        // map silently leaks one entry per replacement and grows unbounded.
        if let Some(slot) = self.pending.get(&key) {
            self.known.remove(&slot.tx.hash);
        }
        if let Some(old_tx) = self.queued.get(&key) {
            self.known.remove(&old_tx.hash);
        }
        self.known.insert(hash, key);

        if nonce == expected {
            // Ready for inclusion
            if !is_replacement && self.pending.len() >= self.config.max_pending {
                self.known.remove(&hash);
                return Err(MempoolError::PendingFull(self.pending.len()));
            }
            self.nonce_tracker.record_pending(addr, nonce);
            self.pending.insert(key, PendingSlot { tx, priority: tip });
            debug!(hash = hex::encode(&hash[..8]), nonce, "tx added to pending");
            // Promote any queued txs for this sender
            self.promote_queued(addr);
        } else {
            // Future nonce — goes to queued
            if !is_replacement && self.queued.len() >= self.config.max_queued {
                self.known.remove(&hash);
                return Err(MempoolError::QueuedFull(self.queued.len()));
            }
            self.queued.insert(key, tx);
            debug!(hash = hex::encode(&hash[..8]), nonce, "tx added to queued");
        }

        Ok(hash)
    }

    /// Return up to `limit` pending transactions ordered by effective tip (descending).
    pub fn select_transactions(&self, gas_limit: u64) -> Vec<SignedTransaction> {
        let mut slots: Vec<&PendingSlot> = self.pending.values().collect();
        slots.sort_by(|a, b| b.priority.cmp(&a.priority));

        let mut selected = Vec::new();
        let mut total_gas = 0u64;

        for slot in slots {
            let gas = slot.tx.tx.gas_limit;
            if total_gas + gas > gas_limit {
                continue;
            }
            total_gas += gas;
            selected.push(slot.tx.clone());
        }
        selected
    }

    /// Remove confirmed transactions after a block is committed.
    pub fn remove_confirmed(&mut self, txs: &[SignedTransaction]) {
        for tx in txs {
            let key = (tx.from, tx.tx.nonce);
            self.pending.remove(&key);
            self.queued.remove(&key);
            self.known.remove(&tx.hash);
            self.nonce_tracker.set_on_chain(tx.from, tx.tx.nonce + 1);
        }
        // Promote newly eligible queued txs for all affected senders
        let senders: Vec<Address> = txs.iter().map(|t| t.from).collect();
        for addr in senders {
            self.promote_queued(addr);
        }
        info!(
            removed = txs.len(),
            pending = self.pending.len(),
            "mempool pruned after commit"
        );
    }

    fn promote_queued(&mut self, addr: Address) {
        // L-04 fix: cap promotions per call to prevent infinite loop if nonce_tracker
        // ever has a bug that causes next_nonce() to not advance monotonically.
        const MAX_PROMOTE_PER_CALL: usize = 256;
        let mut promoted = 0usize;
        loop {
            if promoted >= MAX_PROMOTE_PER_CALL {
                warn!(addr = ?addr, "promote_queued: hit MAX_PROMOTE_PER_CALL limit ({})", MAX_PROMOTE_PER_CALL);
                break;
            }
            let expected = self.nonce_tracker.next_nonce(&addr);
            let key = (addr, expected);
            if let Some(tx) = self.queued.remove(&key) {
                let tip = tx.effective_gas_price(self.base_fee)
                    .saturating_sub(self.base_fee);
                self.nonce_tracker.record_pending(addr, expected);
                self.pending.insert(key, PendingSlot { tx, priority: tip });
                promoted += 1;
            } else {
                break;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> MempoolConfig {
        MempoolConfig {
            max_pending: 100,
            max_queued: 50,
            min_gas_tip: 1_000_000,
            max_tx_size: 128 * 1024,
            max_slots_per_sender: 10,
            max_nonce_gap: 16,
        }
    }

    #[test]
    fn new_pool_is_empty() {
        let pool = TransactionPool::new(make_config());
        assert_eq!(pool.pending_count(), 0);
        assert_eq!(pool.queued_count(), 0);
        assert_eq!(pool.total_count(), 0);
    }

    #[test]
    fn mempool_config_default_has_sane_limits() {
        let cfg = MempoolConfig::default();
        assert!(cfg.max_pending > 0);
        assert!(cfg.max_queued > 0);
        assert!(cfg.min_gas_tip > 0);
        assert!(cfg.max_slots_per_sender > 0);
        assert!(cfg.max_nonce_gap > 0);
    }

    #[test]
    fn select_transactions_empty_pool_returns_empty() {
        let pool = TransactionPool::new(make_config());
        let selected = pool.select_transactions(30_000_000);
        assert!(selected.is_empty());
    }

    #[test]
    fn update_base_fee_from_header() {
        let mut pool = TransactionPool::new(make_config());
        let mut header = BlockHeader::default();
        header.base_fee_per_gas = 2_000_000_000;
        pool.update_base_fee(&header);
        assert_eq!(pool.base_fee, 2_000_000_000);
    }

    #[test]
    fn remove_confirmed_empty_pool_is_noop() {
        let mut pool = TransactionPool::new(make_config());
        // Should not panic on empty pool
        pool.remove_confirmed(&[]);
        assert_eq!(pool.pending_count(), 0);
    }
}
