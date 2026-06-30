//! Oracle circuit breakers — automatic price sanity enforcement.
//!
//! Circuit breakers prevent the oracle from publishing prices that are:
//! 1. Outside absolute bounds (Chainlink-style min/max answer)
//! 2. Moving too fast between consecutive rounds (velocity check)
//! 3. Stale (heartbeat timeout exceeded)
//! 4. Lacking quorum (too few reporters)
//!
//! ## Circuit breaker states
//!
//! ```text
//! Closed (normal) → [trip condition] → Open (halted)
//!                                           ↓
//!                              [cool-down + manual reset]
//!                                           ↓
//!                                     Half-Open (test)
//!                                           ↓
//!                              [first good round] → Closed
//! ```
//!
//! When a feed's circuit breaker is Open:
//! - No new price is published on-chain for that feed
//! - `latestRoundData()` returns the last known good price + a "stale" flag
//! - Dependent protocols (lending, AMM) are expected to switch to fallback pricing
//!
//! ## Velocity guard
//!
//! Prevents flash-loan / oracle manipulation attacks:
//! - If `|new_price - last_price| / last_price > MAX_VELOCITY_PCT`  in one round → trip
//! - Default: 20% per round (configurable per feed)
//! - ZBX/USD: 20%, ETH/USD: 15%, ZUSD/USD: 5% (stablecoin)

use crate::feed::{FeedId, Price};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// Maximum price velocity (percent change per round) before circuit trips.
pub const DEFAULT_MAX_VELOCITY_PCT: f64 = 20.0;

/// Stablecoin max velocity — tighter bound.
pub const STABLECOIN_MAX_VELOCITY_PCT: f64 = 5.0;

/// Number of consecutive "good" rounds needed to close a Half-Open breaker.
pub const HALF_OPEN_CLOSE_THRESHOLD: u32 = 3;

/// Cool-down period after tripping before breaker enters Half-Open (seconds).
pub const COOLDOWN_SECS: u64 = 300; // 5 minutes

// ── Breaker state ─────────────────────────────────────────────────────────────

/// The state of a circuit breaker for one feed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BreakerState {
    /// Normal operation — price updates pass through.
    Closed,
    /// Tripped — no updates published until reset.
    Open {
        /// Why it tripped.
        reason:   TripReason,
        /// When it tripped (Unix seconds).
        tripped_at: u64,
    },
    /// Test phase — first good round will close the breaker.
    HalfOpen {
        /// Good rounds seen since entering Half-Open.
        good_rounds: u32,
    },
}

impl BreakerState {
    pub fn is_open(&self) -> bool { matches!(self, BreakerState::Open { .. }) }
    pub fn is_closed(&self) -> bool { matches!(self, BreakerState::Closed) }
}

/// Why a circuit breaker tripped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TripReason {
    /// Price exceeded `min_answer` circuit breaker low.
    BelowMinAnswer { price: i128, min: i128 },
    /// Price exceeded `max_answer` circuit breaker high.
    AboveMaxAnswer { price: i128, max: i128 },
    /// Price moved more than `max_velocity_pct` since last round.
    VelocityExceeded { prev: f64, next: f64, pct_change: f64, max_pct: f64 },
    /// Feed heartbeat expired — no update within `heartbeat_secs`.
    HeartbeatExpired { last_update: u64, now: u64, max_secs: u64 },
    /// Insufficient reporters in this round.
    InsufficientReporters { got: u32, required: u32 },
}

impl TripReason {
    pub fn description(&self) -> String {
        match self {
            Self::BelowMinAnswer { price, min } =>
                format!("price {price} below min_answer {min}"),
            Self::AboveMaxAnswer { price, max } =>
                format!("price {price} above max_answer {max}"),
            Self::VelocityExceeded { pct_change, max_pct, .. } =>
                format!("price moved {pct_change:.1}% (max allowed {max_pct:.1}%)"),
            Self::HeartbeatExpired { last_update, now, max_secs } =>
                format!("no update for {}s (max {}s); last at {last_update}", now - last_update, max_secs),
            Self::InsufficientReporters { got, required } =>
                format!("only {got} reporters (need {required})"),
        }
    }
}

