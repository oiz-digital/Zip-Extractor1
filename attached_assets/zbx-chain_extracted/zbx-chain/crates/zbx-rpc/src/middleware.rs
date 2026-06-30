//! RPC middleware: rate limiting, authentication, CORS.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Per-IP rate limiter using a sliding window.
pub struct RateLimiter {
    window: Duration,
    max_requests: usize,
    buckets: HashMap<String, (Instant, usize)>,
}

impl RateLimiter {
    pub fn new(window: Duration, max_requests: usize) -> Self {
        RateLimiter { window, max_requests, buckets: HashMap::new() }
    }

    /// Returns Ok(()) if the request is allowed, Err if rate-limited.
    pub fn check(&mut self, ip: &str) -> Result<(), String> {
        let now = Instant::now();
        let entry = self.buckets.entry(ip.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        if entry.1 > self.max_requests {
            return Err(format!(
                "rate limit exceeded: {} requests per {:?}", self.max_requests, self.window
            ));
        }
        Ok(())
    }

    /// Remove stale buckets.
    pub fn prune(&mut self) {
        let now = Instant::now();
        self.buckets.retain(|_, (ts, _)| now.duration_since(*ts) < self.window * 2);
    }
}

/// CORS headers to inject in HTTP responses.
pub struct CorsConfig {
    pub allowed_origins: Vec<String>,
    pub allowed_methods: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        CorsConfig {
            allowed_origins: vec!["*".to_string()],
            allowed_methods: vec!["POST".to_string(), "OPTIONS".to_string()],
        }
    }
}

impl CorsConfig {
    pub fn is_allowed(&self, origin: &str) -> bool {
        self.allowed_origins.iter().any(|o| o == "*" || o == origin)
    }

    pub fn headers(&self) -> Vec<(&str, String)> {
        vec![
            ("Access-Control-Allow-Origin",  self.allowed_origins.join(", ")),
            ("Access-Control-Allow-Methods", self.allowed_methods.join(", ")),
            ("Access-Control-Allow-Headers", "Content-Type".to_string()),
        ]
    }
}