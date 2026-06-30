//! Market registry — add/update/query trading pairs.

use std::collections::HashMap;
use zbx_types::address::Address;
use crate::error::PerpError;
use crate::funding::{current_funding_rate, next_funding_in, settle_funding};
use crate::types::{Market, MarketView};
use crate::{FUNDING_INTERVAL, MAX_LEVERAGE};

/// In-memory registry of all perpetual markets.
#[derive(Debug, Default)]
pub struct MarketRegistry {
    markets: HashMap<u64, Market>,
    next_id: u64,
}

impl MarketRegistry {
    pub fn new() -> Self {
        Self { markets: HashMap::new(), next_id: 0 }
    }

    /// Add a new market (owner-only in the contract; caller checks auth).
    /// Returns the assigned `market_id`.
    pub fn add_market(
        &mut self,
        oracle: Address,
        symbol: String,
        max_leverage: u64,
        now: u64,
    ) -> Result<u64, PerpError> {
        if oracle == Address([0u8; 20]) { return Err(PerpError::InvalidOracle); }
        if max_leverage == 0 || max_leverage > MAX_LEVERAGE {
            return Err(PerpError::LeverageTooHigh { got: max_leverage, max: MAX_LEVERAGE });
        }
        let id = self.next_id;
        self.markets.insert(id, Market::new(symbol, oracle, max_leverage, now));
        self.next_id += 1;
        tracing::info!(market_id = id, "market added");
        Ok(id)
    }

    /// Update oracle / active flag / max leverage (owner-only in contract).
    pub fn update_market(
        &mut self,
        market_id: u64,
        oracle: Address,
        active: bool,
        max_leverage: u64,
    ) -> Result<(), PerpError> {
        if oracle == Address([0u8; 20]) { return Err(PerpError::InvalidOracle); }
        if max_leverage == 0 || max_leverage > MAX_LEVERAGE {
            return Err(PerpError::LeverageTooHigh { got: max_leverage, max: MAX_LEVERAGE });
        }
        let m = self.get_mut(market_id)?;
        m.oracle       = oracle;
        m.active       = active;
        m.max_leverage = max_leverage;
        Ok(())
    }

    /// Get an immutable reference to a market.
    pub fn get(&self, market_id: u64) -> Result<&Market, PerpError> {
        self.markets.get(&market_id).ok_or(PerpError::MarketNotFound(market_id))
    }

    /// Get a mutable reference to a market.
    pub fn get_mut(&mut self, market_id: u64) -> Result<&mut Market, PerpError> {
        self.markets.get_mut(&market_id).ok_or(PerpError::MarketNotFound(market_id))
    }

    /// Total number of markets registered.
    pub fn market_count(&self) -> u64 {
        self.next_id
    }

    /// Settle funding for a market. Called before every trade.
    pub fn settle_funding(&mut self, market_id: u64, now: u64) -> Result<u64, PerpError> {
        let m = self.get_mut(market_id)?;
        Ok(settle_funding(m, now, FUNDING_INTERVAL))
    }

    /// Build a `MarketView` for the given market + current oracle price.
    pub fn view(&self, market_id: u64, mark_price: u128, now: u64) -> Result<MarketView, PerpError> {
        let m = self.get(market_id)?;
        let long_oi   = m.total_long_oi as i128;
        let short_oi  = m.total_short_oi as i128;
        Ok(MarketView {
            market_id,
            symbol:             m.symbol.clone(),
            oracle:             m.oracle,
            active:             m.active,
            max_leverage:       m.max_leverage,
            total_long_oi:      m.total_long_oi,
            total_short_oi:     m.total_short_oi,
            oi_imbalance:       long_oi - short_oi,
            cumulative_funding: m.cumulative_funding,
            current_funding:    current_funding_rate(m),
            next_funding_in:    next_funding_in(m, now, FUNDING_INTERVAL),
            mark_price,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address { Address([b; 20]) }

    #[test]
    fn add_and_get_market() {
        let mut r = MarketRegistry::new();
        let id = r.add_market(addr(1), "BTC".into(), 200, 0).unwrap();
        assert_eq!(id, 0);
        assert_eq!(r.market_count(), 1);
        let m = r.get(0).unwrap();
        assert_eq!(m.symbol, "BTC");
        assert!(m.active);
    }

    #[test]
    fn zero_oracle_rejected() {
        let mut r = MarketRegistry::new();
        assert!(r.add_market(addr(0), "X".into(), 10, 0).is_err());
    }

    #[test]
    fn leverage_too_high_rejected() {
        let mut r = MarketRegistry::new();
        assert!(matches!(
            r.add_market(addr(1), "X".into(), MAX_LEVERAGE + 1, 0),
            Err(PerpError::LeverageTooHigh { .. })
        ));
    }

    #[test]
    fn update_market_changes_oracle_and_leverage() {
        let mut r = MarketRegistry::new();
        let id = r.add_market(addr(1), "BTC".into(), 10, 0).unwrap();
        r.update_market(id, addr(2), false, 50).unwrap();
        let m = r.get(id).unwrap();
        assert_eq!(m.oracle, addr(2));
        assert!(!m.active);
        assert_eq!(m.max_leverage, 50);
    }
}
