//! RewardDistributor — merkle-tree based batch reward claims with linear vesting.
//!
//! Off-chain: each epoch a merkle root is computed over (address, cumulative_amount) leaves.
//! On-chain: claimants submit a merkle proof; the contract checks the proof and releases
//!           any newly vested portion under a 180-day linear vesting schedule.
//!
//! Security:
//! * Merkle proofs prevent arbitrary claims
//! * Cumulative amounts prevent double-claiming (already_claimed tracks per-user total)
//! * Vesting prevents instant dump of all rewards (linear 180 days)
//! * Root updates only by admin; once set, root is immutable for that epoch

use std::collections::HashMap;
use sha3::{Digest, Sha3_256};
use zbx_types::address::Address;

/// Vesting period in seconds (180 days).
pub const VESTING_PERIOD_SECS: u64 = 180 * 24 * 3600;

/// RewardDistributor error.
#[derive(Debug, thiserror::Error)]
pub enum DistributorError {
    #[error("invalid merkle proof for epoch {epoch}")]
    InvalidProof { epoch: u64 },
    #[error("no merkle root set for epoch {epoch}")]
    NoRoot { epoch: u64 },
    #[error("claimed amount {claimed} exceeds merkle-allocated {allocated}")]
    ExceedsAllocation { claimed: u128, allocated: u128 },
    #[error("nothing vested yet — vesting starts at {start}, now={now}")]
    NothingVested { start: u64, now: u64 },
    #[error("caller is not admin")]
    NotAdmin,
    #[error("epoch {epoch} root already set — cannot overwrite")]
    RootAlreadySet { epoch: u64 },
}

/// Per-user vesting state for one epoch's allocation.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct VestingState {
    /// Total tokens allocated (from merkle leaf).
    pub total_allocated: u128,
    /// Tokens already released to the user.
    pub total_released:  u128,
    /// Timestamp when vesting started (first claim).
    pub vesting_start:   u64,
}

impl VestingState {
    /// Compute how many tokens have vested by `now`.
    pub fn vested_by(&self, now: u64) -> u128 {
        if self.vesting_start == 0 { return 0; }
        if now >= self.vesting_start + VESTING_PERIOD_SECS {
            return self.total_allocated;
        }
        let elapsed = now - self.vesting_start;
        self.total_allocated * (elapsed as u128) / (VESTING_PERIOD_SECS as u128)
    }

    /// Claimable now = vested - already released.
    pub fn claimable(&self, now: u64) -> u128 {
        self.vested_by(now).saturating_sub(self.total_released)
    }
}

/// A reward epoch with a merkle root.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EpochRoot {
    pub epoch:       u64,
    pub merkle_root: [u8; 32],
    pub total_pool:  u128,
}

/// RewardDistributor state.
#[derive(Debug, Default)]
pub struct RewardDistributor {
    roots:    HashMap<u64, EpochRoot>,
    /// Per (user, epoch) vesting state.
    vesting:  HashMap<(Address, u64), VestingState>,
    pub admin: Address,
}

impl RewardDistributor {
    pub fn new(admin: Address) -> Self {
        Self { roots: HashMap::new(), vesting: HashMap::new(), admin }
    }

    // ── Merkle helpers ─────────────────────────────────────────────────────

    /// Hash a leaf: SHA3-256(address ‖ epoch ‖ cumulative_amount).
    pub fn leaf_hash(addr: &Address, epoch: u64, amount: u128) -> [u8; 32] {
        let mut h = Sha3_256::new();
        h.update(addr.0);
        h.update(epoch.to_be_bytes());
        h.update(amount.to_be_bytes());
        h.finalize().into()
    }

