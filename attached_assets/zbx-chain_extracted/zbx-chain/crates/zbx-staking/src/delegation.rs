//! Delegation: per-address delegation records and reward-share tracking.
//!
//! ## Overview
//!
//! A delegator assigns their ZBX stake to a validator.  The validator's
//! `delegated_stake` increases and their `pool_denominator` is updated at
//! the next reward-distribution boundary.  Delegators earn a pro-rata
//! share of the validator's `delegator_reward_pool`, less the validator's
//! commission rate.
//!
//! ## Delegation invariants
//!
//! 1. One delegator may delegate to multiple validators simultaneously.
//! 2. Delegations are subject to the same `UNBONDING_PERIOD` as self-stake.
//! 3. The stake contributed to `pool_denominator` is the snapshot at the
//!    last reward-distribution block — not the live `delegated_stake`.
//!    (See `validator.rs` STK-DEL-01 fix for rationale.)
//! 4. A delegator cannot delegate more than their free EVM balance (the
//!    caller must deduct from the EVM account before calling `delegate`).
//!
//! ## Reward claiming
//!
//! Delegators do not receive continuous micro-payments.  Instead the
//! validator's `delegator_reward_pool` accumulates and delegators call
//! `ClaimDelegatorRewards` to pull their proportional share.  The share is
//! computed as:
//!
//! ```text
//! delegator_share = (delegator_stake / pool_denominator) × delegator_reward_pool
//! ```
//!
//! where `pool_denominator` is the snapshot taken by `RewardDistributor`.

use crate::{error::StakingError, validator::ValidatorSet, UNBONDING_PERIOD};
use zbx_types::address::Address;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

// ── DelegationRecord ──────────────────────────────────────────────────────────

/// A single delegation from one delegator to one validator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationRecord {
    /// The validator receiving the delegation.
    pub validator: Address,
    /// Current active delegation amount (wei).
    pub amount: u128,
    /// Block number of the most recent delegation or top-up.
    pub last_updated_block: u64,
    /// Pending unbonding entries for this delegation.
    pub unbonding: Vec<DelegationUnbond>,
}

/// An unbonding chunk from a delegation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationUnbond {
    pub amount: u128,
    pub release_block: u64,
}

impl DelegationRecord {
    pub fn new(validator: Address, amount: u128, block: u64) -> Self {
        DelegationRecord {
            validator,
            amount,
            last_updated_block: block,
            unbonding: vec![],
        }
    }

    /// Active stake amount.
    pub fn active(&self) -> u128 { self.amount }

    /// Sum of unbonding chunks.
    pub fn total_unbonding(&self) -> u128 {
        self.unbonding.iter().map(|u| u.amount).sum()
    }
}

// ── DelegationKey ─────────────────────────────────────────────────────────────

/// Compound key: (delegator, validator).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DelegationKey {
    pub delegator: Address,
    pub validator: Address,
}

// ── DelegationRegistry ────────────────────────────────────────────────────────

/// Manages all delegation records across the network.
pub struct DelegationRegistry {
    /// (delegator → validator) → DelegationRecord
    records: HashMap<DelegationKey, DelegationRecord>,
    /// Reverse index: validator → set of delegators (for reward distribution)
    validator_delegators: HashMap<Address, Vec<Address>>,
}

impl DelegationRegistry {
    pub fn new() -> Self {
        DelegationRegistry {
            records: HashMap::new(),
            validator_delegators: HashMap::new(),
        }
    }

    /// Delegate `amount` wei from `delegator` to `validator`.
    ///
    /// Updates `ValidatorSet` to reflect the new delegated stake.
    pub fn delegate(
        &mut self,
        delegator: Address,
        validator: Address,
        amount: u128,
        current_block: u64,
        validators: &mut ValidatorSet,
    ) -> Result<(), StakingError> {
        if amount == 0 {
            return Err(StakingError::InvalidAmount);
        }
        // Verify validator exists and is not inactive.
        validators.delegate(&validator, amount)?;

        let key = DelegationKey { delegator: delegator.clone(), validator: validator.clone() };
        let record = self.records.entry(key).or_insert_with(|| {
            DelegationRecord::new(validator.clone(), 0, current_block)
        });
        record.amount = record.amount.saturating_add(amount);
        record.last_updated_block = current_block;

        // Update reverse index.
        self.validator_delegators
            .entry(validator.clone())
            .or_default()
            .push(delegator.clone());
        self.validator_delegators
            .get_mut(&validator)
            .unwrap()
            .dedup();

        info!(
            ?delegator,
            ?validator,
            amount,
            total = record.amount,
            "delegation: delegated"
        );
        Ok(())
    }

