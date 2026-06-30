//! Output / logging setup for the CLI.
//!
//! All structured logging goes through `tracing`. The CLI prints
//! human-readable status to STDOUT/STDERR; structured JSON output is
//! reserved for future `--output json` flag work.

use tracing_subscriber::{EnvFilter, fmt};

/// Install a tracing subscriber. Honours `RUST_LOG`; if unset, defaults to
/// `info` (or `debug` with `--verbose`).
pub fn init_tracing(verbose: bool) {
    let default_level = if verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_level));

    // `try_init` lets the test binary install its own subscriber without
    // panicking on a duplicate global default.
    let _ = fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}
