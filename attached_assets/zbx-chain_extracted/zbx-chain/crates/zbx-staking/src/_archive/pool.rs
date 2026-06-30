//! StakingPool — pool-based delegation for ZBX validators.
//!
//! Each active validator has a StakingPool that delegators can join.
//! The pool tracks each delegator's share and accumulated rewards.
//!
//! # Reward Algorithm (Synthetix-style)
//!
//! Instead of iterating over all delegators on each reward (O(n)),
//! we use a global accumulator:
//!
//! ```
//! reward_per_token += (new_rewards × PRECISION) / total_staked
//!
//! Per user:
//!   pending_reward = stake × (reward_per_token - checkpoint) / PRECISION
//! ```
//!
//! This is O(1) per operation — used by Synthetix, Compound, Curve.
//!
//! # Pool Lifecycle
//!
//! ```
//! Validator registers → StakingPool created
//! Delegator calls delegate(amount) → shares minted
//! Epoch ends → add_rewards(amount) called by chain
//! Delegator calls claim_reward() → rewards sent
//! Delegator calls undelegate(amount) → enters unbonding
//! After STAKE_LOCK → withdraw()
//! ```

use std::collections::HashMap;
use crate::lock::{STAKE_LOCK, MIN_STAKE_WEI};

/// Precision factor for reward_per_token (prevents rounding loss).
pub const REWARD_PRECISION: u128 = 1_000_000_000_000_000_000u128; // 1e18

/// Pool-based staking pool — one per validator.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StakingPool {
    /// Validator this pool belongs to.
    pub validator:          [u8; 20],
    /// Validator commission rate in basis points (e.g. 500 = 5%).
    pub commission_bps:     u16,
    /// Total ZBX staked in pool (validator self-stake + all delegators).
    pub total_staked:       u128,
    /// Accumulated reward per token unit (× REWARD_PRECISION).
    pub reward_per_token:   u128,
    /// Total rewards distributed to this pool (for APR calculation).
    pub total_rewards_ever: u128,
    /// Per-delegator state.
    pub delegators:         HashMap<[u8; 20], DelegatorPosition>,
    /// Pool creation timestamp.
    pub created_at:         u64,
    /// Whether pool is active (validator is elected).
    pub is_active:          bool,
}

/// A single delegator's position in a staking pool.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DelegatorPosition {
    /// Amount of ZBX staked.
    pub staked:          u128,
    /// Snapshot of reward_per_token at last claim/stake.
    pub reward_checkpoint: u128,
    /// Rewards accumulated but not yet claimed.
    pub pending_reward:  u128,
    /// Timestamp of last action.
    pub last_action:     u64,
}

impl StakingPool {
    /// Create a new staking pool for a validator.
    pub fn new(validator: [u8; 20], commission_bps: u16, now: u64) -> Self {
        Self {
            validator,
            commission_bps,
            total_staked:       0,
            reward_per_token:   0,
            total_rewards_ever: 0,
            delegators:         HashMap::new(),
            created_at:         now,
            is_active:          true,
        }
    }

    /// Delegator stakes ZBX into this pool.
    pub fn delegate(
        &mut self,
        delegator: [u8; 20],
        amount:    u128,
        now:       u64,
    ) -> Result<(), PoolError> {
        if amount < MIN_STAKE_WEI / 100 { // min 10 ZBX for delegation
            return Err(PoolError::BelowMinDelegation);
        }
        if !self.is_active {
            return Err(PoolError::PoolInactive);
        }

        // Settle existing rewards before changing stake
        self.settle_rewards(&delegator);

        let pos = self.delegators.entry(delegator).or_insert(DelegatorPosition {
            staked:            0,
            reward_checkpoint: self.reward_per_token,
            pending_reward:    0,
            last_action:       now,
        });

        pos.staked      += amount;
        pos.last_action  = now;
        self.total_staked += amount;

        tracing::info!(
            delegator = hex::encode(delegator),
            amount    = amount,
            pool      = hex::encode(self.validator),
            "Delegated to staking pool"
        );
        Ok(())
    }

