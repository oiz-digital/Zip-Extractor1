//! TelemetryService — high-level node runner for the observability stack.
//!
//! Wraps `zbx_telemetry::init()` into a long-running async task suitable for
//! `spawn_supervised` wiring in `node/src/node.rs`.
//!
//! On start:
//!  1. Initializes tracing subscriber (JSON + OTLP if endpoint configured).
//!  2. Starts the Prometheus `/metrics` HTTP server on `prometheus_port`.
//!  3. Keeps the service alive until shutdown (Prometheus server runs in-task).

use tokio::sync::watch;
use tracing::info;

use crate::{TelemetryConfig, init};

/// High-level telemetry service runner.
pub struct TelemetryService {
    otlp_endpoint:   String,
    log_filter:      String,
    json_logs:       bool,
    prometheus_port: u16,
}

impl TelemetryService {
    /// Create a new `TelemetryService`.
    ///
    /// * `otlp_endpoint`   — OTLP gRPC endpoint (empty = disable traces).
    /// * `log_filter`      — `tracing-subscriber` env-filter string.
    /// * `json_logs`       — emit JSON-formatted structured log lines.
    /// * `prometheus_port` — port for Prometheus `/metrics` HTTP endpoint.
    pub fn new(
        otlp_endpoint:   String,
        log_filter:      String,
        json_logs:       bool,
        prometheus_port: u16,
    ) -> Self {
        Self { otlp_endpoint, log_filter, json_logs, prometheus_port }
    }

    /// Initialize telemetry and run until shutdown.
    pub async fn run_until_shutdown(
        self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        let cfg = TelemetryConfig {
            otlp_endpoint:   if self.otlp_endpoint.is_empty() {
                None
            } else {
                Some(self.otlp_endpoint.clone())
            },
            prometheus_port: self.prometheus_port,
            log_filter:      self.log_filter.clone(),
            json_logs:       self.json_logs,
            node_id:         "zbx-node".into(),
        };

        let _metrics = init(cfg).map_err(|e| format!("telemetry init: {e}"))?;

        info!(
            prometheus_port = self.prometheus_port,
            otlp            = if self.otlp_endpoint.is_empty() { "disabled" } else { &self.otlp_endpoint },
            log_filter      = %self.log_filter,
            "telemetry service initialized"
        );

        // Keep the service alive — Prometheus server runs inside `init()`.
        // When shutdown fires, tracing/metrics handles are dropped.
        shutdown.changed().await.ok();
        info!("telemetry service received shutdown signal");
        Ok(())
    }
}
