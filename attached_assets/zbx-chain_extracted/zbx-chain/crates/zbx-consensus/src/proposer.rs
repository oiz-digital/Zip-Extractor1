//! VRF-based proposer election for HotStuff-2 (ZBX Chain).
//!
//! ## Leader election
//!
//! The leader (block proposer) for round `r` is determined by:
//!
//! ```text
//! seed  = keccak256(highest_qc.block_hash || round.to_be_bytes())
//! index = u64::from_be_bytes(seed[0..8]) % n_validators
//! leader = validator_set[index]
//! ```
//!
//! The QC block hash acts as an unpredictable VRF seed — it changes every
//! block so future leaders cannot be predicted until the preceding block is
//! committed (ZBX-M-01 fix for round-robin predictability).
//!
//! ## Sub-committee election
//!
//! For light-client proofs and DA sampling, a sub-committee of size `k` can
//! be elected from the full validator set using a deterministic Fisher-Yates
//! shuffle seeded from `keccak256(epoch_hash || round)`.

use zbx_types::{address::Address, H256};
use zbx_crypto::keccak::keccak256;

/// Determines the block proposer and sub-committees for each round.
#[derive(Debug, Clone)]
pub struct ProposerElection {
    /// Ordered validator set for the current epoch.
    validators: Vec<Address>,
}

impl ProposerElection {
    /// Create a new `ProposerElection` for the given ordered validator set.
    pub fn new(validators: Vec<Address>) -> Self {
        ProposerElection { validators }
    }

    /// Replace the validator set (called on epoch transition).
    pub fn update_validators(&mut self, validators: Vec<Address>) {
        self.validators = validators;
    }

    /// Number of validators in the current set.
    pub fn n(&self) -> usize {
        self.validators.len()
    }

    /// Elect the proposer for `round` using `qc_block_hash` as the VRF seed.
    ///
    /// Returns `None` if the validator set is empty.
    pub fn elect(&self, round: u64, qc_block_hash: H256) -> Option<Address> {
        if self.validators.is_empty() {
            return None;
        }
        let idx = self.vrf_index(qc_block_hash, round, self.validators.len());
        Some(self.validators[idx])
    }

    /// True if `addr` is the proposer for `round` given `qc_block_hash`.
    pub fn is_proposer(&self, addr: Address, round: u64, qc_block_hash: H256) -> bool {
        self.elect(round, qc_block_hash)
            .map(|p| p == addr)
            .unwrap_or(false)
    }

    /// Elect a sub-committee of `size` validators for `round` (for DA / light
    /// client sampling).
    ///
    /// Uses a seeded Fisher-Yates shuffle: we draw `size` indices without
    /// replacement from `0..n`, deriving each draw from a deterministic hash.
    ///
    /// Returns the full set if `size >= n`.
    pub fn elect_committee(
        &self,
        round:         u64,
        epoch_hash:    H256,
        size:          usize,
    ) -> Vec<Address> {
        let n = self.validators.len();
        if n == 0 || size == 0 {
            return Vec::new();
        }
        if size >= n {
            return self.validators.clone();
        }

        // Build a mutable index array and perform size draws.
        let mut indices: Vec<usize> = (0..n).collect();
        let mut selected = Vec::with_capacity(size);

        for i in 0..size {
            // Seed for draw i: keccak256(epoch_hash || round || i)
            let mut input = [0u8; 48];
            input[..32].copy_from_slice(&epoch_hash.0);
            input[32..40].copy_from_slice(&round.to_be_bytes());
            input[40..48].copy_from_slice(&(i as u64).to_be_bytes());
            let hash = keccak256(&input);
            let idx_raw = u64::from_be_bytes(hash.0[..8].try_into().expect("32-byte hash"));
            let pool_size = n - i;
            let pick = (idx_raw as usize) % pool_size;
            selected.push(self.validators[indices[i + pick]]);
            indices.swap(i, i + pick);
        }

        selected
    }

    /// Derive the VRF index for (seed_hash, round) over a pool of `pool_size`.
    fn vrf_index(&self, seed: H256, round: u64, pool_size: usize) -> usize {
        let mut input = [0u8; 40];
        input[..32].copy_from_slice(&seed.0);
        input[32..40].copy_from_slice(&round.to_be_bytes());
        let hash = keccak256(&input);
        let idx_raw = u64::from_be_bytes(hash.0[..8].try_into().expect("32-byte hash"));
        (idx_raw as usize) % pool_size
    }

