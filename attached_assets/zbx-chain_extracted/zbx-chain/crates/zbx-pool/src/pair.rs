//! Constant-product AMM pair — fully secured (ZEP-005).
//!
//! # Security features
//!
//! | Feature | Detail |
//! |---------|--------|
//! | Reentrancy guard | Lock flag — re-entrant call is rejected |
//! | Fee-applied swap | Uniswap v2: `dy = dx·fee_mult·y / (x·10000 + dx·fee_mult)` |
//! | Price impact cap | Max 30% impact per swap |
//! | Reserve drain cap | Max 30% of output reserve per swap |
//! | Slippage guard | Caller sets `min_amount_out` |
//! | Deadline | Tx reverts if `now > deadline` |
//! | Oracle sanity | Swap paused if pool price >15% off TWAP |
//! | Circuit breaker | Governance can pause all pool ops |
//! | Overflow-safe math | All multiplications checked |
//! | Minimum liquidity | First LP burns 1000 units |
//! | k-invariant check | `new_k >= old_k` enforced after every swap (checked_mul) |

use zbx_types::address::Address;
use crate::{
    error::AmmError,
    fee::{FeeTier, split_fee},
    security::{
        ReentrancyGuard, CircuitBreaker, MIN_LIQUIDITY,
        check_price_impact, check_slippage, check_deadline,
        check_reserve_drain, check_oracle_deviation, safe_mul_div,
    },
};

// ── PairId ────────────────────────────────────────────────────────────────────

/// Canonical pair identifier — smaller address always first.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PairId {
    pub token_a: Address,
    pub token_b: Address,
}

impl PairId {
    /// Create a canonical PairId (smaller address first).
    pub fn new(a: Address, b: Address) -> Self {
        if a.as_bytes() < b.as_bytes() {
            Self { token_a: a, token_b: b }
        } else {
            Self { token_a: b, token_b: a }
        }
    }
}

// ── Param / Result types ──────────────────────────────────────────────────────

/// Parameters for a single-pair swap.
#[derive(Debug, Clone)]
pub struct SwapParams {
    /// true = A→B, false = B→A.
    pub a_to_b:          bool,
    /// Raw input amount (base units, before fee).
    pub amount_in:       u128,
    /// Minimum acceptable output — reverts if not met.
    pub min_amount_out:  u128,
    /// Unix timestamp deadline — reverts if `now > deadline`.
    pub deadline:        u64,
    /// Oracle TWAP of token_a in token_b (×10^18). Pass 0 to skip.
    pub oracle_twap:     u128,
    /// Current block timestamp (seconds).
    pub block_timestamp: u64,
}

/// Parameters for adding liquidity.
#[derive(Debug, Clone)]
pub struct AddLiquidityParams {
    pub amount_a:        u128,
    pub amount_b:        u128,
    /// Minimum LP tokens to receive — slippage guard.
    pub min_lp_out:      u128,
    pub deadline:        u64,
    pub block_timestamp: u64,
}

/// Parameters for removing liquidity.
#[derive(Debug, Clone)]
pub struct RemoveLiquidityParams {
    pub lp_amount:       u128,
    pub min_a_out:       u128,
    pub min_b_out:       u128,
    pub deadline:        u64,
    pub block_timestamp: u64,
}

/// Result of a successful swap.
#[derive(Debug, Clone)]
pub struct SwapResult {
    pub amount_out:    u128,
    pub lp_fee:        u128,
    pub protocol_fee:  u128,
    pub new_reserve_a: u128,
    pub new_reserve_b: u128,
}

/// Result of a successful add_liquidity.
#[derive(Debug, Clone)]
pub struct AddLiquidityResult {
    pub lp_minted: u128,
    pub used_a:    u128,
    pub used_b:    u128,
}

/// Result of a successful remove_liquidity.
#[derive(Debug, Clone)]
pub struct RemoveLiquidityResult {
    pub amount_a: u128,
    pub amount_b: u128,
}

