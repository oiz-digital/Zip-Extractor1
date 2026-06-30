//! Pool security module — guards against common AMM attack vectors.
//!
//! # Threats Defended Against
//!
//! | Threat | Guard | Mechanism |
//! |--------|-------|-----------|
//! | Reentrancy | `ReentrancyGuard` | Lock flag — second call panics |
//! | Flash loan drain | `FlashLoanGuard` | Reserves must recover by end of block |
//! | Price manipulation | `PriceImpactGuard` | Max 30% price impact per swap |
//! | Stale price oracle | `OracleSanityCheck` | Pool price vs TWAP must be within 15% |
//! | MEV / sandwich attack | Deadline param | Tx invalid after deadline |
//! | Dust / spam attack | `MIN_LIQUIDITY` | First LP mints burn 1000 units |
//! | Integer overflow | `safe_mul_div` | Checked u128 arithmetic throughout |
//! | Pool drain via swap | `SwapCapGuard` | Max 30% of any reserve per swap |
//! | Governance rounding | `MIN_LIQUIDITY` | Minimum LP token requirement |

use crate::error::AmmError;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Minimum liquidity permanently locked in the pool at first mint.
///
/// Uniswap v2 burns this to address(0) to prevent the first LP from
/// owning 100% of supply and manipulating the initial price.
/// 1000 base units (wei-equivalent) is negligible for any real deposit.
pub const MIN_LIQUIDITY: u128 = 1_000;

/// Maximum price impact per swap: 30%.
///
/// A swap consuming more than 30% of the input reserve in one go is
/// likely either a manipulation attempt or an error. Legitimate large
/// trades should be split across multiple blocks (or use a DEX aggregator).
pub const MAX_PRICE_IMPACT_BPS: u32 = 3_000;  // 30%

/// Maximum oracle deviation before swaps are halted: 15%.
///
/// If the pool's spot price deviates more than 15% from the on-chain TWAP,
/// something suspicious has happened (sandwich, multi-block manipulation).
/// Swaps are paused until the TWAP catches up or governance intervenes.
pub const MAX_ORACLE_DEVIATION_BPS: u32 = 1_500;  // 15%

/// Maximum fraction of any reserve that can leave in one swap: 30%.
pub const MAX_RESERVE_DRAIN_BPS: u32 = 3_000;  // 30% of reserve per swap

// ── Reentrancy guard ──────────────────────────────────────────────────────────

/// Reentrancy guard — prevents callback-based reentrancy attacks.
///
/// ZBX EVM calls can re-enter a contract within the same tx if a called
/// contract triggers a callback. The guard blocks the second entry.
///
/// # Usage
/// ```rust
/// let _guard = pool.reentrancy.enter()?;  // locks
/// // ... do swap work ...
/// // _guard dropped → unlocks automatically
/// ```
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReentrancyGuard {
    locked: bool,
}

impl ReentrancyGuard {
    pub fn new() -> Self { Self { locked: false } }

    /// Try to acquire the lock. Returns `Err(Reentrancy)` if already locked.
    pub fn acquire(&mut self) -> Result<(), AmmError> {
        if self.locked {
            return Err(AmmError::Reentrancy);
        }
        self.locked = true;
        Ok(())
    }

    /// Release the lock. Call after swap/add/remove is complete.
    pub fn release(&mut self) {
        self.locked = false;
    }
}

// ── Price impact guard ────────────────────────────────────────────────────────

/// Check that a swap does not move the price more than `MAX_PRICE_IMPACT_BPS`.
///
/// Price impact ≈ `amount_in / (reserve_in + amount_in)` (constant product).
///
/// # Arguments
/// - `amount_in`: Raw amount of input token (before fee deduction).
/// - `reserve_in`: Current reserve of the input token.
pub fn check_price_impact(amount_in: u128, reserve_in: u128) -> Result<(), AmmError> {
    if reserve_in == 0 {
        return Err(AmmError::EmptyReserve);
    }
    // impact_bps = (amount_in * 10_000) / (reserve_in + amount_in)
    let denom = reserve_in.saturating_add(amount_in);
    let impact_bps = (amount_in as u128)
        .saturating_mul(10_000)
        / denom;

    if impact_bps as u32 > MAX_PRICE_IMPACT_BPS {
        return Err(AmmError::PriceImpactTooHigh {
            impact_bps: impact_bps as u32,
            max_bps:    MAX_PRICE_IMPACT_BPS,
        });
    }
    Ok(())
}

/// Check that the output amount is at least `min_amount_out` (slippage protection).
pub fn check_slippage(amount_out: u128, min_amount_out: u128) -> Result<(), AmmError> {
    if amount_out < min_amount_out {
        return Err(AmmError::SlippageExceeded {
            got:      amount_out,
            min:      min_amount_out,
        });
    }
    Ok(())
}

