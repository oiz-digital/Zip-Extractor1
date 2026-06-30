//! Time-Weighted Average Price (TWAP) oracle.
//!
//! TWAP provides manipulation-resistant prices by averaging price observations
//! over a configurable time window. It is harder to manipulate than a spot price
//! because an attacker must sustain the false price for the entire TWAP window.
//!
//! ## Algorithm
//!
//! ```text
//! TWAP(T) = Σ(price_i × duration_i) / Σ(duration_i)
//!
//! Where:
//!   price_i    = price during interval i
//!   duration_i = seconds interval i was active
//!   T          = total window length (e.g. 1800s = 30 min)
//! ```
//!
//! ## Window sizes
//!
//! | Window | Use case | Manipulation cost |
//! |--------|----------|-------------------|
//! | 5 min  | DEX arbitrage signals | Low |
//! | 30 min | Lending protocol collateral | Medium |
//! | 2 hour | Options settlement | High |
//! | 24 hour | Index rebalancing | Very high |
//!
//! ## Ring buffer
//!
//! Observations are stored in a fixed-size ring buffer (`MAX_OBSERVATIONS = 1024`).
//! Older observations outside the window are excluded from computation but
//! kept in the buffer so any past window size can be computed without a refetch.

use crate::feed::{FeedId, Price};
use serde::{Serialize, Deserialize};
use std::collections::VecDeque;

/// Maximum number of price observations to retain per feed.
/// At one observation per second this covers ~17 minutes.
/// In practice observations arrive every 5–60 seconds so this covers hours.
pub const MAX_OBSERVATIONS: usize = 1_024;

/// Minimum observations needed to compute a valid TWAP.
pub const MIN_TWAP_OBSERVATIONS: usize = 3;

/// Default TWAP window sizes in seconds.
pub const TWAP_5MIN:  u64 = 300;
pub const TWAP_30MIN: u64 = 1_800;
pub const TWAP_2H:    u64 = 7_200;
pub const TWAP_24H:   u64 = 86_400;

// ── Price observation ─────────────────────────────────────────────────────────

/// A single price observation recorded at a point in time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriceObservation {
    /// The reported price at this instant.
    pub price:     Price,
    /// Unix timestamp (seconds) when this price was observed.
    pub timestamp: u64,
    /// Volume weight of this observation (optional — used for VWAP-TWAP hybrid).
    pub volume:    f64,
}

impl PriceObservation {
    pub fn new(price: Price, timestamp: u64) -> Self {
        Self { price, timestamp, volume: 1.0 }
    }

    pub fn with_volume(price: Price, timestamp: u64, volume: f64) -> Self {
        Self { price, timestamp, volume }
    }
}

// ── TWAP accumulator ──────────────────────────────────────────────────────────

/// TWAP accumulator for a single price feed.
///
/// Maintains a ring buffer of observations and can compute TWAP
/// for any window within the retained history.
#[derive(Debug)]
pub struct TwapAccumulator {
    pub feed_id:      FeedId,
    observations:    VecDeque<PriceObservation>,
    max_obs:         usize,
}

impl TwapAccumulator {
    pub fn new(feed_id: FeedId) -> Self {
        Self { feed_id, observations: VecDeque::new(), max_obs: MAX_OBSERVATIONS }
    }

    pub fn with_capacity(feed_id: FeedId, capacity: usize) -> Self {
        Self { feed_id, observations: VecDeque::new(), max_obs: capacity }
    }

    /// Record a new price observation.
    ///
    /// Observations must be added in non-decreasing timestamp order.
    /// If the buffer is full, the oldest observation is dropped.
    pub fn record(&mut self, price: Price, timestamp: u64) {
        // Drop leading observations that are too old (keep ring buffer bounded).
        if self.observations.len() >= self.max_obs {
            self.observations.pop_front();
        }
        self.observations.push_back(PriceObservation::new(price, timestamp));
    }

