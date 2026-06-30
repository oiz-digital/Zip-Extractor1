//! Tracing setup: structured JSON logs + OpenTelemetry.

use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Tracing configuration.
#[derive(Debug, Clone)]
pub struct TracingConfig {
    pub filter: String,
    pub json:   bool,
    pub otlp:   Option<String>,
}

/// Initialize the global tracing subscriber.
pub fn init_tracing(config: TracingConfig) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.filter));

    if config.json {
        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().json().with_current_span(true));
        subscriber.try_init().map_err(|e| anyhow::anyhow!("{}", e))?;
    } else {
        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_target(true));
        subscriber.try_init().map_err(|e| anyhow::anyhow!("{}", e))?;
    }

    Ok(())
}