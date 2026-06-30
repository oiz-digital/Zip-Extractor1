//! Metrics registry — exports all metrics in Prometheus format.

use prometheus::{Encoder, Registry, TextEncoder};

lazy_static::lazy_static! {
    pub static ref REGISTRY: Registry = Registry::new();
}

/// Render all metrics to Prometheus text format.
pub fn render() -> String {
    let encoder  = TextEncoder::new();
    let families = REGISTRY.gather();
    let mut buf  = vec![];
    encoder.encode(&families, &mut buf).unwrap_or_default();
    String::from_utf8(buf).unwrap_or_default()
}