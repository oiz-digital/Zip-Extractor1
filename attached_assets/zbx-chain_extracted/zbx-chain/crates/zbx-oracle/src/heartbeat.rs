//! Feed heartbeat monitor — detects stale / silent oracle feeds.
//!
//! Each price feed has a configured `heartbeat_secs` interval (the maximum
//! time allowed between two consecutive on-chain price updates). If a feed
//! goes silent for longer than its heartbeat, the monitor fires an alert.
//!
//! ## Why heartbeats matter
//!
//! A stale oracle price is dangerous for DeFi protocols:
//! - Lending protocols may use an outdated collateral value
//! - Stablecoins pegged to the oracle price drift without correction
//! - Liquidations may fail or execute at the wrong price
//!
//! ## Standard heartbeat periods
//!
//! | Feed        | Heartbeat | Reasoning |
//! |-------------|-----------|-----------|
//! | ZUSD/USD    | 30 min    | Stablecoin peg — most time-sensitive |
//! | ZBX/USD     | 1 hour    | Governance token — moderate urgency |
//! | ETH/USD     | 1 hour    | High-liquidity reference price |
//! | BTC/USD     | 1 hour    | High-liquidity reference price |
//! | SOL/USD     | 1 hour    | Liquid alt-coin |
//! | AVAX/USD    | 1 hour    | Liquid alt-coin |
//! | USD/INR     | 1 hour    | Forex — RBI updates daily but 1h is safe |
//! | MATIC/USD   | 2 hours   | Lower liquidity |
//! | ARB/USD     | 2 hours   | Newer token |
//! | OP/USD      | 2 hours   | Newer token |
//! | LINK/USD    | 2 hours   | Reference feed |
//! | DOT/USD     | 2 hours   | Cross-chain reference |
//! | ZNS/USD     | 4 hours   | Low volume — updates less often |

use crate::feed::{FeedId, Price};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

// ── Heartbeat configuration ───────────────────────────────────────────────────

/// Default heartbeat configurations per feed (seconds).
pub const HEARTBEAT_ZUSD_USD:  u64 = 1_800;  // 30 min
pub const HEARTBEAT_STD:       u64 = 3_600;  // 1 hour  (ZBX, ETH, BTC, SOL, AVAX, INR)
pub const HEARTBEAT_MEDIUM:    u64 = 7_200;  // 2 hours (MATIC, ARB, OP, LINK, DOT)
pub const HEARTBEAT_LOW:       u64 = 14_400; // 4 hours (ZNS)

/// Grace period before a heartbeat miss becomes a critical alert (seconds).
pub const HEARTBEAT_GRACE_SECS: u64 = 300; // 5 minutes

// ── Alert types ───────────────────────────────────────────────────────────────

/// Severity of a heartbeat alert.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum HeartbeatAlertLevel {
    /// Feed is approaching its heartbeat window (75% elapsed).
    Warning,
    /// Feed has exceeded its heartbeat but is within grace period.
    Critical,
    /// Feed has exceeded heartbeat + grace period. Price is stale.
    Stale,
}

/// A heartbeat alert for one feed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeartbeatAlert {
    pub feed_id:        FeedId,
    pub level:          HeartbeatAlertLevel,
    /// Last known good price.
    pub last_price:     Option<Price>,
    /// When the last update was accepted (Unix seconds).
    pub last_update:    u64,
    /// Current time when alert was raised (Unix seconds).
    pub now:            u64,
    /// Seconds since last update.
    pub age_secs:       u64,
    /// Configured heartbeat for this feed.
    pub heartbeat_secs: u64,
}

impl HeartbeatAlert {
    pub fn overdue_secs(&self) -> u64 {
        self.age_secs.saturating_sub(self.heartbeat_secs)
    }

    pub fn description(&self) -> String {
        format!(
            "[{}] {} — {}s since last update (heartbeat {}s, overdue {}s)",
            self.level.label(),
            self.feed_id,
            self.age_secs,
            self.heartbeat_secs,
            self.overdue_secs(),
        )
    }
}

impl HeartbeatAlertLevel {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Warning  => "WARN",
            Self::Critical => "CRIT",
            Self::Stale    => "STALE",
        }
    }
}

// ── Feed health record ────────────────────────────────────────────────────────