    /// For every round in `rounds`, return a map (round → proposer).
    pub fn elect_batch(
        &self,
        rounds:       &[u64],
        qc_block_hash: H256,
    ) -> Vec<(u64, Option<Address>)> {
        rounds.iter()
            .map(|&r| (r, self.elect(r, qc_block_hash)))
            .collect()
    }

    /// Return the proposer rotation schedule for rounds `start..start+count`.
    /// Useful for look-ahead to determine when to pre-build blocks.
    pub fn lookahead(
        &self,
        start:          u64,
        count:          usize,
        qc_block_hash:  H256,
    ) -> Vec<(u64, Address)> {
        if self.validators.is_empty() {
            return Vec::new();
        }
        (start..start + count as u64)
            .filter_map(|r| self.elect(r, qc_block_hash).map(|p| (r, p)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address { Address([b; 20]) }

    fn validators_n(n: u8) -> Vec<Address> {
        (1..=n).map(addr).collect()
    }

    #[test]
    fn elect_returns_valid_address() {
        let pe = ProposerElection::new(validators_n(3));
        let p = pe.elect(1, H256::zero());
        assert!(p.is_some());
        let p = p.unwrap();
        assert!(pe.validators.contains(&p), "elected proposer must be in the set");
    }

    #[test]
    fn exactly_one_proposer_per_round() {
        let v = validators_n(4);
        let pe = ProposerElection::new(v.clone());
        for round in 0..20u64 {
            let count = v.iter().filter(|&&a| pe.is_proposer(a, round, H256::zero())).count();
            assert_eq!(count, 1, "round {}: expected exactly 1 proposer", round);
        }
    }

    #[test]
    fn determinism() {
        let pe = ProposerElection::new(validators_n(5));
        let p1 = pe.elect(7, H256([0xABu8; 32]));
        let p2 = pe.elect(7, H256([0xABu8; 32]));
        assert_eq!(p1, p2, "same inputs must produce same proposer");
    }

    #[test]
    fn different_rounds_different_proposers() {
        let pe = ProposerElection::new(validators_n(10));
        let mut seen = std::collections::HashSet::new();
        for r in 0..30u64 {
            let p = pe.elect(r, H256::zero()).unwrap();
            seen.insert(p);
        }
        // With 10 validators over 30 rounds, expect more than 1 unique proposer
        assert!(seen.len() > 1, "all rounds should not pick the same proposer");
    }

    #[test]
    fn empty_validator_set_returns_none() {
        let pe = ProposerElection::new(Vec::new());
        assert!(pe.elect(1, H256::zero()).is_none());
    }

    #[test]
    fn committee_size_capped_at_n() {
        let pe = ProposerElection::new(validators_n(5));
        let c = pe.elect_committee(1, H256::zero(), 100);
        assert_eq!(c.len(), 5);
    }

    #[test]
    fn committee_no_duplicates() {
        let pe = ProposerElection::new(validators_n(10));
        let c = pe.elect_committee(1, H256::zero(), 4);
        assert_eq!(c.len(), 4);
        let unique: std::collections::HashSet<_> = c.iter().copied().collect();
        assert_eq!(unique.len(), 4, "committee must have no duplicate members");
    }

    #[test]
    fn committee_deterministic() {
        let pe = ProposerElection::new(validators_n(10));
        let c1 = pe.elect_committee(5, H256([0x11; 32]), 4);
        let c2 = pe.elect_committee(5, H256([0x11; 32]), 4);
        assert_eq!(c1, c2);
    }

    #[test]
    fn lookahead_returns_consecutive_rounds() {
        let pe = ProposerElection::new(validators_n(3));
        let schedule = pe.lookahead(10, 5, H256::zero());
        assert_eq!(schedule.len(), 5);
        let rounds: Vec<u64> = schedule.iter().map(|(r, _)| *r).collect();
        assert_eq!(rounds, vec![10, 11, 12, 13, 14]);
    }

    #[test]
    fn update_validators_takes_effect() {
        let mut pe = ProposerElection::new(validators_n(2));
        let p_before = pe.elect(1, H256::zero()).unwrap();
        pe.update_validators(validators_n(5));
        assert_eq!(pe.n(), 5);
        // Proposer may or may not change — but election must still work
        let p_after = pe.elect(1, H256::zero()).unwrap();
        assert!(pe.validators.contains(&p_after));
        let _ = p_before; // suppress unused warning
    }
}
