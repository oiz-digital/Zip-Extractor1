//! Block reward — 3 ZBX initial, halves every 25 million blocks.
//!
//! All math runs in u128 (sufficient for any realistic reward+fee combination)
//! and then widens to U256 for storage / external API. The previous
//! implementation called `.as_u64()` on a U256 which silently dropped the
//! high 192 bits, truncating large rewards to `reward % 2^64`.
//! See AUDIT_2026-04-30.md C-12.

use serde::{Deserialize, Serialize};
use zbx_primitives::{Address, U256};

pub const INITIAL_REWARD: u128 = 3_000_000_000_000_000_000u128; // 3 ZBX
pub const HALVING_INTERVAL: u64 = 25_000_000;
pub const VALIDATOR_BPS: u64 = 8000;  // 80%
pub const STAKER_BPS:    u64 = 1500;  // 15%
pub const FOUNDATION_BPS: u64 = 500;  //  5%

/// Maximum allowed (base + fee) per block. Acts as an inflation-safety bound:
/// any block whose computed reward would exceed this is rejected at the
/// executor level rather than silently producing wrapped values. 2^120 wei is
/// vastly larger than any plausible per-block reward (≈10^36 ZBX).
pub const MAX_REWARD_PER_BLOCK: u128 = 1u128 << 120;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockReward {
    pub block_number:      u64,
    pub validator:         Address,
    pub total_reward:      U256,
    pub validator_reward:  U256,
    pub staker_reward:     U256,
    pub foundation_reward: U256,
    pub fee_reward:        U256,
    pub halving_epoch:     u32,
}

#[derive(Debug, thiserror::Error)]
pub enum RewardError {
    #[error("reward overflow: base+fee exceeds MAX_REWARD_PER_BLOCK")]
    Overflow,
}

pub struct RewardEngine { pub foundation: Address }

impl RewardEngine {
    pub fn new(foundation: Address) -> Self { Self { foundation } }

    /// Returns the base subsidy (in wei) at `block`.
    pub fn base_reward_u128(&self, block: u64) -> u128 {
        let halvings = block / HALVING_INTERVAL;
        if halvings >= 64 { return 0; }
        INITIAL_REWARD >> halvings
    }

    pub fn base_reward(&self, block: u64) -> U256 {
        U256::from_u128(self.base_reward_u128(block))
    }

    /// Compute a `BlockReward`. `priority_fee` is per-gas in wei.
    /// Returns `Err(Overflow)` if the math would wrap.
    pub fn compute_checked(
        &self,
        block: u64,
        validator: Address,
        gas_used: u64,
        priority_fee: u128,
    ) -> Result<BlockReward, RewardError> {
        let base = self.base_reward_u128(block);
        let fee  = (gas_used as u128).checked_mul(priority_fee).ok_or(RewardError::Overflow)?;
        let total = base.checked_add(fee).ok_or(RewardError::Overflow)?;
        if total > MAX_REWARD_PER_BLOCK { return Err(RewardError::Overflow); }

        let vr = bps(total, VALIDATOR_BPS);
        let sr = bps(total, STAKER_BPS);
        // Foundation gets the remainder (avoids dust loss from integer division).
        let fr = total.saturating_sub(vr).saturating_sub(sr);

        Ok(BlockReward {
            block_number: block,
            validator,
            total_reward:      U256::from_u128(total),
            validator_reward:  U256::from_u128(vr),
            staker_reward:     U256::from_u128(sr),
            foundation_reward: U256::from_u128(fr),
            fee_reward:        U256::from_u128(fee),
            halving_epoch:     (block / HALVING_INTERVAL) as u32,
        })
    }