/// Live health record for one oracle feed.
#[derive(Clone, Debug)]
pub struct FeedHealth {
    pub feed_id:        FeedId,
    /// Configured maximum update interval.
    pub heartbeat_secs: u64,
    /// Last accepted price.
    pub last_price:     Option<Price>,
    /// Unix timestamp of last accepted update.
    pub last_update:    u64,
    /// Total updates received (since process start).
    pub updates_total:  u64,
    /// Total heartbeat misses (since process start).
    pub misses_total:   u64,
    /// Whether this feed is currently marked stale.
    pub is_stale:       bool,
}

impl FeedHealth {
    pub fn new(feed_id: FeedId, heartbeat_secs: u64) -> Self {
        Self {
            feed_id,
            heartbeat_secs,
            last_price:    None,
            last_update:   0,
            updates_total: 0,
            misses_total:  0,
            is_stale:      false,
        }
    }

    /// Record a successful price update.
    pub fn record_update(&mut self, price: Price, now: u64) {
        self.last_price   = Some(price);
        self.last_update  = now;
        self.updates_total += 1;
        self.is_stale     = false;
    }

    /// Age of the last update (seconds).
    pub fn age_secs(&self, now: u64) -> u64 {
        if self.last_update == 0 { u64::MAX }
        else { now.saturating_sub(self.last_update) }
    }

    /// Check heartbeat and return an alert if triggered.
    pub fn check(&mut self, now: u64) -> Option<HeartbeatAlert> {
        let age = self.age_secs(now);

        // Not yet had first update — skip
        if self.last_update == 0 { return None; }

        let alert_level = if age > self.heartbeat_secs + HEARTBEAT_GRACE_SECS {
            self.is_stale = true;
            self.misses_total += 1;
            Some(HeartbeatAlertLevel::Stale)
        } else if age > self.heartbeat_secs {
            Some(HeartbeatAlertLevel::Critical)
        } else if age > (self.heartbeat_secs * 3) / 4 {
            Some(HeartbeatAlertLevel::Warning)
        } else {
            None
        };

        alert_level.map(|level| HeartbeatAlert {
            feed_id:        self.feed_id.clone(),
            level,
            last_price:     self.last_price,
            last_update:    self.last_update,
            now,
            age_secs:       age,
            heartbeat_secs: self.heartbeat_secs,
        })
    }

    pub fn uptime_pct(&self) -> f64 {
        if self.updates_total == 0 { return 0.0; }
        let missed = self.misses_total as f64;
        let total  = (self.updates_total + self.misses_total) as f64;
        ((total - missed) / total) * 100.0
    }
}

// ── Heartbeat monitor ─────────────────────────────────────────────────────────

/// Global heartbeat monitor for all oracle feeds.
pub struct HeartbeatMonitor {
    feeds: HashMap<FeedId, FeedHealth>,
}

impl HeartbeatMonitor {
    pub fn new() -> Self { Self { feeds: HashMap::new() } }

    /// Register a feed with its heartbeat configuration.
    pub fn register(&mut self, feed_id: FeedId, heartbeat_secs: u64) {
        self.feeds.insert(
            feed_id.clone(),
            FeedHealth::new(feed_id, heartbeat_secs),
        );
    }

    /// Register all standard ZBX oracle feeds.
    pub fn register_all_feeds(&mut self) {
        // Stablecoin — tightest
        self.register(FeedId::zusd_usd(), HEARTBEAT_ZUSD_USD);
        // Standard 1h feeds
        self.register(FeedId::zbx_usd(),  HEARTBEAT_STD);
        self.register(FeedId::eth_usd(),  HEARTBEAT_STD);
        self.register(FeedId::btc_usd(),  HEARTBEAT_STD);
        self.register(FeedId::usd_inr(),  HEARTBEAT_STD);
        // New feeds from this session
        self.register(FeedId(String::from("SOL/USD")),  HEARTBEAT_STD);
        self.register(FeedId(String::from("AVAX/USD")), HEARTBEAT_STD);
        // 2h feeds
        self.register(FeedId(String::from("MATIC/USD")), HEARTBEAT_MEDIUM);
        self.register(FeedId(String::from("ARB/USD")),   HEARTBEAT_MEDIUM);
        self.register(FeedId(String::from("OP/USD")),    HEARTBEAT_MEDIUM);
        self.register(FeedId(String::from("LINK/USD")),  HEARTBEAT_MEDIUM);
        self.register(FeedId(String::from("DOT/USD")),   HEARTBEAT_MEDIUM);
        // Low-frequency
        self.register(FeedId::zns_usd(),  HEARTBEAT_LOW);
    }