    /// Record a price observation with explicit volume weight.
    pub fn record_with_volume(&mut self, price: Price, timestamp: u64, volume: f64) {
        if self.observations.len() >= self.max_obs {
            self.observations.pop_front();
        }
        self.observations.push_back(PriceObservation::with_volume(price, timestamp, volume));
    }

    /// Compute TWAP over the last `window_secs` seconds ending at `now`.
    ///
    /// Returns `None` if fewer than `MIN_TWAP_OBSERVATIONS` exist in the window.
    pub fn twap(&self, now: u64, window_secs: u64) -> Option<TwapResult> {
        let window_start = now.saturating_sub(window_secs);

        // Collect observations within the window.
        let in_window: Vec<&PriceObservation> = self.observations.iter()
            .filter(|o| o.timestamp >= window_start && o.timestamp <= now)
            .collect();

        if in_window.len() < MIN_TWAP_OBSERVATIONS {
            return None;
        }

        // Compute time-weighted average.
        // Each observation is weighted by the time until the next observation
        // (or until `now` for the last observation).
        let mut weighted_sum = 0.0_f64;
        let mut total_time   = 0.0_f64;
        let mut min_price    = in_window[0].price;
        let mut max_price    = in_window[0].price;

        for i in 0..in_window.len() {
            let duration = if i + 1 < in_window.len() {
                (in_window[i + 1].timestamp - in_window[i].timestamp) as f64
            } else {
                (now - in_window[i].timestamp) as f64
            };

            // Guard: zero-duration intervals (two obs at same timestamp) use 1s
            let duration = if duration <= 0.0 { 1.0 } else { duration };

            weighted_sum += in_window[i].price.to_f64() * duration;
            total_time   += duration;

            if in_window[i].price < min_price { min_price = in_window[i].price; }
            if in_window[i].price > max_price { max_price = in_window[i].price; }
        }

        if total_time <= 0.0 { return None; }

        let twap_price = Price::from_f64(weighted_sum / total_time);

        Some(TwapResult {
            price:            twap_price,
            window_secs,
            observations:     in_window.len() as u32,
            min_price,
            max_price,
            computed_at:      now,
            oldest_obs_ts:    in_window[0].timestamp,
        })
    }

    /// Compute TWAP over the standard 30-minute window.
    pub fn twap_30min(&self, now: u64) -> Option<TwapResult> {
        self.twap(now, TWAP_30MIN)
    }

    /// Compute TWAP over the standard 5-minute window.
    pub fn twap_5min(&self, now: u64) -> Option<TwapResult> {
        self.twap(now, TWAP_5MIN)
    }

    /// Compute TWAP over the standard 24-hour window.
    pub fn twap_24h(&self, now: u64) -> Option<TwapResult> {
        self.twap(now, TWAP_24H)
    }

    /// Number of observations currently retained.
    pub fn len(&self) -> usize { self.observations.len() }

    /// Whether any observations have been recorded.
    pub fn is_empty(&self) -> bool { self.observations.is_empty() }

    /// Most recent observation, if any.
    pub fn latest(&self) -> Option<&PriceObservation> {
        self.observations.back()
    }

    /// Spot price (most recent observation).
    pub fn spot(&self) -> Option<Price> {
        self.latest().map(|o| o.price)
    }

    /// All observations within the given window (for external analysis).
    pub fn observations_in_window(&self, now: u64, window_secs: u64) -> Vec<&PriceObservation> {
        let start = now.saturating_sub(window_secs);
        self.observations.iter()
            .filter(|o| o.timestamp >= start && o.timestamp <= now)
            .collect()
    }
}

// ── TWAP result ───────────────────────────────────────────────────────────────

