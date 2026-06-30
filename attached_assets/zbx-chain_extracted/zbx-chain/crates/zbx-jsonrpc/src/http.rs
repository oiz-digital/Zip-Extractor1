//! HTTP transport: POST /rpc handler using axum.
//!
//! Security hardening applied (NODE-SEC-2026):
//!   - Body size limited to MAX_BODY_BYTES (4 MiB) via RequestBodyLimitLayer.
//!   - CORS restricted to operator-configured origins instead of permissive wildcard.
//!   - Batch request size capped at MAX_BATCH_SIZE (100 items).

use crate::{
    error::RpcErrorObject,
    request::{JsonRpcCall, JsonRpcRequest},
    response::JsonRpcResponse,
    router::RpcRouter,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, HeaderValue},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use tower_http::cors::{CorsLayer, AllowOrigin};
use tower_http::limit::RequestBodyLimitLayer;
use tracing::{debug, warn};
use std::{net::SocketAddr, sync::Arc};

/// Maximum HTTP request body size: 4 MiB.
/// Prevents unbounded memory allocation from oversized POST bodies.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of items in a JSON-RPC batch request.
/// Without a cap, a single large batch can monopolise the handler.
pub const MAX_BATCH_SIZE: usize = 100;

#[derive(Clone)]
pub struct HttpState {
    pub router: Arc<RpcRouter>,
}

async fn rpc_handler(
    State(state): State<HttpState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Response {
    let text = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            let resp = JsonRpcResponse::error(None, RpcErrorObject::parse_error());
            return (StatusCode::OK, axum::Json(resp)).into_response();
        }
    };

    let call: JsonRpcCall = match serde_json::from_str(text) {
        Ok(c) => c,
        Err(e) => {
            warn!("rpc: parse error: {}", e);
            let resp = JsonRpcResponse::error(None, RpcErrorObject::parse_error());
            return (StatusCode::OK, axum::Json(resp)).into_response();
        }
    };

    match call {
        JsonRpcCall::Single(req) => {
            let id = req.id.clone();
            if !req.validate() {
                let resp = JsonRpcResponse::error(id, RpcErrorObject::invalid_request());
                return (StatusCode::OK, axum::Json(resp)).into_response();
            }
            debug!("rpc: {} {:?}", req.method, id);
            let result = state.router.dispatch(req).await;
            let resp = match result {
                Ok(v)    => JsonRpcResponse::success(id, v),
                Err(err) => JsonRpcResponse::error(id, err),
            };
            (StatusCode::OK, axum::Json(resp)).into_response()
        }
        JsonRpcCall::Batch(reqs) => {
            // Enforce batch size cap — prevents a single request from spawning
            // an unbounded number of handler invocations.
            if reqs.len() > MAX_BATCH_SIZE {
                warn!("rpc: batch size {} exceeds cap {}", reqs.len(), MAX_BATCH_SIZE);
                let resp = JsonRpcResponse::error(
                    None,
                    RpcErrorObject::server_error(
                        -32600,
                        &format!("batch size {} exceeds maximum {}", reqs.len(), MAX_BATCH_SIZE),
                    ),
                );
                return (StatusCode::OK, axum::Json(resp)).into_response();
            }
            let mut responses = Vec::with_capacity(reqs.len());
            for req in reqs {
                let id = req.id.clone();
                let result = state.router.dispatch(req).await;
                let resp = match result {
                    Ok(v)    => JsonRpcResponse::success(id, v),
                    Err(err) => JsonRpcResponse::error(id, err),
                };
                responses.push(resp);
            }
            (StatusCode::OK, axum::Json(responses)).into_response()
        }
    }
}

pub struct HttpTransport {
    addr:         SocketAddr,
    router:       Arc<RpcRouter>,
    /// Allowed CORS origins. Empty list → deny all cross-origin requests.
    /// Use `["*"]` only for public read-only endpoints (never for validator nodes).
    cors_origins: Vec<String>,
}

impl HttpTransport {
    pub fn new(addr: SocketAddr, router: RpcRouter) -> Self {
        Self {
            addr,
            router: Arc::new(router),
            cors_origins: vec![],
        }
    }

    /// Set the CORS allowed origins (replaces the default empty list).
    pub fn with_cors_origins(mut self, origins: Vec<String>) -> Self {
        self.cors_origins = origins;
        self
    }

    pub async fn start(self) -> Result<(), std::io::Error> {
        let state = HttpState { router: self.router };

        // Build a restrictive CorsLayer from the operator-configured origins.
        // Wildcards ("*") are only permitted if explicitly listed — they will
        // never be enabled silently by default.
        let cors = if self.cors_origins.is_empty() {
            // No origins configured → reject all cross-origin requests.
            CorsLayer::new()
        } else if self.cors_origins.iter().any(|o| o == "*") {
            // Operator explicitly opted into wildcard — honour it but warn.
            tracing::warn!("json-rpc HTTP: CORS wildcard '*' enabled — \
                            disable for validator or admin nodes");
            CorsLayer::permissive()
        } else {
            // Specific origin list — parse each into a HeaderValue.
            let values: Vec<HeaderValue> = self.cors_origins.iter()
                .filter_map(|o| HeaderValue::from_str(o).ok())
                .collect();
            CorsLayer::new().allow_origin(AllowOrigin::list(values))
        };

        let app = Router::new()
            .route("/", post(rpc_handler))
            .route("/rpc", post(rpc_handler))
            .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
            .layer(cors)
            .with_state(state);

        tracing::info!("json-rpc HTTP listening on {}", self.addr);
        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        axum::serve(listener, app).await
    }
}
