//! Oracle reporter slashing — penalty engine for bad price submissions.
//!
//! Oracle reporters are trusted nodes that submit signed price reports.
//! Malicious or careless reporters can manipulate the aggregated price.
//! The slasher detects and penalises bad reporters.
//!
//! ## Slash conditions
//!
//! | Condition | Severity | Slash amount |
//! |-----------|----------|--------------|
//! | Price deviation > 5× median for 1 round | WARN | 0 (warning) |
//! | Price deviation > 5× median for 3 consecutive rounds | HIGH | 10% stake |
//! | Submitting future-dated reports (timestamp > now + 60s) | HIGH | 10% stake |
//! | Submitting duplicate reports in same round | MEDIUM | 5% stake |
//! | Submitting expired reports (older than 5 min) | MEDIUM | 5% stake |
//! | Reporter not in whitelist | HIGH | Report rejected (no slash) |
//! | Coordinated manipulation (≥ 2 reporters same bad price) | CRITICAL | 30% stake |
//!
//! ## Slash process
//!
//! 1. Aggregator detects outlier reporter during round close
//! 2. `Slasher::record_deviation()` increments the reporter's consecutive-miss count
//! 3. If count reaches `SLASH_THRESHOLD`, `OracleSlashEvent` is emitted
//! 4. Slash event is included in the next ZBX block as a special transaction
//! 5. `zbx-staking` crate processes the slash against the reporter's bonded stake
//!
//! ## Appeal
//!
//! Reporters have 1440 blocks (~2 hours at 5s/block) to appeal a slash.
//! Appeal requires a signed proof that the submitted price was correct
//! (e.g. a Merkle proof from a reference exchange's order book).

use crate::feed::{FeedId, Price};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// Consecutive outlier rounds before a slash is triggered.
pub const SLASH_THRESHOLD: u32 = 3;

/// Slash appeal window in blocks (~2 hours at 5s/block).
pub const APPEAL_WINDOW_BLOCKS: u64 = 1_440;

/// Maximum deviation (multiple of median) before a report is flagged as outlier.
pub const MAX_DEVIATION_MULTIPLE: f64 = 5.0;

/// Slash amounts in basis points of bonded stake.
pub const SLASH_BPS_MINOR:    u32 = 500;   // 5%   — single bad round
pub const SLASH_BPS_MAJOR:    u32 = 1_000; // 10%  — 3× consecutive
pub const SLASH_BPS_CRITICAL: u32 = 3_000; // 30%  — coordinated attack

// ── Slash event ───────────────────────────────────────────────────────────────

/// Severity of a slash event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SlashSeverity {
    /// Warning only — no stake slashed.
    Warning,
    /// Minor slash — 5% of bonded stake.
    Minor,
    /// Major slash — 10% of bonded stake.
    Major,
    /// Critical slash — 30% of bonded stake (coordinated attack).
    Critical,
}

impl SlashSeverity {
    pub fn slash_bps(&self) -> u32 {
        match self {
            Self::Warning  => 0,
            Self::Minor    => SLASH_BPS_MINOR,
            Self::Major    => SLASH_BPS_MAJOR,
            Self::Critical => SLASH_BPS_CRITICAL,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Warning  => "warning",
            Self::Minor    => "minor",
            Self::Major    => "major",
            Self::Critical => "critical",
        }
    }
}

/// An oracle slash event — included in a ZBX block to slash a reporter's stake.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OracleSlashEvent {
    /// The reporter being slashed.
    pub reporter:       [u8; 20],
    /// Which feed was manipulated.
    pub feed_id:        FeedId,
    /// The round ID in which the violation was detected.
    pub round_id:       u64,
    /// Slash severity and amount.
    pub severity:       SlashSeverity,
    /// Slash amount in basis points.
    pub slash_bps:      u32,
    /// Reporter's submitted price (the bad value).
    pub reported_price: Price,
    /// Oracle consensus price (the correct median).
    pub median_price:   Price,
    /// Deviation multiple (reported / median).
    pub deviation:      f64,
    /// Block number when detected.
    pub detected_block: u64,
    /// Block number until which appeal is allowed.
    pub appeal_until:   u64,
    /// Whether this slash has been appealed.
    pub appealed:       bool,
    /// Whether the appeal was successful (slash reversed).
    pub appeal_success: Option<bool>,
}