    /// Record a price update for a feed.
    pub fn record_update(&mut self, feed_id: &FeedId, price: Price, now: u64) {
        if let Some(health) = self.feeds.get_mut(feed_id) {
            health.record_update(price, now);
        }
    }

    /// Run heartbeat check across all feeds.
    ///
    /// Returns all alerts that are currently firing.
    pub fn check_all(&mut self, now: u64) -> Vec<HeartbeatAlert> {
        let mut alerts = Vec::new();
        for health in self.feeds.values_mut() {
            if let Some(alert) = health.check(now) {
                tracing::warn!(
                    feed  = %alert.feed_id,
                    level = alert.level.label(),
                    age   = alert.age_secs,
                    "Heartbeat alert"
                );
                alerts.push(alert);
            }
        }
        alerts
    }

    /// Feeds currently marked stale.
    pub fn stale_feeds(&self) -> Vec<&FeedId> {
        self.feeds.iter()
            .filter(|(_, h)| h.is_stale)
            .map(|(id, _)| id)
            .collect()
    }

    /// Health summary for a specific feed.
    pub fn health(&self, feed_id: &FeedId) -> Option<&FeedHealth> {
        self.feeds.get(feed_id)
    }

    /// Number of registered feeds.
    pub fn feed_count(&self) -> usize { self.feeds.len() }
}

impl Default for HeartbeatMonitor {
    fn default() -> Self { Self::new() }
}

fn _now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_alert_within_heartbeat() {
        let mut h = FeedHealth::new(FeedId::zbx_usd(), HEARTBEAT_STD);
        let base = 10_000u64;
        h.record_update(Price::from_f64(2.50), base);
        // 30 minutes later — still within 1h heartbeat
        let alert = h.check(base + 1_800);
        assert!(alert.is_none(), "should not alert within heartbeat window");
    }

    #[test]
    fn warning_at_75_percent() {
        let mut h = FeedHealth::new(FeedId::zbx_usd(), HEARTBEAT_STD);
        let base = 10_000u64;
        h.record_update(Price::from_f64(2.50), base);
        // 75% of 3600 = 2700s
        let alert = h.check(base + 2_750).unwrap();
        assert_eq!(alert.level, HeartbeatAlertLevel::Warning);
    }

    #[test]
    fn critical_at_100_percent() {
        let mut h = FeedHealth::new(FeedId::zbx_usd(), HEARTBEAT_STD);
        let base = 10_000u64;
        h.record_update(Price::from_f64(2.50), base);
        // Exactly at heartbeat (3600s)
        let alert = h.check(base + 3_601).unwrap();
        assert_eq!(alert.level, HeartbeatAlertLevel::Critical);
    }

    #[test]
    fn stale_after_grace_period() {
        let mut h = FeedHealth::new(FeedId::zbx_usd(), HEARTBEAT_STD);
        let base = 10_000u64;
        h.record_update(Price::from_f64(2.50), base);
        // heartbeat (3600) + grace (300) + 1
        let alert = h.check(base + 3_901 + 1).unwrap();
        assert_eq!(alert.level, HeartbeatAlertLevel::Stale);
        assert!(h.is_stale);
    }

    #[test]
    fn update_clears_stale() {
        let mut h = FeedHealth::new(FeedId::zbx_usd(), HEARTBEAT_STD);
        let base = 10_000u64;
        h.record_update(Price::from_f64(2.50), base);
        h.check(base + 4_000); // make stale
        assert!(h.is_stale);
        h.record_update(Price::from_f64(2.50), base + 4_001);
        assert!(!h.is_stale, "update should clear stale state");
    }

    #[test]
    fn monitor_registers_all_standard_feeds() {
        let mut monitor = HeartbeatMonitor::new();
        monitor.register_all_feeds();
        // 13 feeds registered: ZUSD, ZBX, ETH, BTC, INR, SOL, AVAX, MATIC, ARB, OP, LINK, DOT, ZNS
        assert_eq!(monitor.feed_count(), 13,
            "must register all 13 standard feeds, got {}", monitor.feed_count());
    }
}
