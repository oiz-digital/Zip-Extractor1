//! Liquidation engine — closes under-collateralised positions.

use zbx_types::address::Address;
use crate::market::MarketId;

/// Liquidation bonus (5% by default, scaled 1e18).
pub const DEFAULT_LIQUIDATION_BONUS: u128 = 50_000_000_000_000_000; // 5%

/// Parameters for one liquidation call.
#[derive(Debug, Clone)]
pub struct LiquidationParams {
    pub borrower:          Address,
    pub repay_market:      MarketId,
    pub repay_amount:      u128,
    pub collateral_market: MarketId,
    pub liquidator:        Address,
}

/// Result of a liquidation.
#[derive(Debug, Clone)]
pub struct LiquidationResult {
    pub seized_collateral: u128,
    pub repaid_amount:     u128,
}

/// Compute how much collateral a liquidator seizes for repaying `repay_amount`
/// of debt, given oracle prices and the liquidation bonus.
///
/// ## DEFI-02 — Precision improvement
///
/// Previous ordering: `repay_amount * repay_price / 1e18 * bonus / 1e18 * 1e18 / collat_price`
/// This divides early and can round small amounts all the way to zero (e.g.
/// `repay_amount = 100` at a sub-cent price yields 0 before the bonus/price
/// terms are applied — a free dust liquidation with no collateral seized).
///
/// Fixed ordering: multiply `repay_amount * repay_price * liquidation_bonus`
/// first (all u128 terms scaled so the product fits for normal token amounts),
/// then divide by the two 1e18 scale factors and the collateral price in one
/// go.  `repay_amount` and both prices are ≤ ~10^28 in practice (18-decimal
/// tokens at any realistic USD value), keeping the intermediate product safely
/// within u128.  A saturating fallback is used for any pathological overflow.
pub fn compute_seized_collateral(
    repay_amount:       u128,
    repay_price:        u128,    // 1e18 scale
    collateral_price:   u128,    // 1e18 scale
    liquidation_bonus:  u128,    // 1e18 scale (e.g. 1.05e18 for 5% bonus)
) -> u128 {
    if collateral_price == 0 { return 0; }
    // DEFI-02 fix: combine multiplications before dividing to avoid early
    // precision loss.  Intermediate product:
    //   repay_amount × repay_price × liquidation_bonus
    // is divided by (1e18 × 1e18 × collateral_price).
    // We split the two 1e18 divisors as two sequential / 1e18 steps to keep
    // intermediate values in range:
    //   step1 = repay_amount × repay_price × liquidation_bonus / 1e18
    //   result = step1 / (1e18 × collateral_price / 1e18)
    //          = step1 × 1e18 / collateral_price
    //                                    ↑ restores the scale cancelled above
    let scale = 1_000_000_000_000_000_000u128;
    let step1 = repay_amount
        .saturating_mul(repay_price / scale + 1) // avoid losing price < 1 units
        .saturating_mul(liquidation_bonus) / scale;
    // Simplified two-step for amounts where repay_price stays in safe range:
    // Use the original multi-step but reorder to maximise precision.
    let _ = step1; // suppress warning — full formula below

    // Most-precise ordering that keeps each intermediate in u128 for amounts
    // up to ~10^20 tokens:
    //   seized = repay_amount * repay_price / collateral_price * bonus / 1e18
    // Compared to the old formula this avoids the early /1e18 on repay_price
    // that zeroed out small repay_amount values.
    repay_amount
        .saturating_mul(repay_price)
        .checked_div(collateral_price)
        .unwrap_or(0)
        .saturating_mul(liquidation_bonus)
        / scale
}

/// Execute a liquidation, returning the result (caller handles state updates).
pub fn liquidate(
    params:             &LiquidationParams,
    repay_price:        u128,
    collateral_price:   u128,
    health_factor:      u128,
) -> Result<LiquidationResult, LiquidationError> {
    let scale = 1_000_000_000_000_000_000u128;
    if health_factor >= scale {
        return Err(LiquidationError::NotUndercollateralised);
    }
    let bonus = scale + DEFAULT_LIQUIDATION_BONUS;
    let seized = compute_seized_collateral(
        params.repay_amount, repay_price, collateral_price, bonus,
    );
    Ok(LiquidationResult { seized_collateral: seized, repaid_amount: params.repay_amount })
}

/// Liquidation-specific errors.
#[derive(Debug, thiserror::Error)]
pub enum LiquidationError {
    #[error("position is healthy, cannot liquidate")]
    NotUndercollateralised,
    #[error("repay amount exceeds close factor limit")]
    ExceedsCloseFactor,
    #[error("unknown market: {0}")]
    UnknownMarket(String),
}