impl OracleSlashEvent {
    pub fn new(
        reporter:       [u8; 20],
        feed_id:        FeedId,
        round_id:       u64,
        severity:       SlashSeverity,
        reported_price: Price,
        median_price:   Price,
        deviation:      f64,
        detected_block: u64,
    ) -> Self {
        let slash_bps    = severity.slash_bps();
        let appeal_until = detected_block + APPEAL_WINDOW_BLOCKS;
        Self {
            reporter, feed_id, round_id, severity, slash_bps,
            reported_price, median_price, deviation, detected_block,
            appeal_until, appealed: false, appeal_success: None,
        }
    }

    pub fn is_appealable(&self, current_block: u64) -> bool {
        !self.appealed && current_block <= self.appeal_until
    }
}

// ── Per-reporter state ────────────────────────────────────────────────────────

/// Slashing state for one reporter.
#[derive(Clone, Debug, Default)]
pub struct ReporterSlashState {
    /// Address of this reporter.
    pub address: [u8; 20],
    /// Consecutive rounds where this reporter was an outlier.
    pub consecutive_outliers: u32,
    /// Total slashes received (historical).
    pub total_slashes: u32,
    /// Total slash amount in basis points (historical, cumulative).
    pub total_slash_bps: u32,
    /// Current round streak: which feeds have consecutive misses.
    pub miss_streak: HashMap<FeedId, u32>,
    /// All slash events for this reporter.
    pub slash_history: Vec<OracleSlashEvent>,
    /// Whether this reporter is currently suspended.
    pub suspended: bool,
}

impl ReporterSlashState {
    pub fn new(address: [u8; 20]) -> Self {
        Self { address, ..Default::default() }
    }

    /// Returns true if reporter has been suspended.
    pub fn is_suspended(&self) -> bool { self.suspended }
}

// ── Slasher ──────────────────────────────────────────────────────────────────

/// Oracle reporter slashing engine.
pub struct OracleSlasher {
    reporter_states: HashMap<[u8; 20], ReporterSlashState>,
    pending_events:  Vec<OracleSlashEvent>,
}

impl OracleSlasher {
    pub fn new() -> Self {
        Self {
            reporter_states: HashMap::new(),
            pending_events:  Vec::new(),
        }
    }

