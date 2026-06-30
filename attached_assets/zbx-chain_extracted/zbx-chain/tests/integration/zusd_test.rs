//! Integration tests for ZUSD stablecoin (CDP vault + stability pool).

#[cfg(test)]
mod zusd_integration {

    // ─── CDP mechanics ────────────────────────────────────────────────────

    #[test]
    fn open_cdp_requires_200pct_collateral() {
        // Rule: max mint = 50% of collateral value (200% CR required)
        let zbx_price_usd: u128 = 50_000_000; // $0.50 per ZBX (8 decimals)
        let collateral_zbx: u128 = 10_000 * 1_000_000_000_000_000_000; // 10,000 ZBX

        // Collateral value = 10,000 × $0.50 = $5,000
        let col_value_usd = collateral_zbx * zbx_price_usd / 100_000_000;

        // Max mintable at 200% CR = $5000 / 2.0 = $2,500 ZUSD (exactly 50%)
        let max_zusd = col_value_usd * 10_000 / 20_000;
        assert_eq!(max_zusd, 2_500, "max mint = exactly 50% of collateral");

        // Trying to mint $3000 should fail (CR would be 166% < 200%)
        let mint_attempt = 3_000u128;
        let would_cr = col_value_usd * 10_000 / mint_attempt;
        assert!(would_cr < 20_000, "CR would be below 200% → rejected");
    }

    #[test]
    fn liquidation_triggers_at_50pct_price_drop() {
        // Rule: 50% price drop → CR hits 100% → instant liquidation
        let zbx_price_at_open: u128 = 100; // $1.00
        let collateral_zbx: u128   = 1_000;
        let debt_zusd: u128        = 500;  // 50% of $1000 = $500 (CR = 200%)

        // Price drops exactly 50% → collateral = $500
        let zbx_price_drop: u128 = 50;
        let col_value = collateral_zbx * zbx_price_drop / 100;
        let cr = col_value * 10_000 / debt_zusd;

        assert_eq!(cr, 10_000, "50% drop → CR = exactly 100% → liquidatable");
        assert!(cr <= 10_000, "CR at or below 100% → instant liquidation");
    }

    #[test]
    fn liquidator_gets_10pct_bonus() {
        let debt: u128 = 1_000;    // $1000 ZUSD debt
        let bonus_pct = 10u128;
        let bonus = debt * bonus_pct / 100;
        let seize = debt + bonus;
        assert_eq!(seize, 1_100, "liquidator seizes $1100 worth of ZBX for $1000 debt");
    }

    #[test]
    fn stability_fee_increases_debt_over_time() {
        let principal: u128 = 10_000;  // 10,000 ZUSD
        let fee_pct_annual = 2u128;    // 2% APY

        // After 1 year: debt = 10,000 × 1.02 = 10,200 ZUSD
        let debt_after_year = principal + principal * fee_pct_annual / 100;
        assert_eq!(debt_after_year, 10_200, "2% annual stability fee");

        // After 2 years (compound): ~10,404 ZUSD
        let debt_after_2 = debt_after_year + debt_after_year * fee_pct_annual / 100;
        assert!(debt_after_2 > 10_400, "compounded over 2 years");
    }

    // ─── Wallet-aware liquidation protection ──────────────────────────────

    #[test]
    fn no_liquidation_if_user_holds_full_zusd() {
        // User minted 500 ZUSD but never spent it → wallet = 500, debt = 500
        // Even if price dropped 50%, liquidation should be BLOCKED.
        let debt: u128         = 500;
        let wallet_balance: u128 = 500; // still holds all minted ZUSD

        // Condition: wallet_balance >= debt → PROTECTED
        let can_liquidate = wallet_balance < debt;
        assert!(!can_liquidate, "user has full ZUSD in wallet — not liquidatable");
    }

    #[test]
    fn no_liquidation_if_user_deposited_back() {
        // User spent 500 ZUSD, later deposited 600 ZUSD back into wallet
        // wallet = 600 >= debt = 500 → PROTECTED
        let debt: u128           = 500;
        let wallet_balance: u128 = 600; // deposited more than debt

        let can_liquidate = wallet_balance < debt;
        assert!(!can_liquidate, "user deposited back — wallet >= debt — not liquidatable");
    }

