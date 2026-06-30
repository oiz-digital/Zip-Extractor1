//! Epoch Manager: validator set rotation and epoch state (ZBX Chain PoS).
//!
//! ## Epoch lifecycle
//!
//! ```text
//! Block 1                Block EPOCH_LENGTH        Block 2×EPOCH_LENGTH
//!  │ ← Epoch 1 ────────────────────────┤ ← Epoch 2 ──────────────────────────┤
//!  │ validator set V1                   │ rotation: top-100 by stake → V2      │
//! ```
//!
//! At the end of each epoch (`block_number % epoch_length == 0`) the chain:
//! 1. Selects the new active validator set (top-N by total stake).
//! 2. Emits an `EpochTransition` event with new validator set + epoch number.
//! 3. Advances the pacemaker to the new epoch so new BLS keys are used.
//!
//! ## Epoch state persistence
//!
//! `EpochState` is serializable so the node can persist it across restarts.
//! On startup the caller should restore `EpochState` from disk before entering
//! the consensus loop (prevents epoch-number drift after a crash).

use zbx_types::{address::Address, H256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

/// Epoch length in blocks (4 days at 2s/block = 172,800 blocks).
///
/// Must match `zbx_staking::EPOCH_LENGTH` exactly.  The node populates
/// `ConsensusConfig::epoch_length` from `zbx_staking::EPOCH_LENGTH`
/// (see `node/src/node.rs`), so any divergence here would cause the
/// `EpochManager` to fire rotation ~2.5× more often than the consensus
/// driver, producing a silent validator-set split.
///
/// Previous value (69_120, "4 days at 5s/block") was wrong — ZBX Chain
/// targets 2 s blocks, and both staking and node.rs already used 172_800.
pub const EPOCH_LENGTH: u64 = 172_800;

/// Maximum number of active validators per epoch.
pub const MAX_VALIDATORS: usize = 100;

/// Minimum stake to qualify as an active validator (100,000 ZBX in wei).
pub const MIN_VALIDATOR_STAKE: u128 = 100_000 * 10u128.pow(18);

/// An entry in the candidate validator set (address + total delegated stake).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorEntry {
    pub address:       Address,
    pub stake_wei:     u128,
    pub bls_pubkey:    Vec<u8>, // 48-byte G1 compressed
}

/// Immutable snapshot of one epoch's parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochState {
    /// Epoch number (1-indexed).
    pub epoch:           u64,
    /// First block of this epoch.
    pub start_block:     u64,
    /// Last block of this epoch (inclusive).
    pub end_block:       u64,
    /// Active validator set for this epoch (ordered by descending stake).
    pub validators:      Vec<ValidatorEntry>,
    /// Keccak256 of the validator set (for quick equality checks).
    pub validator_hash:  H256,
    /// State root at the start of this epoch.
    pub start_state_root: H256,
}

impl EpochState {
    /// Address list extracted from the validator set.
    pub fn validator_addresses(&self) -> Vec<Address> {
        self.validators.iter().map(|v| v.address).collect()
    }

    /// Total active stake in this epoch.
    pub fn total_stake(&self) -> u128 {
        self.validators.iter().map(|v| v.stake_wei).sum()
    }

    /// BFT quorum size: ⌈2n/3⌉ + 1.
    pub fn quorum(&self) -> usize {
        let n = self.validators.len();
        (n * 2 / 3) + 1
    }

    /// True if the block is the last block of this epoch.
    pub fn is_last_block(&self, block_number: u64) -> bool {
        block_number == self.end_block
    }

    /// True if the block is the first block of this epoch.
    pub fn is_first_block(&self, block_number: u64) -> bool {
        block_number == self.start_block
    }

    fn compute_validator_hash(validators: &[ValidatorEntry]) -> H256 {
        let mut data = Vec::with_capacity(validators.len() * 36);
        for v in validators {
            data.extend_from_slice(&v.address.0);
            data.extend_from_slice(&v.stake_wei.to_be_bytes());
        }
        zbx_crypto::keccak::keccak256(&data)
    }

    pub fn new(
        epoch:            u64,
        epoch_length:     u64,
        mut validators:   Vec<ValidatorEntry>,
        start_state_root: H256,
    ) -> Self {
        validators.sort_by(|a, b| b.stake_wei.cmp(&a.stake_wei));
        validators.truncate(MAX_VALIDATORS);
        let validator_hash = Self::compute_validator_hash(&validators);
        let start_block = (epoch - 1) * epoch_length + 1;
        EpochState {
            epoch,
            start_block,
            end_block: epoch * epoch_length,
            validators,
            validator_hash,
            start_state_root,
        }
    }
}