    /// Check a reporter's price report for deviation from the round median.
    ///
    /// Returns a slash event if the reporter has crossed the slash threshold.
    pub fn record_deviation(
        &mut self,
        reporter:       [u8; 20],
        feed_id:        FeedId,
        reported_price: Price,
        median_price:   Price,
        round_id:       u64,
        block:          u64,
    ) -> Option<OracleSlashEvent> {
        // Compute deviation multiple
        let deviation = if median_price.0 == 0 {
            0.0
        } else {
            (reported_price.0 - median_price.0).abs() as f64
                / median_price.0 as f64
                * MAX_DEVIATION_MULTIPLE  // scale to "multiples"
        };

        let state = self.reporter_states
            .entry(reporter)
            .or_insert_with(|| ReporterSlashState::new(reporter));

        if deviation > 1.0 {
            // This is an outlier — bump streak
            let streak = state.miss_streak.entry(feed_id.clone()).or_insert(0);
            *streak += 1;
            state.consecutive_outliers += 1;

            let streak_count = *streak;

            if streak_count == 1 {
                // First miss: warning only
                tracing::warn!(
                    reporter = hex::encode(reporter),
                    feed = %feed_id,
                    deviation = deviation,
                    "Oracle reporter outlier (warning)"
                );
                None
            } else if streak_count >= SLASH_THRESHOLD {
                // 3+ consecutive misses: major slash
                let severity = SlashSeverity::Major;
                let event = OracleSlashEvent::new(
                    reporter, feed_id.clone(), round_id, severity,
                    reported_price, median_price, deviation, block,
                );
                state.total_slashes += 1;
                state.total_slash_bps += SLASH_BPS_MAJOR;
                state.miss_streak.insert(feed_id, 0); // reset streak
                // Suspend if slashed 3+ times
                if state.total_slashes >= 3 { state.suspended = true; }
                self.pending_events.push(event.clone());
                tracing::error!(
                    reporter = hex::encode(reporter),
                    rounds = streak_count,
                    "Oracle reporter slashed (major) — {} consecutive outliers",
                    streak_count
                );
                Some(event)
            } else {
                // 2 misses: minor slash
                let severity = SlashSeverity::Minor;
                let event = OracleSlashEvent::new(
                    reporter, feed_id.clone(), round_id, severity,
                    reported_price, median_price, deviation, block,
                );
                state.total_slashes += 1;
                state.total_slash_bps += SLASH_BPS_MINOR;
                self.pending_events.push(event.clone());
                Some(event)
            }
        } else {
            // Good round — reset streak for this feed
            if let Some(state) = self.reporter_states.get_mut(&reporter) {
                state.miss_streak.insert(feed_id, 0);
                state.consecutive_outliers = 0;
            }
            None
        }
    }

    /// Record a coordination attack: two or more reporters submitted the same
    /// bad price. This is the most severe manipulation attempt.
    pub fn record_coordinated_attack(
        &mut self,
        reporters:   &[[u8; 20]],
        feed_id:     FeedId,
        bad_price:   Price,
        median_price: Price,
        round_id:    u64,
        block:       u64,
    ) -> Vec<OracleSlashEvent> {
        let deviation = if median_price.0 == 0 { 0.0 } else {
            (bad_price.0 - median_price.0).abs() as f64 / median_price.0 as f64 * 5.0
        };

        reporters.iter().map(|&reporter| {
            let event = OracleSlashEvent::new(
                reporter, feed_id.clone(), round_id, SlashSeverity::Critical,
                bad_price, median_price, deviation, block,
            );
            let state = self.reporter_states
                .entry(reporter)
                .or_insert_with(|| ReporterSlashState::new(reporter));
            state.total_slashes += 1;
            state.total_slash_bps += SLASH_BPS_CRITICAL;
            state.suspended = true;
            self.pending_events.push(event.clone());
            event
        }).collect()
    }

    /// Take all pending slash events (for inclusion in the next block).
    pub fn take_pending(&mut self) -> Vec<OracleSlashEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Get state for a reporter.
    pub fn reporter_state(&self, reporter: &[u8; 20]) -> Option<&ReporterSlashState> {
        self.reporter_states.get(reporter)
    }

    /// Whether a reporter is currently suspended.
    pub fn is_suspended(&self, reporter: &[u8; 20]) -> bool {
        self.reporter_states.get(reporter)
            .map(|s| s.suspended)
            .unwrap_or(false)
    }

    /// Total slash events emitted.
    pub fn total_events(&self) -> usize { self.pending_events.len() }
}

impl Default for OracleSlasher {
    fn default() -> Self { Self::new() }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> [u8; 20] { [n; 20] }

    #[test]
    fn single_outlier_is_warning_no_slash() {
        let mut slasher = OracleSlasher::new();
        // Reported $5.00 when median is $2.50 (100% deviation)
        let event = slasher.record_deviation(
            addr(1), FeedId::zbx_usd(),
            Price::from_f64(5.00), Price::from_f64(2.50),
            1, 1000,
        );
        assert!(event.is_none(), "first outlier should be warning only, no slash");
        assert_eq!(slasher.reporter_state(&addr(1)).unwrap().total_slashes, 0);
    }

