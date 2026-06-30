//! Delegated staking: token holders delegate ZBX to validators.
//!
//! Delegation model:
//! - Delegators stake to a validator (not directly to consensus).
//! - Rewards are split proportionally among delegators after commission.
//! - Undelegation has a 28-epoch cooldown before funds are released.

use crate::{error::StakingError, EPOCH_LENGTH};
use zbx_types::{address::Address, U256};
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// Cooldown before delegated tokens can be withdrawn after undelegation.
pub const UNDELEGATE_COOLDOWN_EPOCHS: u64 = 28;

/// A delegation record from a delegator to a validator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delegation {
    pub delegator:     Address,
    pub validator:     Address,
    pub staked:        U256,       // amount currently staked
    pub pending_rewards: U256,     // accumulated unclaimed rewards
    pub since_epoch:   u64,        // epoch when delegation was created
    pub last_claim:    u64,        // epoch of last reward claim
}

/// A pending undelegation (funds locked for cooldown period).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUndelegation {
    pub delegator:       Address,
    pub validator:       Address,
    pub amount:          U256,
    pub release_epoch:   u64,
    pub created_epoch:   u64,
}

/// The full delegation state.
#[derive(Debug, Default)]
pub struct DelegationRegistry {
    /// delegator → validator → delegation record
    delegations: HashMap<Address, HashMap<Address, Delegation>>,
    /// pending undelegations
    pending:     Vec<PendingUndelegation>,
}

impl DelegationRegistry {
    pub fn new() -> Self { Self::default() }

    /// Delegate `amount` from `delegator` to `validator` at `epoch`.
    pub fn delegate(
        &mut self,
        delegator: Address,
        validator: Address,
        amount:    U256,
        epoch:     u64,
    ) -> Result<(), StakingError> {
        if amount.is_zero() {
            return Err(StakingError::InvalidAmount(amount));
        }
        let entry = self.delegations
            .entry(delegator).or_default()
            .entry(validator).or_insert_with(|| Delegation {
                delegator,
                validator,
                staked:          U256::zero(),
                pending_rewards: U256::zero(),
                since_epoch:     epoch,
                last_claim:      epoch,
            });
        entry.staked = entry.staked + amount;
        Ok(())
    }

    /// Begin undelegation of `amount` — starts cooldown timer.
    pub fn undelegate(
        &mut self,
        delegator: Address,
        validator: Address,
        amount:    U256,
        epoch:     u64,
    ) -> Result<(), StakingError> {
        let entry = self.delegations
            .get_mut(&delegator)
            .and_then(|m| m.get_mut(&validator))
            .ok_or(StakingError::ValidatorNotFound(validator))?;

        if amount > entry.staked {
            return Err(StakingError::InvalidAmount(amount));
        }
        entry.staked = entry.staked - amount;

        self.pending.push(PendingUndelegation {
            delegator,
            validator,
            amount,
            release_epoch: epoch + UNDELEGATE_COOLDOWN_EPOCHS,
            created_epoch: epoch,
        });
        Ok(())
    }

    /// Claim matured undelegations for a delegator.
    /// Returns total amount released.
    pub fn claim_undelegations(
        &mut self,
        delegator: Address,
        current_epoch: u64,
    ) -> U256 {
        let mut released = U256::zero();
        self.pending.retain(|p| {
            if p.delegator == delegator && p.release_epoch <= current_epoch {
                released = released + p.amount;
                false
            } else {
                true
            }
        });
        released
    }

    /// Credit epoch rewards to all delegators of `validator`.
    pub fn credit_rewards(
        &mut self,
        validator: Address,
        total_reward: U256,
        commission_bps: u64, // e.g. 500 = 5%
    ) {
        let commission = total_reward * U256::from(commission_bps) / U256::from(10_000u64);
        let distributable = total_reward - commission;

        // Compute total stake delegated to this validator.
        let total_stake: U256 = self.delegations.values()
            .filter_map(|m| m.get(&validator))
            .map(|d| d.staked)
            .fold(U256::zero(), |a, b| a + b);

        if total_stake.is_zero() { return; }

        // Distribute proportionally.
        for delegator_map in self.delegations.values_mut() {
            if let Some(d) = delegator_map.get_mut(&validator) {
                if d.staked.is_zero() { continue; }
                let share = distributable * d.staked / total_stake;
                d.pending_rewards = d.pending_rewards + share;
            }
        }
    }

    /// Claim pending rewards for a delegator → validator pair.
    pub fn claim_rewards(
        &mut self,
        delegator: Address,
        validator: Address,
        epoch:     u64,
    ) -> Result<U256, StakingError> {
        let entry = self.delegations
            .get_mut(&delegator)
            .and_then(|m| m.get_mut(&validator))
            .ok_or(StakingError::ValidatorNotFound(validator))?;
        let rewards = entry.pending_rewards;
        entry.pending_rewards = U256::zero();
        entry.last_claim = epoch;
        Ok(rewards)
    }

    /// Get all delegations for a validator.
    pub fn delegators_of(&self, validator: Address) -> Vec<&Delegation> {
        self.delegations.values()
            .filter_map(|m| m.get(&validator))
            .collect()
    }

    /// Get all delegations by a delegator.
    pub fn delegations_of(&self, delegator: Address) -> Vec<&Delegation> {
        self.delegations.get(&delegator)
            .map(|m| m.values().collect())
            .unwrap_or_default()
    }

    /// Total delegated stake to a validator.
    pub fn total_delegated(&self, validator: Address) -> U256 {
        self.delegators_of(validator).iter().map(|d| d.staked).fold(U256::zero(), |a, b| a + b)
    }
}