/// Events emitted by the EpochManager.
#[derive(Debug)]
pub enum EpochEvent {
    /// An epoch boundary was crossed — new epoch parameters attached.
    EpochTransition {
        old_epoch: u64,
        new_epoch: u64,
        new_state: EpochState,
    },
    /// Block emitted within an epoch (no transition).
    BlockProcessed { block_number: u64, epoch: u64 },
}

/// Manages epoch lifecycle and validator set rotation.
pub struct EpochManager {
    /// Current epoch state.
    pub current: EpochState,
    /// Historical epoch states (kept for evidence verification within SLASH_WINDOW).
    history: HashMap<u64, EpochState>,
    /// Epoch length override (for testing).
    epoch_length: u64,
    /// Pending validator candidates from staking module for next epoch.
    pending_candidates: Vec<ValidatorEntry>,
}

impl EpochManager {
    /// Create a new EpochManager with genesis epoch (epoch 1).
    pub fn new(
        genesis_validators: Vec<ValidatorEntry>,
        epoch_length:       u64,
    ) -> Self {
        let state = EpochState::new(1, epoch_length, genesis_validators, H256::zero());
        EpochManager {
            current: state,
            history: HashMap::new(),
            epoch_length,
            pending_candidates: Vec::new(),
        }
    }

    /// Create from a previously persisted `EpochState` (restart recovery).
    pub fn from_state(state: EpochState, epoch_length: u64) -> Self {
        EpochManager {
            current: state,
            history: HashMap::new(),
            epoch_length,
            pending_candidates: Vec::new(),
        }
    }

    /// Update the pending validator candidates from the staking module.
    ///
    /// Should be called once per epoch with the current full validator set
    /// (including stake amounts) before the epoch boundary is crossed.
    pub fn update_candidates(&mut self, candidates: Vec<ValidatorEntry>) {
        self.pending_candidates = candidates;
    }

    /// Process a committed block. Returns an `EpochTransition` event if this
    /// block is the last block of the current epoch.
    pub fn on_block_committed(
        &mut self,
        block_number:   u64,
        state_root:     H256,
    ) -> EpochEvent {
        if self.current.is_last_block(block_number) {
            self.rotate(block_number, state_root)
        } else {
            EpochEvent::BlockProcessed {
                block_number,
                epoch: self.current.epoch,
            }
        }
    }

    /// Perform the epoch rotation — select new validator set and advance epoch.
    fn rotate(&mut self, end_block: u64, state_root: H256) -> EpochEvent {
        let old_epoch = self.current.epoch;
        let new_epoch = old_epoch + 1;

        // Select candidates for new epoch (use pending_candidates if available,
        // otherwise keep current set to avoid chain halt on staking outage).
        let candidates = if self.pending_candidates.is_empty() {
            warn!(
                epoch = new_epoch,
                "no candidate update received — carrying over current validator set"
            );
            self.current.validators.clone()
        } else {
            let mut c = self.pending_candidates.drain(..).collect::<Vec<_>>();
            c.retain(|v| v.stake_wei >= MIN_VALIDATOR_STAKE);
            c
        };

        let new_state = EpochState::new(new_epoch, self.epoch_length, candidates, state_root);

        info!(
            old_epoch,
            new_epoch,
            validators = new_state.validators.len(),
            total_stake = new_state.total_stake(),
            "epoch rotation"
        );

        let old_state = std::mem::replace(&mut self.current, new_state.clone());
        self.history.insert(old_epoch, old_state);

        EpochEvent::EpochTransition {
            old_epoch,
            new_epoch,
            new_state,
        }
    }

    /// Look up the validator set for a past epoch (for evidence verification).
    pub fn epoch_state(&self, epoch: u64) -> Option<&EpochState> {
        if epoch == self.current.epoch {
            Some(&self.current)
        } else {
            self.history.get(&epoch)
        }
    }

    pub fn current_epoch(&self) -> u64 {
        self.current.epoch
    }

    pub fn current_validators(&self) -> &[ValidatorEntry] {
        &self.current.validators
    }

    pub fn epoch_length(&self) -> u64 {
        self.epoch_length
    }

    /// Block number of the next epoch boundary.
    pub fn next_epoch_boundary(&self) -> u64 {
        self.current.end_block + 1
    }