/// Result of a TWAP computation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TwapResult {
    /// The computed TWAP price.
    pub price:         Price,
    /// Window duration used (seconds).
    pub window_secs:   u64,
    /// Number of observations used.
    pub observations:  u32,
    /// Lowest price observed in the window.
    pub min_price:     Price,
    /// Highest price observed in the window.
    pub max_price:     Price,
    /// When TWAP was computed (Unix seconds).
    pub computed_at:   u64,
    /// Timestamp of the oldest observation used.
    pub oldest_obs_ts: u64,
}

impl TwapResult {
    /// Price spread within the window (max − min) as a percentage of TWAP.
    pub fn spread_pct(&self) -> f64 {
        if self.price.0 == 0 { return 0.0; }
        let spread = (self.max_price.0 - self.min_price.0).abs() as f64;
        (spread / self.price.0 as f64) * 100.0
    }

    /// How volatile was this window? spread_pct as a category.
    pub fn volatility_label(&self) -> &'static str {
        match self.spread_pct() as u64 {
            0..=1  => "low",
            2..=5  => "medium",
            6..=20 => "high",
            _      => "extreme",
        }
    }
}

// ── TWAP registry ─────────────────────────────────────────────────────────────

/// Registry of TWAP accumulators for all active feeds.
pub struct TwapRegistry {
    accumulators: std::collections::HashMap<FeedId, TwapAccumulator>,
}

impl TwapRegistry {
    pub fn new() -> Self {
        Self { accumulators: std::collections::HashMap::new() }
    }

    /// Get or create an accumulator for a feed.
    pub fn accumulator_for(&mut self, feed_id: FeedId) -> &mut TwapAccumulator {
        self.accumulators
            .entry(feed_id.clone())
            .or_insert_with(|| TwapAccumulator::new(feed_id))
    }

    /// Record a price observation for a feed.
    pub fn record(&mut self, feed_id: FeedId, price: Price, timestamp: u64) {
        self.accumulator_for(feed_id).record(price, timestamp);
    }

    /// Compute TWAP for a feed over the given window.
    pub fn twap(&self, feed_id: &FeedId, now: u64, window_secs: u64) -> Option<TwapResult> {
        self.accumulators.get(feed_id)?.twap(now, window_secs)
    }

    /// Spot price (latest observation) for a feed.
    pub fn spot(&self, feed_id: &FeedId) -> Option<Price> {
        self.accumulators.get(feed_id)?.spot()
    }

    /// All active feed IDs with at least one observation.
    pub fn active_feeds(&self) -> Vec<&FeedId> {
        self.accumulators.keys().collect()
    }
}

impl Default for TwapRegistry {
    fn default() -> Self { Self::new() }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_acc(feed: &str) -> TwapAccumulator {
        TwapAccumulator::new(FeedId(feed.into()))
    }

    #[test]
    fn twap_single_price_across_window() {
        let mut acc = make_acc("ZBX/USD");
        // Record 5 observations at 100 USD, spread over 1800s
        for i in 0..5u64 {
            acc.record(Price::from_f64(100.0), 1000 + i * 360);
        }
        let now = 1000 + 4 * 360; // = 2440
        let result = acc.twap(now, TWAP_30MIN).unwrap();
        assert!((result.price.to_f64() - 100.0).abs() < 0.01,
            "constant price TWAP should equal spot: got {}", result.price.to_f64());
    }

    #[test]
    fn twap_weighted_toward_longer_intervals() {
        let mut acc = make_acc("ETH/USD");
        let base = 1_000_000u64;
        // Price at 1000 for 1800s, then jumps to 2000 for 1 second
        acc.record(Price::from_f64(1000.0), base);
        acc.record(Price::from_f64(1000.0), base + 600);
        acc.record(Price::from_f64(1000.0), base + 1200);
        acc.record(Price::from_f64(2000.0), base + 1799);
        // now = base + 1800
        let result = acc.twap(base + 1800, TWAP_30MIN).unwrap();
        // TWAP should be close to 1000 — the 2000 spike only lasted 1 second
        assert!(result.price.to_f64() < 1010.0,
            "spike of 1s should barely move TWAP: got {}", result.price.to_f64());
    }