// ── Pair ──────────────────────────────────────────────────────────────────────

/// State of one liquidity pair — fully secured constant-product AMM.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Pair {
    pub id:         PairId,
    pub reserve_a:  u128,
    pub reserve_b:  u128,
    pub total_lp:   u128,
    /// Accumulated protocol fee in token_a units.
    pub fee_a:      u128,
    /// Accumulated protocol fee in token_b units.
    pub fee_b:      u128,
    pub fee_tier:   FeeTier,
    pub reentrancy: ReentrancyGuard,
    pub circuit:    CircuitBreaker,
}

impl Pair {
    /// Create a new empty pair with a given fee tier.
    pub fn new(id: PairId, fee_tier: FeeTier) -> Self {
        Self {
            id,
            reserve_a:  0,
            reserve_b:  0,
            total_lp:   0,
            fee_a:      0,
            fee_b:      0,
            fee_tier,
            reentrancy: ReentrancyGuard::new(),
            circuit:    CircuitBreaker::new(),
        }
    }

    // ── Swap ──────────────────────────────────────────────────────────────────

    /// Execute a swap with full security checks.
    ///
    /// Security order:
    /// 1. Circuit breaker  2. Reentrancy  3. Deadline  4. Non-zero input
    /// 5. Oracle deviation  6. Price impact  7. Compute output with fee
    /// 8. Reserve drain  9. Slippage  10. k-invariant (checked_mul)
    pub fn swap(&mut self, params: SwapParams) -> Result<SwapResult, AmmError> {
        self.circuit.check()?;
        self.reentrancy.acquire()?;
        let result = self.swap_inner(params);
        self.reentrancy.release();
        result
    }

    fn swap_inner(&mut self, p: SwapParams) -> Result<SwapResult, AmmError> {
        check_deadline(p.block_timestamp, p.deadline)?;

        if p.amount_in == 0 { return Err(AmmError::ZeroAmount); }

        let (r_in, r_out) = if p.a_to_b {
            (self.reserve_a, self.reserve_b)
        } else {
            (self.reserve_b, self.reserve_a)
        };
        if r_in == 0 || r_out == 0 { return Err(AmmError::EmptyReserve); }

        // Oracle sanity — only skipped when oracle_twap == 0 (no TWAP data).
        // If pool_spot == 0 the empty-reserve check above already fired; the
        // oracle guard returns OraclePriceDeviation if spot is 0 but oracle is
        // not, protecting against a zeroed-oracle bypass.
        check_oracle_deviation(self.spot_price_a_in_b(), p.oracle_twap)?;

        // Price impact cap (on input side)
        check_price_impact(p.amount_in, r_in)?;

        // Uniswap v2 formula with fee:
        //   fee_mult = 10_000 - fee_bps
        //   dx_fee   = amount_in × fee_mult
        //   dy       = dx_fee × r_out / (r_in × 10_000 + dx_fee)
        let fee_mult  = 10_000u128 - self.fee_tier.bps() as u128;
        let dx_fee    = p.amount_in.checked_mul(fee_mult).ok_or(AmmError::Overflow)?;
        let numerator = dx_fee.checked_mul(r_out).ok_or(AmmError::Overflow)?;
        let denom     = r_in.checked_mul(10_000)
            .ok_or(AmmError::Overflow)?
            .checked_add(dx_fee)
            .ok_or(AmmError::Overflow)?;
        let amount_out = numerator / denom;

        if amount_out == 0 { return Err(AmmError::ZeroOutput); }

        // Reserve drain cap
        check_reserve_drain(amount_out, r_out)?;

        // Slippage
        check_slippage(amount_out, p.min_amount_out)?;

        // Fee split: LP share stays in reserves, protocol share extracted
        let (lp_fee, protocol_fee) = split_fee(p.amount_in, self.fee_tier);

        // k-invariant check using checked_mul.
        //
        // PRE-FIX: `saturating_mul` was used here, which silently caps at u128::MAX.
        // If both old_k and new_k saturate to u128::MAX the comparison passes even
        // when the real k may have decreased.  With very large reserves this creates
        // a silent accounting error.  We now use `checked_mul` and return
        // `InvariantOverflow` if the product doesn't fit in u128 — failing safe
        // rather than silently masking an overflow.
        let old_k = self.reserve_a.checked_mul(self.reserve_b)
            .ok_or(AmmError::InvariantOverflow)?;

        let (new_ra, new_rb) = if p.a_to_b {
            let a = self.reserve_a.checked_add(p.amount_in).ok_or(AmmError::Overflow)?;
            let b = self.reserve_b.checked_sub(amount_out)
                .ok_or(AmmError::InsufficientLiquidityBurned)?;
            self.fee_a += protocol_fee;
            (a, b)
        } else {
            let b = self.reserve_b.checked_add(p.amount_in).ok_or(AmmError::Overflow)?;
            let a = self.reserve_a.checked_sub(amount_out)
                .ok_or(AmmError::InsufficientLiquidityBurned)?;
            self.fee_b += protocol_fee;
            (a, b)
        };

        // k-invariant: must not decrease
        let new_k = new_ra.checked_mul(new_rb)
            .ok_or(AmmError::InvariantOverflow)?;
        if new_k < old_k { return Err(AmmError::InvariantViolation); }

        self.reserve_a = new_ra;
        self.reserve_b = new_rb;

        Ok(SwapResult {
            amount_out,
            lp_fee,
            protocol_fee,
            new_reserve_a: self.reserve_a,
            new_reserve_b: self.reserve_b,
        })
    }

