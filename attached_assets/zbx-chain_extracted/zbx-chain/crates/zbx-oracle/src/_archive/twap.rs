//! TWAP Oracle — Time-Weighted Average Price for ZBX DeFi.
//! Manipulation-resistant price averaging over configurable windows.

use std::collections::{VecDeque, HashMap};
use std::time::{Duration, Instant};

use crate::types::{Address, U256};

/// TWAP window duration
pub const DEFAULT_TWAP_WINDOW: u64 = 1800; // 30 minutes
/// Minimum observations for valid TWAP
pub const MIN_TWAP_OBSERVATIONS: usize = 5;
/// Maximum staleness allowed for TWAP
pub const MAX_TWAP_STALENESS: u64 = 300; // 5 minutes
/// Cardinality (max observations stored per pair)
pub const DEFAULT_CARDINALITY: usize = 65535;

/// Price observation
#[derive(Debug, Clone)]
pub struct PriceObservation {
    pub timestamp: u64,
    pub price: U256,               // price in base units (18 decimals)
    pub cumulative_price: u128,    // sum of price * dt
    pub liquidity: u128,           // pool liquidity snapshot
}

/// TWAP state per token pair
#[derive(Debug)]
pub struct TwapState {
    pub token0: Address,
    pub token1: Address,
    pub observations: VecDeque<PriceObservation>,
    pub cardinality: usize,
    pub last_update: u64,
    pub initialized: bool,
}

impl TwapState {
    pub fn new(token0: Address, token1: Address, cardinality: usize) -> Self {
        Self {
            token0, token1,
            observations: VecDeque::with_capacity(cardinality),
            cardinality,
            last_update: 0,
            initialized: false,
        }
    }

    /// Record a new price observation
    pub fn record(&mut self, timestamp: u64, price: U256, liquidity: u128) -> Result<(), TwapError> {
        if timestamp <= self.last_update && self.initialized {
            return Err(TwapError::StaleObservation { ts: timestamp, last: self.last_update });
        }

        let dt = if self.initialized { timestamp - self.last_update } else { 1 };
        let prev_cumulative = self.observations.back().map(|o| o.cumulative_price).unwrap_or(0);
        let price_u128 = price.as_u128();
        let cumulative = prev_cumulative.saturating_add(price_u128.saturating_mul(dt as u128));

        let obs = PriceObservation { timestamp, price, cumulative_price: cumulative, liquidity };

        if self.observations.len() >= self.cardinality {
            self.observations.pop_front();
        }
        self.observations.push_back(obs);
        self.last_update = timestamp;
        self.initialized = true;
        Ok(())
    }

    /// Compute TWAP over [now - window, now]
    pub fn get_twap(&self, window: u64, current_ts: u64) -> Result<U256, TwapError> {
        if !self.initialized { return Err(TwapError::NotInitialized); }
        if current_ts.saturating_sub(self.last_update) > MAX_TWAP_STALENESS {
            return Err(TwapError::Stale { age: current_ts - self.last_update });
        }
        if self.observations.len() < MIN_TWAP_OBSERVATIONS {
            return Err(TwapError::InsufficientData { got: self.observations.len(), min: MIN_TWAP_OBSERVATIONS });
        }

        let target_ts = current_ts.saturating_sub(window);

        // Find observations bracketing [target_ts, current_ts]
        let latest = self.observations.back().ok_or(TwapError::NotInitialized)?;
        let oldest = self.observations.iter()
            .find(|o| o.timestamp >= target_ts)
            .unwrap_or_else(|| self.observations.front().unwrap());

        let time_delta = latest.timestamp.saturating_sub(oldest.timestamp);
        if time_delta == 0 { return Ok(latest.price); }

        let price_delta = latest.cumulative_price.saturating_sub(oldest.cumulative_price);
        let twap = price_delta / time_delta as u128;
        Ok(U256::from(twap))
    }