// ── Per-feed circuit breaker ──────────────────────────────────────────────────

/// Circuit breaker configuration for one feed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BreakerConfig {
    pub feed_id:         FeedId,
    /// Minimum valid price (absolute lower bound).
    pub min_answer:      i128,
    /// Maximum valid price (absolute upper bound).
    pub max_answer:      i128,
    /// Maximum percent change per round (velocity guard).
    pub max_velocity_pct: f64,
    /// Maximum seconds between updates before tripping heartbeat.
    pub heartbeat_secs:  u64,
    /// Minimum number of reporters required.
    pub min_reporters:   u32,
}

impl BreakerConfig {
    pub fn zbx_usd() -> Self {
        Self {
            feed_id:          FeedId::zbx_usd(),
            min_answer:       1_000_000,             // $0.01
            max_answer:       100_000_000_000,       // $1,000
            max_velocity_pct: DEFAULT_MAX_VELOCITY_PCT,
            heartbeat_secs:   3_600,
            min_reporters:    5,
        }
    }

    pub fn zusd_usd() -> Self {
        Self {
            feed_id:          FeedId::zusd_usd(),
            min_answer:       90_000_000,    // $0.90
            max_answer:       110_000_000,   // $1.10
            max_velocity_pct: STABLECOIN_MAX_VELOCITY_PCT,
            heartbeat_secs:   1_800,
            min_reporters:    5,
        }
    }

    pub fn eth_usd() -> Self {
        Self {
            feed_id:          FeedId::eth_usd(),
            min_answer:       1_00000000,            // $1
            max_answer:       1_000_000_00000000,    // $1,000,000
            max_velocity_pct: 15.0,
            heartbeat_secs:   3_600,
            min_reporters:    5,
        }
    }

    /// Generic config for a new feed.
    pub fn generic(feed_id: FeedId, min_answer: i128, max_answer: i128) -> Self {
        Self {
            feed_id,
            min_answer,
            max_answer,
            max_velocity_pct: DEFAULT_MAX_VELOCITY_PCT,
            heartbeat_secs:   3_600,
            min_reporters:    3,
        }
    }
}

/// Live circuit breaker state tracker for one feed.
#[derive(Debug)]
pub struct CircuitBreaker {
    pub config:     BreakerConfig,
    pub state:      BreakerState,
    pub last_price: Option<Price>,
    pub last_update: u64,
    pub trips_total: u32,
}

impl CircuitBreaker {
    pub fn new(config: BreakerConfig) -> Self {
        Self {
            config,
            state:       BreakerState::Closed,
            last_price:  None,
            last_update: 0,
            trips_total: 0,
        }
    }

