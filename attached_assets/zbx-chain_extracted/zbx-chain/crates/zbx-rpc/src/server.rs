//! HTTP JSON-RPC server using Tokio.
//!
//! Wires every connection to a shared `RpcState` (storage + mempool + chain id)
//! so that `eth_*` and `zbx_*` handlers can serve real data.

use crate::{
    error::{JsonRpcError, RpcError},
    eth_api::dispatch_eth,
    middleware::RateLimiter,
    state::RpcState,
    types::{JsonRpcRequest, JsonRpcResponse},
    zbx_api::dispatch_zbx,
};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info, warn};

/// Maximum HTTP body size (1 MiB) — JSON-RPC bodies must fit in one read.
const MAX_BODY: usize = 1 * 1024 * 1024;

/// Maximum number of sub-requests in a single JSON-RPC batch. Without a cap,
/// a single 1 MiB body containing thousands of cheap-but-not-free calls can
/// monopolise the request handler — see AUDIT_2026-04-30.md C-13.
pub const MAX_BATCH_SIZE: usize = 50;

/// SEC-2026-05-09 (Pass-6): cumulative gas budget across one batch.
///
/// `eth_call` and `eth_estimateGas` were individually capped at
/// `RPC_GAS_CAP = 50M` in Pass-5, but a 50-request batch could still
/// pin the CPU for `50 × 50M = 2.5B` gas worth of work per HTTP body
/// (architect-flagged in Pass-5 review).  This constant bounds the
/// *cumulative* EVM gas any one batch may consume.  100M ≈ 3.3 block
/// limits — generous enough for legitimate dashboard / explorer batched
/// reads, far below DoS-amplification territory.
pub const RPC_BATCH_GAS_BUDGET: u64 = 100_000_000;

pub struct RpcServer {
    pub http_port: u16,
    pub ws_port: u16,
    /// Bind address (overridable for production: "127.0.0.1" for localhost-only).
    pub bind_addr: String,
    /// Optional bearer token required on Authorization header for admin methods.
    pub admin_token: Option<String>,
    /// Comma-separated origins for CORS Allow-Origin (defaults to "*").
    pub cors_allow_origin: String,
    state: RpcState,
    rate_limiter: Arc<Mutex<RateLimiter>>,
}

