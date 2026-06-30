//! OpenTelemetry OTLP exporter configuration.

use opentelemetry_sdk::trace::TracerProvider;

/// Initialize the OTLP gRPC exporter.
/// `install_batch` already registers the global tracer provider internally.
pub fn init_otlp(endpoint: &str) -> anyhow::Result<()> {
    use opentelemetry_otlp::WithExportConfig;

    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(endpoint);

    let _tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .install_batch(opentelemetry_sdk::runtime::Tokio)?;

    Ok(())
}

/// Shutdown the OTLP exporter (flush pending spans).
pub fn shutdown_otlp() {
    opentelemetry::global::shutdown_tracer_provider();
}
