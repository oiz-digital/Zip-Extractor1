//! Stake lock period — validators cannot withdraw stake immediately.
//!
//! # Why Lock Stake?
//!
//! Without a lock period, a validator can:
//!   1. Get elected as block proposer
//!   2. Produce a malicious block (double-spend, front-run, etc.)
//!   3. Immediately withdraw stake before slashing can occur
//!
//! With a lock period, validators must wait 14 days (STAKE_LOCK) after
//! requesting withdrawal — plenty of time for slashing to be applied.
//!
//! # Lock Phases
//!
//! ```
//!  Active staking → [withdraw_request()] → Unbonding (14 days) → Withdrawable
//!
//!  If slashed during unbonding:
//!    → 50% of stake burned, rest returned after lock
//! ```
//!
//! # Constants
//!
//! - STAKE_LOCK:          14 days = 604,800 seconds
//! - UNBONDING_EPOCHS:    14 epochs (1 epoch ≈ 1 day)
//! - MIN_STAKE:           1,000 ZBX
//! - MAX_VALIDATOR_STAKE: 10,000,000 ZBX (prevents centralization)

/// Lock period after withdraw request: 14 days.
pub const STAKE_LOCK: u64 = 14 * 24 * 3600; // 604,800 seconds

/// Number of unbonding epochs.
pub const UNBONDING_EPOCHS: u64 = 14;

/// Minimum stake to become a validator.
pub const MIN_STAKE_WEI: u128 = 1_000 * 1_000_000_000_000_000_000u128; // 1,000 ZBX

/// Maximum stake per validator (prevents >10% concentration).
pub const MAX_STAKE_WEI: u128 = 10_000_000 * 1_000_000_000_000_000_000u128; // 10M ZBX

/// State of a validator's stake.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StakeLockState {
    /// Actively staking — participating in consensus.
    Active {
        amount:      u128,
        staked_at:   u64,
    },
    /// Unbonding — waiting for STAKE_LOCK to expire.
    Unbonding {
        amount:       u128,
        requested_at: u64,
        /// Timestamp after which stake can be withdrawn.
        unlock_at:    u64,
    },
    /// Ready to withdraw — lock has expired.
    Withdrawable {
        amount:    u128,
        unlock_at: u64,
    },
}

impl StakeLockState {
    /// Request withdrawal — start the unbonding clock.
    pub fn request_withdraw(self, now: u64) -> Result<Self, LockError> {
        match self {
            Self::Active { amount, .. } => {
                if amount < MIN_STAKE_WEI {
                    return Err(LockError::BelowMinStake);
                }
                Ok(Self::Unbonding {
                    amount,
                    requested_at: now,
                    unlock_at:    now + STAKE_LOCK,
                })
            }
            Self::Unbonding { .. } => Err(LockError::AlreadyUnbonding),
            Self::Withdrawable { .. } => Err(LockError::AlreadyWithdrawable),
        }
    }

    /// Advance state — check if unbonding period has passed.
    pub fn try_unlock(self, now: u64) -> Self {
        if let Self::Unbonding { amount, unlock_at, .. } = self {
            if now >= unlock_at {
                tracing::info!(amount = amount, "Stake unlock period expired — withdrawable");
                return Self::Withdrawable { amount, unlock_at };
            }
        }
        self
    }

    /// Apply slashing (burns `pct` percent of stake, e.g. 50 = 50%).
    pub fn slash(&mut self, pct: u8) -> u128 {
        let slash_pct = pct.min(100) as u128;
        match self {
            Self::Active { amount, .. } | Self::Unbonding { amount, .. } => {
                let burned = *amount * slash_pct / 100;
                *amount  -= burned;
                tracing::warn!(burned = burned, remaining = *amount, "Stake slashed");
                burned
            }
            Self::Withdrawable { .. } => 0,
        }
    }

    /// Remaining seconds until stake can be withdrawn.
    pub fn time_until_unlock(&self, now: u64) -> Option<u64> {
        if let Self::Unbonding { unlock_at, .. } = self {
            Some(unlock_at.saturating_sub(now))
        } else { None }
    }

    /// Current staked amount.
    pub fn amount(&self) -> u128 {
        match self {
            Self::Active { amount, .. }
            | Self::Unbonding { amount, .. }
            | Self::Withdrawable { amount, .. } => *amount,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("amount below minimum stake (1,000 ZBX)")]
    BelowMinStake,
    #[error("already in unbonding period")]
    AlreadyUnbonding,
    #[error("stake already withdrawable")]
    AlreadyWithdrawable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stake_lock_period_is_14_days() {
        assert_eq!(STAKE_LOCK, 14 * 24 * 3600);
    }

    #[test]
    fn active_stake_enters_unbonding() {
        let stake = StakeLockState::Active {
            amount:    MIN_STAKE_WEI,
            staked_at: 1000,
        };
        let unbonding = stake.request_withdraw(2000).unwrap();
        if let StakeLockState::Unbonding { unlock_at, .. } = unbonding {
            assert_eq!(unlock_at, 2000 + STAKE_LOCK);
        } else { panic!("Expected Unbonding"); }
    }

    #[test]
    fn unbonding_unlocks_after_14_days() {
        let stake = StakeLockState::Unbonding {
            amount:       MIN_STAKE_WEI,
            requested_at: 1000,
            unlock_at:    1000 + STAKE_LOCK,
        };
        let now_after = 1000 + STAKE_LOCK + 1;
        let unlocked = stake.try_unlock(now_after);
        assert!(matches!(unlocked, StakeLockState::Withdrawable { .. }));
    }

    #[test]
    fn unbonding_stays_locked_before_14_days() {
        let stake = StakeLockState::Unbonding {
            amount:       MIN_STAKE_WEI,
            requested_at: 1000,
            unlock_at:    1000 + STAKE_LOCK,
        };
        let now_before = 1000 + STAKE_LOCK - 1;
        let still_locked = stake.try_unlock(now_before);
        assert!(matches!(still_locked, StakeLockState::Unbonding { .. }));
    }

    #[test]
    fn slashing_reduces_stake() {
        let mut stake = StakeLockState::Active {
            amount:    MIN_STAKE_WEI,
            staked_at: 1000,
        };
        let burned = stake.slash(50);
        assert_eq!(burned, MIN_STAKE_WEI / 2);
        assert_eq!(stake.amount(), MIN_STAKE_WEI / 2);
    }
}