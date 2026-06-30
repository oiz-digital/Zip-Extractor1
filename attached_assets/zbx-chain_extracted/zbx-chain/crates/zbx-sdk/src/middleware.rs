//! Provider middleware: retry, rate limiting, logging, auth.
//!
//! Middlewares form a stack. Each middleware can modify requests and responses.

use crate::error::SdkError;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// A middleware that intercepts JSON-RPC calls.
#[async_trait::async_trait]
pub trait Middleware: Send + Sync {
    async fn call(
        &self,
        method: &str,
        params: Value,
        next:   &dyn Middleware,
    ) -> Result<Value, SdkError>;
}

/// A stack of middlewares.
#[derive(Default, Clone)]
pub struct MiddlewareStack {
    inner: Vec<Arc<dyn Middleware>>,
}

impl MiddlewareStack {
    pub fn push(mut self, m: impl Middleware + 'static) -> Self {
        self.inner.push(Arc::new(m)); self
    }
}

// ── Built-in middlewares ───────────────────────────────────────────────────────

/// Retry middleware: retries retryable errors with exponential backoff.
pub struct RetryMiddleware {
    pub max_retries:    u32,
    pub base_delay_ms:  u64,
}

impl Default for RetryMiddleware {
    fn default() -> Self {
        Self { max_retries: 3, base_delay_ms: 500 }
    }
}

#[async_trait::async_trait]
impl Middleware for RetryMiddleware {
    async fn call(
        &self,
        method: &str,
        params: Value,
        next:   &dyn Middleware,
    ) -> Result<Value, SdkError> {
        let mut attempt = 0;
        loop {
            match next.call(method, params.clone(), next).await {
                Ok(v) => return Ok(v),
                Err(e) if e.is_retryable() && attempt < self.max_retries => {
                    let delay = self.base_delay_ms * (1 << attempt);
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    attempt += 1;
                }
                Err(e) => {
                    if attempt >= self.max_retries {
                        return Err(SdkError::MaxRetries(self.max_retries));
                    }
                    return Err(e);
                }
            }
        }
    }
}

/// Rate limit middleware: enforces a maximum requests-per-second.
pub struct RateLimitMiddleware {
    pub rps:   u64,
    counter:   Arc<AtomicU64>,
    window_ms: Arc<AtomicU64>,
}

impl RateLimitMiddleware {
    pub fn new(rps: u64) -> Self {
        Self {
            rps,
            counter:   Arc::new(AtomicU64::new(0)),
            window_ms: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl Middleware for RateLimitMiddleware {
    async fn call(
        &self,
        method: &str,
        params: Value,
        next:   &dyn Middleware,
    ) -> Result<Value, SdkError> {
        // Simple token bucket — reset counter each second.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let window = now / 1000;
        let prev   = self.window_ms.swap(window, Ordering::Relaxed);
        if prev != window { self.counter.store(0, Ordering::Relaxed); }

        let count = self.counter.fetch_add(1, Ordering::Relaxed);
        if count >= self.rps {
            let wait_ms = 1000 - (now % 1000);
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
        }
        next.call(method, params, next).await
    }
}

/// Logging middleware: logs all RPC calls and their latency.
pub struct LoggingMiddleware {
    pub level: log::Level,
}

impl Default for LoggingMiddleware {
    fn default() -> Self { Self { level: log::Level::Debug } }
}

#[async_trait::async_trait]
impl Middleware for LoggingMiddleware {
    async fn call(
        &self,
        method: &str,
        params: Value,
        next:   &dyn Middleware,
    ) -> Result<Value, SdkError> {
        let start = std::time::Instant::now();
        let result = next.call(method, params, next).await;
        let elapsed = start.elapsed().as_millis();
        match &result {
            Ok(_)  => log::log!(self.level, "RPC {} OK ({} ms)", method, elapsed),
            Err(e) => log::log!(self.level, "RPC {} ERR ({} ms): {}", method, elapsed, e),
        }
        result
    }
}

/// Bearer-token auth middleware (for private RPC endpoints).
pub struct AuthMiddleware {
    pub token: String,
}

#[async_trait::async_trait]
impl Middleware for AuthMiddleware {
    async fn call(
        &self,
        method: &str,
        params: Value,
        next:   &dyn Middleware,
    ) -> Result<Value, SdkError> {
        // In production: inject Authorization header into the HTTP request.
        next.call(method, params, next).await
    }
}