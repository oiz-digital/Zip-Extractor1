//! RewardPool — Rust bindings and logic for the on-chain reward pool.
//!
//! Mirrors RewardPool.sol logic in Rust for the ZBX node's internal accounting.
//! The on-chain contract is the canonical source of truth;
//! this Rust module is used for pre-validation and analytics.

/// Annual emission cap: 5% of 1B ZBX supply = 50M ZBX/year
pub const ANNUAL_EMISSION_CAP_WEI: u128 = 50_000_000 * 1_000_000_000_000_000_000u128;

/// Per-epoch emission = 50M / 365 epochs (≈ 136,986 ZBX/epoch)
pub const EPOCH_EMISSION_WEI: u128 = ANNUAL_EMISSION_CAP_WEI / 365;

/// Epoch length in blocks (43,200 blocks × 2s = ~1 day)
pub const EPOCH_BLOCKS: u64 = 43_200;

/// Precision for reward_per_token
pub const REWARD_PRECISION: u128 = 1_000_000_000_000_000_000u128;

/// Rust-side reward pool state (mirrors RewardPool.sol).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RewardPool {
    /// Accumulated reward per staked token (× PRECISION)
    pub reward_per_token:   u128,
    /// ZBX available for rewards (not yet distributed)
    pub pool_balance:       u128,
    /// Total rewards distributed since genesis
    pub total_rewards_ever: u128,
    /// Current epoch number
    pub current_epoch:      u64,
    /// Block at which current epoch started
    pub epoch_start_block:  u64,
}

impl RewardPool {
    pub fn new(epoch_start_block: u64) -> Self {
        Self {
            reward_per_token:   0,
            pool_balance:       0,
            total_rewards_ever: 0,
            current_epoch:      0,
            epoch_start_block,
        }
    }

    /// Deposit ZBX into the pool (from block rewards / fee surplus).
    pub fn deposit(&mut self, amount: u128) {
        self.pool_balance += amount;
        tracing::debug!(amount = amount, balance = self.pool_balance, "RewardPool deposit");
    }

    /// Settle epoch rewards (called when EPOCH_BLOCKS have passed).
    pub fn settle_epoch(
        &mut self,
        current_block: u64,
        total_staked:  u128,
        epoch_reward:  u128,
    ) -> Result<u128, RewardPoolError> {
        let blocks_elapsed = current_block.saturating_sub(self.epoch_start_block);
        if blocks_elapsed < EPOCH_BLOCKS {
            return Err(RewardPoolError::EpochNotFinished {
                remaining: EPOCH_BLOCKS - blocks_elapsed,
            });
        }
        if epoch_reward > EPOCH_EMISSION_WEI {
            return Err(RewardPoolError::EmissionCapExceeded {
                requested: epoch_reward,
                cap:       EPOCH_EMISSION_WEI,
            });
        }

        let actual_reward = epoch_reward.min(self.pool_balance);
        if total_staked == 0 || actual_reward == 0 {
            self.advance_epoch(current_block);
            return Ok(0);
        }

        // Update global accumulator — O(1)
        self.reward_per_token   += (actual_reward * REWARD_PRECISION) / total_staked;
        self.pool_balance       -= actual_reward;
        self.total_rewards_ever += actual_reward;

        tracing::info!(
            epoch     = self.current_epoch,
            reward    = actual_reward,
            rpt       = self.reward_per_token,
            staked    = total_staked,
            "Epoch reward settled"
        );

        self.advance_epoch(current_block);
        Ok(actual_reward)
    }

    /// Compute pending reward for a delegator (view, no state change).
    pub fn pending_reward(
        &self,
        staked:      u128,
        checkpoint:  u128,
        accumulated: u128,
    ) -> u128 {
        let delta = self.reward_per_token.saturating_sub(checkpoint);
        accumulated + (staked * delta / REWARD_PRECISION)
    }

    /// Estimated APR in basis points for a given epoch reward.
    pub fn estimated_apr_bps(&self, epoch_reward: u128, total_staked: u128) -> u16 {
        if total_staked == 0 { return 0; }
        let annual = epoch_reward * 365;
        let bps = (annual * 10_000) / total_staked;
        bps.min(u16::MAX as u128) as u16
    }

    fn advance_epoch(&mut self, current_block: u64) {
        self.current_epoch     += 1;
        self.epoch_start_block  = current_block;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RewardPoolError {
    #[error("epoch not finished: {remaining} blocks remaining")]
    EpochNotFinished { remaining: u64 },
    #[error("emission cap exceeded: requested {requested}, cap {cap}")]
    EmissionCapExceeded { requested: u128, cap: u128 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_emission_cap_is_reasonable() {
        // 50M ZBX/year ÷ 365 ≈ 136,986 ZBX/epoch
        let per_epoch = EPOCH_EMISSION_WEI / 1_000_000_000_000_000_000u128;
        assert!(per_epoch > 100_000 && per_epoch < 200_000);
    }

    #[test]
    fn deposit_and_settle_epoch() {
        let mut pool = RewardPool::new(0);
        let total_staked = 1_000_000 * 1_000_000_000_000_000_000u128; // 1M ZBX

        // Deposit 1000 ZBX
        let deposit = 1_000 * 1_000_000_000_000_000_000u128;
        pool.deposit(deposit);

        let distributed = pool
            .settle_epoch(EPOCH_BLOCKS, total_staked, deposit)
            .unwrap();
        assert_eq!(distributed, deposit);
        assert_eq!(pool.pool_balance, 0);
        assert_eq!(pool.current_epoch, 1);
        assert!(pool.reward_per_token > 0);
    }

    #[test]
    fn epoch_not_finished_error() {
        let mut pool = RewardPool::new(0);
        pool.deposit(1_000_000_000_000_000_000u128);
        let err = pool.settle_epoch(100, 1_000_000_000_000_000_000u128, 100).unwrap_err();
        assert!(matches!(err, RewardPoolError::EpochNotFinished { .. }));
    }

    #[test]
    fn pending_reward_calculation() {
        let pool = RewardPool {
            reward_per_token:   1_000_000_000_000_000_000u128, // 1.0 (normalized)
            pool_balance:       0,
            total_rewards_ever: 0,
            current_epoch:      1,
            epoch_start_block:  0,
        };
        let staked      = 100 * 1_000_000_000_000_000_000u128; // 100 ZBX staked
        let checkpoint  = 0u128;
        let accumulated = 0u128;
        let pending = pool.pending_reward(staked, checkpoint, accumulated);
        // 100 ZBX × 1.0 rpt = 100 ZBX pending
        assert_eq!(pending, staked);
    }
}