    #[test]
    fn twap_none_when_too_few_observations() {
        let mut acc = make_acc("BTC/USD");
        acc.record(Price::from_f64(68000.0), 1000);
        acc.record(Price::from_f64(68100.0), 1100);
        // Only 2 observations — below MIN_TWAP_OBSERVATIONS (3)
        let result = acc.twap(2000, TWAP_30MIN);
        assert!(result.is_none(), "2 observations should return None");
    }

    #[test]
    fn twap_none_when_outside_window() {
        let mut acc = make_acc("ZBX/USD");
        // Record old observations (2 hours ago)
        for i in 0..5u64 {
            acc.record(Price::from_f64(2.50), 1000 + i * 100);
        }
        // now = 8000 (>30min since all observations)
        let result = acc.twap(8000, TWAP_30MIN);
        assert!(result.is_none(), "all obs outside window should return None");
    }

    #[test]
    fn spread_pct_calculation() {
        let result = TwapResult {
            price:         Price::from_f64(100.0),
            window_secs:   TWAP_30MIN,
            observations:  10,
            min_price:     Price::from_f64(90.0),
            max_price:     Price::from_f64(110.0),
            computed_at:   2000,
            oldest_obs_ts: 200,
        };
        // spread = 20 / 100 = 20%
        assert!((result.spread_pct() - 20.0).abs() < 0.1,
            "spread should be 20%: got {:.2}%", result.spread_pct());
        assert_eq!(result.volatility_label(), "high");
    }

    #[test]
    fn ring_buffer_bounded() {
        let mut acc = TwapAccumulator::with_capacity(FeedId("TEST".into()), 8);
        for i in 0..20u64 {
            acc.record(Price::from_f64(1.0), i * 10);
        }
        assert_eq!(acc.len(), 8, "ring buffer must not exceed capacity");
        // Latest should be the 20th observation
        assert_eq!(acc.latest().unwrap().timestamp, 190);
    }

    #[test]
    fn twap_registry_multi_feed() {
        let mut reg = TwapRegistry::new();
        let now = 10_000u64;
        for i in 0..5u64 {
            reg.record(FeedId::zbx_usd(), Price::from_f64(2.50), now - 1500 + i * 300);
            reg.record(FeedId::eth_usd(), Price::from_f64(3500.0), now - 1500 + i * 300);
        }
        let zbx = reg.twap(&FeedId::zbx_usd(), now, TWAP_30MIN).unwrap();
        let eth = reg.twap(&FeedId::eth_usd(), now, TWAP_30MIN).unwrap();
        assert!((zbx.price.to_f64() - 2.50).abs() < 0.01);
        assert!((eth.price.to_f64() - 3500.0).abs() < 1.0);
    }

    #[test]
    fn spot_price_is_latest_observation() {
        let mut acc = make_acc("ZBX/USD");
        acc.record(Price::from_f64(2.00), 1000);
        acc.record(Price::from_f64(2.50), 2000);
        acc.record(Price::from_f64(3.00), 3000);
        assert!((acc.spot().unwrap().to_f64() - 3.00).abs() < 0.001);
    }

    #[test]
    fn twap_2h_window_covers_enough_obs() {
        let mut acc = make_acc("ZBX/USD");
        let base = 0u64;
        // 13 observations spread over 2 hours (900s apart)
        for i in 0..13u64 {
            acc.record(Price::from_f64(2.50 + i as f64 * 0.01), base + i * 720);
        }
        let now = base + 12 * 720;
        let result = acc.twap(now, TWAP_2H).unwrap();
        assert!(result.observations >= 3);
        // Average of linear series 2.50..2.62 → near 2.56
        assert!(result.price.to_f64() > 2.50 && result.price.to_f64() < 2.63);
    }
}
