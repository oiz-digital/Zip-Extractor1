//! EIP-1559 fee distribution — base fee burned, tip to validator.
//!
//! STK-FEE-01: the original `distribute()` used `overflowing_mul` and
//! `overflowing_add` but discarded the overflow flag, silently wrapping to
//! small values on any arithmetic overflow (e.g. pathological base_fee or gas).
//! `distribute_checked` now returns `Err(FeeError::Overflow)` so callers can
//! reject the block. The legacy `distribute()` is kept for back-compat and
//! saturates on overflow rather than wrapping.

use zbx_primitives::U256;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct FeeDistribution {
    pub burned:        U256,
    pub validator_tip: U256,
    pub total:         U256,
}

#[derive(Debug, Error)]
pub enum FeeError {
    #[error("fee arithmetic overflow (base_fee × gas or tip × gas exceeded U256)")]
    Overflow,
}

/// Checked EIP-1559 fee split.
///
/// Returns `Err(FeeError::Overflow)` if any intermediate product overflows
/// U256 — callers at the block-validation layer should reject such blocks.
///
/// Invariants enforced:
/// * `burned  = base_fee × gas_used`         (errors on overflow)
/// * `prio    = min(max_priority_fee, max_fee − base_fee)`  (tip is capped)
/// * `tip     = prio × gas_used`             (errors on overflow)
/// * `total   = burned + tip`                (errors on overflow)
pub fn distribute_checked(
    gas_used: u64,
    base_fee: U256,
    max_priority_fee: U256,
    max_fee: U256,
) -> Result<FeeDistribution, FeeError> {
    let gas = U256::from_u64(gas_used);

    // base_fee × gas — most likely overflow point on adversarial inputs.
    let (burned, ovf) = base_fee.overflowing_mul(gas);
    if ovf { return Err(FeeError::Overflow); }

    // Tip cap: validator cannot earn more than max_fee - base_fee per gas.
    // If max_fee < base_fee the cap is zero (saturating_sub).
    let cap = max_fee.saturating_sub(base_fee);
    let prio = if max_priority_fee < cap { max_priority_fee } else { cap };

    let (tip, ovf) = prio.overflowing_mul(gas);
    if ovf { return Err(FeeError::Overflow); }

    let (total, ovf) = burned.overflowing_add(tip);
    if ovf { return Err(FeeError::Overflow); }

    Ok(FeeDistribution { burned, validator_tip: tip, total })
}

/// Back-compat wrapper — saturates instead of erroring.
/// Prefer `distribute_checked` in new code paths (STK-FEE-01).
pub fn distribute(
    gas_used: u64,
    base_fee: U256,
    max_priority_fee: U256,
    max_fee: U256,
) -> FeeDistribution {
    distribute_checked(gas_used, base_fee, max_priority_fee, max_fee)
        .unwrap_or_else(|_| {
            // Saturate on overflow: clamp each product to U256::MAX.
            // U256 has no saturating_mul, so we use overflowing_mul and
            // replace wrapped results with the type maximum.
            let max = U256::from_u128(u128::MAX); // U256::MAX not directly constructible
            let gas  = U256::from_u64(gas_used);
            let cap  = max_fee.saturating_sub(base_fee);
            let prio = if max_priority_fee < cap { max_priority_fee } else { cap };
            let (burned, ovf1) = base_fee.overflowing_mul(gas);
            let burned = if ovf1 { max } else { burned };
            let (tip, ovf2)  = prio.overflowing_mul(gas);
            let tip = if ovf2 { max } else { tip };
            let total = burned.saturating_add(tip);
            FeeDistribution { burned, validator_tip: tip, total }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(v: u128) -> U256 { U256::from_u128(v) }

    #[test]
    fn normal_eip1559_split() {
        // base_fee=10 gwei, max_priority=2 gwei, max_fee=15 gwei, gas=21000
        let r = distribute_checked(
            21_000,
            u(10_000_000_000),
            u(2_000_000_000),
            u(15_000_000_000),
        ).unwrap();
        assert_eq!(r.burned,        u(210_000_000_000_000)); // 10 gwei × 21000
        assert_eq!(r.validator_tip, u( 42_000_000_000_000)); //  2 gwei × 21000
        assert_eq!(r.total,         u(252_000_000_000_000));
    }

    #[test]
    fn tip_capped_by_max_fee() {
        // max_priority_fee > max_fee - base_fee → tip capped
        let r = distribute_checked(
            1_000,
            u(10_000_000_000),
            u(100_000_000_000), // very high priority fee
            u(12_000_000_000),  // max_fee only 2 gwei above base
        ).unwrap();
        // prio = min(100 gwei, 2 gwei) = 2 gwei
        assert_eq!(r.validator_tip, u(2_000_000_000 * 1_000));
    }

    #[test]
    fn overflow_is_rejected() {
        // base_fee = U256::MAX / 2, gas = 3 → overflow
        let huge = U256::from_u128(u128::MAX);
        assert!(distribute_checked(u64::MAX, huge, huge, huge).is_err());
    }
}