    /// Back-compat wrapper that saturates instead of erroring. Prefer
    /// `compute_checked` in new code paths.
    pub fn compute(&self, block: u64, validator: Address, gas_used: u64, priority_fee: U256) -> BlockReward {
        let pfee = priority_fee.as_u128_lossy();
        match self.compute_checked(block, validator, gas_used, pfee) {
            Ok(r) => r,
            Err(_) => BlockReward {
                block_number: block,
                validator,
                total_reward:      U256::from_u128(MAX_REWARD_PER_BLOCK),
                validator_reward:  U256::from_u128(bps(MAX_REWARD_PER_BLOCK, VALIDATOR_BPS)),
                staker_reward:     U256::from_u128(bps(MAX_REWARD_PER_BLOCK, STAKER_BPS)),
                foundation_reward: U256::from_u128(
                    MAX_REWARD_PER_BLOCK
                        .saturating_sub(bps(MAX_REWARD_PER_BLOCK, VALIDATOR_BPS))
                        .saturating_sub(bps(MAX_REWARD_PER_BLOCK, STAKER_BPS))
                ),
                fee_reward:        U256::from_u128(MAX_REWARD_PER_BLOCK.saturating_sub(self.base_reward_u128(block))),
                halving_epoch:     (block / HALVING_INTERVAL) as u32,
            }
        }
    }
}

fn bps(v: u128, bps: u64) -> u128 {
    // (v * bps) / 10_000, fully in u128. Since v ≤ 2^120 and bps ≤ 10_000 < 2^14,
    // the product is ≤ 2^134 → safe in u128 only via splitting; we widen to u256-like
    // via a mul_high helper:
    let (lo, hi) = mul_u128_split(v, bps as u128);
    div_by_10000(lo, hi)
}

#[inline]
fn mul_u128_split(a: u128, b: u128) -> (u128, u128) {
    let a_lo = a as u64 as u128;
    let a_hi = a >> 64;
    let b_lo = b as u64 as u128;
    let b_hi = b >> 64;
    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;
    let mid = (ll >> 64) + (lh & ((1u128 << 64) - 1)) + (hl & ((1u128 << 64) - 1));
    let lo = (ll & ((1u128 << 64) - 1)) | (mid << 64);
    let hi = hh + (lh >> 64) + (hl >> 64) + (mid >> 64);
    (lo, hi)
}

#[inline]
fn div_by_10000(lo: u128, hi: u128) -> u128 {
    // Long-division of (hi:lo) by 10_000, returning low 128 bits of quotient.
    let mut rem: u128 = hi % 10_000;
    let _q_hi = hi / 10_000;
    let lo_hi = lo >> 64;
    let lo_lo = lo & ((1u128 << 64) - 1);
    let n1 = (rem << 64) | lo_hi;
    let q1 = n1 / 10_000;
    rem = n1 % 10_000;
    let n0 = (rem << 64) | lo_lo;
    let q0 = n0 / 10_000;
    (q1 << 64) | q0
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_primitives::Address;

    #[test]
    fn no_truncation_on_high_fee() {
        let eng = RewardEngine::new(Address::default());
        // 30 M gas at 100 gwei priority = 3e18, plus 3 ZBX subsidy = 6e18.
        // The legacy `as_u64()` path would have wrapped at 1.8e19 — this stays clean.
        let r = eng.compute_checked(1_000, Address::default(), 30_000_000, 100_000_000_000).unwrap();
        // total = 3e18 + 30e6 * 1e11 = 3e18 + 3e18 = 6e18
        assert_eq!(r.total_reward, U256::from_u128(6_000_000_000_000_000_000u128));
        // validator gets 80% = 4.8e18
        assert_eq!(r.validator_reward, U256::from_u128(4_800_000_000_000_000_000u128));
    }

    #[test]
    fn shares_sum_to_total() {
        let eng = RewardEngine::new(Address::default());
        let r = eng.compute_checked(0, Address::default(), 21_000, 1_000_000_000).unwrap();
        let sum = r.validator_reward.as_u128_lossy()
            + r.staker_reward.as_u128_lossy()
            + r.foundation_reward.as_u128_lossy();
        assert_eq!(sum, r.total_reward.as_u128_lossy());
    }

    #[test]
    fn rejects_overflow() {
        let eng = RewardEngine::new(Address::default());
        // u128::MAX gas at u128::MAX fee → overflow
        assert!(eng.compute_checked(0, Address::default(), u64::MAX, u128::MAX).is_err());
    }
}
