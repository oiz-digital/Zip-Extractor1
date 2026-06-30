//! zbx-telemetry — observability stack for Zebvix Chain.
//!
//! Provides:
//! - **Metrics** (Prometheus + OpenTelemetry): counters, gauges, histograms
//!   for blocks, transactions, mempool, consensus, P2P, and RPC.
//! - **Tracing** (OpenTelemetry OTLP): distributed traces for block execution,
//!   consensus rounds, and RPC calls.
//! - **Prometheus HTTP endpoint**: `/metrics` on port 9100.
//! - **Structured logging**: JSON logs via `tracing-subscriber` with
//!   environment-level filtering.
//!
//! # Initialisation
//! ```rust
//! zbx_telemetry::init(TelemetryConfig::default())?;
//! ```
//! After this, all `tracing::` macros emit structured JSON logs,
//! and metrics are exported to Prometheus automatically.

pub mod metrics;
pub mod tracing_setup;
pub mod otlp;
pub mod prometheus_server;

pub use metrics::ZbxMetrics;
pub use tracing_setup::{init_tracing, TracingConfig};

// ── High-level runner (telemetry node wiring) ─────────────────────────────────
pub mod service;
pub use service::TelemetryService;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error("failed to init tracing: {0}")]
    Tracing(String),
    #[error("failed to init metrics: {0}")]
    Metrics(String),
    #[error("failed to start Prometheus server: {0}")]
    PrometheusServer(String),
}

/// Telemetry configuration.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// OTLP gRPC endpoint (e.g. "http://localhost:4317").
    pub otlp_endpoint: Option<String>,
    /// Prometheus metrics port.
    pub prometheus_port: u16,
    /// Log level filter (e.g. "info,zbx_consensus=debug").
    pub log_filter: String,
    /// Whether to emit JSON-formatted logs.
    pub json_logs: bool,
    /// Node identity label (added to all metrics).
    pub node_id: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            otlp_endpoint: None,
            prometheus_port: 9100,
            log_filter: "info".into(),
            json_logs: true,
            node_id: "zbx-node-0".into(),
        }
    }
}

/// Initialize the full telemetry stack.
pub fn init(config: TelemetryConfig) -> Result<ZbxMetrics, TelemetryError> {
    init_tracing(TracingConfig {
        filter: config.log_filter.clone(),
        json:   config.json_logs,
        otlp:   config.otlp_endpoint.clone(),
    }).map_err(|e| TelemetryError::Tracing(e.to_string()))?;

    let metrics = ZbxMetrics::new(&config.node_id)
        .map_err(|e| TelemetryError::Metrics(e.to_string()))?;

    // Start Prometheus server in background.
    let port = config.prometheus_port;
    tokio::spawn(async move {
        if let Err(e) = prometheus_server::start(port).await {
            tracing::error!("prometheus server error: {}", e);
        }
    });

    Ok(metrics)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_values() {
        let cfg = TelemetryConfig::default();
        assert_eq!(cfg.prometheus_port, 9100);
        assert_eq!(cfg.log_filter, "info");
        assert!(cfg.json_logs);
        assert_eq!(cfg.node_id, "zbx-node-0");
        assert!(cfg.otlp_endpoint.is_none());
    }

    #[test]
    fn custom_config_fields_set_correctly() {
        let cfg = TelemetryConfig {
            otlp_endpoint:   Some("http://localhost:4317".into()),
            prometheus_port: 9200,
            log_filter:      "debug".into(),
            json_logs:       false,
            node_id:         "validator-1".into(),
        };
        assert_eq!(cfg.prometheus_port, 9200);
        assert_eq!(cfg.log_filter, "debug");
        assert_eq!(cfg.node_id, "validator-1");
        assert!(cfg.otlp_endpoint.is_some());
    }

    #[test]
    fn telemetry_config_clone_is_equal() {
        let cfg = TelemetryConfig::default();
        let cloned = cfg.clone();
        assert_eq!(cloned.prometheus_port, cfg.prometheus_port);
        assert_eq!(cloned.log_filter, cfg.log_filter);
    }

    #[test]
    fn telemetry_error_display_contains_message() {
        let e = TelemetryError::Tracing("bad filter".into());
        assert!(e.to_string().contains("bad filter"));
        let e2 = TelemetryError::Metrics("counter overflow".into());
        assert!(e2.to_string().contains("counter overflow"));
        let e3 = TelemetryError::PrometheusServer("bind failed".into());
        assert!(e3.to_string().contains("bind failed"));
    }
}
