//! Per-account nonce tracking.

use std::collections::HashMap;
use zbx_types::address::Address;

/// Tracks the expected next nonce for each sender.
pub struct NonceTracker {
    /// Nonces confirmed on-chain (from world state).
    on_chain: HashMap<Address, u64>,
    /// Nonces of transactions currently in the pending pool.
    pending_max: HashMap<Address, u64>,
}

impl NonceTracker {
    pub fn new() -> Self {
        NonceTracker {
            on_chain: HashMap::new(),
            pending_max: HashMap::new(),
        }
    }

    /// Update the committed on-chain nonce for an address.
    pub fn set_on_chain(&mut self, addr: Address, nonce: u64) {
        self.on_chain.insert(addr, nonce);
        // Evict pending nonces that are now confirmed.
        if let Some(max) = self.pending_max.get(&addr) {
            if *max <= nonce {
                self.pending_max.remove(&addr);
            }
        }
    }

    /// The next expected nonce for a sender (pending-aware).
    pub fn next_nonce(&self, addr: &Address) -> u64 {
        self.pending_max
            .get(addr)
            .copied()
            .map(|n| n + 1)
            .unwrap_or_else(|| self.on_chain.get(addr).copied().unwrap_or(0))
    }

    /// The last confirmed on-chain nonce.
    pub fn on_chain_nonce(&self, addr: &Address) -> u64 {
        self.on_chain.get(addr).copied().unwrap_or(0)
    }

    /// Record that a pending tx with the given nonce was added.
    pub fn record_pending(&mut self, addr: Address, nonce: u64) {
        let entry = self.pending_max.entry(addr).or_insert(0);
        if nonce > *entry {
            *entry = nonce;
        }
    }

    /// Remove pending record when a tx is evicted.
    pub fn remove_pending(&mut self, addr: &Address) {
        self.pending_max.remove(addr);
    }
}

impl Default for NonceTracker {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address { Address([b; 20]) }

    #[test]
    fn new_account_nonce_is_zero() {
        let t = NonceTracker::new();
        assert_eq!(t.next_nonce(&addr(1)), 0);
        assert_eq!(t.on_chain_nonce(&addr(1)), 0);
    }

    #[test]
    fn set_on_chain_advances_next_nonce() {
        let mut t = NonceTracker::new();
        t.set_on_chain(addr(1), 5);
        assert_eq!(t.next_nonce(&addr(1)), 5);
    }

    #[test]
    fn record_pending_advances_next_nonce() {
        let mut t = NonceTracker::new();
        t.set_on_chain(addr(1), 3);
        t.record_pending(addr(1), 3);
        assert_eq!(t.next_nonce(&addr(1)), 4);
    }

    #[test]
    fn pending_higher_than_chain_wins() {
        let mut t = NonceTracker::new();
        t.set_on_chain(addr(2), 10);
        t.record_pending(addr(2), 15);
        assert_eq!(t.next_nonce(&addr(2)), 16);
    }

    #[test]
    fn set_on_chain_above_pending_evicts_pending() {
        let mut t = NonceTracker::new();
        t.set_on_chain(addr(3), 0);
        t.record_pending(addr(3), 4);
        assert_eq!(t.next_nonce(&addr(3)), 5);
        t.set_on_chain(addr(3), 5); // commit through nonce 4
        // pending_max == 4 <= 5, so evicted
        assert_eq!(t.next_nonce(&addr(3)), 5);
    }

    #[test]
    fn remove_pending_falls_back_to_chain() {
        let mut t = NonceTracker::new();
        t.set_on_chain(addr(4), 7);
        t.record_pending(addr(4), 9);
        t.remove_pending(&addr(4));
        assert_eq!(t.next_nonce(&addr(4)), 7);
    }

    #[test]
    fn independent_accounts_dont_interfere() {
        let mut t = NonceTracker::new();
        t.set_on_chain(addr(1), 10);
        t.set_on_chain(addr(2), 20);
        assert_eq!(t.next_nonce(&addr(1)), 10);
        assert_eq!(t.next_nonce(&addr(2)), 20);
    }

    #[test]
    fn default_is_same_as_new() {
        let t = NonceTracker::default();
        assert_eq!(t.next_nonce(&addr(5)), 0);
    }
}