    /// Delegator requests withdrawal of ZBX from pool.
    pub fn undelegate(
        &mut self,
        delegator: [u8; 20],
        amount:    u128,
        now:       u64,
    ) -> Result<UndelegateReceipt, PoolError> {
        let pos = self.delegators.get_mut(&delegator)
            .ok_or(PoolError::NotDelegating)?;

        if amount > pos.staked {
            return Err(PoolError::InsufficientStake { have: pos.staked, want: amount });
        }

        // Settle rewards first
        self.settle_rewards(&delegator);
        let pos = self.delegators.get_mut(&delegator).unwrap();

        pos.staked        -= amount;
        pos.last_action    = now;
        self.total_staked -= amount;

        if pos.staked == 0 {
            // Auto-claim any pending rewards
            let pending = pos.pending_reward;
            self.delegators.remove(&delegator);
            return Ok(UndelegateReceipt {
                amount,
                unlock_at:      now + STAKE_LOCK,
                auto_claimed:   pending,
            });
        }

        Ok(UndelegateReceipt {
            amount,
            unlock_at:    now + STAKE_LOCK,
            auto_claimed: 0,
        })
    }

    /// Add new rewards to the pool (called by chain at epoch end).
    /// Validator commission is deducted first.
    pub fn add_rewards(&mut self, gross_reward: u128) -> u128 {
        if self.total_staked == 0 { return 0; }

        // Validator takes commission
        let commission = gross_reward * self.commission_bps as u128 / 10_000;
        let net_reward = gross_reward - commission;

        // Distribute net reward proportionally via reward_per_token accumulator
        self.reward_per_token += (net_reward * REWARD_PRECISION) / self.total_staked;
        self.total_rewards_ever += gross_reward;

        tracing::debug!(
            pool       = hex::encode(self.validator),
            gross      = gross_reward,
            commission = commission,
            net        = net_reward,
            rpt        = self.reward_per_token,
            "Pool rewards added"
        );

        commission // Return commission amount for validator payout
    }

    /// Claim pending rewards for a delegator.
    pub fn claim_reward(&mut self, delegator: [u8; 20], now: u64) -> Result<u128, PoolError> {
        self.settle_rewards(&delegator);
        let pos = self.delegators.get_mut(&delegator)
            .ok_or(PoolError::NotDelegating)?;

        let reward = pos.pending_reward;
        pos.pending_reward = 0;
        pos.last_action    = now;

        tracing::info!(
            delegator = hex::encode(delegator),
            reward    = reward,
            "Reward claimed from staking pool"
        );

        Ok(reward)
    }

    /// Current pending reward for a delegator (view function).
    pub fn pending_reward_of(&self, delegator: &[u8; 20]) -> u128 {
        let pos = match self.delegators.get(delegator) { Some(p) => p, None => return 0 };
        let delta = self.reward_per_token.saturating_sub(pos.reward_checkpoint);
        pos.pending_reward + (pos.staked * delta / REWARD_PRECISION)
    }

    /// Annualized APR for delegators (approximate, based on last 24h rewards).
    pub fn estimated_apr_bps(&self, last_24h_rewards: u128) -> u16 {
        if self.total_staked == 0 { return 0; }
        // APR = (daily_reward / total_staked) × 365 × 10000 (bps)
        let annual_reward = last_24h_rewards * 365;
        let apr_bps = (annual_reward * 10_000) / self.total_staked;
        apr_bps.min(u16::MAX as u128) as u16
    }

    /// Apply slashing to the entire pool (validator + all delegators).
    pub fn slash_pool(&mut self, pct: u8) -> u128 {
        let slash_pct = pct.min(100) as u128;
        let mut total_burned = 0u128;

        for pos in self.delegators.values_mut() {
            let burned = pos.staked * slash_pct / 100;
            pos.staked    -= burned;
            total_burned  += burned;
        }
        self.total_staked -= total_burned;
        self.is_active     = pct >= 100; // fully slashed → deactivate
        total_burned
    }

    /// Number of delegators in the pool.
    pub fn delegator_count(&self) -> usize { self.delegators.len() }

    // ── Private ──────────────────────────────────────────────────────────────