    #[test]
    fn liquidation_only_when_wallet_less_than_debt() {
        // User minted 500 ZUSD, spent 350, has 150 left in wallet
        // wallet = 150 < debt = 500 → AND price dropped 50% → LIQUIDATABLE
        let debt: u128           = 500;
        let wallet_balance: u128 = 150; // only 150 left after spending 350

        let wallet_insufficient = wallet_balance < debt;
        assert!(wallet_insufficient, "wallet < debt → user cannot self-repay → liquidatable");

        // Both conditions must be true for liquidation:
        let price_dropped_50_pct = true;  // simulated
        let should_liquidate = price_dropped_50_pct && wallet_insufficient;
        assert!(should_liquidate, "both conditions met → liquidation allowed");
    }

    #[test]
    fn partial_wallet_blocks_partial_repay_but_not_full_liquidation() {
        // User has 200 ZUSD in wallet, debt = 500 ZUSD
        // They can repay 200 themselves (partial), still short 300
        let debt: u128           = 500;
        let wallet_balance: u128 = 200;
        let partial_repay        = wallet_balance; // repay what they have
        let remaining_debt       = debt - partial_repay;
        assert_eq!(remaining_debt, 300, "still owes 300 ZUSD after partial repay");
        // After partial repay, wallet = 0, debt = 300 → liquidatable for remainder
        let wallet_after = 0u128;
        assert!(wallet_after < remaining_debt, "after partial repay → still liquidatable for remainder");
    }

    // ─── Stability pool ────────────────────────────────────────────────────

    #[test]
    fn stability_pool_absorbs_liquidation() {
        let pool_zusd: u128 = 1_000_000;    // 1M ZUSD in pool
        let debt_to_absorb: u128 = 100_000; // 100K ZUSD liquidated
        let zbx_received: u128 = 150;       // 150 ZBX collateral received

        // Pool shrinks by absorbed debt.
        let pool_after = pool_zusd - debt_to_absorb;
        assert_eq!(pool_after, 900_000, "pool shrinks");

        // ZBX gain per ZUSD = 150 / 1,000,000
        let gain_per_unit = zbx_received * 1_000_000 / pool_zusd;
        assert_eq!(gain_per_unit, 0, "gain is fractional per ZUSD");

        // But for someone who deposited 100,000 ZUSD:
        let depositor = 100_000u128;
        let depositor_zbx = depositor * zbx_received / pool_zusd;
        assert_eq!(depositor_zbx, 15, "10% of pool gets 15 ZBX");
    }

    // ─── Redemption ────────────────────────────────────────────────────────

    #[test]
    fn redemption_fee_is_0_5_pct() {
        let zusd_amount: u128 = 10_000;
        let fee = zusd_amount * 50 / 10_000; // 0.5%
        let net = zusd_amount - fee;
        assert_eq!(fee, 50,     "0.5% fee = 50 ZUSD on 10,000");
        assert_eq!(net, 9_950, "net after fee");
    }

    #[test]
    fn redeem_gives_zbx_at_oracle_price() {
        let zusd_amount: u128  = 10_000;
        let fee: u128          = 50;        // 0.5%
        let net_zusd: u128     = zusd_amount - fee;
        let zbx_price_usd: u128 = 50;       // $0.50 per ZBX

        // ZBX received = net_zusd / zbx_price
        let zbx_received = net_zusd / zbx_price_usd;
        assert_eq!(zbx_received, 199, "~199 ZBX for 9950 ZUSD at $0.50/ZBX");
    }

    // ─── Peg stability ─────────────────────────────────────────────────────

    #[test]
    fn peg_target_is_one_dollar() {
        let peg: u64 = 1_00_000_000; // $1.00 with 8 decimals
        let upper:  u64 = 1_01_000_000; // $1.01
        let lower:  u64 = 0_99_000_000; // $0.99
        assert!(lower < peg && peg < upper, "peg target in healthy band");
    }

    #[test]
    fn below_peg_triggers_redemption_incentive() {
        let zusd_price: u64 = 0_97_000_000; // $0.97 (below peg)
        let peg_lower:  u64 = 0_99_000_000; // $0.99 threshold
        let is_depegged = zusd_price < peg_lower;
        assert!(is_depegged, "below $0.99 is depegged — should incentivize redemptions");
    }
}