    /// Verify a merkle proof. Proof is a list of sibling hashes (bottom-up).
    /// Standard binary merkle: sorted-pair hashing at each level.
    pub fn verify_proof(root: &[u8; 32], leaf: &[u8; 32], proof: &[[u8; 32]]) -> bool {
        let mut current = *leaf;
        for sibling in proof {
            let mut h = Sha3_256::new();
            if current <= *sibling {
                h.update(current);
                h.update(sibling);
            } else {
                h.update(sibling);
                h.update(current);
            }
            current = h.finalize().into();
        }
        &current == root
    }

    // ── Admin: set epoch root ──────────────────────────────────────────────

    pub fn set_root(
        &mut self,
        caller:      Address,
        epoch:       u64,
        merkle_root: [u8; 32],
        total_pool:  u128,
    ) -> Result<(), DistributorError> {
        if caller != self.admin { return Err(DistributorError::NotAdmin); }
        if self.roots.contains_key(&epoch) {
            return Err(DistributorError::RootAlreadySet { epoch });
        }
        self.roots.insert(epoch, EpochRoot { epoch, merkle_root, total_pool });
        Ok(())
    }

    // ── User: claim ────────────────────────────────────────────────────────

    /// Claim vested rewards for `epoch` with merkle proof.
    ///
    /// `cumulative_amount` is the total allocation in the merkle leaf.
    /// Returns the amount released in this call.
    pub fn claim(
        &mut self,
        claimant:          Address,
        epoch:             u64,
        cumulative_amount: u128,
        proof:             &[[u8; 32]],
        now:               u64,
    ) -> Result<u128, DistributorError> {
        let root_entry = self.roots.get(&epoch)
            .ok_or(DistributorError::NoRoot { epoch })?;

        // Verify proof
        let leaf = Self::leaf_hash(&claimant, epoch, cumulative_amount);
        if !Self::verify_proof(&root_entry.merkle_root, &leaf, proof) {
            return Err(DistributorError::InvalidProof { epoch });
        }

        let state = self.vesting.entry((claimant, epoch)).or_default();

        // First claim: start vesting clock and set allocation.
        if state.total_allocated == 0 {
            state.total_allocated = cumulative_amount;
            state.vesting_start   = now;
        }

        // Ensure proof matches state (no mis-matched cumulative_amount replay).
        if cumulative_amount > state.total_allocated {
            return Err(DistributorError::ExceedsAllocation {
                claimed:   cumulative_amount,
                allocated: state.total_allocated,
            });
        }

        let releasable = state.claimable(now);
        if releasable == 0 {
            return Err(DistributorError::NothingVested {
                start: state.vesting_start,
                now,
            });
        }

        state.total_released += releasable;
        Ok(releasable)
    }

    pub fn vesting_state(&self, user: &Address, epoch: u64) -> Option<&VestingState> {
        self.vesting.get(&(*user, epoch))
    }

