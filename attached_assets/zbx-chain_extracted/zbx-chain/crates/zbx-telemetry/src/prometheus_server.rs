//! Prometheus HTTP metrics endpoint on /metrics.

use axum::{Router, routing::get, http::StatusCode};
use prometheus::Encoder;
use std::net::SocketAddr;
use tracing::info;

async fn metrics_handler() -> Result<String, StatusCode> {
    let encoder = prometheus::TextEncoder::new();
    let mut buf = Vec::new();
    let metric_families = prometheus::gather();
    encoder.encode(&metric_families, &mut buf)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    String::from_utf8(buf).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn healthz() -> &'static str { "ok" }

pub async fn start(port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/healthz",  get(healthz));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("telemetry: Prometheus metrics on http://{}/metrics", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await.map_err(Into::into)
}