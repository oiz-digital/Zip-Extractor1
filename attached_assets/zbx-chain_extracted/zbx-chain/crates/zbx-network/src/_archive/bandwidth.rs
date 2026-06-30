//! Per-peer bandwidth monitoring and throttling.

use std::collections::VecDeque;
use std::time::Instant;

/// Sliding-window bandwidth tracker.
pub struct BandwidthTracker {
    window_secs:  u64,
    samples:      VecDeque<(Instant, u64)>,   // (time, bytes)
    total_bytes:  u64,
}

impl BandwidthTracker {
    pub fn new(window_secs: u64) -> Self {
        Self { window_secs, samples: VecDeque::new(), total_bytes: 0 }
    }

    /// Record that `bytes` were transferred right now.
    pub fn record(&mut self, bytes: u64) {
        let now = Instant::now();
        self.samples.push_back((now, bytes));
        self.total_bytes += bytes;
        self.evict(now);
    }

    /// Current throughput in bytes/second (over the sliding window).
    pub fn bytes_per_sec(&mut self) -> f64 {
        self.evict(Instant::now());
        if self.samples.is_empty() { return 0.0; }
        let total: u64 = self.samples.iter().map(|(_, b)| b).sum();
        total as f64 / self.window_secs as f64
    }

    fn evict(&mut self, now: Instant) {
        let cutoff = std::time::Duration::from_secs(self.window_secs);
        while self.samples.front().map(|(t, _)| now.duration_since(*t) > cutoff).unwrap_or(false) {
            if let Some((_, b)) = self.samples.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(b);
            }
        }
    }

    pub fn total_bytes(&self) -> u64 { self.total_bytes }
}

/// Throttle: limit bytes per second.
pub struct Throttle {
    limit_bps:   u64,
    bucket:      f64,
    last_refill: Instant,
}

impl Throttle {
    pub fn new(limit_bps: u64) -> Self {
        Self { limit_bps, bucket: limit_bps as f64, last_refill: Instant::now() }
    }

    /// Request to send `bytes`. Returns how many milliseconds to wait (0 = send now).
    pub fn request(&mut self, bytes: u64) -> u64 {
        self.refill();
        if self.bucket >= bytes as f64 {
            self.bucket -= bytes as f64;
            0
        } else {
            let deficit = bytes as f64 - self.bucket;
            self.bucket = 0.0;
            (deficit / self.limit_bps as f64 * 1000.0) as u64
        }
    }

    fn refill(&mut self) {
        let now     = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.bucket = (self.bucket + self.limit_bps as f64 * elapsed).min(self.limit_bps as f64);
        self.last_refill = now;
    }
}