    /// Evaluate a proposed new price update.
    ///
    /// Returns `Ok(())` if the update should be published, or `Err(TripReason)` if
    /// the circuit breaker should trip and the update should be suppressed.
    pub fn check(
        &mut self,
        new_price:    Price,
        now:          u64,
        reporter_count: u32,
    ) -> Result<(), TripReason> {
        // 1. Absolute bounds (always check, even if already open)
        if new_price.0 < self.config.min_answer {
            let reason = TripReason::BelowMinAnswer {
                price: new_price.0,
                min:   self.config.min_answer,
            };
            self.trip(reason.clone(), now);
            return Err(reason);
        }
        if new_price.0 > self.config.max_answer {
            let reason = TripReason::AboveMaxAnswer {
                price: new_price.0,
                max:   self.config.max_answer,
            };
            self.trip(reason.clone(), now);
            return Err(reason);
        }

        // 2. Reporter quorum
        if reporter_count < self.config.min_reporters {
            let reason = TripReason::InsufficientReporters {
                got:      reporter_count,
                required: self.config.min_reporters,
            };
            self.trip(reason.clone(), now);
            return Err(reason);
        }

        // 3. Velocity check (only if we have a previous price)
        if let Some(last) = self.last_price {
            if last.0 > 0 {
                let pct_change = ((new_price.0 - last.0).abs() as f64
                    / last.0 as f64) * 100.0;
                if pct_change > self.config.max_velocity_pct {
                    let reason = TripReason::VelocityExceeded {
                        prev:       last.to_f64(),
                        next:       new_price.to_f64(),
                        pct_change,
                        max_pct:    self.config.max_velocity_pct,
                    };
                    self.trip(reason.clone(), now);
                    return Err(reason);
                }
            }
        }

        // 4. Heartbeat (only check if we've ever published)
        if self.last_update > 0 {
            let elapsed = now.saturating_sub(self.last_update);
            if elapsed > self.config.heartbeat_secs {
                let reason = TripReason::HeartbeatExpired {
                    last_update: self.last_update,
                    now,
                    max_secs:    self.config.heartbeat_secs,
                };
                self.trip(reason.clone(), now);
                return Err(reason);
            }
        }

        // All checks passed — advance Half-Open or stay Closed
        match &mut self.state {
            BreakerState::HalfOpen { good_rounds } => {
                *good_rounds += 1;
                if *good_rounds >= HALF_OPEN_CLOSE_THRESHOLD {
                    self.state = BreakerState::Closed;
                    tracing::info!(
                        feed = %self.config.feed_id,
                        "Circuit breaker closed after {} good rounds",
                        HALF_OPEN_CLOSE_THRESHOLD
                    );
                }
            }
            BreakerState::Open { tripped_at, .. } => {
                // If cool-down has passed, move to Half-Open automatically
                if now.saturating_sub(*tripped_at) >= COOLDOWN_SECS {
                    self.state = BreakerState::HalfOpen { good_rounds: 1 };
                    tracing::info!(feed = %self.config.feed_id, "Circuit breaker half-open");
                } else {
                    return Err(TripReason::BelowMinAnswer {
                        price: new_price.0, min: self.config.min_answer
                    }); // already open — suppress update
                }
            }
            BreakerState::Closed => {}
        }

        // Commit the update
        self.last_price  = Some(new_price);
        self.last_update = now;
        Ok(())
    }

    /// Manually reset the circuit breaker (governance / admin action).
    pub fn reset(&mut self) {
        self.state = BreakerState::Closed;
        tracing::info!(feed = %self.config.feed_id, "Circuit breaker manually reset");
    }

    /// Force the circuit breaker into Half-Open (for testing purposes).
    pub fn half_open(&mut self) {
        self.state = BreakerState::HalfOpen { good_rounds: 0 };
    }

    fn trip(&mut self, reason: TripReason, now: u64) {
        self.trips_total += 1;
        tracing::warn!(
            feed   = %self.config.feed_id,
            reason = %reason.description(),
            trips  = self.trips_total,
            "Circuit breaker tripped"
        );
        self.state = BreakerState::Open { reason, tripped_at: now };
    }

    pub fn is_open(&self)   -> bool { self.state.is_open() }
    pub fn is_closed(&self) -> bool { self.state.is_closed() }
}

// ── Global breaker registry ───────────────────────────────────────────────────

/// Registry of circuit breakers for all active feeds.
pub struct BreakerRegistry {
    breakers: HashMap<FeedId, CircuitBreaker>,
}

impl BreakerRegistry {
    pub fn new() -> Self { Self { breakers: HashMap::new() } }

    /// Register a circuit breaker for a feed.
    pub fn register(&mut self, config: BreakerConfig) {
        let feed_id = config.feed_id.clone();
        self.breakers.insert(feed_id, CircuitBreaker::new(config));
    }

    /// Register all standard feeds with default configs.
    pub fn register_standard_feeds(&mut self) {
        self.register(BreakerConfig::zbx_usd());
        self.register(BreakerConfig::zusd_usd());
        self.register(BreakerConfig::eth_usd());
    }

    /// Check a price update for a feed.
    pub fn check(&mut self, feed_id: &FeedId, price: Price, now: u64, reporters: u32)
        -> Result<(), TripReason>
    {
        match self.breakers.get_mut(feed_id) {
            Some(b) => b.check(price, now, reporters),
            None    => Ok(()), // No breaker registered — pass through
        }
    }

    /// Whether a feed's breaker is open.
    pub fn is_open(&self, feed_id: &FeedId) -> bool {
        self.breakers.get(feed_id).map(|b| b.is_open()).unwrap_or(false)
    }