    /// Blocks remaining in the current epoch.
    pub fn blocks_until_rotation(&self, current_block: u64) -> u64 {
        self.current.end_block.saturating_sub(current_block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_validator(addr: u8, stake: u128) -> ValidatorEntry {
        ValidatorEntry {
            address:    Address([addr; 20]),
            stake_wei:  stake,
            bls_pubkey: vec![addr; 48],
        }
    }

    #[test]
    fn genesis_epoch_state() {
        let validators = vec![
            make_validator(1, MIN_VALIDATOR_STAKE),
            make_validator(2, MIN_VALIDATOR_STAKE * 2),
        ];
        let mgr = EpochManager::new(validators, 100);
        assert_eq!(mgr.current_epoch(), 1);
        assert_eq!(mgr.current.start_block, 1);
        assert_eq!(mgr.current.end_block, 100);
        assert_eq!(mgr.current.quorum(), 2); // ⌈2×2/3⌉+1=2
    }

    #[test]
    fn validators_sorted_by_stake_descending() {
        let validators = vec![
            make_validator(1, MIN_VALIDATOR_STAKE),
            make_validator(3, MIN_VALIDATOR_STAKE * 3),
            make_validator(2, MIN_VALIDATOR_STAKE * 2),
        ];
        let mgr = EpochManager::new(validators, 100);
        let addrs = mgr.current_validators();
        // Highest stake first
        assert_eq!(addrs[0].address, Address([3; 20]));
        assert_eq!(addrs[1].address, Address([2; 20]));
        assert_eq!(addrs[2].address, Address([1; 20]));
    }

    #[test]
    fn epoch_rotation_on_boundary() {
        let v = vec![make_validator(1, MIN_VALIDATOR_STAKE)];
        let mut mgr = EpochManager::new(v.clone(), 10);
        // Provide updated candidates for next epoch
        mgr.update_candidates(vec![
            make_validator(1, MIN_VALIDATOR_STAKE),
            make_validator(2, MIN_VALIDATOR_STAKE * 2),
        ]);
        // Process blocks 1–9 — no rotation
        for b in 1..10 {
            let ev = mgr.on_block_committed(b, H256::zero());
            assert!(matches!(ev, EpochEvent::BlockProcessed { .. }));
        }
        // Block 10 = epoch boundary
        let ev = mgr.on_block_committed(10, H256::zero());
        assert!(matches!(ev, EpochEvent::EpochTransition { new_epoch: 2, .. }));
        assert_eq!(mgr.current_epoch(), 2);
        assert_eq!(mgr.current_validators().len(), 2);
    }

    #[test]
    fn blocks_until_rotation() {
        let v = vec![make_validator(1, MIN_VALIDATOR_STAKE)];
        let mgr = EpochManager::new(v, 100);
        assert_eq!(mgr.blocks_until_rotation(1), 99);
        assert_eq!(mgr.blocks_until_rotation(90), 10);
        assert_eq!(mgr.blocks_until_rotation(100), 0);
    }

    #[test]
    fn history_kept_after_rotation() {
        let v = vec![make_validator(1, MIN_VALIDATOR_STAKE)];
        let mut mgr = EpochManager::new(v, 5);
        mgr.on_block_committed(5, H256::zero()); // → epoch 2
        assert!(mgr.epoch_state(1).is_some(), "epoch 1 should be in history");
        assert!(mgr.epoch_state(2).is_some(), "epoch 2 is current");
    }

    #[test]
    fn below_min_stake_filtered_out() {
        let v = vec![make_validator(1, MIN_VALIDATOR_STAKE)];
        let mut mgr = EpochManager::new(v, 5);
        // Candidate below min stake should be filtered
        mgr.update_candidates(vec![
            make_validator(2, MIN_VALIDATOR_STAKE - 1),
            make_validator(3, MIN_VALIDATOR_STAKE),
        ]);
        mgr.on_block_committed(5, H256::zero());
        let validators = mgr.current_validators();
        assert_eq!(validators.len(), 1);
        assert_eq!(validators[0].address, Address([3; 20]));
    }

    #[test]
    fn quorum_sizes() {
        let make_n = |n: u8| -> Vec<ValidatorEntry> {
            (1..=n).map(|i| make_validator(i, MIN_VALIDATOR_STAKE)).collect()
        };
        let state_1  = EpochState::new(1, 100, make_n(1),  H256::zero());
        let state_3  = EpochState::new(1, 100, make_n(3),  H256::zero());
        let state_10 = EpochState::new(1, 100, make_n(10), H256::zero());
        // quorum = ⌈2n/3⌉ + 1
        assert_eq!(state_1.quorum(),  1);  // 2*1/3+1=1
        assert_eq!(state_3.quorum(),  3);  // 2*3/3+1=3
        assert_eq!(state_10.quorum(), 7);  // 2*10/3+1=7 (6+1)
    }
}
