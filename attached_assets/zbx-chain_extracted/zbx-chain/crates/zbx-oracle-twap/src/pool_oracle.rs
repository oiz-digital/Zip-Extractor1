//! AMM pool TWAP oracle — wraps a ZBX AMM pool for price observation.

use crate::accumulator::{PriceAccumulator, TwapWindow};
use serde::{Serialize, Deserialize};
use std::collections::VecDeque;

/// Maximum number of accumulator snapshots stored per pool.
const MAX_OBSERVATIONS: usize = 720; // ~2 hours of 1-per-10s snapshots

/// TWAP oracle for a single AMM pool.
pub struct PoolOracle {
    pub pool_address: [u8; 20],
    pub token0:       [u8; 20],
    pub token1:       [u8; 20],
    /// Ring buffer of historical accumulators
    observations:     VecDeque<PriceAccumulator>,
    /// Current live accumulator (updated every block)
    current:          PriceAccumulator,
}

impl PoolOracle {
    pub fn new(pool_address: [u8; 20], token0: [u8; 20], token1: [u8; 20]) -> Self {
        Self {
            pool_address,
            token0,
            token1,
            observations: VecDeque::with_capacity(MAX_OBSERVATIONS),
            current: PriceAccumulator {
                price0_cumulative: 0,
                price1_cumulative: 0,
                block_timestamp:   0,
            },
        }
    }

    /// Called every block by the AMM — updates accumulator.
    pub fn on_new_block(&mut self, spot_price: u128, timestamp: u64) {
        self.current.update(spot_price, timestamp);

        // Store snapshot every 10 seconds
        let last_ts = self.observations.back().map(|o| o.block_timestamp).unwrap_or(0);
        if timestamp.saturating_sub(last_ts) >= 10 {
            if self.observations.len() >= MAX_OBSERVATIONS {
                self.observations.pop_front();
            }
            self.observations.push_back(self.current);
        }
    }

    /// Get TWAP for the last N seconds.
    ///
    /// # Errors
    /// Returns None if we don't have enough history.
    pub fn twap(&self, period_secs: u64) -> Option<u128> {
        let target_ts = self.current.block_timestamp.saturating_sub(period_secs);
        // Find the oldest observation within the window
        let start = self.observations.iter()
            .find(|o| o.block_timestamp >= target_ts)?;
        let window = TwapWindow { start: *start, end: self.current };
        window.twap_price()
    }

    /// 5-minute TWAP (fast price, used for liquidations).
    pub fn twap_5m(&self)  -> Option<u128> { self.twap(300)  }
    /// 30-minute TWAP (used for ZUSD collateral).
    pub fn twap_30m(&self) -> Option<u128> { self.twap(1800) }
    /// 1-hour TWAP (used for large settlements).
    pub fn twap_1h(&self)  -> Option<u128> { self.twap(3600) }

    /// How much history do we have?
    pub fn history_secs(&self) -> u64 {
        let oldest = self.observations.front().map(|o| o.block_timestamp).unwrap_or(0);
        self.current.block_timestamp.saturating_sub(oldest)
    }
}

/// Registry of all pool TWAP oracles.
pub struct TwapRegistry {
    pools: std::collections::HashMap<[u8; 20], PoolOracle>,
}

impl TwapRegistry {
    pub fn new() -> Self { Self { pools: std::collections::HashMap::new() } }

    pub fn register_pool(&mut self, pool: PoolOracle) {
        self.pools.insert(pool.pool_address, pool);
    }

    pub fn get_twap(&self, pool: [u8; 20], period_secs: u64) -> Option<u128> {
        self.pools.get(&pool)?.twap(period_secs)
    }

    /// Update all pools on new block.
    pub fn on_new_block(&mut self, pool: [u8; 20], spot_price: u128, timestamp: u64) {
        if let Some(oracle) = self.pools.get_mut(&pool) {
            oracle.on_new_block(spot_price, timestamp);
        }
    }
}