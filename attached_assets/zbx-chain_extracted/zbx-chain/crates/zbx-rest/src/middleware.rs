//! REST API middleware — rate limiting, request ID, CORS.

use axum::{
    http::{HeaderValue, Method, Request},
    middleware::Next,
    response::Response,
};
use std::time::Instant;
use tracing::info;

/// Log request timing and status for every REST call.
pub async fn log_request<B>(req: Request<B>, next: Next<B>) -> Response {
    let method = req.method().clone();
    let uri    = req.uri().clone();
    let start  = Instant::now();

    let resp = next.run(req).await;

    info!(
        method = %method,
        uri = %uri,
        status = resp.status().as_u16(),
        latency_ms = start.elapsed().as_millis(),
        "REST request"
    );
    resp
}

/// Inject `X-Request-Id` header into every response.
pub async fn request_id<B>(req: Request<B>, next: Next<B>) -> Response {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);

    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(
        "X-Request-Id",
        HeaderValue::from_str(&id.to_string()).unwrap_or(HeaderValue::from_static("0")),
    );
    resp
}
