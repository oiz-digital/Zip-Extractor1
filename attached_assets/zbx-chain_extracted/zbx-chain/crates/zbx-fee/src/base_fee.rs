//! EIP-1559 base-fee adjustment algorithm.
//!
//! Each block adjusts the base fee based on how full the previous block was
//! relative to the target (50% of the gas limit).
//!
//! Formula:
//!   if gas_used > gas_target:
//!     base_fee += base_fee * (gas_used - gas_target) / gas_target / DENOMINATOR
//!   else:
//!     base_fee -= base_fee * (gas_target - gas_used) / gas_target / DENOMINATOR
//!
//! ZBX Chain parameters (same as Ethereum post-London):
//!   - ELASTICITY_MULTIPLIER   = 2  (target = limit / 2)
//!   - BASE_FEE_CHANGE_DENOMINATOR = 8  (max 12.5% change per block)
//!   - MIN_BASE_FEE            = 7 wei (floor)

use crate::FeeError;

/// Maximum base-fee change per block expressed as 1/DENOMINATOR.
/// At 8: max ±12.5% change per block.
pub const BASE_FEE_CHANGE_DENOMINATOR: u64 = 8;

/// Gas target = gas_limit / ELASTICITY_MULTIPLIER.
pub const ELASTICITY_MULTIPLIER: u64 = 2;

/// Minimum base fee in wei (floor to prevent zero base fee).
pub const MIN_BASE_FEE: u64 = 7;

pub struct BaseFeeCalculator;

impl BaseFeeCalculator {
    /// Compute next block's base fee.
    ///
    /// # Arguments
    /// * `parent_base_fee`  — Parent block's base fee in wei.
    /// * `parent_gas_used`  — Gas actually used in parent block.
    /// * `parent_gas_limit` — Gas limit of parent block.
    pub fn next_base_fee(
        parent_base_fee:  u64,
        parent_gas_used:  u64,
        parent_gas_limit: u64,
    ) -> Result<u64, FeeError> {
        let gas_target = parent_gas_limit / ELASTICITY_MULTIPLIER;

        let next = if parent_gas_used == gas_target {
            // Perfect utilisation — no change.
            parent_base_fee

        } else if parent_gas_used > gas_target {
            // Block above target — increase base fee.
            let gas_delta  = parent_gas_used - gas_target;
            let fee_delta  = (parent_base_fee as u128)
                .saturating_mul(gas_delta as u128)
                / (gas_target as u128)
                / (BASE_FEE_CHANGE_DENOMINATOR as u128);
            let fee_delta  = fee_delta.max(1) as u64;

            parent_base_fee
                .checked_add(fee_delta)
                .ok_or(FeeError::BaseFeeOverflow)?

        } else {
            // Block below target — decrease base fee.
            let gas_delta  = gas_target - parent_gas_used;
            let fee_delta  = (parent_base_fee as u128)
                .saturating_mul(gas_delta as u128)
                / (gas_target as u128)
                / (BASE_FEE_CHANGE_DENOMINATOR as u128);
            let fee_delta  = fee_delta as u64;

            parent_base_fee.saturating_sub(fee_delta)
        };

        Ok(next.max(MIN_BASE_FEE))
    }

    /// Effective gas price paid by a tx (EIP-1559).
    ///
    /// effective_price = min(max_fee_per_gas, base_fee + max_priority_fee_per_gas)
    pub fn effective_gas_price(
        base_fee:               u64,
        max_fee_per_gas:        u64,
        max_priority_fee_per_gas: u64,
    ) -> Result<u64, FeeError> {
        if max_fee_per_gas < base_fee {
            return Err(FeeError::MaxFeeBelowBaseFee {
                max_fee: max_fee_per_gas,
                base_fee,
            });
        }
        let tip = max_priority_fee_per_gas.min(max_fee_per_gas - base_fee);
        Ok(base_fee + tip)
    }

    /// Miner tip (priority fee) extracted from a tx.
    pub fn miner_tip(
        base_fee:               u64,
        max_fee_per_gas:        u64,
        max_priority_fee_per_gas: u64,
    ) -> u64 {
        max_priority_fee_per_gas.min(max_fee_per_gas.saturating_sub(base_fee))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_fee_increases_when_block_full() {
        // Block 100% full (used = limit)
        let next = BaseFeeCalculator::next_base_fee(1_000_000_000, 30_000_000, 30_000_000).unwrap();
        // +12.5%: 1_000_000_000 + 125_000_000 = 1_125_000_000
        assert_eq!(next, 1_125_000_000, "full block should increase base fee by 12.5%");
    }

    #[test]
    fn base_fee_unchanged_at_target() {
        let next = BaseFeeCalculator::next_base_fee(1_000_000_000, 15_000_000, 30_000_000).unwrap();
        assert_eq!(next, 1_000_000_000, "target utilisation: no change");
    }

    #[test]
    fn base_fee_decreases_empty_block() {
        let next = BaseFeeCalculator::next_base_fee(1_000_000_000, 0, 30_000_000).unwrap();
        // -12.5%: 1_000_000_000 - 125_000_000 = 875_000_000
        assert_eq!(next, 875_000_000, "empty block should decrease base fee by 12.5%");
    }

    #[test]
    fn floor_is_min_base_fee() {
        let next = BaseFeeCalculator::next_base_fee(MIN_BASE_FEE, 0, 30_000_000).unwrap();
        assert_eq!(next, MIN_BASE_FEE, "base fee cannot go below MIN_BASE_FEE");
    }

    #[test]
    fn effective_gas_price_capped_by_max_fee() {
        let price = BaseFeeCalculator::effective_gas_price(100, 150, 200).unwrap();
        assert_eq!(price, 150, "effective price capped by max_fee_per_gas");
    }
}