    /// Settle rewards for a delegator — update pending_reward and checkpoint.
    fn settle_rewards(&mut self, delegator: &[u8; 20]) {
        let rpt = self.reward_per_token;
        if let Some(pos) = self.delegators.get_mut(delegator) {
            let delta = rpt.saturating_sub(pos.reward_checkpoint);
            pos.pending_reward    += pos.staked * delta / REWARD_PRECISION;
            pos.reward_checkpoint  = rpt;
        }
    }
}

/// Receipt returned after undelegation.
#[derive(Debug)]
pub struct UndelegateReceipt {
    pub amount:       u128,
    pub unlock_at:    u64,
    pub auto_claimed: u128,
}

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("below minimum delegation amount (10 ZBX)")]
    BelowMinDelegation,
    #[error("pool is not active")]
    PoolInactive,
    #[error("not delegating to this pool")]
    NotDelegating,
    #[error("insufficient stake: have {have}, want {want}")]
    InsufficientStake { have: u128, want: u128 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validator() -> [u8; 20] { [0xAA; 20] }
    fn delegator1() -> [u8; 20] { [0x01; 20] }
    fn delegator2() -> [u8; 20] { [0x02; 20] }

    #[test]
    fn delegate_and_add_rewards() {
        let mut pool = StakingPool::new(validator(), 500, 0); // 5% commission
        let stake = 10_000 * 1_000_000_000_000_000_000u128; // 10,000 ZBX

        pool.delegate(delegator1(), stake, 0).unwrap();
        pool.delegate(delegator2(), stake * 3, 0).unwrap(); // 3x stake

        assert_eq!(pool.total_staked, stake * 4);

        // Add 400 ZBX reward (gross)
        let reward = 400 * 1_000_000_000_000_000_000u128;
        let commission = pool.add_rewards(reward);
        // 5% of 400 = 20 ZBX commission
        assert_eq!(commission, reward * 5 / 100);

        // delegator1 has 1/4 of pool → earns 1/4 of 380 = 95 ZBX
        let d1_reward = pool.pending_reward_of(&delegator1());
        let d2_reward = pool.pending_reward_of(&delegator2());

        // d2 has 3× stake of d1 → 3× reward
        assert!(d2_reward > d1_reward * 2);
        // Total should be 380 ZBX net (allow small rounding)
        let total_delegator_rewards = d1_reward + d2_reward;
        let net_reward = reward - commission;
        let diff = if total_delegator_rewards > net_reward {
            total_delegator_rewards - net_reward
        } else { net_reward - total_delegator_rewards };
        assert!(diff < 1000); // within 1000 wei rounding
    }

    #[test]
    fn claim_reward_clears_pending() {
        let mut pool = StakingPool::new(validator(), 0, 0);
        let stake = MIN_STAKE_WEI;
        pool.delegate(delegator1(), stake, 0).unwrap();
        pool.add_rewards(1_000_000_000_000_000_000u128);

        let claimed = pool.claim_reward(delegator1(), 1).unwrap();
        assert!(claimed > 0);
        // After claim, pending is 0
        assert_eq!(pool.pending_reward_of(&delegator1()), 0);
    }

    #[test]
    fn slash_reduces_all_stakes() {
        let mut pool = StakingPool::new(validator(), 0, 0);
        let stake = MIN_STAKE_WEI;
        pool.delegate(delegator1(), stake, 0).unwrap();
        pool.delegate(delegator2(), stake, 0).unwrap();

        let burned = pool.slash_pool(50);
        assert_eq!(burned, stake); // 50% of 2×stake
        assert_eq!(pool.total_staked, stake); // 50% remains
    }

    #[test]
    fn undelegate_enters_unbonding() {
        let mut pool = StakingPool::new(validator(), 0, 0);
        let stake = MIN_STAKE_WEI;
        pool.delegate(delegator1(), stake, 1000).unwrap();

        let receipt = pool.undelegate(delegator1(), stake / 2, 2000).unwrap();
        assert_eq!(receipt.amount, stake / 2);
        assert_eq!(receipt.unlock_at, 2000 + STAKE_LOCK);
    }
}