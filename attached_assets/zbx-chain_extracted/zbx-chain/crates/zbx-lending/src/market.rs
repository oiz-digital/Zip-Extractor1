//! Lending market — supply and borrow pools for each supported asset.

use std::collections::HashMap;
use zbx_types::address::Address;

/// Unique identifier for a lending market (e.g. "ZBX", "ZUSD", "ETH").
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MarketId(pub String);

impl std::fmt::Display for MarketId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Current state of one lending market.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Market {
    pub id: MarketId,
    /// Total tokens supplied (in base units).
    pub total_supply: u128,
    /// Total tokens borrowed (in base units).
    pub total_borrows: u128,
    /// Reserve factor (basis points, 0–10000).
    pub reserve_factor_bps: u32,
    /// Protocol reserves accumulated.
    pub reserves: u128,
}

impl Market {
    pub fn new(id: MarketId, reserve_factor_bps: u32) -> Self {
        Self { id, total_supply: 0, total_borrows: 0, reserve_factor_bps, reserves: 0 }
    }

    /// Exchange rate: tokens per cToken (scaled by 1e18).
    pub fn exchange_rate(&self) -> u128 {
        if self.total_supply == 0 { return 1_000_000_000_000_000_000; }
        (self.total_supply + self.total_borrows) * 1_000_000_000_000_000_000
            / self.total_supply
    }
}

/// Per-account position in a single market.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AccountMarket {
    pub c_tokens: u128,
    pub borrow_balance: u128,
    pub borrow_index_snapshot: u128,
}

/// Registry of all active lending markets.
#[derive(Debug, Default)]
pub struct MarketRegistry {
    markets: HashMap<MarketId, Market>,
    positions: HashMap<(Address, MarketId), AccountMarket>,
}

impl MarketRegistry {
    pub fn add_market(&mut self, market: Market) {
        self.markets.insert(market.id.clone(), market);
    }

    pub fn market(&self, id: &MarketId) -> Option<&Market> {
        self.markets.get(id)
    }

    pub fn position(&self, addr: &Address, id: &MarketId) -> AccountMarket {
        self.positions.get(&(*addr, id.clone())).cloned().unwrap_or_default()
    }
}
