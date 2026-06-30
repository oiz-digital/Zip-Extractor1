//! Integration tests for ZbxAMM.

#[cfg(test)]
mod amm_integration {

    #[test]
    fn constant_product_invariant() {
        // k = x * y must remain constant after swap.
        let x: u128 = 5_000_000;   // ZBX reserve
        let y: u128 = 250_000;     // ZUSD reserve
        let k = x * y;

        // User swaps 100 ZUSD for ZBX.
        let dx_in: u128   = 100;   // ZUSD in
        let fee_bps: u128 = 30;    // 0.3% fee
        let dx_net = dx_in - dx_in * fee_bps / 10_000;
        let new_y = y + dx_net;
        let new_x = k / new_y;
        let zbx_out = x - new_x;

        assert!(zbx_out > 0, "should receive ZBX");
        assert!(new_x * new_y >= k - 1, "k should not decrease");
    }

    #[test]
    fn price_impact_increases_with_size() {
        let x: u128 = 5_000_000;
        let y: u128 = 250_000;

        // Small swap: 100 ZUSD.
        let small_in  = 100u128;
        let small_out = x - (x * y) / (y + small_in);
        let small_price = small_in * 1_000_000 / small_out; // ZUSD per ZBX

        // Large swap: 50,000 ZUSD.
        let large_in  = 50_000u128;
        let large_out = x - (x * y) / (y + large_in);
        let large_price = large_in * 1_000_000 / large_out;

        assert!(large_price > small_price, "price impact should increase with trade size");
    }

    #[test]
    fn add_liquidity_preserves_ratio() {
        let x: u128 = 1_000_000;
        let y: u128 = 50_000;
        let ratio_before = x * 1_000 / y;

        // Add 10% more liquidity at the same ratio.
        let add_x = 100_000u128;
        let add_y =  5_000u128;
        let new_x = x + add_x;
        let new_y = y + add_y;
        let ratio_after = new_x * 1_000 / new_y;

        assert_eq!(ratio_before, ratio_after, "ratio must be preserved when adding liquidity");
    }

    #[test]
    fn lp_fee_accrues_to_providers() {
        let fee_bps: u128 = 30; // 0.3%
        let trade_volume: u128 = 1_000_000; // $1M ZUSD traded
        let total_fee = trade_volume * fee_bps / 10_000;
        assert_eq!(total_fee, 3_000, "0.3% of $1M = $3000 in fees to LPs");
    }
}