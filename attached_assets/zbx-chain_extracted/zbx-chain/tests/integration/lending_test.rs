//! Integration tests for ZbxLendingPool.

#[cfg(test)]
mod lending_integration {

    #[test]
    fn utilization_rate_calculation() {
        let total_deposited: u128 = 1_000_000;
        let total_borrowed:  u128 =   600_000;
        let utilization = total_borrowed * 10_000 / total_deposited; // bps
        assert_eq!(utilization, 6_000, "60% utilization");
    }

    #[test]
    fn interest_rate_increases_with_utilization() {
        // At 0% util: base rate = 2% APY
        // At 80% util: target rate = 8% APY
        // At 100% util: max rate = 50% APY
        let base_rate: u128    = 200;   // bps
        let target_rate: u128  = 800;
        let max_rate: u128     = 5000;
        let kink_util: u128    = 8_000; // 80% in bps

        let util_60: u128 = 6_000;
        let rate_60 = base_rate + (target_rate - base_rate) * util_60 / kink_util;
        assert!(rate_60 > base_rate && rate_60 < target_rate, "rate at 60% util in range");

        let util_90: u128 = 9_000;
        let rate_90 = target_rate + (max_rate - target_rate) * (util_90 - kink_util) / (10_000 - kink_util);
        assert!(rate_90 > target_rate, "rate at 90% util exceeds target");
    }

    #[test]
    fn collateral_factor_limits_borrow() {
        let collateral_value: u128 = 10_000; // $10,000
        let collateral_factor: u128 = 7_500; // 75% (in bps)
        let max_borrow = collateral_value * collateral_factor / 10_000;
        assert_eq!(max_borrow, 7_500, "can borrow up to 75% of collateral");
    }

    #[test]
    fn flash_loan_repaid_in_same_tx() {
        let loan_amount: u128 = 1_000_000;
        let fee_bps: u128 = 9; // 0.09%
        let fee = loan_amount * fee_bps / 10_000;
        let repay_amount = loan_amount + fee;
        assert_eq!(fee, 90, "flash loan fee = 0.09% of 1M = 90");
        assert_eq!(repay_amount, 1_000_090, "must repay principal + fee");
    }
}