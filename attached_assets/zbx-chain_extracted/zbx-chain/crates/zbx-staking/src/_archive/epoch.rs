//! Epoch management: transitions, validator set rotation, reward settlement.
//!
//! An epoch is a fixed number of blocks (`EPOCH_LENGTH = 14_400` ≈ 20 hours at 5s blocks).
//! At each epoch boundary:
//! 1. Settle rewards for the outgoing epoch.
//! 2. Compute next validator set (top N by total stake).
//! 3. Emit an epoch-transition event.

use crate::{
    error::StakingError,
    validator::{ValidatorSet, ValidatorStatus},
    rewards::RewardDistributor,
    EPOCH_LENGTH, MAX_VALIDATORS,
};
use zbx_types::{address::Address, U256};
use serde::{Serialize, Deserialize};
use tracing::info;

/// Epoch metadata stored on-chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochInfo {
    pub epoch:              u64,
    pub start_block:        u64,
    pub end_block:          u64,
    pub validator_count:    u32,
    pub total_stake:        U256,
    pub total_rewards:      U256,
    pub base_reward_rate:   U256,  // per-block reward
}

impl EpochInfo {
    pub fn genesis() -> Self {
        Self {
            epoch:            0,
            start_block:      0,
            end_block:        EPOCH_LENGTH - 1,
            validator_count:  0,
            total_stake:      U256::zero(),
            total_rewards:    U256::zero(),
            base_reward_rate: U256::from(3_000_000_000_000_000_000u128), // 3 ZBX
        }
    }
}

/// Manager for epoch transitions.
pub struct EpochManager {
    current:    EpochInfo,
    history:    Vec<EpochInfo>,
    validators: ValidatorSet,
}

impl EpochManager {
    pub fn new(genesis_validators: ValidatorSet) -> Self {
        Self {
            current:    EpochInfo::genesis(),
            history:    Vec::new(),
            validators: genesis_validators,
        }
    }

    pub fn current_epoch(&self) -> u64 { self.current.epoch }

    pub fn epoch_for_block(&self, block: u64) -> u64 { block / EPOCH_LENGTH }

    pub fn is_epoch_boundary(&self, block: u64) -> bool {
        block > 0 && block % EPOCH_LENGTH == 0
    }

    /// Process an epoch transition at `block_number`.
    pub fn transition(
        &mut self,
        block_number:    u64,
        total_gas_fees:  U256,
    ) -> Result<EpochTransition, StakingError> {
        let new_epoch = block_number / EPOCH_LENGTH;
        if new_epoch <= self.current.epoch {
            return Err(StakingError::EpochNotAdvanced);
        }

        info!("epoch transition: {} → {} at block #{}", self.current.epoch, new_epoch, block_number);

        // 1. Settle block rewards for the outgoing epoch.
        let blocks_in_epoch = EPOCH_LENGTH;
        let block_rewards = self.current.base_reward_rate * U256::from(blocks_in_epoch);
        let total_rewards = block_rewards + total_gas_fees;

        // 2. Rotate validator set: top MAX_VALIDATORS by stake.
        let active_validators = self.validators.active_sorted();
        let next_validators: Vec<Address> = active_validators
            .iter()
            .take(MAX_VALIDATORS)
            .map(|v| v.address)
            .collect();

        let total_stake: U256 = active_validators.iter()
            .take(MAX_VALIDATORS)
            .map(|v| v.self_stake + v.total_delegated)
            .fold(U256::zero(), |a, b| a + b);

        // 3. Update epoch info.
        let completed = std::mem::replace(&mut self.current, EpochInfo {
            epoch:            new_epoch,
            start_block:      block_number,
            end_block:        block_number + EPOCH_LENGTH - 1,
            validator_count:  next_validators.len() as u32,
            total_stake,
            total_rewards:    U256::zero(),
            base_reward_rate: self.halving_reward(new_epoch),
        });

        self.history.push(completed.clone());

        Ok(EpochTransition {
            epoch:             new_epoch,
            new_validators:    next_validators,
            total_rewards,
            completed_epoch:   completed,
        })
    }

    /// Calculate the block reward for a given epoch (halving every 25M blocks).
    fn halving_reward(&self, epoch: u64) -> U256 {
        let blocks = epoch * EPOCH_LENGTH;
        let halvings = blocks / 25_000_000;
        let base = 3_000_000_000_000_000_000u128; // 3 ZBX in wei
        if halvings >= 64 { return U256::zero(); }
        U256::from(base >> halvings)
    }

    pub fn history(&self) -> &[EpochInfo] { &self.history }

    pub fn get_epoch_info(&self, epoch: u64) -> Option<&EpochInfo> {
        if epoch == self.current.epoch { return Some(&self.current); }
        self.history.iter().rev().find(|e| e.epoch == epoch)
    }
}

/// Emitted at the end of each epoch.
#[derive(Debug, Clone)]
pub struct EpochTransition {
    pub epoch:           u64,
    pub new_validators:  Vec<Address>,
    pub total_rewards:   U256,
    pub completed_epoch: EpochInfo,
}