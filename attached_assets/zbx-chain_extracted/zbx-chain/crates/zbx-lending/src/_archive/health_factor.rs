//! Health factor computation for ZBX lending pool.
//!
//! The health factor (HF) determines if a position is solvent.
//!
//! ## Formula
//!   HF = (sum(collateral_i * price_i * liquidation_threshold_i)) /
//!         total_debt_value_in_USD
//!
//!   Where liquidation_threshold is the max LTV at which collateral is counted.
//!   e.g. ETH: threshold = 80% (every $100 ETH counts as $80 collateral)
//!
//! ## Health factor meaning
//!   HF > 1.0 : Position is HEALTHY (safe)
//!   HF = 1.0 : Position is at the LIQUIDATION THRESHOLD
//!   HF < 1.0 : Position is UNHEALTHY -> can be liquidated
//!
//! ## Liquidation
//!   When HF < 1.0, any caller can liquidate up to 50% of the debt
//!   (CLOSE_FACTOR = 50%). Liquidator receives collateral at a discount
//!   (LIQUIDATION_BONUS = 5-10% above market price).
//!
//! ## ZBX-specific collateral factors
//!   ZBX  : LTV 70%, liquidation_threshold 75%, bonus 10%
//!   ZUSD : LTV 90%, liquidation_threshold 95%, bonus 5%
//!   ETH  : LTV 80%, liquidation_threshold 85%, bonus 7%
//!   BTC  : LTV 75%, liquidation_threshold 80%, bonus 8%

use std::collections::HashMap;

/// Liquidation configuration per collateral asset.
#[derive(Debug, Clone)]
pub struct CollateralConfig {
    pub token:                   [u8; 20],
    pub symbol:                  String,
    /// Maximum loan-to-value ratio (basis points, 7000 = 70%)
    pub max_ltv_bps:             u16,
    /// Liquidation threshold (basis points, 7500 = 75%)
    pub liquidation_threshold_bps: u16,
    /// Liquidation bonus (basis points, 1000 = 10%)
    pub liquidation_bonus_bps:   u16,
    /// Whether this asset can be used as collateral
    pub collateral_enabled:      bool,
}

/// A user's collateral position (one entry per deposited asset).
#[derive(Debug, Clone)]
pub struct CollateralPosition {
    pub token:    [u8; 20],
    pub amount:   u128,     // token units (raw, not scaled)
    pub price_usd: u128,    // 8-decimal price (e.g. 250_000_000 = $2.50)
}

/// A user's debt position (one entry per borrowed asset).
#[derive(Debug, Clone)]
pub struct DebtPosition {
    pub token:       [u8; 20],
    pub amount:      u128,     // principal + accrued interest
    pub price_usd:   u128,     // 8-decimal price
}

/// Health factor precision (8 decimal places, like prices).
/// HF = 1.5 -> HealthFactor(150_000_000)
/// HF = 1.0 -> HealthFactor(100_000_000)  (liquidation boundary)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct HealthFactor(pub u128);

impl HealthFactor {
    pub const ONE:          Self = Self(100_000_000);
    pub const PRECISION:    u128 = 100_000_000; // 10^8
    pub const CLOSE_FACTOR: u16  = 5_000; // 50% in basis points

    pub fn is_healthy(self)     -> bool { self.0 >= Self::ONE.0 }
    pub fn is_liquidatable(self) -> bool { self.0 < Self::ONE.0 }
    pub fn to_f64(self)         -> f64  { self.0 as f64 / Self::PRECISION as f64 }

    pub fn from_f64(f: f64) -> Self { Self((f * Self::PRECISION as f64) as u128) }
}

impl std::fmt::Display for HealthFactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HF={:.4}", self.to_f64())
    }
}

/// Compute the health factor for a user's position.
///
/// Returns HealthFactor::ONE if there is no debt (can't be liquidated).
pub fn compute_health_factor(
    collaterals: &[CollateralPosition],
    debts:       &[DebtPosition],
    configs:     &HashMap<[u8; 20], CollateralConfig>,
) -> HealthFactor {
    // If no debt, health factor is infinite (represent as u128::MAX)
    if debts.is_empty() { return HealthFactor(u128::MAX); }

    // Sum collateral value weighted by liquidation threshold
    let weighted_collateral: u128 = collaterals.iter().filter_map(|pos| {
        let cfg = configs.get(&pos.token)?;
        if !cfg.collateral_enabled { return None; }

        // collateral_value = amount * price (8 decimals)
        let value = pos.amount.checked_mul(pos.price_usd)?
            .checked_div(1_000_000_000_000_000_000)?; // token decimals (18)

        // weighted = value * liquidation_threshold / 10000
        let weighted = value.checked_mul(cfg.liquidation_threshold_bps as u128)?
            .checked_div(10_000)?;
        Some(weighted)
    }).sum();

    // Sum total debt value
    let total_debt: u128 = debts.iter().filter_map(|d| {
        let value = d.amount.checked_mul(d.price_usd)?
            .checked_div(1_000_000_000_000_000_000)?;
        Some(value)
    }).sum();

    if total_debt == 0 { return HealthFactor(u128::MAX); }

    // HF = weighted_collateral * PRECISION / total_debt
    let hf = weighted_collateral
        .saturating_mul(HealthFactor::PRECISION)
        .checked_div(total_debt)
        .unwrap_or(0);

    HealthFactor(hf)
}

/// Maximum amount that can be liquidated in one call (CLOSE_FACTOR = 50%).
pub fn max_liquidatable_amount(debt_amount: u128) -> u128 {
    debt_amount * HealthFactor::CLOSE_FACTOR as u128 / 10_000
}

/// Amount of collateral received for liquidating a given debt amount.
///
/// liquidation_return = debt_amount * (1 + liquidation_bonus_bps/10000)
/// In the collateral token's units.
pub fn liquidation_collateral_return(
    debt_amount:        u128,
    debt_price_usd:     u128,
    collateral_price_usd: u128,
    liquidation_bonus_bps: u16,
) -> u128 {
    if collateral_price_usd == 0 { return 0; }

    // Debt value in USD
    let debt_usd = debt_amount.saturating_mul(debt_price_usd)
        .checked_div(1_000_000_000_000_000_000).unwrap_or(0);

    // With bonus
    let with_bonus = debt_usd.saturating_mul(10_000 + liquidation_bonus_bps as u128)
        .checked_div(10_000).unwrap_or(0);

    // Convert back to collateral token units
    with_bonus.saturating_mul(1_000_000_000_000_000_000)
        .checked_div(collateral_price_usd).unwrap_or(0)
}