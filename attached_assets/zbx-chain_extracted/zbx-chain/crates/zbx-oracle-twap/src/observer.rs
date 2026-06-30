//! TWAP observer — subscribes to AMM pool events and updates accumulators.

use crate::accumulator::{PriceAccumulator, TwapWindow};
use std::collections::HashMap;

/// A (base_token, quote_token) pair identifier.
pub type PairId = [u8; 32];

/// Observes AMM pool events and keeps per-pair TWAP accumulators up to date.
#[derive(Debug, Default)]
pub struct TwapObserver {
    /// Current accumulator snapshot per pair.
    accumulators: HashMap<PairId, PriceAccumulator>,
    /// The snapshot taken at the start of the observation window.
    window_starts: HashMap<PairId, PriceAccumulator>,
}

impl TwapObserver {
    pub fn new() -> Self { Self::default() }

    /// Record a new price observation for `pair` at `now` (unix seconds).
    /// `spot_price` uses the same fixed-point scale as `PriceAccumulator`.
    pub fn observe(&mut self, pair: PairId, now: u64, spot_price: u128) {
        let acc = self.accumulators.entry(pair).or_insert(PriceAccumulator {
            price0_cumulative: 0,
            price1_cumulative: 0,
            block_timestamp: now,
        });
        // Save start snapshot the first time we see this pair.
        self.window_starts.entry(pair).or_insert(*acc);
        acc.update(spot_price, now);
    }

    /// Return the TWAP for `pair` over the full observation window collected so far.
    /// Returns `None` if insufficient data (< 2 observations).
    pub fn twap(&self, pair: &PairId) -> Option<u128> {
        let start = self.window_starts.get(pair)?;
        let end   = self.accumulators.get(pair)?;
        let win = TwapWindow { start: *start, end: *end };
        win.twap_price()
    }

    /// Return the raw cumulative accumulator for a pair.
    pub fn accumulator(&self, pair: &PairId) -> Option<&PriceAccumulator> {
        self.accumulators.get(pair)
    }
}
