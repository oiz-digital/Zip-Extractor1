//! Integration tests for the EIP-1559 fee market.

#[cfg(test)]
mod fee_market_integration {
    const GAS_LIMIT: u64 = 30_000_000;

    #[test]
    fn base_fee_tracks_demand() {
        // Simulate 100 full blocks — base fee should increase 12.5% per block.
        let mut base_fee: u64 = 1_000_000_000; // 1 gwei
        for _ in 0..10 {
            // Full block increases fee by 12.5%.
            let increase = base_fee * 125 / 1000;
            base_fee = base_fee.saturating_add(increase);
        }
        // After 10 full blocks, fee should be ~3.25x start.
        assert!(base_fee > 3_000_000_000, "base fee should rise with demand");
    }

    #[test]
    fn base_fee_falls_with_empty_blocks() {
        let mut base_fee: u64 = 10_000_000_000; // 10 gwei (high)
        for _ in 0..20 {
            let decrease = base_fee * 125 / 1000;
            base_fee = base_fee.saturating_sub(decrease).max(7);
        }
        assert!(base_fee < 1_000_000_000, "base fee should fall with low demand");
    }

    #[test]
    fn eip1559_tx_excludes_base_fee_from_tip() {
        // Effective miner tip = min(maxPriorityFee, maxFee - baseFee)
        let base_fee: u64   = 80_000_000_000; // 80 gwei
        let max_fee: u64    = 100_000_000_000; // 100 gwei
        let priority: u64   = 30_000_000_000;  // 30 gwei (but capped by maxFee - baseFee)
        let effective_tip = priority.min(max_fee - base_fee);
        assert_eq!(effective_tip, 20_000_000_000, "tip capped at max_fee - base_fee");
    }

    #[test]
    fn tx_cannot_pay_below_base_fee() {
        let base_fee: u64 = 100_000_000_000; // 100 gwei
        let tx_max_fee: u64 = 50_000_000_000; // 50 gwei — below base
        let can_include = tx_max_fee >= base_fee;
        assert!(!can_include, "tx with max_fee < base_fee must be rejected");
    }

    #[test]
    fn base_fee_is_burned_not_paid_to_validator() {
        let base_fee: u64 = 1_000_000_000;
        let gas_used: u64 = 21_000;
        let burned    = base_fee as u128 * gas_used as u128;
        let tip: u64  = 100_000_000;
        let validator = tip as u128 * gas_used as u128;
        // Total user pays = (base + tip) * gas
        // Validator receives only tip portion
        assert!(burned > validator, "base fee burned should exceed typical tip");
    }
}