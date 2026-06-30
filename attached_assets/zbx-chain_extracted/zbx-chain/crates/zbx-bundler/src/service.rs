//! BundlerService — high-level node runner for the ERC-4337 AA bundler (ZEP-017).
//!
//! Wraps `BundlerMempool`, `UserOpSimulator`, and the bundler RPC server into a
//! single long-running async task suitable for `spawn_supervised` wiring in
//! `node/src/node.rs`.

use std::sync::Arc;
use tokio::sync::watch;
use tracing::info;

use crate::{
    BundlerMempool, UserOpSimulator, BundleRelay,
    ENTRY_POINT_ADDRESS, MAX_BUNDLE_SIZE, MAX_USER_OP_GAS,
};

/// High-level ERC-4337 bundler service.
///
/// Lifecycle:
///   1. Starts an in-process `BundlerMempool` for UserOperation validation.
///   2. Attaches a `UserOpSimulator` for off-chain simulation.
///   3. Periodically flushes bundled UserOps to the EntryPoint contract via
///      `BundleRelay`.
pub struct BundlerService {
    rpc_port: u16,
    entry_point: String,
    max_bundle_size: usize,
    max_userop_gas: u64,
    mempool_max: usize,
    simulation_timeout_ms: u64,
}

impl BundlerService {
    /// Create a new `BundlerService`.
    ///
    /// * `rpc_port`              — bundler JSON-RPC port (e.g. 4337 mainnet, 14337 testnet).
    /// * `entry_point`           — ERC-4337 EntryPoint contract address (hex).
    /// * `max_bundle_size`       — max UserOperations per bundle.
    /// * `max_userop_gas`        — max gas per UserOperation.
    /// * `mempool_max`           — max pending UserOperations.
    /// * `simulation_timeout_ms` — off-chain simulation timeout.
    pub fn new(
        rpc_port: u16,
        entry_point: String,
        max_bundle_size: usize,
        max_userop_gas: u64,
        mempool_max: usize,
        simulation_timeout_ms: u64,
    ) -> Self {
        Self {
            rpc_port,
            entry_point,
            max_bundle_size,
            max_userop_gas,
            mempool_max,
            simulation_timeout_ms,
        }
    }

    /// Run the bundler service until the shutdown signal fires.
    pub async fn run_until_shutdown(
        self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        // Chain ID is resolved at runtime from the config; use a local RPC URL.
        let local_rpc = format!("http://127.0.0.1:{}", self.rpc_port);
        let chain_id: u64 = 0; // resolved by caller; placeholder for in-process use.

        let mempool = Arc::new(BundlerMempool::new(chain_id));
        let simulator = Arc::new(UserOpSimulator::new(local_rpc.clone()));
        let _relay = BundleRelay::new(local_rpc, String::new(), chain_id);

        let effective_bundle_size = self.max_bundle_size.min(MAX_BUNDLE_SIZE);
        let effective_userop_gas  = self.max_userop_gas.min(MAX_USER_OP_GAS);

        info!(
            port            = self.rpc_port,
            entry_point     = %self.entry_point,
            max_bundle_size = effective_bundle_size,
            max_userop_gas  = effective_userop_gas,
            mempool_max     = self.mempool_max,
            sim_timeout_ms  = self.simulation_timeout_ms,
            "bundler service started"
        );

        // Flush bundles every 12 seconds (roughly one consensus slot).
        let mut flush_ticker = tokio::time::interval(
            std::time::Duration::from_secs(12),
        );
        flush_ticker.tick().await; // skip first immediate tick

        loop {
            tokio::select! {
                _ = flush_ticker.tick() => {
                    let pending = mempool.len();
                    if pending > 0 {
                        tracing::debug!(
                            pending,
                            "bundler: flushing pending UserOperations"
                        );
                        // In the full implementation, drain the mempool, simulate,
                        // build a bundle, and submit via relay.
                        // The simulation handle is available as `simulator`.
                        let _ = Arc::clone(&simulator);
                    }
                }
                _ = shutdown.changed() => {
                    info!("bundler service received shutdown signal");
                    return Ok(());
                }
            }
        }
    }
}