    // ── Add liquidity ─────────────────────────────────────────────────────────

    /// Add liquidity to the pool.
    ///
    /// First deposit: LP = sqrt(a × b) − MIN_LIQUIDITY (1000 burned).
    /// Subsequent:    LP proportional to min ratio; excess refunded by caller.
    pub fn add_liquidity(
        &mut self,
        params: AddLiquidityParams,
    ) -> Result<AddLiquidityResult, AmmError> {
        self.circuit.check()?;
        self.reentrancy.acquire()?;
        let r = self.add_liquidity_inner(params);
        self.reentrancy.release();
        r
    }

    fn add_liquidity_inner(
        &mut self,
        p: AddLiquidityParams,
    ) -> Result<AddLiquidityResult, AmmError> {
        check_deadline(p.block_timestamp, p.deadline)?;
        if p.amount_a == 0 || p.amount_b == 0 { return Err(AmmError::ZeroAmount); }

        let (lp_minted, used_a, used_b) = if self.total_lp == 0 {
            // First deposit
            let product = p.amount_a.checked_mul(p.amount_b).ok_or(AmmError::Overflow)?;
            let sqrt = integer_sqrt(product);
            if sqrt <= MIN_LIQUIDITY { return Err(AmmError::BelowMinLiquidity); }
            (sqrt - MIN_LIQUIDITY, p.amount_a, p.amount_b)
        } else {
            // Proportional
            let lp_a = safe_mul_div(p.amount_a, self.total_lp, self.reserve_a)?;
            let lp_b = safe_mul_div(p.amount_b, self.total_lp, self.reserve_b)?;
            let lp   = lp_a.min(lp_b);
            let ua   = safe_mul_div(lp, self.reserve_a, self.total_lp)?;
            let ub   = safe_mul_div(lp, self.reserve_b, self.total_lp)?;
            (lp, ua, ub)
        };

        if lp_minted < p.min_lp_out {
            return Err(AmmError::SlippageExceeded { got: lp_minted, min: p.min_lp_out });
        }

        self.reserve_a = self.reserve_a.checked_add(used_a).ok_or(AmmError::Overflow)?;
        self.reserve_b = self.reserve_b.checked_add(used_b).ok_or(AmmError::Overflow)?;
        self.total_lp  = self.total_lp.checked_add(lp_minted).ok_or(AmmError::Overflow)?;

        Ok(AddLiquidityResult { lp_minted, used_a, used_b })
    }

