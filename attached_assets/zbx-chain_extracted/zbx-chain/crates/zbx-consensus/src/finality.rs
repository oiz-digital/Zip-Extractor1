//! Single-slot finality gadget types for ZBX Chain.
//!
//! Replaces the orphan `zbx-finality` crate (merged 2026-06-27).
//! `zbx-finality` now re-exports these types for backward compatibility.
//!
//! # Overview
//!
//! * [`Checkpoint`]      — a block candidate that collects 2f+1 validator votes
//!                         before being declared finalized.
//! * [`FinalityTracker`] — in-process tracker of pending checkpoints, updated
//!                         as blocks arrive and votes are cast.
//! * [`Justification`]   — a validator's signed vote for a specific checkpoint.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use zbx_types::{address::Address, H256};

// ── Checkpoint ────────────────────────────────────────────────────────────────

/// A block that is gathering 2f+1 validator votes toward finality.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub block_number: u64,
    pub block_hash:   H256,
    pub epoch:        u64,
    pub finalized:    bool,
    pub votes:        u32,
    pub required:     u32,
    pub signers:      Vec<Address>,
}

impl Checkpoint {
    /// Create a new, un-finalized checkpoint.
    ///
    /// `required` is the quorum threshold (typically ⌈(2n+1)/3⌉ for n validators).
    pub fn new(block: u64, hash: H256, epoch: u64, required: u32) -> Self {
        Self {
            block_number: block,
            block_hash:   hash,
            epoch,
            finalized:    false,
            votes:        0,
            required,
            signers:      vec![],
        }
    }

    /// Record a vote from `signer`. Returns `true` when the checkpoint reaches
    /// quorum and transitions to finalized. Duplicate votes from the same signer
    /// are silently ignored (idempotent).
    pub fn add_vote(&mut self, signer: Address) -> bool {
        if self.signers.contains(&signer) {
            return false;
        }
        self.signers.push(signer);
        self.votes += 1;
        if self.votes >= self.required {
            self.finalized = true;
            tracing::info!(block = self.block_number, "Block FINALIZED");
        }
        self.finalized
    }
}

// ── FinalityTracker ───────────────────────────────────────────────────────────

/// Tracks block finality state for an in-process node.
///
/// Accumulates [`Checkpoint`]s as new blocks arrive and finalizes them when
/// 2f+1 validators cast matching votes via [`FinalityTracker::on_vote`].
pub struct FinalityTracker {
    pub checkpoints:     HashMap<u64, Checkpoint>,
    pub last_finalized:  u64,
    pub finalized_hash:  H256,
    pub epoch_length:    u64,
    pub validator_count: u32,
}

impl FinalityTracker {
    pub fn new(epoch_length: u64, validator_count: u32) -> Self {
        Self {
            checkpoints:     HashMap::new(),
            last_finalized:  0,
            finalized_hash:  H256::zero(),
            epoch_length,
            validator_count,
        }
    }

    /// Minimum votes needed for a 2f+1 quorum given `self.validator_count`.
    pub fn required_votes(&self) -> u32 {
        2 * ((self.validator_count.saturating_sub(1)) / 3) + 1
    }

    /// Register a new block and create a pending checkpoint for it.
    pub fn on_block(&mut self, number: u64, hash: H256) {
        let epoch = number / self.epoch_length;
        self.checkpoints.insert(
            number,
            Checkpoint::new(number, hash, epoch, self.required_votes()),
        );
    }

    /// Record a validator vote for `block`. Returns `true` when the block is
    /// newly finalized by this vote (i.e. quorum just crossed). Old checkpoints
    /// below the finalized height are pruned to keep memory bounded.
    pub fn on_vote(&mut self, block: u64, signer: Address) -> bool {
        if let Some(cp) = self.checkpoints.get_mut(&block) {
            if cp.add_vote(signer) && block > self.last_finalized {
                self.last_finalized = block;
                self.finalized_hash = cp.block_hash;
                self.checkpoints.retain(|&n, _| n >= block);
                return true;
            }
        }
        false
    }