    /// Begin undelegation of `amount` wei from `validator`.
    ///
    /// Moves stake from active → unbonding.  Updates `ValidatorSet`.
    pub fn begin_undelegate(
        &mut self,
        delegator: &Address,
        validator: &Address,
        amount: u128,
        current_block: u64,
        validators: &mut ValidatorSet,
    ) -> Result<u64, StakingError> {
        if amount == 0 {
            return Err(StakingError::InvalidAmount);
        }
        let key = DelegationKey { delegator: delegator.clone(), validator: validator.clone() };
        let record = self.records.get_mut(&key).ok_or(StakingError::NoDelegation)?;

        if record.amount < amount {
            return Err(StakingError::InsufficientStake);
        }
        record.amount -= amount;
        let release_block = current_block + UNBONDING_PERIOD;
        record.unbonding.push(DelegationUnbond { amount, release_block });

        validators.undelegate(validator, amount)?;

        info!(
            ?delegator,
            ?validator,
            amount,
            release_block,
            "delegation: undelegation started"
        );
        Ok(release_block)
    }

    /// Finalise matured undelegations for `delegator` + `validator`.
    ///
    /// Returns the total claimable amount.
    pub fn finalise_undelegate(
        &mut self,
        delegator: &Address,
        validator: &Address,
        current_block: u64,
    ) -> u128 {
        let key = DelegationKey { delegator: delegator.clone(), validator: validator.clone() };
        let record = match self.records.get_mut(&key) {
            Some(r) => r,
            None => return 0,
        };
        let mut released = 0u128;
        record.unbonding.retain(|u| {
            if current_block >= u.release_block {
                released = released.saturating_add(u.amount);
                false
            } else {
                true
            }
        });
        if released > 0 {
            debug!(?delegator, ?validator, released, "delegation: unbonding matured");
        }
        released
    }

    /// Compute the claimable reward for `delegator` from `validator`.
    ///
    /// Uses the snapshot `pool_denominator` from the `Validator` struct
    /// (STK-DEL-01) to avoid the front-running dilution attack.
    pub fn claimable_reward(
        &self,
        delegator: &Address,
        validator: &Address,
        validators: &ValidatorSet,
    ) -> Result<u128, StakingError> {
        let key = DelegationKey { delegator: delegator.clone(), validator: validator.clone() };
        let record = self.records.get(&key).ok_or(StakingError::NoDelegation)?;

        let v = validators.get(validator).ok_or(StakingError::UnknownValidator)?;

        if v.pool_denominator == 0 || record.amount == 0 {
            return Ok(0);
        }

        // share = (delegator_stake / pool_denominator) × delegator_reward_pool
        // Use u256-like arithmetic to avoid overflow.
        let reward = (v.delegator_reward_pool as u128)
            .checked_mul(record.amount)
            .map(|n| n / v.pool_denominator)
            .unwrap_or(0);

        Ok(reward)
    }

    /// Apply a slash to all delegators of `validator` proportionally.
    ///
    /// Called by the slashing pipeline after a validator is slashed.
    /// Each delegator's `amount` is reduced proportionally to the fraction
    /// of the validator's total `delegated_stake` they represent.
    pub fn apply_delegator_slash(
        &mut self,
        validator: &Address,
        slash_fraction_bps: u128, // basis points, e.g. 500 = 5%
    ) {
        let delegators: Vec<Address> = self
            .validator_delegators
            .get(validator)
            .cloned()
            .unwrap_or_default();

        for delegator in delegators {
            let key = DelegationKey { delegator: delegator.clone(), validator: validator.clone() };
            if let Some(record) = self.records.get_mut(&key) {
                let slash = record.amount * slash_fraction_bps / 10_000;
                record.amount = record.amount.saturating_sub(slash);
                debug!(
                    ?delegator,
                    ?validator,
                    slash,
                    remaining = record.amount,
                    "delegation: proportional slash applied"
                );
            }
        }
    }

    /// Get all delegators for a given validator.
    pub fn delegators_of(&self, validator: &Address) -> Vec<Address> {
        self.validator_delegators
            .get(validator)
            .cloned()
            .unwrap_or_default()
    }

    /// Get delegation record for (delegator, validator).
    pub fn get(
        &self,
        delegator: &Address,
        validator: &Address,
    ) -> Option<&DelegationRecord> {
        let key = DelegationKey { delegator: delegator.clone(), validator: validator.clone() };
        self.records.get(&key)
    }

    /// Iterate over all delegation records.
    pub fn iter(&self) -> impl Iterator<Item = (&DelegationKey, &DelegationRecord)> {
        self.records.iter()
    }
}

impl Default for DelegationRegistry {
    fn default() -> Self { Self::new() }
}