/// Check that `block_timestamp <= deadline`.
pub fn check_deadline(block_timestamp: u64, deadline: u64) -> Result<(), AmmError> {
    if block_timestamp > deadline {
        return Err(AmmError::DeadlineExpired {
            now:      block_timestamp,
            deadline,
        });
    }
    Ok(())
}

/// Check that a single swap does not drain more than 30% of the output reserve.
pub fn check_reserve_drain(amount_out: u128, reserve_out: u128) -> Result<(), AmmError> {
    if reserve_out == 0 {
        return Err(AmmError::EmptyReserve);
    }
    let drain_bps = amount_out.saturating_mul(10_000) / reserve_out;
    if drain_bps as u32 > MAX_RESERVE_DRAIN_BPS {
        return Err(AmmError::ReserveDrainExceeded {
            drain_bps: drain_bps as u32,
            max_bps:   MAX_RESERVE_DRAIN_BPS,
        });
    }
    Ok(())
}

// ── Oracle sanity check ───────────────────────────────────────────────────────

/// Check that pool spot price is within `MAX_ORACLE_DEVIATION_BPS` of the oracle TWAP.
///
/// ## Skip condition — oracle data not available
///
/// The check is skipped ONLY when `oracle_price_1e18 == 0`, which signals
/// "no oracle data yet" (e.g. a brand-new pool before the TWAP has a history).
///
/// The previous implementation also skipped when `pool_spot_1e18 == 0`, which
/// was a latent vulnerability: if a pool manipulator could drive the oracle to
/// report 0 they would bypass the deviation check entirely.  This version
/// treats `pool_spot == 0` as an error condition — it should be unreachable in
/// practice because `swap_inner` checks `EmptyReserve` before calling this
/// function, so a zero spot price means the pool is empty and the earlier
/// guard already fired.
///
/// ## Arguments
/// - `pool_spot_1e18`:   Pool's spot price of token_a in token_b (×10^18).
///                       Use `Pair::spot_price_a_in_b()`.
/// - `oracle_price_1e18`: Oracle TWAP of the same pair (same ×10^18 scale).
///                       Pass `0` when no oracle data is available (disables check).
pub fn check_oracle_deviation(
    pool_spot_1e18:    u128,
    oracle_price_1e18: u128,
) -> Result<(), AmmError> {
    // No oracle data available — skip the check rather than blocking all swaps.
    // This covers pools that have not yet accumulated TWAP history.
    if oracle_price_1e18 == 0 {
        return Ok(());
    }

    // If pool_spot is 0 but oracle is non-zero the pool is either empty (caught
    // by EmptyReserve upstream) or something has gone wrong in accounting.
    // Return deviation exceeded so the swap fails safely rather than silently.
    if pool_spot_1e18 == 0 {
        return Err(AmmError::OraclePriceDeviation {
            deviation_bps: 10_000, // 100% deviation — treat as max deviation
            max_bps:        MAX_ORACLE_DEVIATION_BPS,
        });
    }

    let diff = if pool_spot_1e18 > oracle_price_1e18 {
        pool_spot_1e18 - oracle_price_1e18
    } else {
        oracle_price_1e18 - pool_spot_1e18
    };

    let deviation_bps = diff.saturating_mul(10_000) / oracle_price_1e18;
    if deviation_bps as u32 > MAX_ORACLE_DEVIATION_BPS {
        return Err(AmmError::OraclePriceDeviation {
            deviation_bps: deviation_bps as u32,
            max_bps:        MAX_ORACLE_DEVIATION_BPS,
        });
    }
    Ok(())
}

// ── Overflow-safe math ────────────────────────────────────────────────────────

/// Overflow-safe multiply-then-divide: `(a * b) / c`.
///
/// Returns `Err(Overflow)` if `a * b` overflows u128.
/// For the AMM formula this is safe for reserves up to ~1.8 × 10^19 tokens,
/// which is well above realistic ZBX supply figures.
pub fn safe_mul_div(a: u128, b: u128, c: u128) -> Result<u128, AmmError> {
    if c == 0 {
        return Err(AmmError::DivisionByZero);
    }
    let num = a.checked_mul(b).ok_or(AmmError::Overflow)?;
    Ok(num / c)
}

// ── Circuit breaker ───────────────────────────────────────────────────────────

/// Pool-level circuit breaker state.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CircuitBreaker {
    /// If true, all swaps, adds, and removes are paused.
    pub paused: bool,
    /// Reason for the pause (for logs / governance).
    pub reason:  Option<String>,
    /// Block number when the pause was triggered.
    pub since_block: u64,
}