impl RpcServer {
    pub fn new(state: RpcState, http_port: u16, ws_port: u16) -> Self {
        Self {
            http_port,
            ws_port,
            bind_addr: "0.0.0.0".to_string(),
            admin_token: std::env::var("ZBX_RPC_ADMIN_TOKEN").ok(),
            // Default to empty (no Allow-Origin header → browsers block
            // cross-origin requests by default). Operators must opt-in via
            // ZBX_RPC_CORS_ORIGIN or `with_cors_origins`. The previous default
            // of "*" exposed every node to drive-by browser-based RPC calls.
            // See AUDIT_2026-04-30.md H-08.
            cors_allow_origin: std::env::var("ZBX_RPC_CORS_ORIGIN")
                .unwrap_or_else(|_| String::new()),
            state,
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new(
                Duration::from_secs(60),
                600,
            ))),
        }
    }


    /// Override the bind address (e.g. "127.0.0.1" for localhost-only listening).
    pub fn with_bind(mut self, bind_addr: impl Into<String>) -> Self {
        self.bind_addr = bind_addr.into();
        self
    }

    /// Wire the CORS allow-origin list from config. Joins multiple origins with ", ".
    /// Empty list → no `Access-Control-Allow-Origin` header (browsers block
    /// cross-origin). To intentionally allow all origins, pass `["*"]`.
    /// See AUDIT_2026-04-30.md H-08.
    pub fn with_cors_origins(mut self, origins: &[String]) -> Self {
        self.cors_allow_origin = if origins.is_empty() {
            String::new()
        } else {
            origins.join(", ")
        };
        self
    }

    /// Wire the per-IP request rate limit (requests per minute) from config.
    ///
    /// L-5 fix: rpm=0 previously silently disabled rate limiting (max_requests=0
    /// means every request is over the limit but the check was "> 0" so it would
    /// reject ALL requests). Now rpm=0 is treated as "use the safe default (600)"
    /// so misconfigured nodes don't either block all traffic or skip rate limiting.
    pub fn with_rate_limit_rpm(mut self, rpm: u32) -> Self {
        let effective_rpm = if rpm == 0 {
            tracing::warn!(
                "with_rate_limit_rpm(0): rpm=0 is invalid; \
                 falling back to default 600 req/min to prevent denial of service"
            );
            600
        } else {
            rpm as usize
        };
        self.rate_limiter = Arc::new(Mutex::new(RateLimiter::new(
            Duration::from_secs(60),
            effective_rpm,
        )));
        self
    }

    /// Override the admin bearer token (otherwise read from `ZBX_RPC_ADMIN_TOKEN`).
    pub fn with_admin_token(mut self, token: Option<String>) -> Self {
        if token.is_some() {
            self.admin_token = token;
        }
        self
    }

    /// Start the HTTP RPC listener. Loops forever until the TCP listener fails.
    pub async fn run(self) -> std::io::Result<()> {
        let addr = format!("{}:{}", self.bind_addr, self.http_port);
        let listener = TcpListener::bind(&addr).await?;
        info!(addr = addr, "JSON-RPC HTTP server listening");

        let state = self.state.clone();
        let rate_limiter = self.rate_limiter.clone();
        let admin_token = self.admin_token.clone();
        let cors = self.cors_allow_origin.clone();

        // P5-PROD: prune stale rate-limiter buckets periodically to prevent
        // unbounded HashMap growth from unique client IPs over long uptimes.
        let mut conn_count: u64 = 0;

        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(p) => p,
                Err(e) => {
                    error!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            conn_count += 1;
            if conn_count % 1_000 == 0 {
                rate_limiter.lock().prune();
            }
            let peer_ip = peer.ip().to_string();
            if rate_limiter.lock().check(&peer_ip).is_err() {
                warn!(ip = peer_ip, "rate limited");
                let _ = send_simple_response(stream, 429, "Too Many Requests").await;
                continue;
            }
            let s = state.clone();
            let token = admin_token.clone();
            let cors = cors.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, s, token, cors).await {
                    debug!(peer = %peer, "RPC connection closed: {}", e);
                }
            });
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    state: RpcState,
    admin_token: Option<String>,
    cors_origin: String,
) -> std::io::Result<()> {
    // Read until we have full headers (CRLF CRLF). Cap at 16 KiB header window.
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if buf.len() > 16 * 1024 {
            return write_http(
                &mut stream, 431, "Request Header Fields Too Large",
                &cors_origin, "text/plain", b"headers too large",
            ).await;
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = find_double_crlf(&buf) {
            break p;
        }
    };

    // Pre-flight CORS / OPTIONS short-circuit (cheap detection on the verb).
    if buf.starts_with(b"OPTIONS ") {
        return write_http(
            &mut stream, 204, "No Content",
            &cors_origin, "application/json", b"",
        ).await;
    }

    let head = &buf[..header_end];
    let auth_header = parse_auth_header(head);
    let content_length = parse_content_length(head).unwrap_or(0);
    if content_length > MAX_BODY {
        return write_http(
            &mut stream, 413, "Payload Too Large",
            &cors_origin, "text/plain", b"body exceeds max",
        ).await;
    }

    // Drain any body bytes already received past the header separator,
    // then keep reading until we have `content_length` bytes total.
    let body_start = header_end + 4;
    let mut body: Vec<u8> = buf[body_start..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    if body.len() > content_length && content_length > 0 {
        body.truncate(content_length);
    }

    if body.is_empty() {
        return write_http(
            &mut stream, 400, "Bad Request",
            &cors_origin, "text/plain", b"empty body",
        ).await;
    }

    let response = handle_request(&body, &state, auth_header.as_deref(), admin_token.as_deref());
    write_http(
        &mut stream, 200, "OK",
        &cors_origin, "application/json", response.as_bytes(),
    ).await
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn handle_request(
    body: &[u8],
    state: &RpcState,
    auth_header: Option<&str>,
    admin_token: Option<&str>,
) -> String {
    // Try batch request first.
    if let Ok(batch) = serde_json::from_slice::<Vec<JsonRpcRequest>>(body) {
        // Reject oversize batches with a single Invalid Request response, so
        // the client gets a meaningful error rather than the server quietly
        // chewing CPU. See AUDIT_2026-04-30.md C-13.
        if batch.len() > MAX_BATCH_SIZE {
            let err = JsonRpcError::from(RpcError::Parse(format!(
                "batch size {} exceeds MAX_BATCH_SIZE ({})",
                batch.len(), MAX_BATCH_SIZE
            )));
            return serde_json::to_string(&JsonRpcResponse::error(Value::Null, err))
                .unwrap_or_default();
        }
        // SEC-2026-05-09 (Pass-6): set the per-batch cumulative gas
        // budget for the duration of this batch.  `eth_call` /
        // `eth_estimateGas` (in `eth_api.rs`) consume from this budget
        // via `crate::eth_api::batch_budget_consume`.  Cleared on exit
        // (including the panic path via the RAII guard below) so a
        // subsequent non-batch request on the same OS thread is not
        // affected.
        struct BatchBudgetGuard;
        impl Drop for BatchBudgetGuard {
            fn drop(&mut self) {
                crate::eth_api::set_batch_budget(None);
            }
        }
        crate::eth_api::set_batch_budget(Some(RPC_BATCH_GAS_BUDGET));
        let _guard = BatchBudgetGuard;

        let responses: Vec<JsonRpcResponse> = batch
            .into_iter()
            .map(|req| handle_single(req, state, auth_header, admin_token))
            .collect();
        return serde_json::to_string(&responses).unwrap_or_default();
    }

    let req: JsonRpcRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            let err = JsonRpcError::from(RpcError::Parse(e.to_string()));
            return serde_json::to_string(&JsonRpcResponse::error(Value::Null, err))
                .unwrap_or_default();
        }
    };
    serde_json::to_string(&handle_single(req, state, auth_header, admin_token))
        .unwrap_or_default()
}

