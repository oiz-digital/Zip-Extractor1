//! Indexer REST API server (axum-based).
//!
//! ## Routes
//!
//! | Path                    | Method | Description                                   |
//! |-------------------------|--------|-----------------------------------------------|
//! | `/healthz`              | GET    | liveness probe                                |
//! | `/v1/transactions`      | GET    | tx history with filters                       |
//! | `/v1/logs`              | GET    | (placeholder)                                 |
//! | `/v1/tvl`               | GET    | latest full snapshot (ZEP-007)                |
//! | `/v1/tvl/global`        | GET    | latest `total_usd` only                       |
//! | `/v1/tvl/by-source`     | GET    | latest per-source breakdown                   |
//! | `/v1/tvl/history`       | GET    | paginated time-series                         |
//!
//! All TVL routes accept an optional `?oracle=0x…` query param. When the
//! indexer is recording snapshots from multiple oracle deployments, this
//! disambiguates which one to read; omit to use the latest across all.

use crate::query::{
    LogFilter, QueryEngine, TvlHistoryFilter, TxFilter,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::info;

/// Shared API state.
pub struct ApiState {
    pub engine: Arc<QueryEngine>,
}

/// Build the API router.
pub fn build_router(engine: Arc<QueryEngine>) -> Router {
    let state = Arc::new(ApiState { engine });

    Router::new()
        .route("/v1/transactions",  get(list_transactions))
        .route("/v1/logs",           get(list_logs))
        .route("/v1/tvl",            get(get_tvl_latest))
        .route("/v1/tvl/global",     get(get_tvl_global))
        .route("/v1/tvl/by-source",  get(get_tvl_by_source))
        .route("/v1/tvl/history",    get(get_tvl_history))
        .route("/healthz",           get(healthz))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

// ─── Existing routes ─────────────────────────────────────────────────────────

async fn list_transactions(
    Query(filter): Query<TxFilter>,
    State(state): State<Arc<ApiState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    state.engine.query_transactions(filter).await
        .map(|p| Json(serde_json::json!(p)))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn list_logs(
    Query(filter): Query<LogFilter>,
    State(state): State<Arc<ApiState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // H-8 fix: delegate to the query engine instead of returning a static placeholder.
    // The engine reads from the RocksDB log index built by the block processor.
    state.engine.query_logs(filter).await
        .map(|p| Json(serde_json::json!(p)))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn healthz() -> &'static str { "ok" }

// ─── TVL routes (S17 Phase 3) ────────────────────────────────────────────────

/// Common query string for the three "latest snapshot" routes.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OracleScopeQuery {
    /// Hex address of the oracle contract (with or without `0x`).
    /// Omit to use whichever oracle wrote the most recent snapshot.
    pub oracle: Option<String>,
}

async fn get_tvl_latest(
    Query(q): Query<OracleScopeQuery>,
    State(state): State<Arc<ApiState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.engine.query_latest_tvl(q.oracle).await {
        Ok(Some(row)) => Ok(Json(serde_json::json!(row))),
        Ok(None)      => Err(StatusCode::NOT_FOUND),
        Err(_)        => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn get_tvl_global(
    Query(q): Query<OracleScopeQuery>,
    State(state): State<Arc<ApiState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.engine.query_tvl_global(q.oracle).await {
        Ok(Some(row)) => Ok(Json(serde_json::json!(row))),
        Ok(None)      => Err(StatusCode::NOT_FOUND),
        Err(_)        => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn get_tvl_by_source(
    Query(q): Query<OracleScopeQuery>,
    State(state): State<Arc<ApiState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.engine.query_tvl_by_source(q.oracle).await {
        Ok(Some(row)) => Ok(Json(serde_json::json!(row))),
        Ok(None)      => Err(StatusCode::NOT_FOUND),
        Err(_)        => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn get_tvl_history(
    Query(filter): Query<TvlHistoryFilter>,
    State(state): State<Arc<ApiState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    state.engine.query_tvl_history(filter).await
        .map(|p| Json(serde_json::json!(p)))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

// ─── Bind ────────────────────────────────────────────────────────────────────

pub async fn start_server(engine: Arc<QueryEngine>, addr: &str) -> anyhow::Result<()> {
    let router = build_router(engine);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("indexer-api: listening on {}", addr);
    axum::serve(listener, router).await.map_err(Into::into)
}
