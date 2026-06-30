//! Nonce manager — tracks pending nonces per sender in the mempool.
//!
//! # Why Nonce Management Is Hard
//!
//! Ethereum/ZBX nonces are strictly sequential per sender.
//! Problem: User sends tx with nonce=5, then tx with nonce=7.
//!   Nonce 7 is "stuck" until nonce 6 arrives.
//!
//! # NonceManager Responsibilities
//!
//! 1. Track "current on-chain nonce" per sender (from state)
//! 2. Track "pending nonces" per sender (from mempool)
//! 3. Detect and handle gaps (nonce 5 → 7, nonce 6 missing)
//! 4. Allow nonce replacement (EIP-1559 fee bump = same nonce, higher fee)
//! 5. Prune abandoned transactions (nonce < on-chain nonce)
//!
//! # Nonce Gap Handling
//!
//! If nonce=7 arrives before nonce=6:
//!   - Store tx 7 in "future" queue
//!   - When tx 6 arrives → promote tx 7 to "pending"
//!   - If tx 6 never arrives → evict after FUTURE_TTL

use std::collections::{HashMap, BTreeMap};

/// How long future transactions (with nonce gaps) are kept.
pub const FUTURE_TTL_SECS: u64 = 3600; // 1 hour

/// Maximum future transactions per sender (prevents DoS).
pub const MAX_FUTURES_PER_SENDER: usize = 64;

/// Nonce manager — one instance per mempool.
pub struct NonceManager {
    /// On-chain nonce per sender (from state DB).
    on_chain: HashMap<[u8; 20], u64>,
    /// Pending nonces per sender: nonce → gas_price
    pending:  HashMap<[u8; 20], BTreeMap<u64, u128>>,
    /// Future txs with nonce gaps: sender → (nonce → (gas_price, added_at))
    future:   HashMap<[u8; 20], BTreeMap<u64, (u128, u64)>>,
}

impl NonceManager {
    pub fn new() -> Self {
        Self {
            on_chain: HashMap::new(),
            pending:  HashMap::new(),
            future:   HashMap::new(),
        }
    }

    /// Update on-chain nonce from state (called on new block).
    pub fn set_on_chain_nonce(&mut self, sender: [u8; 20], nonce: u64) {
        self.on_chain.insert(sender, nonce);
        // Prune pending/future txs with nonce < on-chain nonce
        if let Some(p) = self.pending.get_mut(&sender) {
            p.retain(|&n, _| n >= nonce);
        }
        if let Some(f) = self.future.get_mut(&sender) {
            f.retain(|&n, _| n >= nonce);
        }
    }

    /// Get the next expected nonce for a sender (on-chain + pending count).
    pub fn next_nonce(&self, sender: [u8; 20]) -> u64 {
        let on_chain = self.on_chain.get(&sender).copied().unwrap_or(0);
        let pending_count = self.pending.get(&sender)
            .map(|p| p.len() as u64)
            .unwrap_or(0);
        on_chain + pending_count
    }

    /// Add a transaction's nonce to the manager.
    ///
    /// Returns:
    ///   Ok(true)  — nonce is valid and added to pending
    ///   Ok(false) — nonce has a gap (added to future)
    ///   Err       — nonce too low (already used on-chain)
    pub fn add_nonce(
        &mut self,
        sender:    [u8; 20],
        nonce:     u64,
        gas_price: u128,
        now:       u64,
    ) -> Result<bool, NonceError> {
        let on_chain = self.on_chain.get(&sender).copied().unwrap_or(0);

        if nonce < on_chain {
            return Err(NonceError::TooLow { got: nonce, expected: on_chain });
        }

        let pending = self.pending.entry(sender).or_default();
        let expected = on_chain + pending.len() as u64;

        if nonce == expected {
            // Perfect nonce — add to pending
            pending.insert(nonce, gas_price);
            // Promote any future txs that are now contiguous
            self.promote_futures(sender, expected + 1);
            Ok(true)
        } else {
            // Gap exists — add to future queue
            let futures = self.future.entry(sender).or_default();
            if futures.len() >= MAX_FUTURES_PER_SENDER {
                return Err(NonceError::TooManyFutures);
            }
            futures.insert(nonce, (gas_price, now));
            Ok(false)
        }
    }