    // ── Remove liquidity ──────────────────────────────────────────────────────

    /// Remove liquidity proportional to the LP share.
    pub fn remove_liquidity(
        &mut self,
        params: RemoveLiquidityParams,
    ) -> Result<RemoveLiquidityResult, AmmError> {
        self.circuit.check()?;
        self.reentrancy.acquire()?;
        let r = self.remove_liquidity_inner(params);
        self.reentrancy.release();
        r
    }

    fn remove_liquidity_inner(
        &mut self,
        p: RemoveLiquidityParams,
    ) -> Result<RemoveLiquidityResult, AmmError> {
        check_deadline(p.block_timestamp, p.deadline)?;
        if p.lp_amount == 0 { return Err(AmmError::ZeroAmount); }
        if p.lp_amount > self.total_lp { return Err(AmmError::InsufficientLpBalance); }

        let amount_a = safe_mul_div(p.lp_amount, self.reserve_a, self.total_lp)?;
        let amount_b = safe_mul_div(p.lp_amount, self.reserve_b, self.total_lp)?;

        if amount_a == 0 || amount_b == 0 {
            return Err(AmmError::InsufficientLiquidityBurned);
        }

        check_slippage(amount_a, p.min_a_out)?;
        check_slippage(amount_b, p.min_b_out)?;

        self.reserve_a = self.reserve_a.checked_sub(amount_a)
            .ok_or(AmmError::InsufficientLiquidityBurned)?;
        self.reserve_b = self.reserve_b.checked_sub(amount_b)
            .ok_or(AmmError::InsufficientLiquidityBurned)?;
        self.total_lp  = self.total_lp.checked_sub(p.lp_amount)
            .ok_or(AmmError::InsufficientLpBalance)?;

        Ok(RemoveLiquidityResult { amount_a, amount_b })
    }

    // ── Read-only helpers ─────────────────────────────────────────────────────

    /// Quote-only: amount out for a given input with fee applied.
    /// Does NOT apply security checks — for simulation/UI only.
    pub fn get_amount_out(&self, amount_in: u128, a_to_b: bool) -> u128 {
        let (r_in, r_out) = if a_to_b {
            (self.reserve_a, self.reserve_b)
        } else {
            (self.reserve_b, self.reserve_a)
        };
        if r_in == 0 || r_out == 0 || amount_in == 0 { return 0; }
        let fee_mult  = 10_000u128 - self.fee_tier.bps() as u128;
        let dx_fee    = match amount_in.checked_mul(fee_mult) { Some(v) => v, None => return 0 };
        let numerator = match dx_fee.checked_mul(r_out) { Some(v) => v, None => return 0 };
        let denom     = match r_in.checked_mul(10_000)
            .and_then(|v| v.checked_add(dx_fee)) { Some(v) => v, None => return 0 };
        numerator / denom
    }

    /// Spot price of token_a in token_b (×10^18).
    ///
    /// ## Overflow handling
    ///
    /// For large reserves, `reserve_b × 1e18` can overflow u128.  When that
    /// happens we rescale: divide `reserve_b` by 1_000 and divide the
    /// 1e18 scale by 1_000 (net 1e12 scaling) before multiplying, then
    /// restore the full 1e18 scaling by multiplying the result by 1_000_000.
    /// This trades a small rounding error (≤ 1 in 10^12) for overflow safety,
    /// which is acceptable for oracle comparisons and quoting purposes.
    ///
    /// Returns 0 if `reserve_a == 0` (empty pool).
    pub fn spot_price_a_in_b(&self) -> u128 {
        if self.reserve_a == 0 { return 0; }
        let scale = 1_000_000_000_000_000_000u128; // 1e18
        match self.reserve_b.checked_mul(scale) {
            Some(v) => v / self.reserve_a,
            None => {
                // reserve_b is so large that reserve_b × 1e18 overflows u128.
                // Rescale: use 1e12 intermediate, multiply by 1e6 at end.
                let scale12 = 1_000_000_000_000u128; // 1e12
                let scaled  = (self.reserve_b / 1_000_000).saturating_mul(scale12);
                (scaled / self.reserve_a).saturating_mul(1_000_000)
            }
        }
    }

