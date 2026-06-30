//! Collateral management — tracks collateral factors and account health.

use std::collections::HashMap;
use zbx_types::address::Address;
use crate::market::MarketId;

/// Collateral factor for a market (0–1 scaled by 1e18).
/// A factor of 0.75e18 means 1 unit of supplied asset counts as 0.75 units of borrowing power.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CollateralFactor {
    pub market: MarketId,
    /// Collateral factor (1e18 = 100%).
    pub factor: u128,
    /// Liquidation threshold (slightly above collateral factor, 1e18 = 100%).
    pub liquidation_threshold: u128,
}

impl CollateralFactor {
    pub fn new(market: MarketId, factor_pct: u8, liquidation_pct: u8) -> Self {
        Self {
            market,
            factor: (factor_pct as u128) * 1_000_000_000_000_000_000 / 100,
            liquidation_threshold: (liquidation_pct as u128) * 1_000_000_000_000_000_000 / 100,
        }
    }
}

/// Collateral registry — stores factors and computes account health.
#[derive(Debug, Default)]
pub struct CollateralRegistry {
    factors: HashMap<MarketId, CollateralFactor>,
}

impl CollateralRegistry {
    pub fn set_factor(&mut self, cf: CollateralFactor) {
        self.factors.insert(cf.market.clone(), cf);
    }

    pub fn factor(&self, market: &MarketId) -> Option<&CollateralFactor> {
        self.factors.get(market)
    }

    /// Health factor = weighted_collateral / total_borrows (1e18 scale).
    /// >1e18 means healthy; <1e18 means under-collateralised.
    pub fn health_factor(
        &self,
        _addr: &Address,
        supplied: &[(MarketId, u128)],
        borrows:  &[(MarketId, u128)],
        prices:   &HashMap<MarketId, u128>,
    ) -> u128 {
        let weighted_collateral: u128 = supplied.iter().map(|(mid, amount)| {
            let price  = prices.get(mid).copied().unwrap_or(0);
            let factor = self.factors.get(mid).map(|f| f.factor).unwrap_or(0);
            amount * price / 1_000_000_000_000_000_000
                   * factor / 1_000_000_000_000_000_000
        }).sum();

        let total_debt: u128 = borrows.iter().map(|(mid, amount)| {
            let price = prices.get(mid).copied().unwrap_or(0);
            amount * price / 1_000_000_000_000_000_000
        }).sum();

        if total_debt == 0 { return u128::MAX; }
        weighted_collateral * 1_000_000_000_000_000_000 / total_debt
    }
}