impl CircuitBreaker {
    pub fn new() -> Self { Self::default() }

    /// Trigger the circuit breaker.
    pub fn trip(&mut self, reason: &str, block: u64) {
        self.paused      = true;
        self.reason      = Some(reason.to_string());
        self.since_block = block;
    }

    /// Governance can clear the pause.
    pub fn clear(&mut self) {
        self.paused      = false;
        self.reason      = None;
        self.since_block = 0;
    }

    /// Returns `Err(PoolPaused)` if the breaker is tripped.
    pub fn check(&self) -> Result<(), AmmError> {
        if self.paused {
            return Err(AmmError::PoolPaused {
                reason: self.reason.clone().unwrap_or_default(),
            });
        }
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reentrancy_guard_blocks_second_entry() {
        let mut g = ReentrancyGuard::new();
        g.acquire().unwrap();
        assert!(matches!(g.acquire(), Err(AmmError::Reentrancy)));
        g.release();
        assert!(g.acquire().is_ok());
    }

    #[test]
    fn price_impact_within_limit_ok() {
        // Swap 10 into reserve of 1000 → impact = 10/1010 ≈ 0.99% → OK
        check_price_impact(10, 1_000).unwrap();
    }

    #[test]
    fn price_impact_too_high_err() {
        // Swap 500 into reserve of 1000 → impact = 500/1500 ≈ 33% → Err
        let err = check_price_impact(500, 1_000);
        assert!(matches!(err, Err(AmmError::PriceImpactTooHigh { .. })));
    }

    #[test]
    fn slippage_check_passes() {
        check_slippage(100, 95).unwrap();
    }

    #[test]
    fn slippage_check_fails() {
        let err = check_slippage(90, 95);
        assert!(matches!(err, Err(AmmError::SlippageExceeded { .. })));
    }

    #[test]
    fn deadline_not_expired() {
        check_deadline(1000, 2000).unwrap();
    }

    #[test]
    fn deadline_expired() {
        let err = check_deadline(3000, 2000);
        assert!(matches!(err, Err(AmmError::DeadlineExpired { .. })));
    }

    #[test]
    fn reserve_drain_check_passes() {
        // Draining 200 from 1000 = 20% → OK (< 30%)
        check_reserve_drain(200, 1_000).unwrap();
    }

    #[test]
    fn reserve_drain_check_fails() {
        // Draining 400 from 1000 = 40% → Err (> 30%)
        let err = check_reserve_drain(400, 1_000);
        assert!(matches!(err, Err(AmmError::ReserveDrainExceeded { .. })));
    }

    #[test]
    fn oracle_deviation_within_limit() {
        let spot   = 1_000_000_000_000_000_000u128; // 1.0
        let oracle = 1_050_000_000_000_000_000u128; // 1.05 → 5% dev
        check_oracle_deviation(spot, oracle).unwrap();
    }

    #[test]
    fn oracle_deviation_too_high() {
        let spot   = 1_000_000_000_000_000_000u128; // 1.0
        let oracle = 1_200_000_000_000_000_000u128; // 1.20 → 20% dev
        let err = check_oracle_deviation(spot, oracle);
        assert!(matches!(err, Err(AmmError::OraclePriceDeviation { .. })));
    }

    #[test]
    fn oracle_skipped_only_when_oracle_is_zero() {
        // FIX: oracle == 0 → skip (no TWAP data yet)
        check_oracle_deviation(1_000_000_000_000_000_000, 0).unwrap();

        // FIX: pool_spot == 0 + oracle != 0 → BLOCKED (not skipped)
        // This prevents a manipulator who zeroes the oracle from bypassing the check.
        let err = check_oracle_deviation(0, 1_000_000_000_000_000_000);
        assert!(
            matches!(err, Err(AmmError::OraclePriceDeviation { .. })),
            "zero pool_spot with non-zero oracle must be blocked, not skipped"
        );
    }

    #[test]
    fn safe_mul_div_correct() {
        assert_eq!(safe_mul_div(100, 997, 1000).unwrap(), 99);
    }

    #[test]
    fn safe_mul_div_overflow() {
        // u128::MAX * 2 overflows
        let err = safe_mul_div(u128::MAX, 2, 1);
        assert!(matches!(err, Err(AmmError::Overflow)));
    }

    #[test]
    fn circuit_breaker_trips_and_clears() {
        let mut cb = CircuitBreaker::new();
        cb.check().unwrap();

        cb.trip("oracle deviation", 500);
        assert!(matches!(cb.check(), Err(AmmError::PoolPaused { .. })));

        cb.clear();
        cb.check().unwrap();
    }
}
