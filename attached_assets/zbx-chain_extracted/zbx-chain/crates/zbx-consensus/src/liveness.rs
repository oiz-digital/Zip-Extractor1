//! Pacemaker: liveness module for round advancement and timeouts.
//!
//! ## N-06 fix (2026-05-05) — deterministic integer backoff
//!
//! The previous implementation stored `timeout_backoff: f64 = 1.5` and
//! used `Duration::mul_f64(backoff.powi(n))` to compute the new timeout.
//! IEEE 754 `f64` arithmetic is non-deterministic across CPU architectures
//! (x86 extended-precision vs ARM strict 64-bit), so two validators running
//! on different hardware could compute different timeout values, causing
//! divergent pacemaker behaviour and split-brain view-changes.
//!
//! Fix: replace `f64` with a rational `(backoff_num, backoff_den)` pair
//! stored as `u64`.  The new timeout is computed as:
//!
//!   timeout_ms = base_ms × backoff_num^consecutive / backoff_den^consecutive
//!
//! using integer-only arithmetic (saturating_mul / checked_div), capped at
//! `max_timeout`.  The default 3/2 ratio reproduces the former 1.5× factor
//! exactly and identically on every CPU.

use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Configurable pacemaker timing.
#[derive(Debug, Clone)]
pub struct PacemakerConfig {
    /// Base round timeout (2 seconds for Zebvix target block time).
    pub base_timeout: Duration,
    /// Backoff numerator.  New timeout = base × (num/den)^consecutive_timeouts.
    /// Default 3 (combined with den=2 gives the former 1.5× factor).
    pub backoff_num: u64,
    /// Backoff denominator.  Must be > 0.  Default 2.
    pub backoff_den: u64,
    /// Maximum timeout after repeated failures.
    pub max_timeout: Duration,
}

impl Default for PacemakerConfig {
    fn default() -> Self {
        PacemakerConfig {
            base_timeout: Duration::from_secs(2),
            backoff_num:  3,   // N-06: replaces f64 1.5 with exact rational 3/2
            backoff_den:  2,
            max_timeout:  Duration::from_secs(30),
        }
    }
}

/// Round state machine for one consensus round.
#[derive(Debug, Clone)]
pub struct RoundState {
    pub round: u64,
    pub epoch: u64,
    pub started_at: Instant,
    pub timeout: Duration,
    pub consecutive_timeouts: u32,
}

impl RoundState {
    pub fn new(round: u64, epoch: u64, timeout: Duration) -> Self {
        RoundState {
            round,
            epoch,
            started_at: Instant::now(),
            timeout,
            consecutive_timeouts: 0,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn is_timed_out(&self) -> bool {
        self.elapsed() >= self.timeout
    }
}

/// Manages round timing and timeout votes.
pub struct Pacemaker {
    config: PacemakerConfig,
    current: RoundState,
}

impl Pacemaker {
    pub fn new(config: PacemakerConfig) -> Self {
        let timeout = config.base_timeout;
        Pacemaker {
            config,
            current: RoundState::new(1, 1, timeout),
        }
    }

    pub fn current_round(&self) -> u64 {
        self.current.round
    }

    pub fn current_epoch(&self) -> u64 {
        self.current.epoch
    }

    pub fn current_state(&self) -> &RoundState {
        &self.current
    }

    /// Advance to the next round on receiving a QC or TC.
    pub fn advance_round(&mut self, new_round: u64, epoch: u64) {
        if new_round <= self.current.round {
            return;
        }
        info!(
            from = self.current.round,
            to = new_round,
            epoch,
            "pacemaker advancing round"
        );
        self.current = RoundState::new(new_round, epoch, self.config.base_timeout);
    }

    /// Whether the current round's deadline has elapsed. Thin
    /// delegate to `RoundState::is_timed_out` so callers can poll
    /// without reaching into `current_state()` themselves.
    pub fn is_timed_out(&self) -> bool {
        self.current.is_timed_out()
    }

    /// Start a fresh round at `(round, epoch)`, resetting the timer
    /// to `config.base_timeout`. Unlike `advance_round` this does NOT
    /// require `round > current.round` — callers (e.g. the
    /// `HotStuff2Pacemaker` view-change path) gate that check
    /// themselves and want an unconditional restart.
    pub fn start_round(&mut self, round: u64, epoch: u64) {
        info!(round, epoch, "pacemaker: starting round");
        self.current = RoundState::new(round, epoch, self.config.base_timeout);
    }

    /// Handle a round timeout: increase backoff and emit timeout vote.
    ///
    /// N-06 fix: timeout is computed using integer-only arithmetic
    /// (no f64) so every validator produces the same value regardless
    /// of CPU architecture.
    pub fn on_timeout(&mut self) -> u64 {
        let timed_out_round = self.current.round;
        warn!(round = timed_out_round, "round timeout — advancing");
        let consecutive = self.current.consecutive_timeouts + 1;

        // Compute base_ms × num^n / den^n using integer arithmetic.
        // Saturate on overflow so we always cap gracefully at max_timeout.
        let base_ms = self.config.base_timeout.as_millis() as u64;
        let den = self.config.backoff_den.max(1);
        let num = self.config.backoff_num;
        let new_ms = (0..consecutive).fold(Some(base_ms), |acc, _| {
            acc?.checked_mul(num)?.checked_div(den)
        });
        let new_timeout = new_ms
            .map(Duration::from_millis)
            .unwrap_or(self.config.max_timeout)
            .min(self.config.max_timeout);

        self.current = RoundState {
            round: self.current.round + 1,
            epoch: self.current.epoch,
            started_at: Instant::now(),
            timeout: new_timeout,
            consecutive_timeouts: consecutive,
        };
        timed_out_round
    }
}