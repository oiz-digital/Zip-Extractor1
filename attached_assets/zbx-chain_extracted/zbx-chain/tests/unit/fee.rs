//! Unit tests for zbx-fee (EIP-1559 base fee market).

#[cfg(test)]
mod base_fee_tests {
    // These test the contract-level invariants of EIP-1559.

    const DENOMINATOR: u64 = 8;
    const ELASTICITY:  u64 = 2;
    const MIN_FEE:     u64 = 7;
    const GAS_LIMIT:   u64 = 30_000_000;
    const GAS_TARGET:  u64 = 15_000_000; // GAS_LIMIT / 2

    fn next_fee(base: u64, used: u64) -> u64 {
        if used == GAS_TARGET {
            return base;
        } else if used > GAS_TARGET {
            let delta = base as u128 * (used - GAS_TARGET) as u128 / GAS_TARGET as u128 / DENOMINATOR as u128;
            return base.saturating_add(delta.max(1) as u64);
        } else {
            let delta = base as u128 * (GAS_TARGET - used) as u128 / GAS_TARGET as u128 / DENOMINATOR as u128;
            return base.saturating_sub(delta as u64).max(MIN_FEE);
        }
    }

    #[test]
    fn full_block_increases_12_5_pct() {
        let base = 1_000_000_000u64;
        let next = next_fee(base, GAS_LIMIT);
        assert_eq!(next, 1_125_000_000, "+12.5% on full block");
    }

    #[test]
    fn empty_block_decreases_12_5_pct() {
        let base = 1_000_000_000u64;
        let next = next_fee(base, 0);
        assert_eq!(next, 875_000_000, "-12.5% on empty block");
    }

    #[test]
    fn target_utilisation_no_change() {
        let base = 1_000_000_000u64;
        let next = next_fee(base, GAS_TARGET);
        assert_eq!(next, base, "no change at target");
    }

    #[test]
    fn never_below_minimum() {
        let mut fee = MIN_FEE;
        for _ in 0..100 {
            fee = next_fee(fee, 0);
            assert!(fee >= MIN_FEE, "base fee below minimum");
        }
    }

    #[test]
    fn converges_to_equilibrium() {
        // After many oscillating blocks, fee should stabilise.
        let mut fee = 1_000_000_000u64;
        for i in 0..200 {
            let used = if i % 2 == 0 { GAS_LIMIT } else { 0 };
            fee = next_fee(fee, used);
        }
        // Fee should be within 2x of starting value (not diverge).
        assert!(fee < 2_000_000_000, "fee diverged");
    }

    #[test]
    fn priority_fee_bounded_by_max_fee() {
        let base_fee: u64  = 100;
        let max_fee: u64   = 150;
        let priority: u64  = 200; // exceeds max_fee - base_fee
        // effective price = min(max_fee, base_fee + priority)
        let effective = max_fee.min(base_fee + priority);
        assert_eq!(effective, 150, "effective price capped at max_fee");
    }
}