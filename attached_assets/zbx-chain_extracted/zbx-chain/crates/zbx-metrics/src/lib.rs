//! zbx-metrics: Prometheus-compatible metrics for the Zebvix node.
//!
//! Exposes a /metrics HTTP endpoint at port 9000 (default).
//! All metrics are thread-safe atomic counters / gauges.

pub mod counters;
pub mod server;

pub use counters::{
    BlockMetrics, ConsensusMetrics, MempoolMetrics, NetworkMetrics,
    RpcMetrics, BridgeMetrics, StakingMetrics, Registry,
    Counter, Gauge,
    render_prometheus, render_prometheus_full,
};