    /// Get TWAP with volume-weighted averaging
    pub fn get_vwap(&self, window: u64, current_ts: u64) -> Result<U256, TwapError> {
        if !self.initialized { return Err(TwapError::NotInitialized); }
        let target_ts = current_ts.saturating_sub(window);

        let relevant: Vec<&PriceObservation> = self.observations.iter()
            .filter(|o| o.timestamp >= target_ts)
            .collect();

        if relevant.len() < MIN_TWAP_OBSERVATIONS {
            return Err(TwapError::InsufficientData { got: relevant.len(), min: MIN_TWAP_OBSERVATIONS });
        }

        // VWAP = sum(price * liquidity) / sum(liquidity)
        let (price_vol, total_vol) = relevant.iter().fold((0u128, 0u128), |(pv, tv), o| {
            let price = o.price.as_u128();
            let liq = o.liquidity;
            (pv.saturating_add(price.saturating_mul(liq)), tv.saturating_add(liq))
        });
        if total_vol == 0 { return self.get_twap(window, current_ts); }
        Ok(U256::from(price_vol / total_vol))
    }

    /// Get price observations in a time range
    pub fn observations_in_range(&self, from_ts: u64, to_ts: u64) -> Vec<&PriceObservation> {
        self.observations.iter()
            .filter(|o| o.timestamp >= from_ts && o.timestamp <= to_ts)
            .collect()
    }
}

/// TWAP Oracle manager (all pairs)
pub struct TwapOracle {
    pub pairs: HashMap<(Address, Address), TwapState>,
    pub default_window: u64,
    pub default_cardinality: usize,
}

impl TwapOracle {
    pub fn new(window: u64) -> Self {
        Self { pairs: HashMap::new(), default_window: window, default_cardinality: DEFAULT_CARDINALITY }
    }

    /// Initialize a pair
    pub fn initialize_pair(&mut self, token0: Address, token1: Address) {
        let key = Self::pair_key(token0, token1);
        self.pairs.entry(key).or_insert_with(|| TwapState::new(token0, token1, self.default_cardinality));
    }

    /// Record a price update
    pub fn update(&mut self, token0: Address, token1: Address, price: U256, liquidity: u128, timestamp: u64) -> Result<(), TwapError> {
        let key = Self::pair_key(token0, token1);
        let state = self.pairs.get_mut(&key).ok_or(TwapError::PairNotFound(token0, token1))?;
        state.record(timestamp, price, liquidity)
    }

    /// Get TWAP for a pair
    pub fn get_price(&self, token0: Address, token1: Address, current_ts: u64) -> Result<U256, TwapError> {
        let key = Self::pair_key(token0, token1);
        let state = self.pairs.get(&key).ok_or(TwapError::PairNotFound(token0, token1))?;
        state.get_twap(self.default_window, current_ts)
    }

    /// Get VWAP for a pair
    pub fn get_vwap(&self, token0: Address, token1: Address, current_ts: u64) -> Result<U256, TwapError> {
        let key = Self::pair_key(token0, token1);
        let state = self.pairs.get(&key).ok_or(TwapError::PairNotFound(token0, token1))?;
        state.get_vwap(self.default_window, current_ts)
    }

    fn pair_key(a: Address, b: Address) -> (Address, Address) {
        if a.0 < b.0 { (a, b) } else { (b, a) }
    }
}

/// TWAP errors
#[derive(Debug, thiserror::Error)]
pub enum TwapError {
    #[error("TWAP not initialized")]
    NotInitialized,
    #[error("Stale data: age {age}s")]
    Stale { age: u64 },
    #[error("Stale observation: ts {ts} <= last {last}")]
    StaleObservation { ts: u64, last: u64 },
    #[error("Insufficient data: got {got}, min {min}")]
    InsufficientData { got: usize, min: usize },
    #[error("Pair not found: {0:?}/{1:?}")]
    PairNotFound(Address, Address),
    #[error("Invalid window: {0}")]
    InvalidWindow(u64),
}