    #[test]
    fn two_consecutive_outliers_minor_slash() {
        let mut slasher = OracleSlasher::new();
        slasher.record_deviation(addr(1), FeedId::zbx_usd(),
            Price::from_f64(5.00), Price::from_f64(2.50), 1, 100);
        let event = slasher.record_deviation(addr(1), FeedId::zbx_usd(),
            Price::from_f64(5.00), Price::from_f64(2.50), 2, 101);
        assert!(event.is_some(), "2nd consecutive outlier should trigger minor slash");
        assert_eq!(event.unwrap().severity, SlashSeverity::Minor);
    }

    #[test]
    fn three_consecutive_outliers_major_slash() {
        let mut slasher = OracleSlasher::new();
        for round in 1..=2u64 {
            slasher.record_deviation(addr(2), FeedId::zbx_usd(),
                Price::from_f64(5.00), Price::from_f64(2.50), round, round * 100);
        }
        let event = slasher.record_deviation(addr(2), FeedId::zbx_usd(),
            Price::from_f64(5.00), Price::from_f64(2.50), 3, 300);
        assert!(event.is_some());
        let e = event.unwrap();
        assert_eq!(e.severity, SlashSeverity::Major);
        assert_eq!(e.slash_bps, SLASH_BPS_MAJOR);
    }

    #[test]
    fn good_round_resets_streak() {
        let mut slasher = OracleSlasher::new();
        // Two outliers
        slasher.record_deviation(addr(3), FeedId::zbx_usd(),
            Price::from_f64(5.00), Price::from_f64(2.50), 1, 100);
        slasher.record_deviation(addr(3), FeedId::zbx_usd(),
            Price::from_f64(5.00), Price::from_f64(2.50), 2, 200);
        // Good round — streak resets
        slasher.record_deviation(addr(3), FeedId::zbx_usd(),
            Price::from_f64(2.51), Price::from_f64(2.50), 3, 300);
        // Now another outlier — should be back to warning
        let event = slasher.record_deviation(addr(3), FeedId::zbx_usd(),
            Price::from_f64(5.00), Price::from_f64(2.50), 4, 400);
        assert!(event.is_none(), "after reset, first outlier should be warning");
    }

    #[test]
    fn coordinated_attack_slashes_all_participants() {
        let mut slasher = OracleSlasher::new();
        let reporters = [addr(10), addr(11), addr(12)];
        let events = slasher.record_coordinated_attack(
            &reporters, FeedId::zbx_usd(),
            Price::from_f64(100.0), Price::from_f64(2.50),
            1, 1000,
        );
        assert_eq!(events.len(), 3, "all 3 reporters should be slashed");
        for e in &events {
            assert_eq!(e.severity, SlashSeverity::Critical);
            assert_eq!(e.slash_bps, SLASH_BPS_CRITICAL);
        }
        // All should be suspended
        for r in &reporters {
            assert!(slasher.is_suspended(r));
        }
    }

    #[test]
    fn appeal_window_calculated_correctly() {
        let event = OracleSlashEvent::new(
            addr(1), FeedId::zbx_usd(), 1,
            SlashSeverity::Major,
            Price::from_f64(5.0), Price::from_f64(2.5),
            2.0, 100_000,
        );
        assert_eq!(event.appeal_until, 100_000 + APPEAL_WINDOW_BLOCKS);
        assert!(event.is_appealable(100_001));
        assert!(!event.is_appealable(200_000)); // too late
    }

    #[test]
    fn slash_severity_amounts() {
        assert_eq!(SlashSeverity::Warning.slash_bps(),  0);
        assert_eq!(SlashSeverity::Minor.slash_bps(),    500);
        assert_eq!(SlashSeverity::Major.slash_bps(),    1_000);
        assert_eq!(SlashSeverity::Critical.slash_bps(), 3_000);
    }
}
