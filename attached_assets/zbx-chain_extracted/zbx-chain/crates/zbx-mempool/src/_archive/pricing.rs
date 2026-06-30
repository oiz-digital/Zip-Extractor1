//! EIP-1559 fee market helpers for mempool transaction ordering and filtering.

use zbx_types::U256;

/// Minimum gas price accepted by the mempool (configurable floor).
pub const MIN_GAS_PRICE: u64 = 1_000_000_000; // 1 gwei

/// Minimum percentage bump required to replace a pending transaction (10%).
pub const REPLACEMENT_BUMP_PERCENT: u64 = 10;

/// EIP-1559 effective gas price: min(max_fee, base_fee + max_priority_fee).
pub fn effective_gas_price(
    max_fee:          U256,
    max_priority_fee: U256,
    base_fee:         U256,
) -> U256 {
    let fee_cap = base_fee + max_priority_fee;
    fee_cap.min(max_fee)
}

/// Check whether a replacement transaction meets the 10% bump requirement.
/// Both the base fee and tip must be >= old * 1.10.
pub fn is_valid_replacement(
    old_max_fee:          U256,
    old_max_priority_fee: U256,
    new_max_fee:          U256,
    new_max_priority_fee: U256,
) -> bool {
    let bump = U256::from(100 + REPLACEMENT_BUMP_PERCENT);
    let den  = U256::from(100u64);
    let min_fee  = old_max_fee          * bump / den;
    let min_tip  = old_max_priority_fee * bump / den;
    new_max_fee >= min_fee && new_max_priority_fee >= min_tip
}

/// Calculate the mining priority of a transaction (tip earned by validator).
/// `priority = min(max_priority_fee, max_fee - base_fee)`
pub fn miner_tip(max_fee: U256, max_priority_fee: U256, base_fee: U256) -> U256 {
    if max_fee < base_fee { return U256::zero(); }
    (max_fee - base_fee).min(max_priority_fee)
}

/// Gas price comparator: sort by effective price descending (best first).
pub fn cmp_by_price(
    a_max_fee: U256, a_priority: U256,
    b_max_fee: U256, b_priority: U256,
    base_fee: U256,
) -> std::cmp::Ordering {
    let a_eff = effective_gas_price(a_max_fee, a_priority, base_fee);
    let b_eff = effective_gas_price(b_max_fee, b_priority, base_fee);
    b_eff.cmp(&a_eff) // descending
}

/// Predict the next block's base fee using the EIP-1559 update rule.
/// `next_base = current_base * (1 + 1/8 * (gas_used / target - 1))`
pub fn next_base_fee(current_base: U256, gas_used: u64, gas_target: u64) -> U256 {
    if gas_used == gas_target {
        return current_base;
    }
    let base = current_base.as_u128();
    if gas_used > gas_target {
        let delta = (gas_used - gas_target) as u128;
        let increase = base * delta / gas_target as u128 / 8;
        U256::from(base + increase.max(1))
    } else {
        let delta = (gas_target - gas_used) as u128;
        let decrease = base * delta / gas_target as u128 / 8;
        U256::from(base.saturating_sub(decrease))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effective_gas_price_capped_by_max_fee() {
        let max_fee      = U256::from(10_000_000_000u64); // 10 gwei
        let priority_fee = U256::from(5_000_000_000u64);  // 5 gwei
        let base_fee     = U256::from(8_000_000_000u64);  // 8 gwei
        // base + priority = 13 gwei → capped to max_fee = 10 gwei
        assert_eq!(effective_gas_price(max_fee, priority_fee, base_fee), max_fee);
    }

    #[test]
    fn test_valid_replacement_10pct_bump() {
        let old = U256::from(1_000_000_000u64);
        let new = U256::from(1_100_000_000u64);
        assert!(is_valid_replacement(old, old, new, new));
    }

    #[test]
    fn test_invalid_replacement_insufficient_bump() {
        let old = U256::from(1_000_000_000u64);
        let new = U256::from(1_050_000_000u64); // only 5%
        assert!(!is_valid_replacement(old, old, new, new));
    }

    #[test]
    fn test_next_base_fee_full_block() {
        let base     = U256::from(1_000_000_000u64); // 1 gwei
        let target   = 15_000_000u64;
        let gas_used = 30_000_000u64; // 2x target
        let next = next_base_fee(base, gas_used, target);
        // increase = 1 gwei * 15M / 15M / 8 = 125_000_000 wei
        assert!(next > base);
    }

    #[test]
    fn test_next_base_fee_empty_block() {
        let base     = U256::from(1_000_000_000u64);
        let next = next_base_fee(base, 0, 15_000_000);
        assert!(next < base);
    }
}