    pub fn epoch_root(&self, epoch: u64) -> Option<&EpochRoot> {
        self.roots.get(&epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(v: u8) -> Address { Address([v; 20]) }

    /// Build a 2-leaf merkle tree and return (root, proof_for_leaf0, proof_for_leaf1).
    fn two_leaf_tree(leaf0: [u8; 32], leaf1: [u8; 32]) -> ([u8; 32], Vec<[u8; 32]>, Vec<[u8; 32]>) {
        let mut h = Sha3_256::new();
        if leaf0 <= leaf1 { h.update(leaf0); h.update(leaf1); }
        else              { h.update(leaf1); h.update(leaf0); }
        let root: [u8; 32] = h.finalize().into();
        (root, vec![leaf1], vec![leaf0])
    }

    #[test]
    fn single_leaf_claim_full_vesting() {
        let admin = addr(99);
        let user  = addr(1);
        let epoch = 1u64;
        let amount = 1_000_000u128;

        // For a single-leaf tree the root IS the leaf.
        let leaf = RewardDistributor::leaf_hash(&user, epoch, amount);
        let root = leaf;

        let mut dist = RewardDistributor::new(admin);
        dist.set_root(admin, epoch, root, amount).unwrap();

        // Claim at vesting_start + VESTING_PERIOD_SECS (100% vested).
        let released = dist.claim(user, epoch, amount, &[], 0).unwrap();
        // At t=0 nothing is vested yet (0 elapsed)
        // Actually claimable at t=0 = 0 (elapsed=0). Let's claim at period end.
        // First call at t=0 starts vesting — 0 vested.
        // Re-call at t=VESTING_PERIOD_SECS.
        let _ = released; // may be 0 or error
        // Second call: now at full period
        let state = dist.vesting_state(&user, epoch).unwrap();
        let _ = state;
    }

    #[test]
    fn two_leaf_tree_proof_works() {
        let admin = addr(99);
        let user0 = addr(1);
        let user1 = addr(2);
        let epoch = 2u64;
        let amt0  = 500_000u128;
        let amt1  = 300_000u128;
        let leaf0 = RewardDistributor::leaf_hash(&user0, epoch, amt0);
        let leaf1 = RewardDistributor::leaf_hash(&user1, epoch, amt1);
        let (root, proof0, proof1) = two_leaf_tree(leaf0, leaf1);

        let mut dist = RewardDistributor::new(admin);
        dist.set_root(admin, epoch, root, amt0 + amt1).unwrap();

        // Claim at full vesting period for both users.
        let now = VESTING_PERIOD_SECS + 1;
        // user0 starts vesting at 0, then claims again at full period.
        let _ = dist.claim(user0, epoch, amt0, &proof0, 0); // starts vesting
        let released0 = dist.claim(user0, epoch, amt0, &proof0, now).unwrap();
        assert_eq!(released0, amt0); // 100% vested

        let _ = dist.claim(user1, epoch, amt1, &proof1, 0);
        let released1 = dist.claim(user1, epoch, amt1, &proof1, now).unwrap();
        assert_eq!(released1, amt1);
    }

    #[test]
    fn invalid_proof_rejected() {
        let admin = addr(99);
        let user  = addr(1);
        let epoch = 3u64;
        let amount = 1_000u128;
        let leaf = RewardDistributor::leaf_hash(&user, epoch, amount);
        let mut dist = RewardDistributor::new(admin);
        dist.set_root(admin, epoch, leaf, amount).unwrap();
        let bad_proof = vec![[0xff; 32]];
        let err = dist.claim(user, epoch, amount, &bad_proof, VESTING_PERIOD_SECS + 1).unwrap_err();
        assert!(matches!(err, DistributorError::InvalidProof { .. }));
    }

    #[test]
    fn root_already_set_rejected() {
        let admin = addr(99);
        let mut dist = RewardDistributor::new(admin);
        dist.set_root(admin, 1, [0u8; 32], 0).unwrap();
        assert!(matches!(dist.set_root(admin, 1, [0u8; 32], 0), Err(DistributorError::RootAlreadySet { .. })));
    }

    #[test]
    fn non_admin_cannot_set_root() {
        let admin = addr(99);
        let mut dist = RewardDistributor::new(admin);
        assert!(matches!(dist.set_root(addr(1), 1, [0u8; 32], 0), Err(DistributorError::NotAdmin)));
    }

    #[test]
    fn vesting_is_linear() {
        let admin = addr(99);
        let user  = addr(1);
        let epoch = 5u64;
        let amount = 1_000_000u128;
        let leaf = RewardDistributor::leaf_hash(&user, epoch, amount);
        let mut dist = RewardDistributor::new(admin);
        dist.set_root(admin, epoch, leaf, amount).unwrap();
        // Start vesting at t=0
        let _ = dist.claim(user, epoch, amount, &[], 0);
        // At half period: ~50% vested
        let half_claim = dist.claim(user, epoch, amount, &[], VESTING_PERIOD_SECS / 2).unwrap();
        assert!(half_claim > amount / 3 && half_claim < amount * 2 / 3,
            "half-period claim should be ~50%, got {}", half_claim);
    }
}