    /// Replace a pending tx with same nonce (fee bump — EIP-1559 replacement).
    /// New gas_price must be at least 10% higher than old.
    pub fn replace_nonce(
        &mut self,
        sender:    [u8; 20],
        nonce:     u64,
        new_price: u128,
    ) -> Result<(), NonceError> {
        let pending = self.pending.entry(sender).or_default();
        let old_price = pending.get(&nonce).copied().ok_or(NonceError::NotPending(nonce))?;
        // Require ≥10% higher gas price
        if new_price < old_price * 110 / 100 {
            return Err(NonceError::InsufficientFeeBump { old: old_price, new: new_price });
        }
        pending.insert(nonce, new_price);
        Ok(())
    }

    /// Promote future transactions that are now contiguous with pending.
    fn promote_futures(&mut self, sender: [u8; 20], start_nonce: u64) {
        let futures = match self.future.get_mut(&sender) { Some(f) => f, None => return };
        let pending = self.pending.entry(sender).or_default();
        let mut next = start_nonce;
        loop {
            if let Some(&(gas_price, _)) = futures.get(&next) {
                pending.insert(next, gas_price);
                futures.remove(&next);
                next += 1;
            } else { break; }
        }
    }

    /// Evict expired future transactions.
    pub fn evict_expired(&mut self, now: u64) {
        for futures in self.future.values_mut() {
            futures.retain(|_, (_, added_at)| now.saturating_sub(*added_at) < FUTURE_TTL_SECS);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NonceError {
    #[error("nonce too low: got {got}, on-chain is {expected}")]
    TooLow { got: u64, expected: u64 },
    #[error("too many future transactions queued")]
    TooManyFutures,
    #[error("nonce {0} not in pending queue")]
    NotPending(u64),
    #[error("fee bump insufficient: old={old}, new={new} (need ≥10% higher)")]
    InsufficientFeeBump { old: u128, new: u128 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender() -> [u8; 20] { [0x01; 20] }

    #[test]
    fn next_nonce_starts_at_zero() {
        let mgr = NonceManager::new();
        assert_eq!(mgr.next_nonce(sender()), 0);
    }

    #[test]
    fn add_sequential_nonces() {
        let mut mgr = NonceManager::new();
        assert!(mgr.add_nonce(sender(), 0, 1_000, 100).unwrap());
        assert!(mgr.add_nonce(sender(), 1, 1_000, 100).unwrap());
        assert_eq!(mgr.next_nonce(sender()), 2);
    }

    #[test]
    fn nonce_gap_goes_to_future() {
        let mut mgr = NonceManager::new();
        // Add nonce 0 first
        mgr.add_nonce(sender(), 0, 1_000, 100).unwrap();
        // Skip nonce 1, add nonce 2 (gap!)
        let in_future = !mgr.add_nonce(sender(), 2, 1_000, 100).unwrap();
        assert!(in_future);
    }

    #[test]
    fn future_promoted_when_gap_filled() {
        let mut mgr = NonceManager::new();
        mgr.add_nonce(sender(), 0, 1_000, 100).unwrap(); // pending
        mgr.add_nonce(sender(), 2, 1_000, 100).unwrap(); // future (gap at 1)
        assert_eq!(mgr.next_nonce(sender()), 1);
        mgr.add_nonce(sender(), 1, 1_000, 100).unwrap(); // fills gap → promotes 2
        assert_eq!(mgr.next_nonce(sender()), 3); // now 0,1,2 all pending
    }

    #[test]
    fn too_low_nonce_rejected() {
        let mut mgr = NonceManager::new();
        mgr.set_on_chain_nonce(sender(), 5);
        let err = mgr.add_nonce(sender(), 3, 1_000, 100).unwrap_err();
        assert!(matches!(err, NonceError::TooLow { got: 3, expected: 5 }));
    }

    #[test]
    fn fee_bump_requires_10_pct() {
        let mut mgr = NonceManager::new();
        mgr.add_nonce(sender(), 0, 1_000, 100).unwrap();
        // 5% bump — not enough
        let err = mgr.replace_nonce(sender(), 0, 1_050).unwrap_err();
        assert!(matches!(err, NonceError::InsufficientFeeBump { .. }));
        // 10% bump — ok
        mgr.replace_nonce(sender(), 0, 1_100).unwrap();
    }
}