/// Maximum method name length. Prevents memory abuse via huge method strings
/// that would otherwise propagate through error messages and log entries.
const MAX_METHOD_LEN: usize = 128;

fn handle_single(
    req: JsonRpcRequest,
    state: &RpcState,
    auth_header: Option<&str>,
    admin_token: Option<&str>,
) -> JsonRpcResponse {
    let id = req.id.clone();
    let method = req.method.clone();

    // P5-PROD: reject oversized method names early (before any allocation into
    // error messages or log fields). This prevents an adversary from sending
    // a 1 MiB method string that gets duplicated into every log entry.
    if method.len() > MAX_METHOD_LEN {
        return JsonRpcResponse::error(
            id,
            JsonRpcError::from(RpcError::InvalidRequest(format!(
                "method name too long ({} bytes, max {})",
                method.len(),
                MAX_METHOD_LEN
            ))),
        );
    }

    // Admin / debug methods require the bearer token if one is configured.
    if (method.starts_with("admin_")
        || method.starts_with("debug_")
        || method.starts_with("personal_"))
        && admin_token.is_some()
    {
        let supplied = auth_header
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(str::trim);
        if supplied != admin_token {
            return JsonRpcResponse::error(
                id,
                JsonRpcError::from(RpcError::InvalidRequest(
                    "unauthorized: admin token required".into(),
                )),
            );
        }
    }

    let result = if method.starts_with("eth_")
        || method.starts_with("net_")
        || method.starts_with("web3_")
        || method.starts_with("txpool_")
    {
        dispatch_eth(&method, &req.params, state)
    } else if method.starts_with("zbx_") {
        dispatch_zbx(&method, &req.params, state)
    } else {
        Err(RpcError::MethodNotFound(method))
    };

    match result {
        Ok(v) => JsonRpcResponse::success(id, v),
        Err(e) => JsonRpcResponse::error(id, JsonRpcError::from(e)),
    }
}

// ---------------------------------------------------------------------------
// HTTP framing helpers
// ---------------------------------------------------------------------------

/// Extract the `Authorization` header value (case-insensitive name match,
/// case-PRESERVING value) from an HTTP request head.
fn parse_auth_header(head: &[u8]) -> Option<String> {
    let head_str = std::str::from_utf8(head).ok()?;
    head_str.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("authorization") {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

/// Extract `Content-Length` from an HTTP request head (case-insensitive).
fn parse_content_length(head: &[u8]) -> Option<usize> {
    let head_str = std::str::from_utf8(head).ok()?;
    head_str.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    })
}

async fn write_http(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    cors_origin: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {ct}\r\n\
         Access-Control-Allow-Origin: {cors}\r\n\
         Access-Control-Allow-Headers: Content-Type, Authorization\r\n\
         Access-Control-Allow-Methods: POST, OPTIONS\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n",
        status = status,
        reason = reason,
        ct = content_type,
        cors = cors_origin,
        len = body.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    if !body.is_empty() {
        stream.write_all(body).await?;
    }
    stream.flush().await?;
    Ok(())
}

async fn send_simple_response(
    mut stream: TcpStream,
    status: u16,
    reason: &str,
) -> std::io::Result<()> {
    write_http(&mut stream, status, reason, "*", "text/plain", reason.as_bytes()).await
}