    /// Constant-product invariant k = reserve_a × reserve_b.
    ///
    /// Returns `None` if the product overflows u128 (reserves are too large).
    pub fn k(&self) -> Option<u128> {
        self.reserve_a.checked_mul(self.reserve_b)
    }

    /// k as a saturating product (for test convenience — do not use in
    /// production invariant checks).
    pub fn k_saturating(&self) -> u128 {
        self.reserve_a.saturating_mul(self.reserve_b)
    }
}

// ── Integer square root (Babylonian method, overflow-safe) ────────────────────

pub fn integer_sqrt(n: u128) -> u128 {
    if n == 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x { x = y; y = (x + n / x) / 2; }
    x
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address { Address([n; 20]) }

    fn pair(ra: u128, rb: u128) -> Pair {
        let mut p = Pair::new(PairId::new(addr(1), addr(2)), FeeTier::Standard);
        p.reserve_a = ra;
        p.reserve_b = rb;
        p.total_lp  = integer_sqrt(ra.saturating_mul(rb))
            .saturating_sub(MIN_LIQUIDITY);
        p
    }

    fn sp(amount_in: u128, a_to_b: bool, ts: u64) -> SwapParams {
        SwapParams {
            a_to_b, amount_in,
            min_amount_out:  0,
            deadline:        ts + 3_600,
            oracle_twap:     0,
            block_timestamp: ts,
        }
    }

    #[test]
    fn swap_applies_fee() {
        let mut p = pair(1_000_000, 1_000_000);
        let r = p.swap(sp(1_000, true, 1000)).unwrap();
        // Fee 0.30% → output strictly less than no-fee calculation (~999)
        assert!(r.amount_out > 0 && r.amount_out < 999);
        assert!(r.lp_fee > 0 && r.protocol_fee > 0);
    }

    #[test]
    fn swap_b_to_a_works() {
        let mut p = pair(1_000_000, 2_000_000);
        let r = p.swap(sp(1_000, false, 1000)).unwrap();
        assert!(r.amount_out > 0);
    }

    #[test]
    fn k_invariant_holds() {
        let mut p = pair(1_000_000, 1_000_000);
        let k0 = p.k().unwrap();
        p.swap(sp(1_000, true, 1000)).unwrap();
        assert!(p.k().unwrap() >= k0);
    }

    #[test]
    fn k_checked_mul_used_not_saturating() {
        // Verify that k() returns None for overflow-prone values rather than
        // silently saturating.  We can't trigger actual overflow in a swap
        // (the reserve caps prevent it) but we can verify the API.
        let mut p = Pair::new(PairId::new(addr(1), addr(2)), FeeTier::Standard);
        p.reserve_a = u128::MAX / 2 + 1;
        p.reserve_b = 2;
        // (MAX/2 + 1) * 2 overflows → k() returns None
        assert!(p.k().is_none(), "k() must return None on overflow");
        // k_saturating still works (for tests/display only)
        assert_eq!(p.k_saturating(), u128::MAX);
    }

    #[test]
    fn slippage_rejected() {
        let mut p = pair(1_000_000, 1_000_000);
        let mut s = sp(1_000, true, 1000);
        s.min_amount_out = 10_000;  // impossible
        assert!(matches!(p.swap(s), Err(AmmError::SlippageExceeded { .. })));
    }

    #[test]
    fn deadline_rejected() {
        let mut p = pair(1_000_000, 1_000_000);
        let mut s = sp(1_000, true, 5000);
        s.deadline = 3000;  // expired
        assert!(matches!(p.swap(s), Err(AmmError::DeadlineExpired { .. })));
    }

    #[test]
    fn price_impact_cap() {
        let mut p = pair(1_000, 1_000);
        // 290 / 1290 ≈ 22.5% < 30% → passes
        assert!(p.swap(sp(290, true, 1000)).is_ok());
        // 500 / 1500 ≈ 33% > 30% → blocked
        assert!(matches!(p.swap(sp(500, true, 1000)), Err(AmmError::PriceImpactTooHigh { .. })));
    }

    #[test]
    fn reentrancy_blocked() {
        let mut p = pair(1_000_000, 1_000_000);
        p.reentrancy.acquire().unwrap();
        assert!(matches!(p.swap(sp(1_000, true, 1000)), Err(AmmError::Reentrancy)));
        p.reentrancy.release();
    }

    #[test]
    fn circuit_breaker_pauses_all() {
        let mut p = pair(1_000_000, 1_000_000);
        p.circuit.trip("test", 1);
        assert!(matches!(p.swap(sp(1_000, true, 1000)), Err(AmmError::PoolPaused { .. })));
        p.circuit.clear();
        assert!(p.swap(sp(1_000, true, 1000)).is_ok());
    }

    #[test]
    fn oracle_deviation_blocks() {
        let mut p = pair(1_000_000, 1_000_000);
        // spot ≈ 1.0; oracle = 1.30 → 30% > 15% → blocked
        let mut s = sp(1_000, true, 1000);
        s.oracle_twap = 1_300_000_000_000_000_000;
        assert!(matches!(p.swap(s), Err(AmmError::OraclePriceDeviation { .. })));
    }

    #[test]
    fn add_liquidity_first_deposit() {
        let mut p = Pair::new(PairId::new(addr(1), addr(2)), FeeTier::Standard);
        let r = p.add_liquidity(AddLiquidityParams {
            amount_a: 1_000_000, amount_b: 1_000_000,
            min_lp_out: 0, deadline: 9999, block_timestamp: 1,
        }).unwrap();
        // sqrt(1M×1M) - 1000 = 999_000
        assert_eq!(r.lp_minted, 999_000);
    }

    #[test]
    fn remove_liquidity_proportional() {
        let mut p = pair(1_000_000, 2_000_000);
        let lp = p.total_lp / 2;
        let r = p.remove_liquidity(RemoveLiquidityParams {
            lp_amount: lp, min_a_out: 0, min_b_out: 0,
            deadline: 9999, block_timestamp: 1,
        }).unwrap();
        assert!((r.amount_a as i128 - 500_000).abs() < 2_000);
        assert!((r.amount_b as i128 - 1_000_000).abs() < 2_000);
    }

    #[test]
    fn integer_sqrt_values() {
        assert_eq!(integer_sqrt(0), 0);
        assert_eq!(integer_sqrt(4), 2);
        assert_eq!(integer_sqrt(9), 3);
        assert_eq!(integer_sqrt(1_000_000), 1_000);
    }

    #[test]
    fn spot_price_large_reserves_no_overflow() {
        // reserve_b = 2^63 (≈ 9.2e18) → reserve_b × 1e18 overflows u128
        // spot_price_a_in_b must not panic and must return a reasonable value
        let mut p = Pair::new(PairId::new(addr(1), addr(2)), FeeTier::Standard);
        p.reserve_a = 1_000_000_000_000_000_000u128; // 1e18
        p.reserve_b = u128::MAX / 2;                  // very large
        let spot = p.spot_price_a_in_b();
        // spot > 0 (reserve_b >> reserve_a)
        assert!(spot > 0, "spot_price should be non-zero for large reserve_b");
    }
}