    /// Returns `true` if `block` is at or below the last finalized height.
    pub fn is_finalized(&self, block: u64) -> bool {
        block <= self.last_finalized
    }

    /// Blocks between the last finalized block and `head`.
    pub fn finality_lag(&self, head: u64) -> u64 {
        head.saturating_sub(self.last_finalized)
    }
}

// ── Justification ─────────────────────────────────────────────────────────────

/// A validator's signed vote for a specific checkpoint.
///
/// The 65-byte ECDSA signature covers the canonical payload produced by
/// [`Justification::sign_payload`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Justification {
    pub block_number: u64,
    pub block_hash:   H256,
    pub epoch:        u64,
    pub validator:    Address,
    /// Raw 65-byte ECDSA signature (r ‖ s ‖ v).
    pub signature:    Vec<u8>,
}

impl Justification {
    /// Canonical bytes that a validator signs to produce a [`Justification`].
    ///
    /// Layout: `"ZBX_FINALITY_V1:" ‖ block_number_be8 ‖ block_hash_32 ‖ epoch_be8`
    pub fn sign_payload(block: u64, hash: H256, epoch: u64) -> Vec<u8> {
        let mut m = b"ZBX_FINALITY_V1:".to_vec();
        m.extend_from_slice(&block.to_be_bytes());
        m.extend_from_slice(hash.as_bytes());
        m.extend_from_slice(&epoch.to_be_bytes());
        m
    }

    /// Basic structural validity check (non-zero block, non-zero hash,
    /// 65-byte signature).
    pub fn is_valid(&self) -> bool {
        !self.block_hash.is_zero() && self.block_number > 0 && self.signature.len() == 65
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_types::H256;

    fn dummy_addr(b: u8) -> Address {
        Address([b; 20])
    }

    fn dummy_hash(b: u8) -> H256 {
        H256::from([b; 32])
    }

    #[test]
    fn checkpoint_quorum_at_threshold() {
        let mut cp = Checkpoint::new(10, dummy_hash(1), 0, 3);
        assert!(!cp.add_vote(dummy_addr(1)));
        assert!(!cp.add_vote(dummy_addr(2)));
        assert!(cp.add_vote(dummy_addr(3)));
        assert!(cp.finalized);
    }

    #[test]
    fn checkpoint_dedup_votes() {
        let mut cp = Checkpoint::new(10, dummy_hash(1), 0, 2);
        cp.add_vote(dummy_addr(1));
        // Duplicate — should not count.
        cp.add_vote(dummy_addr(1));
        assert_eq!(cp.votes, 1);
        assert!(!cp.finalized);
    }

    #[test]
    fn tracker_on_vote_finalizes_and_prunes() {
        let mut t = FinalityTracker::new(100, 4);
        t.on_block(10, dummy_hash(10));
        t.on_block(11, dummy_hash(11));
        // required_votes = 2*((4-1)/3)+1 = 3
        assert_eq!(t.required_votes(), 3);
        t.on_vote(10, dummy_addr(1));
        t.on_vote(10, dummy_addr(2));
        let finalized = t.on_vote(10, dummy_addr(3));
        assert!(finalized);
        assert_eq!(t.last_finalized, 10);
        // Checkpoint for block 10 is kept (retained), block 11 still present.
        assert!(t.is_finalized(9));
        assert!(t.is_finalized(10));
        assert!(!t.is_finalized(11));
    }

    #[test]
    fn justification_sign_payload_stable() {
        let p = Justification::sign_payload(42, dummy_hash(5), 1);
        assert!(p.starts_with(b"ZBX_FINALITY_V1:"));
        assert_eq!(p.len(), 16 + 8 + 32 + 8);
    }

    #[test]
    fn justification_is_valid() {
        let j = Justification {
            block_number: 1,
            block_hash:   dummy_hash(1),
            epoch:        0,
            validator:    dummy_addr(0),
            signature:    vec![0u8; 65],
        };
        assert!(j.is_valid());

        let bad = Justification { block_number: 0, ..j.clone() };
        assert!(!bad.is_valid());

        let short_sig = Justification { signature: vec![0u8; 64], ..j };
        assert!(!short_sig.is_valid());
    }
}