    /// All feeds with an open circuit breaker.
    pub fn open_feeds(&self) -> Vec<&FeedId> {
        self.breakers.iter()
            .filter(|(_, b)| b.is_open())
            .map(|(id, _)| id)
            .collect()
    }

    /// Manual reset for a feed (admin action).
    pub fn reset(&mut self, feed_id: &FeedId) {
        if let Some(b) = self.breakers.get_mut(feed_id) { b.reset(); }
    }

    /// Total number of trips across all feeds.
    pub fn total_trips(&self) -> u32 {
        self.breakers.values().map(|b| b.trips_total).sum()
    }
}

impl Default for BreakerRegistry {
    fn default() -> Self { Self::new() }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_breaker() -> CircuitBreaker {
        CircuitBreaker::new(BreakerConfig::zbx_usd())
    }

    #[test]
    fn normal_price_passes() {
        let mut cb = make_breaker();
        let r = cb.check(Price::from_f64(2.50), 1000, 5);
        assert!(r.is_ok());
        assert!(cb.is_closed());
    }

    #[test]
    fn below_min_trips() {
        let mut cb = make_breaker();
        // ZBX/USD min = $0.01 (1_000_000 raw)
        let r = cb.check(Price::from_f64(0.001), 1000, 5);
        assert!(r.is_err());
        assert!(cb.is_open());
        assert_eq!(cb.trips_total, 1);
        assert!(matches!(r.unwrap_err(), TripReason::BelowMinAnswer { .. }));
    }

    #[test]
    fn velocity_guard_trips_on_large_move() {
        let mut cb = make_breaker();
        // First price: $2.50
        cb.check(Price::from_f64(2.50), 1000, 5).unwrap();
        // Second price: $4.00 (60% move — well above 20% limit)
        let r = cb.check(Price::from_f64(4.00), 1060, 5);
        assert!(r.is_err(), "60% move should trip velocity guard");
        assert!(matches!(r.unwrap_err(), TripReason::VelocityExceeded { .. }));
    }

    #[test]
    fn velocity_guard_allows_small_move() {
        let mut cb = make_breaker();
        cb.check(Price::from_f64(2.50), 1000, 5).unwrap();
        // 10% move — within 20% limit
        let r = cb.check(Price::from_f64(2.75), 1060, 5);
        assert!(r.is_ok(), "10% move should be allowed");
    }

    #[test]
    fn insufficient_reporters_trips() {
        let mut cb = make_breaker();
        // ZBX/USD requires 5 reporters; provide only 2
        let r = cb.check(Price::from_f64(2.50), 1000, 2);
        assert!(matches!(r.unwrap_err(), TripReason::InsufficientReporters { got: 2, required: 5 }));
    }

    #[test]
    fn stablecoin_velocity_tighter() {
        let mut cb = CircuitBreaker::new(BreakerConfig::zusd_usd());
        cb.check(Price::from_f64(1.00), 1000, 5).unwrap();
        // 8% move — allowed for ZBX/USD (20%) but not ZUSD/USD (5%)
        let r = cb.check(Price::from_f64(1.08), 1060, 5);
        assert!(r.is_err(), "8% on stablecoin should trip velocity guard");
    }

    #[test]
    fn manual_reset_restores_closed() {
        let mut cb = make_breaker();
        // Trip it
        cb.check(Price::from_f64(0.001), 1000, 5).unwrap_err();
        assert!(cb.is_open());
        // Manual reset
        cb.reset();
        assert!(cb.is_closed());
    }

    #[test]
    fn registry_tracks_open_feeds() {
        let mut reg = BreakerRegistry::new();
        reg.register_standard_feeds();
        // Trip ZBX/USD
        reg.check(&FeedId::zbx_usd(), Price::from_f64(0.001), 1000, 5).unwrap_err();
        assert!(reg.is_open(&FeedId::zbx_usd()));
        assert!(!reg.is_open(&FeedId::eth_usd()));
        let open = reg.open_feeds();
        assert_eq!(open.len(), 1);
    }
}
